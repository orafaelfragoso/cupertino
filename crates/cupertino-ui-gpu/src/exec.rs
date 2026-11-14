//! Executing a recording: the uniforms and instances written once, then each draw in order, a pass begun whenever
//! the target changes (cleared the first time for an offscreen target and for a window drawn whole, kept after).

use cupertino_gpu::{
    BindGroupEntry, BindingResource, BufferUsage, Error as GpuError, LoadOp, Pass, Texture,
};

use crate::record::{Cmd, Program, Recording, TargetId, Tex};
use crate::{GpuRenderer, Programs, TargetMem, INSTANCE_BYTES};

impl<R> GpuRenderer<R> {
    /// Draws what `rec` recorded since the last execution, and empties its draws.
    pub(crate) fn execute(&mut self, rec: &mut Recording) -> Result<(), GpuError> {
        if rec.cmds.is_empty() {
            return Ok(());
        }
        let device = std::rc::Rc::clone(&self.device);
        let instance_bytes: Vec<u8> = rec.instances.iter().flat_map(|f| f.to_le_bytes()).collect();
        crate::upload(
            &device,
            &mut self.uniforms,
            BufferUsage::Uniform,
            &rec.uniforms,
        )?;
        crate::upload(
            &device,
            &mut self.instances,
            BufferUsage::Vertex,
            &instance_bytes,
        )?;
        self.stats.uniform_bytes += rec.uniforms.len() as u64;
        self.stats.instance_bytes += instance_bytes.len() as u64;
        let cmds = std::mem::take(&mut rec.cmds);
        let programs = self
            .programs
            .as_ref()
            .expect("programs live as long as the renderer");
        let uniforms = self.uniforms.as_ref();
        let instances = self.instances.as_ref();
        let mut pass: Option<(TargetId, Pass<'_>)> = None;
        let mut passes = 0;
        for cmd in &cmds {
            let (target, program) = match cmd {
                Cmd::Begin { target } => {
                    if let Some((_, p)) = pass.take() {
                        p.end();
                    }
                    rec.started[target.0] = true;
                    let texture = target_texture(self.window.as_ref(), rec, *target);
                    pass = Some((
                        *target,
                        device.begin_pass(texture, LoadOp::Clear([0.0; 4]))?,
                    ));
                    passes += 1;
                    continue;
                }
                Cmd::Quads {
                    target, program, ..
                }
                | Cmd::Whole {
                    target, program, ..
                } => (*target, *program),
            };
            let whole = matches!(cmd, Cmd::Whole { .. });
            // a whole-target pass reads other targets: each such pass is its own
            if whole || pass.as_ref().is_none_or(|(t, _)| *t != target) {
                if let Some((_, p)) = pass.take() {
                    p.end();
                }
                let first = !rec.started[target.0];
                rec.started[target.0] = true;
                let load = if first && (target.0 != 0 || rec.full) {
                    LoadOp::Clear([0.0; 4])
                } else {
                    LoadOp::Load
                };
                let texture = target_texture(self.window.as_ref(), rec, target);
                pass = Some((target, device.begin_pass(texture, load)?));
                passes += 1;
            }
            let p = &mut pass.as_mut().expect("begun above").1;
            let pipeline = match program {
                Program::Fill => &programs.fill,
                Program::Clear => &programs.clear,
                Program::Texture => &programs.texture,
                Program::Halve => &programs.halve,
                Program::Gauss => &programs.gauss,
                Program::Resolve => &programs.resolve,
            };
            p.set_pipeline(pipeline);
            let uniform_buffer = uniforms.expect("written above");
            match cmd {
                Cmd::Quads {
                    scissor,
                    uniform,
                    textures,
                    instances: (first, count),
                    ..
                } => {
                    let layout = match program {
                        Program::Texture => &programs.texture_layout,
                        _ => &programs.fill_layout,
                    };
                    let mut entries = vec![BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::Buffer {
                            buffer: uniform_buffer,
                            offset: uniform.0 as u64,
                            size: uniform.1 as u64,
                        },
                    }];
                    let count_textures = if program == Program::Texture { 3 } else { 2 };
                    for (i, tex) in textures.iter().take(count_textures).enumerate() {
                        entries.push(BindGroupEntry {
                            binding: i as u32 + 1,
                            resource: BindingResource::Texture(
                                self.texture_of(programs, rec, *tex),
                            ),
                        });
                    }
                    let group = device.create_bind_group(layout, &entries)?;
                    p.set_bind_group(0, &group)?;
                    p.set_vertex_buffer(
                        0,
                        instances.expect("written above"),
                        u64::from(*first) * u64::from(INSTANCE_BYTES),
                    )?;
                    p.set_scissor(Some(*scissor));
                    p.draw(0..4, *count)?;
                }
                Cmd::Begin { .. } => unreachable!("handled above"),
                Cmd::Whole {
                    uniform, textures, ..
                } => {
                    let layout = match program {
                        Program::Resolve => &programs.resolve_layout,
                        _ => &programs.level_layout,
                    };
                    let mut entries = vec![BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::Buffer {
                            buffer: uniform_buffer,
                            offset: uniform.0 as u64,
                            size: uniform.1 as u64,
                        },
                    }];
                    let count_textures = if program == Program::Resolve { 2 } else { 1 };
                    for (i, tex) in textures.iter().take(count_textures).enumerate() {
                        entries.push(BindGroupEntry {
                            binding: i as u32 + 1,
                            resource: BindingResource::Texture(
                                self.texture_of(programs, rec, *tex),
                            ),
                        });
                    }
                    let group = device.create_bind_group(layout, &entries)?;
                    p.set_bind_group(0, &group)?;
                    p.set_scissor(None);
                    p.draw(0..3, 1)?;
                }
            }
        }
        if let Some((_, p)) = pass.take() {
            p.end();
        }
        self.stats.passes += passes;
        rec.uniforms.clear();
        rec.instances.clear();
        Ok(())
    }

    fn texture_of<'a>(
        &'a self,
        programs: &'a Programs,
        rec: &'a Recording,
        tex: Tex,
    ) -> &'a Texture {
        match tex {
            Tex::Empty => &programs.empty,
            Tex::Coverage(page) => self.coverage.page(page).unwrap_or(&programs.empty),
            Tex::Color(page) => self.color.page(page).unwrap_or(&programs.empty),
            Tex::Mask(page) => self.masks.page(page).unwrap_or(&programs.empty),
            Tex::Image(id) => self.images.get(&id).map_or(&programs.empty, |i| &i.texture),
            Tex::Target(t) => target_texture(self.window.as_ref(), rec, t),
            Tex::Own(i) => &rec.own[i],
        }
    }
}

fn target_texture<'a>(window: Option<&'a Texture>, rec: &'a Recording, t: TargetId) -> &'a Texture {
    match rec.target_mem(t) {
        TargetMem::Window => window.expect("the window texture is made before drawing"),
        TargetMem::Pooled(p) => &p.target,
        TargetMem::Own(t) => t,
    }
}
