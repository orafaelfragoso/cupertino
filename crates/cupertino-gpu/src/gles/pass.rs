use super::*;

impl Device {
    /// Start a render pass into `target`, cleared or kept.
    pub fn begin_pass<'a>(&'a self, target: &Texture, load: LoadOp) -> Result<Pass<'a>> {
        self.ready()?;
        let fb = target
            .framebuffer
            .ok_or_else(|| Error::Invalid("the target is not a render target".into()))?;
        let (w, h) = target.desc.size;
        let gl = &self.gl;
        self.touch(
            Touched::FRAMEBUFFERS
                .with(Touched::VIEWPORT)
                .with(Touched::SCISSOR)
                .with(Touched::TESTS)
                .with(Touched::BLEND)
                .with(Touched::MASKS),
        );
        // Safety: a complete framebuffer of this device; the pass's fixed state set before any draw.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fb));
            gl.viewport(0, 0, w as i32, h as i32);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.color_mask(true, true, true, true);
            if let LoadOp::Clear([r, g, b, a]) = load {
                gl.clear_color(r, g, b, a);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        Ok(Pass {
            device: self,
            size: (w, h),
            pipeline: None,
            attributes: Vec::new(),
            index: false,
            owners: false,
        })
    }

    /// Start a render pass into the framebuffer the owner of a borrowed device has bound, at the viewport the owner
    /// set, keeping what the framebuffer holds: how a borrowed device draws among its owner's own draws.
    ///
    /// The pass sets the fixed state of every pass (no scissor, depth, stencil, culling or blending, every channel
    /// written) and leaves the framebuffer and the viewport as the owner has them. The framebuffer's orientation is
    /// the owner's: a pipeline places its vertices through the projection the owner gives it, and
    /// [`Pass::set_scissor`] regions and fragment positions are the framebuffer's pixels as the owner's context
    /// counts them. [`Pass::size`] is the viewport's size. [`Device::leave`] puts back all the state the pass
    /// changed, the owner's framebuffer and viewport among it.
    pub fn begin_owner_pass(&self) -> Result<Pass<'_>> {
        if !self.borrowed {
            return Err(Error::Invalid(
                "only a borrowed device draws into its owner's framebuffer".into(),
            ));
        }
        self.ready()?;
        let gl = &self.gl;
        // the framebuffer and viewport entering read: the owner's, bound again if a pass of the device's own
        // changed them since
        let (framebuffer, viewport) = self.owner_target.get();
        let moved = self
            .touched
            .get()
            .has(Touched::FRAMEBUFFERS.with(Touched::VIEWPORT));
        self.touch(
            Touched::SCISSOR
                .with(Touched::TESTS)
                .with(Touched::BLEND)
                .with(Touched::MASKS),
        );
        // Safety: the owner's context is current and entered; its framebuffer stays bound, the pass's fixed state is
        // set before any draw and restored on leave.
        unsafe {
            if moved {
                gl.bind_framebuffer(glow::FRAMEBUFFER, framebuffer);
                let [x, y, w, h] = viewport;
                gl.viewport(x, y, w, h);
            }
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::STENCIL_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.color_mask(true, true, true, true);
        }
        Ok(Pass {
            device: self,
            size: (viewport[2].max(0) as u32, viewport[3].max(0) as u32),
            pipeline: None,
            attributes: Vec::new(),
            index: false,
            owners: true,
        })
    }

    /// Submit the work recorded so far: a fence that signals when it is done.
    pub fn submit(&self) -> Result<Fence> {
        self.ready()?;
        // Safety: a current context.
        unsafe {
            let raw = self
                .gl
                .fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0)
                .map_err(Error::Driver)?;
            self.gl.flush();
            Ok(Fence { raw })
        }
    }

    /// Submit the work recorded so far with a native fence too: the sync file to hand to the compositor or another
    /// device, which signals when the work is done.
    pub fn submit_exported(&self) -> Result<(Fence, OwnedFd)> {
        self.ready()?;
        // Safety: a current context.
        let raw = unsafe { self.gl.fence_sync(glow::SYNC_GPU_COMMANDS_COMPLETE, 0) }
            .map_err(Error::Driver)?;
        let fence = Fence { raw };
        // Safety: a current context; the native fence is made before the flush that submits it.
        match self.ctx.native_fence(|| unsafe { self.gl.flush() }) {
            Ok(fd) => Ok((fence, fd)),
            Err(e) => {
                self.destroy_fence(fence);
                Err(e)
            }
        }
    }

    /// Make the work submitted after this wait on the GPU for the sync file `fence` (another device's or process's
    /// work), without waiting on the CPU.
    pub fn wait_sync_file(&self, fence: OwnedFd) -> Result<()> {
        self.ready()?;
        self.ctx.wait_native_fence(fence)
    }

    /// Wait on the CPU up to `timeout` for `fence`; whether it signalled.
    pub fn wait(&self, fence: &Fence, timeout: std::time::Duration) -> Result<bool> {
        self.ready()?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            // the GL timeout is a signed 32-bit count of nanoseconds here
            let step = left.as_nanos().min(i32::MAX as u128) as i32;
            // Safety: a fence of this device.
            let r = unsafe {
                self.gl
                    .client_wait_sync(fence.raw, glow::SYNC_FLUSH_COMMANDS_BIT, step)
            };
            match r {
                glow::ALREADY_SIGNALED | glow::CONDITION_SATISFIED => return Ok(true),
                glow::TIMEOUT_EXPIRED if left.is_zero() => return Ok(false),
                glow::TIMEOUT_EXPIRED => {}
                _ => {
                    self.status()?;
                    return Err(Error::Driver("glClientWaitSync failed".into()));
                }
            }
        }
    }

    /// Whether `fence` has signalled, without waiting.
    pub fn is_signalled(&self, fence: &Fence) -> Result<bool> {
        self.wait(fence, std::time::Duration::ZERO)
    }

    pub fn destroy_fence(&self, fence: Fence) {
        if self.ready().is_ok() {
            // Safety: a fence of this device, destroyed once.
            unsafe { self.gl.delete_sync(fence.raw) };
        }
    }

    /// Whether [`Device::gpu_time`] can time the GPU's own work.
    pub fn times_gpu_work(&self) -> bool {
        self.timer_query
    }

    /// Run `work` (recording and submitting on this device) and time what the GPU spends on it, with a timer query,
    /// waiting for its result: the GPU's own time, which other work on the machine does not stretch the way it
    /// stretches the time until the work is finished. None when the driver cannot time its work
    /// ([`Device::times_gpu_work`]) or the GPU's timing was disrupted on the way (a change of its clock, say).
    pub fn gpu_time<T>(
        &self,
        work: impl FnOnce() -> T,
    ) -> Result<(T, Option<std::time::Duration>)> {
        /// The state that says a disruption made the timer's result meaningless since it was last read.
        const GPU_DISJOINT: u32 = 0x8FBB;
        self.ready()?;
        if !self.timer_query {
            return Ok((work(), None));
        }
        // Safety: a current context with the timer query extension; one query, begun once and ended below.
        let query = unsafe {
            let query = self.gl.create_query().map_err(Error::Driver)?;
            // reading the state clears a disruption seen before this work
            self.gl.get_parameter_i32(GPU_DISJOINT);
            self.gl.begin_query(glow::TIME_ELAPSED, query);
            query
        };
        let out = work();
        self.ready()?;
        // Safety: the context current again; the query begun above, ended and deleted once.
        let (elapsed, disjoint) = unsafe {
            self.gl.end_query(glow::TIME_ELAPSED);
            let elapsed = self.gl.get_query_parameter_u32(query, glow::QUERY_RESULT);
            let disjoint = self.gl.get_parameter_i32(GPU_DISJOINT) != 0;
            self.gl.delete_query(query);
            (elapsed, disjoint)
        };
        self.status()?;
        Ok((
            out,
            (!disjoint).then(|| std::time::Duration::from_nanos(u64::from(elapsed))),
        ))
    }

    /// Wait on the CPU for all work submitted so far.
    pub fn finish(&self) -> Result<()> {
        self.ready()?;
        // Safety: a current context.
        unsafe { self.gl.finish() };
        Ok(())
    }
}

/// Wait on the CPU up to `timeout` for a sync file; whether it signalled.
pub fn wait_sync_file(fence: &OwnedFd, timeout: std::time::Duration) -> bool {
    let mut pfd = PollFd {
        fd: fence.as_raw_fd(),
        events: POLLIN,
        revents: 0,
    };
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // Safety: one valid pollfd for an open file descriptor.
    unsafe { poll(&mut pfd, 1, ms) == 1 && pfd.revents & POLLIN != 0 }
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
const POLLIN: i16 = 1;
extern "C" {
    fn poll(fds: *mut PollFd, nfds: u64, timeout: i32) -> i32;
}

/// A render pass being recorded: pipelines, bind groups, vertex buffers and draws into its target.
pub struct Pass<'a> {
    device: &'a Device,
    size: (u32, u32),
    pipeline: Option<&'a Pipeline>,
    attributes: Vec<u32>,
    index: bool,
    /// The target is the framebuffer the owner bound ([`Device::begin_owner_pass`]), which stays bound.
    owners: bool,
}

impl<'a> Pass<'a> {
    pub fn set_pipeline(&mut self, pipeline: &'a Pipeline) {
        let gl = &self.device.gl;
        self.device.touch(Touched::PROGRAM.with(Touched::BLEND));
        // Safety: a program of this device; blending as the pipeline holds it.
        unsafe {
            gl.use_program(Some(pipeline.program));
            match pipeline.blend {
                Blend::Replace => gl.disable(glow::BLEND),
                Blend::PremultipliedOver => {
                    gl.enable(glow::BLEND);
                    gl.blend_equation(glow::FUNC_ADD);
                    gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                }
                Blend::StraightOver => {
                    gl.enable(glow::BLEND);
                    gl.blend_equation(glow::FUNC_ADD);
                    gl.blend_func_separate(
                        glow::SRC_ALPHA,
                        glow::ONE_MINUS_SRC_ALPHA,
                        glow::ZERO,
                        glow::ONE,
                    );
                }
            }
        }
        self.pipeline = Some(pipeline);
    }

    /// Bind `group` at `index` of the current pipeline's layouts.
    pub fn set_bind_group(&mut self, index: u32, group: &BindGroup) -> Result<()> {
        let pipeline = self
            .pipeline
            .ok_or_else(|| Error::Invalid("no pipeline set".into()))?;
        if index as usize >= pipeline.layouts.len() {
            return Err(Error::Invalid(format!("the pipeline has no group {index}")));
        }
        let gl = &self.device.gl;
        self.device.touch(Touched::BUFFERS.with(Touched::TEXTURES));
        for &(binding, bound) in &group.entries {
            let slot = binding_index(index, binding)?;
            // Safety: resources of this device bound at the points the pipeline's program reads.
            unsafe {
                match bound {
                    Bound::Buffer { raw, offset, size } => gl.bind_buffer_range(
                        glow::UNIFORM_BUFFER,
                        slot,
                        Some(raw),
                        offset as i32,
                        size as i32,
                    ),
                    Bound::Texture(raw) => {
                        gl.active_texture(glow::TEXTURE0 + slot);
                        gl.bind_texture(glow::TEXTURE_2D, Some(raw));
                        // a texture read without a sampler uses its own parameters
                        if pipeline
                            .textures
                            .iter()
                            .any(|t| (t.0, t.1, t.2) == (index, binding, None))
                        {
                            gl.bind_sampler(slot, None);
                        }
                    }
                    Bound::Sampler(raw) => {
                        for t in pipeline
                            .textures
                            .iter()
                            .filter(|t| t.2 == Some((index, binding)))
                        {
                            gl.bind_sampler(binding_index(t.0, t.1)?, Some(raw));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Read vertex buffer slot `slot` of the pipeline from `buffer` at `offset`.
    pub fn set_vertex_buffer(&mut self, slot: u32, buffer: &Buffer, offset: u64) -> Result<()> {
        let pipeline = self
            .pipeline
            .ok_or_else(|| Error::Invalid("no pipeline set".into()))?;
        let layout = pipeline
            .vertex_buffers
            .get(slot as usize)
            .ok_or_else(|| Error::Invalid(format!("the pipeline has no vertex buffer {slot}")))?;
        let gl = &self.device.gl;
        self.device.touch(Touched::BUFFERS);
        // Safety: the device's own vertex array is bound; attributes point into a buffer of this device.
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer.raw));
            for a in &layout.attributes {
                gl.enable_vertex_attrib_array(a.location);
                let at = (offset + a.offset as u64) as i32;
                match a.format {
                    VertexFormat::Float32x2 => gl.vertex_attrib_pointer_f32(
                        a.location,
                        2,
                        glow::FLOAT,
                        false,
                        layout.stride as i32,
                        at,
                    ),
                    VertexFormat::Float32x4 => gl.vertex_attrib_pointer_f32(
                        a.location,
                        4,
                        glow::FLOAT,
                        false,
                        layout.stride as i32,
                        at,
                    ),
                    VertexFormat::Unorm8x4 => gl.vertex_attrib_pointer_f32(
                        a.location,
                        4,
                        glow::UNSIGNED_BYTE,
                        true,
                        layout.stride as i32,
                        at,
                    ),
                }
                gl.vertex_attrib_divisor(
                    a.location,
                    matches!(layout.step, StepMode::Instance) as u32,
                );
                self.attributes.push(a.location);
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        Ok(())
    }

    /// Read 16-bit indices from `buffer`.
    pub fn set_index_buffer(&mut self, buffer: &Buffer) -> Result<()> {
        if buffer.usage != BufferUsage::Index {
            return Err(Error::Invalid("not an index buffer".into()));
        }
        // Safety: the device's own vertex array holds the binding.
        unsafe {
            self.device
                .gl
                .bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(buffer.raw))
        };
        self.index = true;
        Ok(())
    }

    /// Draw only inside `region` (pixels from the first row; in a pass into the owner's framebuffer, its pixels as
    /// the owner's context counts them), or everywhere.
    pub fn set_scissor(&mut self, region: Option<Region>) {
        let gl = &self.device.gl;
        self.device.touch(Touched::SCISSOR);
        // Safety: plain state of the pass; fragment row 0 is the target's first row, so no flip.
        unsafe {
            match region {
                Some(r) => {
                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(r.x as i32, r.y as i32, r.width as i32, r.height as i32);
                }
                None => gl.disable(glow::SCISSOR_TEST),
            }
        }
    }

    fn mode(&self) -> Result<u32> {
        let p = self
            .pipeline
            .ok_or_else(|| Error::Invalid("no pipeline set".into()))?;
        Ok(match p.topology {
            Topology::TriangleList => glow::TRIANGLES,
            Topology::TriangleStrip => glow::TRIANGLE_STRIP,
        })
    }

    /// Draw `vertices` vertices of `instances` instances (instance buffers start at their bound offset).
    pub fn draw(&mut self, vertices: std::ops::Range<u32>, instances: u32) -> Result<()> {
        let mode = self.mode()?;
        // Safety: a pipeline is set and its inputs bound.
        unsafe {
            self.device.gl.draw_arrays_instanced(
                mode,
                vertices.start as i32,
                vertices.len() as i32,
                instances as i32,
            )
        };
        Ok(())
    }

    /// Draw `count` indices from the index buffer, `instances` times.
    pub fn draw_indexed(&mut self, count: u32, instances: u32) -> Result<()> {
        let mode = self.mode()?;
        if !self.index {
            return Err(Error::Invalid("no index buffer set".into()));
        }
        // Safety: a pipeline is set, its inputs and an index buffer bound.
        unsafe {
            self.device.gl.draw_elements_instanced(
                mode,
                count as i32,
                glow::UNSIGNED_SHORT,
                0,
                instances as i32,
            )
        };
        Ok(())
    }

    /// The target's size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// End the pass.
    pub fn end(self) {}
}

impl Drop for Pass<'_> {
    fn drop(&mut self) {
        let gl = &self.device.gl;
        // Safety: the device's own vertex array: the attributes the pass enabled are disabled again.
        unsafe {
            for &a in &self.attributes {
                gl.vertex_attrib_divisor(a, 0);
                gl.disable_vertex_attrib_array(a);
            }
            if self.index {
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, None);
            }
            if !self.owners {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            }
        }
    }
}
