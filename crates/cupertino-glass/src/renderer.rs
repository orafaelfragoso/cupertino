//! `MaterialRenderer`: renders N material regions per frame on a `cupertino-gpu` device.
//!
//! Pipeline (all full-screen fragment passes, every program one of the generated programs of [`crate::shaders`], the
//! same the compositor draws):
//!
//! ```text
//! source ─(2× box)→ capture @ backdrop scale ─(2× box pyramid)→ separable Gaussian
//!        ─→ colour filter chain ─→ cached processed backdrop
//! region quad: SDF rounded mask ─ base = backdrop (or source) ─ fill layers ─ alpha blend
//! ```
//!
//! The processed backdrop depends only on the recipe's backdrop settings, the display scale and the
//! source size, so it is shared between regions and cached across frames keyed by those settings and
//! by a caller-provided *damage generation counter*: bump the counter whenever the source content
//! (or the source texture object) changes; while it is unchanged the blur is not recomputed.
//!
//! Work runs on the device as it is recorded; [`Device::submit`] or a read-back finishes it. Resources the renderer
//! makes are destroyed explicitly: call [`MaterialRenderer::destroy`] before the device goes.

use crate::ops::{color_ops, fill_ops, Op, PassParams, Placement, RegionParams, MAX_FILLS};
use crate::shaders::{self, Program};
use crate::vibrant::VibrantDraw;
use cupertino_geometry::{CornerStyle, Rect, RoundedRect};
use cupertino_gpu::{
    BindGroup, BindGroupEntry, BindGroupLayout, BindingKind, BindingLayout, BindingResource, Blend,
    Buffer, BufferDesc, BufferUsage, Device, Filter, LoadOp, Pipeline, PipelineDesc, Result,
    Sampler, SamplerDesc, Texture, TextureDesc, TextureFormat, TextureUsage, Topology,
};
use cupertino_raster_math::blur::{plan_blur, BlurPlan};
use cupertino_theme::recipe::Recipe;
use std::collections::HashMap;

/// Frames a processed backdrop is kept without being drawn.
const BACKDROP_KEPT_FRAMES: u64 = 120;
/// The antialiasing width of a region's mask, in pixels.
const MASK_AA: f32 = 1.0;

/// One material to draw: a rectangle in target pixels (y down) plus its recipe.
#[derive(Clone, Copy, Debug)]
pub struct MaterialRegion<'a> {
    /// x, y, width, height in target pixels.
    pub rect: [f32; 4],
    pub recipe: &'a Recipe,
    /// Corner radius in pixels (nominal radius; the continuous style expands it).
    pub corner_radius: f32,
    pub corner_style: CornerStyle,
    /// Multiplies the mask coverage (0..=1).
    pub opacity: f32,
}

impl<'a> MaterialRegion<'a> {
    /// Region without rounded corners and full opacity.
    pub fn new(rect: [f32; 4], recipe: &'a Recipe) -> Self {
        Self {
            rect,
            recipe,
            corner_radius: 0.0,
            corner_style: CornerStyle::Circular,
            opacity: 1.0,
        }
    }

    /// Region that takes its corner radius (points → pixels with `display_scale`) and curve from the
    /// recipe's archive, if it specifies one.
    pub fn from_recipe(rect: [f32; 4], recipe: &'a Recipe, display_scale: f32) -> Self {
        let mut r = Self::new(rect, recipe);
        if let Some(rad) = recipe.corner_radius {
            r.corner_radius = rad * display_scale;
        }
        if recipe.corner_curve == cupertino_theme::CornerCurve::Continuous {
            r.corner_style = CornerStyle::continuous();
        }
        r
    }

    pub fn with_corner(mut self, radius_px: f32, style: CornerStyle) -> Self {
        self.corner_radius = radius_px;
        self.corner_style = style;
        self
    }

    pub fn with_opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// Resolved corner extent in pixels (after fitting to the rectangle) and superellipse exponent;
    /// shared by the GPU uniform and the CPU reference.
    pub fn shape(&self) -> (f32, f32) {
        let rr = self.rounded_rect();
        (rr.extents()[0] as f32, rr.style.exponent() as f32)
    }

    pub fn rounded_rect(&self) -> RoundedRect {
        RoundedRect::new(
            Rect::new(
                self.rect[0] as f64,
                self.rect[1] as f64,
                self.rect[2] as f64,
                self.rect[3] as f64,
            ),
            self.corner_radius.max(0.0) as f64,
            self.corner_style,
        )
    }
}

/// What the target contains before the regions are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetInit {
    /// Draw over the existing target content.
    Keep,
    /// Overwrite the target with the source first (headless rendering and tests).
    CopySource,
}

pub struct FrameDesc<'a> {
    /// Content below the materials. Must be a different texture than `target`, sampled as
    /// non-sRGB values.
    pub source: &'a Texture,
    pub source_size: (u32, u32),
    /// A render target (8-bit, non-sRGB: values are blended gamma-encoded).
    pub target: &'a Texture,
    pub target_size: (u32, u32),
    /// Device pixels per layout point; scales σ and the recipe corner radii.
    pub display_scale: f32,
    /// Increment when the source content or texture object changes (see module docs).
    pub damage_generation: u64,
    /// Colour of the tint layer (approximate: wallpaper average).
    pub tint_color: [f32; 3],
    pub init: TargetInit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub backdrop_hits: usize,
    pub backdrop_misses: usize,
    pub passes_encoded: usize,
    pub regions_drawn: usize,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct BackdropKey {
    src_size: (u32, u32),
    scale: u32,
    sigma: u32,
    display_scale: u32,
    ops: Vec<u32>,
}

#[derive(Clone, Copy)]
enum PassKind {
    Downsample,
    Blur,
    Color,
}

/// One backdrop pass: its program, its parameters, and the level it reads (`None`: the frame's source).
struct PassDesc {
    kind: PassKind,
    params: PassParams,
    source: Option<usize>,
}

struct BackdropEntry {
    generation: u64,
    last_used: u64,
    /// The levels the passes write, in order; the last is the processed backdrop.
    levels: Vec<Texture>,
}

/// The device objects of one frame's draws, destroyed once the draws are recorded.
#[derive(Default)]
struct Transient {
    buffers: Vec<Buffer>,
}

impl Transient {
    fn uniform(&mut self, device: &Device, bytes: &[u8]) -> Result<usize> {
        let buffer = device.create_buffer(&BufferDesc {
            size: bytes.len() as u64,
            usage: BufferUsage::Uniform,
        })?;
        device.write_buffer(&buffer, 0, bytes)?;
        self.buffers.push(buffer);
        Ok(self.buffers.len() - 1)
    }

    fn whole(&self, i: usize) -> BindingResource<'_> {
        let buffer = &self.buffers[i];
        BindingResource::Buffer {
            buffer,
            offset: 0,
            size: buffer.size(),
        }
    }

    fn destroy(self, device: &Device) {
        for b in self.buffers {
            device.destroy_buffer(b);
        }
    }
}

pub struct MaterialRenderer {
    backdrop_format: TextureFormat,
    layout_pass: BindGroupLayout,
    layout_region: BindGroupLayout,
    layout_place: BindGroupLayout,
    layout_vibrant: BindGroupLayout,
    pipe_downsample: Pipeline,
    pipe_blur: Pipeline,
    pipe_color: Pipeline,
    pipe_blit: Pipeline,
    pipe_material: Pipeline,
    pipe_vibrant: Pipeline,
    sampler: Sampler,
    cache: HashMap<BackdropKey, BackdropEntry>,
    frame: u64,
}

/// The column-major projection from target pixels (y down) to clip space, the first row at clip y = -1.
fn target_projection(size: (u32, u32)) -> [f32; 9] {
    let (w, h) = (size.0 as f32, size.1 as f32);
    [2.0 / w, 0.0, 0.0, 0.0, 2.0 / h, 0.0, -1.0, -1.0, 1.0]
}

fn layout(device: &Device, kinds: &[BindingKind]) -> Result<BindGroupLayout> {
    let entries: Vec<BindingLayout> = kinds
        .iter()
        .enumerate()
        .map(|(i, &kind)| BindingLayout {
            binding: i as u32,
            kind,
        })
        .collect();
    device.create_bind_group_layout(&entries)
}

impl MaterialRenderer {
    /// Create the pipelines on `device`. The backdrop's levels are half floats where the device renders to them.
    pub fn new(device: &Device) -> Result<Self> {
        use BindingKind::{Sampler as S, Texture as T, UniformBuffer as U};
        let layout_pass = layout(device, &[U, T, S])?;
        let layout_region = layout(device, &[U, T, S])?;
        let layout_place = layout(device, &[U])?;
        let layout_vibrant = layout(device, &[U, T, T, S])?;
        let make =
            |program: &Program, layouts: &[&BindGroupLayout], blend: Blend, topology: Topology| {
                device.create_pipeline(&PipelineDesc {
                    shader: &program.source,
                    layouts,
                    vertex_buffers: &[],
                    blend,
                    topology,
                })
            };
        let list = Topology::TriangleList;
        let strip = Topology::TriangleStrip;
        let pass = [&layout_pass];
        let pipe_downsample = make(&shaders::DOWNSAMPLE, &pass, Blend::Replace, list)?;
        let pipe_blur = make(&shaders::BLUR, &pass, Blend::Replace, list)?;
        let pipe_color = make(&shaders::COLOR, &pass, Blend::Replace, list)?;
        let pipe_blit = make(&shaders::BLIT, &pass, Blend::Replace, list)?;
        let pipe_material = make(
            &shaders::MATERIAL,
            &[&layout_region, &layout_place],
            Blend::StraightOver,
            strip,
        )?;
        let pipe_vibrant = make(&shaders::VIBRANT, &[&layout_vibrant], Blend::Replace, strip)?;
        let sampler = device.create_sampler(&SamplerDesc {
            filter: Filter::Linear,
            address: cupertino_gpu::AddressMode::ClampToEdge,
        })?;
        let backdrop_format = if device.info().half_float_targets {
            TextureFormat::Rgba16Float
        } else {
            TextureFormat::Rgba8
        };
        Ok(Self {
            backdrop_format,
            layout_pass,
            layout_region,
            layout_place,
            layout_vibrant,
            pipe_downsample,
            pipe_blur,
            pipe_color,
            pipe_blit,
            pipe_material,
            pipe_vibrant,
            sampler,
            cache: HashMap::new(),
            frame: 0,
        })
    }

    /// Destroy every object of the renderer on `device`.
    pub fn destroy(mut self, device: &Device) {
        self.invalidate(device);
        for p in [
            self.pipe_downsample,
            self.pipe_blur,
            self.pipe_color,
            self.pipe_blit,
            self.pipe_material,
            self.pipe_vibrant,
        ] {
            device.destroy_pipeline(p);
        }
        device.destroy_sampler(self.sampler);
    }

    /// The format of the processed backdrop's levels.
    pub fn backdrop_format(&self) -> TextureFormat {
        self.backdrop_format
    }

    /// Number of cached processed backdrops.
    pub fn cached_backdrops(&self) -> usize {
        self.cache.len()
    }

    /// Drop every cached backdrop (e.g. after a device-wide resource reset).
    pub fn invalidate(&mut self, device: &Device) {
        for (_, e) in self.cache.drain() {
            for t in e.levels {
                device.destroy_texture(t);
            }
        }
    }

    fn key_for(frame: &FrameDesc, b: &cupertino_theme::Backdrop) -> BackdropKey {
        let mut ops = Vec::new();
        for o in color_ops(&b.ops) {
            ops.extend(o.a.iter().chain(o.b.iter()).map(|f| f.to_bits()));
        }
        BackdropKey {
            src_size: frame.source_size,
            scale: b.scale.to_bits(),
            sigma: b.sigma_pt.to_bits(),
            display_scale: frame.display_scale.to_bits(),
            ops,
        }
    }

    /// The passes of one backdrop for the current source, with the size of the level each writes.
    fn plan(frame: &FrameDesc, b: &cupertino_theme::Backdrop) -> Vec<(PassDesc, (u32, u32))> {
        let (w, h) = frame.source_size;
        let plan: BlurPlan = plan_blur(w, h, b.scale, b.sigma_pt, frame.display_scale);
        let mut passes = Vec::new();
        let mut cur: Option<usize> = None;
        let mut cur_size = (w, h);
        let mut push = |passes: &mut Vec<(PassDesc, (u32, u32))>,
                        kind: PassKind,
                        params: PassParams,
                        size: (u32, u32)| {
            passes.push((
                PassDesc {
                    kind,
                    params,
                    source: cur,
                },
                size,
            ));
            cur = Some(passes.len() - 1);
        };
        for &(lw, lh) in &plan.levels {
            push(
                &mut passes,
                PassKind::Downsample,
                PassParams::new(cur_size, (lw, lh)),
                (lw, lh),
            );
            cur_size = (lw, lh);
        }
        if plan.weights.len() > 1 {
            for dir in [[1, 0], [0, 1]] {
                let mut params = PassParams::new(cur_size, cur_size);
                params.dir_radius = [dir[0], dir[1], (plan.weights.len() - 1) as i32, 0];
                params.set_weights(&plan.weights);
                push(&mut passes, PassKind::Blur, params, cur_size);
            }
        }
        let cops = color_ops(&b.ops);
        if !cops.is_empty() {
            let mut params = PassParams::new(cur_size, cur_size);
            params.set_ops(&cops);
            push(&mut passes, PassKind::Color, params, cur_size);
        }
        passes
    }

    /// Run the passes of one backdrop into new levels.
    fn build_entry(
        &self,
        device: &Device,
        frame: &FrameDesc,
        b: &cupertino_theme::Backdrop,
    ) -> Result<(BackdropEntry, usize)> {
        let passes = Self::plan(frame, b);
        let mut levels: Vec<Texture> = Vec::with_capacity(passes.len());
        let mut transient = Transient::default();
        let result = (|| -> Result<()> {
            for (pass, size) in &passes {
                let level = device.create_texture(&TextureDesc {
                    size: *size,
                    format: self.backdrop_format,
                    usage: TextureUsage::TARGET,
                })?;
                levels.push(level);
                let params = transient.uniform(device, bytemuck::bytes_of(&pass.params))?;
                let src = pass.source.map_or(frame.source, |i| &levels[i]);
                let group = self.pass_group(device, &transient, params, src)?;
                let target = levels.last().expect("pushed");
                let mut rp = device.begin_pass(target, LoadOp::Clear([0.0, 0.0, 0.0, 1.0]))?;
                rp.set_pipeline(match pass.kind {
                    PassKind::Downsample => &self.pipe_downsample,
                    PassKind::Blur => &self.pipe_blur,
                    PassKind::Color => &self.pipe_color,
                });
                rp.set_bind_group(0, &group)?;
                rp.draw(0..3, 1)?;
                rp.end();
            }
            Ok(())
        })();
        transient.destroy(device);
        if let Err(e) = result {
            for t in levels {
                device.destroy_texture(t);
            }
            return Err(e);
        }
        Ok((
            BackdropEntry {
                generation: frame.damage_generation,
                last_used: self.frame,
                levels,
            },
            passes.len(),
        ))
    }

    fn pass_group(
        &self,
        device: &Device,
        transient: &Transient,
        params: usize,
        src: &Texture,
    ) -> Result<BindGroup> {
        device.create_bind_group(
            &self.layout_pass,
            &[
                BindGroupEntry {
                    binding: 0,
                    resource: transient.whole(params),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Texture(src),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        )
    }

    /// Draw one frame's regions into its target (the backdrop passes run only for cache misses).
    pub fn render(
        &mut self,
        device: &Device,
        frame: &FrameDesc,
        regions: &[MaterialRegion],
    ) -> Result<RenderStats> {
        self.frame += 1;
        let mut stats = RenderStats::default();

        let mut keys: Vec<Option<BackdropKey>> = Vec::with_capacity(regions.len());
        for r in regions {
            let Some(b) = &r.recipe.backdrop else {
                keys.push(None);
                continue;
            };
            let key = Self::key_for(frame, b);
            let stale = self
                .cache
                .get(&key)
                .is_none_or(|e| e.generation != frame.damage_generation);
            if stale {
                let (entry, passes) = self.build_entry(device, frame, b)?;
                stats.passes_encoded += passes;
                stats.backdrop_misses += 1;
                if let Some(old) = self.cache.insert(key.clone(), entry) {
                    for t in old.levels {
                        device.destroy_texture(t);
                    }
                }
            } else {
                stats.backdrop_hits += 1;
            }
            let e = self.cache.get_mut(&key).expect("entry exists");
            e.last_used = self.frame;
            keys.push(Some(key));
        }
        let frame_no = self.frame;
        let old: Vec<BackdropKey> = self
            .cache
            .iter()
            .filter(|(_, e)| frame_no - e.last_used > BACKDROP_KEPT_FRAMES)
            .map(|(k, _)| k.clone())
            .collect();
        for k in old {
            if let Some(e) = self.cache.remove(&k) {
                for t in e.levels {
                    device.destroy_texture(t);
                }
            }
        }

        if regions.is_empty() && frame.init == TargetInit::Keep {
            return Ok(stats);
        }

        let mut transient = Transient::default();
        let result = self.draw_regions(device, frame, regions, &keys, &mut transient, &mut stats);
        transient.destroy(device);
        result.map(|()| stats)
    }

    fn draw_regions(
        &self,
        device: &Device,
        frame: &FrameDesc,
        regions: &[MaterialRegion],
        keys: &[Option<BackdropKey>],
        transient: &mut Transient,
        stats: &mut RenderStats,
    ) -> Result<()> {
        let projection = target_projection(frame.target_size);
        let target = [frame.target_size.0 as f32, frame.target_size.1 as f32];
        let mut uniforms = Vec::with_capacity(regions.len());
        for r in regions {
            let (extent, exponent) = r.shape();
            let mut p: RegionParams = bytemuck::Zeroable::zeroed();
            p.rect = r.rect;
            p.shape = [extent, exponent, r.opacity.clamp(0.0, 1.0), MASK_AA];
            // the backdrop, or the source where the recipe has none, covers the whole target
            p.info = [0.0, 1.0, 0.0, 0.0];
            p.sizes = [target[0], target[1], target[0], target[1]];
            p.tint = [
                frame.tint_color[0],
                frame.tint_color[1],
                frame.tint_color[2],
                1.0,
            ];
            let fills: Vec<Op> = fill_ops(r.recipe);
            debug_assert!(fills.len() <= MAX_FILLS);
            p.set_fills(&fills);
            // the quad covers the mask's antialiased edge past the rectangle
            let pad = 1.0 + MASK_AA;
            let quad = [
                r.rect[0] - pad,
                r.rect[1] - pad,
                r.rect[2] + 2.0 * pad,
                r.rect[3] + 2.0 * pad,
            ];
            let place = Placement::new(&projection, quad, false);
            uniforms.push((
                transient.uniform(device, bytemuck::bytes_of(&p))?,
                transient.uniform(device, bytemuck::bytes_of(&place))?,
            ));
        }
        let blit = if frame.init == TargetInit::CopySource {
            let mut params = PassParams::new(frame.source_size, frame.target_size);
            params.sizes = [
                frame.source_size.0 as i32,
                frame.source_size.1 as i32,
                frame.target_size.0 as i32,
                frame.target_size.1 as i32,
            ];
            let params = transient.uniform(device, bytemuck::bytes_of(&params))?;
            Some(self.pass_group(device, transient, params, frame.source)?)
        } else {
            None
        };
        let mut groups = Vec::with_capacity(regions.len());
        for (key, (region, place)) in keys.iter().zip(&uniforms) {
            let backdrop = key
                .as_ref()
                .and_then(|k| self.cache.get(k))
                .and_then(|e| e.levels.last())
                .unwrap_or(frame.source);
            let g0 = device.create_bind_group(
                &self.layout_region,
                &[
                    BindGroupEntry {
                        binding: 0,
                        resource: transient.whole(*region),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Texture(backdrop),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::Sampler(&self.sampler),
                    },
                ],
            )?;
            let g1 = device.create_bind_group(
                &self.layout_place,
                &[BindGroupEntry {
                    binding: 0,
                    resource: transient.whole(*place),
                }],
            )?;
            groups.push((g0, g1));
        }
        let load = if blit.is_some() {
            LoadOp::Clear([0.0, 0.0, 0.0, 1.0])
        } else {
            LoadOp::Load
        };
        let mut rp = device.begin_pass(frame.target, load)?;
        if let Some(group) = &blit {
            rp.set_pipeline(&self.pipe_blit);
            rp.set_bind_group(0, group)?;
            rp.draw(0..3, 1)?;
        }
        rp.set_pipeline(&self.pipe_material);
        for (g0, g1) in &groups {
            rp.set_bind_group(0, g0)?;
            rp.set_bind_group(1, g1)?;
            rp.draw(0..4, 1)?;
            stats.regions_drawn += 1;
        }
        rp.end();
        Ok(())
    }

    /// Copy `source` into `target` (a render target), bilinear; exact when both have the same size.
    pub fn copy(&self, device: &Device, source: &Texture, target: &Texture) -> Result<()> {
        let mut transient = Transient::default();
        let result = (|| -> Result<()> {
            let (s, t) = (source.size(), target.size());
            let mut params = PassParams::new(s, t);
            params.sizes = [s.0 as i32, s.1 as i32, t.0 as i32, t.1 as i32];
            let params = transient.uniform(device, bytemuck::bytes_of(&params))?;
            let group = self.pass_group(device, &transient, params, source)?;
            let mut rp = device.begin_pass(target, LoadOp::Clear([0.0, 0.0, 0.0, 1.0]))?;
            rp.set_pipeline(&self.pipe_blit);
            rp.set_bind_group(0, &group)?;
            rp.draw(0..3, 1)?;
            rp.end();
            Ok(())
        })();
        transient.destroy(device);
        result
    }

    /// Draw template glyphs / text with the vibrant blend functions (`shaders/vibrant.wgsl`).
    ///
    /// `backdrop` must hold a copy of the current target content (the shader reads the destination
    /// colour from it and overwrites the covered pixels of `target`).
    pub fn draw_vibrant(
        &mut self,
        device: &Device,
        backdrop: &Texture,
        target: &Texture,
        draws: &[VibrantDraw],
    ) -> Result<()> {
        if draws.is_empty() {
            return Ok(());
        }
        let mut transient = Transient::default();
        let result = (|| -> Result<()> {
            let mut groups = Vec::with_capacity(draws.len());
            for d in draws {
                let u = d.uniform(target.size());
                let u = transient.uniform(device, bytemuck::bytes_of(&u))?;
                groups.push(device.create_bind_group(
                    &self.layout_vibrant,
                    &[
                        BindGroupEntry {
                            binding: 0,
                            resource: transient.whole(u),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Texture(d.glyphs),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: BindingResource::Texture(backdrop),
                        },
                        BindGroupEntry {
                            binding: 3,
                            resource: BindingResource::Sampler(&self.sampler),
                        },
                    ],
                )?);
            }
            let mut rp = device.begin_pass(target, LoadOp::Load)?;
            rp.set_pipeline(&self.pipe_vibrant);
            for g in &groups {
                rp.set_bind_group(0, g)?;
                rp.draw(0..4, 1)?;
            }
            rp.end();
            Ok(())
        })();
        transient.destroy(device);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_projection_puts_the_first_row_at_clip_minus_one() {
        let m = target_projection((200, 100));
        let clip = |x: f32, y: f32| (m[0] * x + m[3] * y + m[6], m[1] * x + m[4] * y + m[7]);
        assert_eq!(clip(0.0, 0.0), (-1.0, -1.0));
        assert_eq!(clip(200.0, 100.0), (1.0, 1.0));
        assert_eq!(clip(100.0, 25.0), (0.0, -0.5));
    }
}
