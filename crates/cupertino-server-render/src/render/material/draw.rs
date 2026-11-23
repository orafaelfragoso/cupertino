use super::*;

impl MaterialGl {
    /// Compile the programs on `renderer`'s context. Fails (with a description) when the context
    /// does not provide GLES 3.0 features.
    pub fn new(renderer: &mut GlesRenderer) -> Result<SharedMaterial, String> {
        Self::new_with(renderer, crate::render::programs::dithering())
    }

    /// [`MaterialGl::new`] with the blur (into 8-bit levels), material and glass outputs dithered or not.
    pub fn new_with(renderer: &mut GlesRenderer, dither: bool) -> Result<SharedMaterial, String> {
        let mut me = MaterialGl {
            progs: None,
            ubo_pass: 0,
            ubo_region: 0,
            ubo_place: 0,
            float_targets: false,
            free: Vec::new(),
            cache: Vec::new(),
            tick: 0,
            stats: MaterialStats::default(),
            timing: false,
            gpu_timers: false,
            timers: PassTimers::default(),
            glass_time: Default::default(),
            failure: None,
            glass: glass::GlassGl::default(),
            scratch: bumpalo::Bump::new(),
        };
        let res = renderer
            .with_context(|gl| unsafe { me.init(gl, dither) })
            .map_err(|e| format!("making the context current failed: {e}"))?;
        res?;
        Ok(Rc::new(RefCell::new(me)))
    }

    pub(super) unsafe fn init(&mut self, gl: &ffi::Gles2, dither: bool) -> Result<(), String> {
        let version = {
            let p = gl.GetString(ffi::VERSION);
            if p.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(p as *const _)
                    .to_string_lossy()
                    .into_owned()
            }
        };
        let es3 = version.contains("OpenGL ES 3") || version.contains("OpenGL ES 4");
        if !es3 {
            return Err(format!(
                "material pass needs OpenGL ES 3.0, context reports {version:?}"
            ));
        }
        // levels kept in half floats hold the blur without banding: only 8-bit levels are dithered
        self.float_targets = self.probe_float_targets(gl);
        // the backdrop passes are generated: their block and sampler are bound by the names beside them
        let pass = |program: &Program| -> Result<(GLuint, GLint), String> {
            let name = program.source.name;
            let p = link(gl, program.source.vertex_glsl, program.source.fragment_glsl)
                .map_err(|e| format!("{name}: {e}"))?;
            let block = program
                .block(PASS_BLOCK.0, PASS_BLOCK.1)
                .ok_or_else(|| format!("{name}: no pass block"))?;
            bind_block(gl, p, &block, BIND_PASS)?;
            let sampler = program
                .sampler(PASS_SOURCE.0, PASS_SOURCE.1)
                .ok_or_else(|| format!("{name}: no source sampler"))?;
            Ok((p, uniform_loc(gl, p, &sampler)))
        };
        let (downsample, downsample_src) = pass(&cupertino_glass::shaders::DOWNSAMPLE)?;
        let (blur, blur_src) = pass(&cupertino_glass::shaders::BLUR)?;
        let (color, color_src) = pass(&cupertino_glass::shaders::COLOR)?;
        let program = &cupertino_glass::shaders::MATERIAL;
        let material = link_generated(
            gl,
            program,
            &[(REGION_BLOCK, BIND_REGION), (PLACEMENT, BIND_PLACE)],
        )?;
        let material_backdrop = sampler_loc(gl, material, program, REGION_BACKDROP)?;
        let material_capture = sampler_loc(gl, material, program, REGION_CAPTURE)?;
        self.glass = glass::GlassGl::init(gl)?;
        let src_loc = [downsample_src, blur_src, color_src];
        self.progs = Some(Programs {
            downsample,
            blur,
            color,
            material,
            material_backdrop,
            material_capture,
            src_loc,
            blur_dither: dither && !self.float_targets,
            place_dither: dither,
        });
        let mut bufs = [0; 3];
        gl.GenBuffers(3, bufs.as_mut_ptr());
        self.ubo_pass = bufs[0];
        self.ubo_region = bufs[1];
        self.ubo_place = bufs[2];
        tracing::info!(version = %version, float_targets = self.float_targets, "material pipeline initialised");
        Ok(())
    }

    /// Whether half-float render targets are usable (otherwise 8-bit targets are used).
    pub(super) unsafe fn probe_float_targets(&mut self, gl: &ffi::Gles2) -> bool {
        let saved_fbo = gl_int(gl, ffi::FRAMEBUFFER_BINDING);
        let saved_tex = gl_int(gl, ffi::TEXTURE_BINDING_2D);
        let t = self.alloc_target(gl, (4, 4), true);
        let ok = t.is_some();
        if let Some(t) = t {
            self.destroy_target(gl, t);
        }
        gl.BindFramebuffer(ffi::FRAMEBUFFER, saved_fbo as GLuint);
        gl.BindTexture(ffi::TEXTURE_2D, saved_tex as GLuint);
        ok
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub fn is_ready(&self) -> bool {
        self.progs.is_some() && self.failure.is_none()
    }

    pub(super) unsafe fn alloc_target(
        &mut self,
        gl: &ffi::Gles2,
        size: (i32, i32),
        float: bool,
    ) -> Option<Target> {
        let mut tex = 0;
        gl.GenTextures(1, &mut tex);
        gl.BindTexture(ffi::TEXTURE_2D, tex);
        if float {
            gl.TexImage2D(
                ffi::TEXTURE_2D,
                0,
                ffi::RGBA16F as GLint,
                size.0,
                size.1,
                0,
                ffi::RGBA,
                ffi::HALF_FLOAT,
                std::ptr::null(),
            );
        } else {
            gl.TexImage2D(
                ffi::TEXTURE_2D,
                0,
                ffi::RGBA8 as GLint,
                size.0,
                size.1,
                0,
                ffi::RGBA,
                ffi::UNSIGNED_BYTE,
                std::ptr::null(),
            );
        }
        for (p, v) in [
            (ffi::TEXTURE_MIN_FILTER, ffi::LINEAR),
            (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR),
            (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE),
            (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE),
        ] {
            gl.TexParameteri(ffi::TEXTURE_2D, p, v as GLint);
        }
        let mut fbo = 0;
        gl.GenFramebuffers(1, &mut fbo);
        gl.BindFramebuffer(ffi::FRAMEBUFFER, fbo);
        gl.FramebufferTexture2D(
            ffi::FRAMEBUFFER,
            ffi::COLOR_ATTACHMENT0,
            ffi::TEXTURE_2D,
            tex,
            0,
        );
        if gl.CheckFramebufferStatus(ffi::FRAMEBUFFER) != ffi::FRAMEBUFFER_COMPLETE {
            gl.DeleteFramebuffers(1, &fbo);
            gl.DeleteTextures(1, &tex);
            return None;
        }
        self.stats.allocations += 1;
        Some(Target { tex, fbo, size })
    }

    pub(super) unsafe fn destroy_target(&self, gl: &ffi::Gles2, t: Target) {
        gl.DeleteFramebuffers(1, &t.fbo);
        gl.DeleteTextures(1, &t.tex);
    }

    pub(super) unsafe fn take_target(
        &mut self,
        gl: &ffi::Gles2,
        size: (i32, i32),
    ) -> Option<Target> {
        if let Some(i) = self.free.iter().rposition(|t| t.size == size) {
            return Some(self.free.remove(i));
        }
        self.trim_free(gl);
        let float = self.float_targets;
        self.alloc_target(gl, size, float)
    }

    /// Delete the released targets past what [`free_excess`] keeps, the oldest first.
    pub(super) unsafe fn trim_free(&mut self, gl: &ffi::Gles2) {
        let texel_bytes = if self.float_targets { 8 } else { 4 };
        let sizes: Vec<(i32, i32)> = self.free.iter().map(|t| t.size).collect();
        let excess = free_excess(&sizes, texel_bytes);
        for t in self.free.drain(..excess).collect::<Vec<_>>() {
            self.destroy_target(gl, t);
        }
    }

    /// Write the placement of one quad of a composite: `quad` (x, y, w, h in target pixels) through `projection`.
    pub(super) unsafe fn place(&self, gl: &ffi::Gles2, projection: &[f32; 9], quad: [f32; 4]) {
        let dither = self.progs.as_ref().is_some_and(|p| p.place_dither);
        let placement = Placement::new(projection, quad, dither);
        Self::upload_ubo(
            gl,
            self.ubo_place,
            BIND_PLACE,
            bytemuck::bytes_of(&placement),
        );
    }

    pub(super) unsafe fn upload_ubo(gl: &ffi::Gles2, buf: GLuint, binding: GLuint, data: &[u8]) {
        gl.BindBuffer(ffi::UNIFORM_BUFFER, buf);
        gl.BufferData(
            ffi::UNIFORM_BUFFER,
            data.len() as isize,
            data.as_ptr() as *const _,
            ffi::DYNAMIC_DRAW,
        );
        gl.BindBufferBase(ffi::UNIFORM_BUFFER, binding, buf);
    }

    /// One full-screen pass: `src` texture → `dst` target using `program` and `params`.
    pub(super) unsafe fn pass(
        &mut self,
        gl: &ffi::Gles2,
        program: GLuint,
        src_loc: GLint,
        src: GLuint,
        dst: &Target,
        params: &PassParams,
    ) {
        self.pass_into(gl, program, src_loc, src, dst, dst.size, params);
    }

    /// [`pass`](Self::pass) into the `size` texels at the origin of `dst`, which may be larger.
    #[allow(clippy::too_many_arguments)]
    pub(super) unsafe fn pass_into(
        &mut self,
        gl: &ffi::Gles2,
        program: GLuint,
        src_loc: GLint,
        src: GLuint,
        dst: &Target,
        size: (i32, i32),
        params: &PassParams,
    ) {
        gl.BindFramebuffer(ffi::FRAMEBUFFER, dst.fbo);
        gl.Viewport(0, 0, size.0, size.1);
        gl.UseProgram(program);
        let mut params = *params;
        if self
            .progs
            .as_ref()
            .is_some_and(|p| program == p.blur && p.blur_dither)
        {
            params.flags[0] = PASS_DITHER;
        }
        Self::upload_ubo(gl, self.ubo_pass, BIND_PASS, bytemuck::bytes_of(&params));
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, src);
        gl.Uniform1i(src_loc, 0);
        gl.DrawArrays(ffi::TRIANGLES, 0, 3);
        self.stats.passes += 1;
        if self
            .progs
            .as_ref()
            .is_some_and(|p| program == p.downsample || program == p.blur)
        {
            self.stats.blur_passes += 1;
            self.stats.blur_texels += (size.0 as u64) * (size.1 as u64);
        }
    }

    /// The last level of the pyramid of the `crop` part of the bound framebuffer (`src_fbo`) for `p`'s recipe: the
    /// first level a linear blit (a 2x2 box average at half size), each next one a 2x2 box average of the one before.
    /// For a `part` of a backdrop made again, the level lies at the origin of a target of [`part_room`] sides.
    pub(super) unsafe fn pyramid(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        src_fbo: GLuint,
        crop: Crop,
        part: bool,
    ) -> Option<Target> {
        let (w, h) = p.output_px;
        let mut plan: BlurPlan = self.plan(p);
        if crop != Crop::whole((w, h)) {
            // the same halvings of the part: its edges lie on the grid of the last level
            for (i, level) in plan.levels.iter_mut().enumerate() {
                *level = ((crop.w >> (i + 1)) as u32, (crop.h >> (i + 1)) as u32);
            }
        }
        // a part is made in the corner of a target of a rounded size, which the parts of later frames find again
        let room = |size: (i32, i32)| {
            if part {
                (part_room(size.0), part_room(size.1))
            } else {
                size
            }
        };
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        let first = plan
            .levels
            .first()
            .map(|l| (l.0 as i32, l.1 as i32))
            .unwrap_or((crop.w, crop.h));
        let mut cur = self.take_target(gl, room(first))?;
        let mut cur_size = first;
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, src_fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, cur.fbo);
        let filter = if plan.levels.is_empty() {
            ffi::NEAREST
        } else {
            ffi::LINEAR
        };
        gl.BlitFramebuffer(
            crop.x,
            crop.y,
            crop.x + crop.w,
            crop.y + crop.h,
            0,
            0,
            first.0,
            first.1,
            ffi::COLOR_BUFFER_BIT,
            filter,
        );
        self.stats.passes += 1;
        self.stats.blur_texels += (first.0 as u64) * (first.1 as u64);
        let (ds, ds_loc) = {
            let progs = self.progs.as_ref()?;
            (progs.downsample, progs.src_loc[0])
        };
        for lvl in plan.levels.iter().skip(1) {
            let size = (lvl.0 as i32, lvl.1 as i32);
            let next = self.take_target(gl, room(size))?;
            let params = PassParams::new(
                (cur_size.0 as u32, cur_size.1 as u32),
                (size.0 as u32, size.1 as u32),
            );
            self.pass_into(gl, ds, ds_loc, cur.tex, &next, size, &params);
            self.free.push(cur);
            (cur, cur_size) = (next, size);
        }
        Some(cur)
    }

    /// Whether `p`'s recipe filters its pyramid (a Gaussian or colour operations) or shows it as it is.
    pub(super) fn filters(&self, p: &DrawParams<'_>) -> bool {
        let ops = p.recipe.backdrop.as_ref().is_some_and(|b| !b.2.is_empty());
        ops || self.plan(p).weights.len() > 1
    }

    /// `pyramid` filtered into a new target for `p`'s recipe: the separable Gaussian, then the colour operations. The
    /// pyramid is left as it is. Only for a recipe that [`filters`](Self::filters).
    pub(super) unsafe fn filtered(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        pyramid: &Target,
    ) -> Option<Target> {
        let plan = self.plan(p);
        let ops = p
            .recipe
            .backdrop
            .as_ref()
            .map_or(&[][..], |b| b.2.as_slice());
        let (bl, bl_loc, co, co_loc) = {
            let progs = self.progs.as_ref()?;
            (progs.blur, progs.src_loc[1], progs.color, progs.src_loc[2])
        };
        let size = pyramid.size;
        let dims = (size.0 as u32, size.1 as u32);
        let mut cur: Option<Target> = None;
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        // separable Gaussian
        if plan.weights.len() > 1 {
            let mut params = PassParams::new(dims, dims);
            params.set_weights(&plan.weights);
            let radius = (plan.weights.len() - 1) as i32;
            let tmp = self.take_target(gl, size)?;
            let out = self.take_target(gl, size)?;
            params.dir_radius = [1, 0, radius, 0];
            self.pass(gl, bl, bl_loc, pyramid.tex, &tmp, &params);
            params.dir_radius = [0, 1, radius, 0];
            self.pass(gl, bl, bl_loc, tmp.tex, &out, &params);
            self.free.push(tmp);
            cur = Some(out);
        }
        // colour operations
        if !ops.is_empty() {
            let mut params = PassParams::new(dims, dims);
            params.set_ops(ops);
            let out = self.take_target(gl, size)?;
            let src = cur.as_ref().map_or(pyramid.tex, |c| c.tex);
            self.pass(gl, co, co_loc, src, &out, &params);
            if let Some(c) = cur {
                self.free.push(c);
            }
            cur = Some(out);
        }
        cur
    }

    /// Process the backdrop for `recipe`: capture the `crop` part of the bound framebuffer and filter it. The
    /// backdrop, and the pyramid it was filtered from when it was ([`filters`](Self::filters)), kept so a later
    /// change makes again only the part of it that changed.
    pub(super) unsafe fn process_backdrop(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        src_fbo: GLuint,
        crop: Crop,
    ) -> Option<(Target, Option<Target>)> {
        let pyramid = self.pyramid(gl, p, src_fbo, crop, false)?;
        if !self.filters(p) {
            return Some((pyramid, None));
        }
        let out = self.filtered(gl, p, &pyramid)?;
        Some((out, Some(pyramid)))
    }

    /// The blur of `p`'s recipe over its whole framebuffer.
    pub(super) fn plan(&self, p: &DrawParams<'_>) -> BlurPlan {
        let (w, h) = p.output_px;
        let (scale, sigma) = p
            .recipe
            .backdrop
            .as_ref()
            .map_or((1.0, 0.0), |b| (b.0, b.1));
        plan_blur(w as u32, h as u32, scale, sigma, p.scale)
    }

    /// The part of the framebuffer `p` needs blurred: around its area for a region that blurs what is under it as it
    /// draws, all of it for a backdrop captured below the windows, which another element composites.
    pub(super) fn crop(&self, p: &DrawParams<'_>) -> Crop {
        if !matches!(p.stage, MaterialStage::Full) {
            return Crop::whole(p.output_px);
        }
        let plan = self.plan(p);
        let radius = plan.weights.len().saturating_sub(1);
        backdrop_crop(
            p.output_px,
            plan.levels.len(),
            radius,
            p.area,
            &p.projection,
        )
    }

    /// How the backdrop `old` kept for `p`'s key is brought up to date for `crop`: when its pyramid was made from the
    /// framebuffer as it is now, or as it was at the generation `p` knows the changes since, its pyramid is made again
    /// only where those changes or `crop` reach past it. None to make it whole.
    pub(super) fn reblur_plan(
        &mut self,
        p: &DrawParams<'_>,
        old: &CachedBackdrop,
        crop: Crop,
    ) -> Option<Reblur> {
        // a backdrop that is not filtered is its own pyramid
        if old.pyramid.is_none() && self.filters(p) {
            return None;
        }
        self.scratch.reset();
        let mut changed = bumpalo::collections::Vec::new_in(&self.scratch);
        if old.generation != p.generation {
            let (_, rects) = p.changed.filter(|(since, _)| *since == old.generation)?;
            changed.reserve(rects.len());
            for r in rects {
                changed.push(framebuffer_bounds(p.output_px, *r, &p.projection)?);
            }
        }
        plan_reblur(
            p.output_px,
            self.plan(p).levels.len(),
            old.crop,
            crop,
            &changed,
        )
    }

    /// One texel rectangle `at` of `from` (whose first texel is `from_at`) into `to` (first texel `to_at`).
    pub(super) unsafe fn copy_texels(
        &mut self,
        gl: &ffi::Gles2,
        from: &Target,
        from_at: (i32, i32),
        to: &Target,
        to_at: (i32, i32),
        at: Texels,
    ) {
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, from.fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, to.fbo);
        let (w, h) = (at.x1 - at.x0, at.y1 - at.y0);
        let (sx, sy) = (at.x0 - from_at.0, at.y0 - from_at.1);
        let (dx, dy) = (at.x0 - to_at.0, at.y0 - to_at.1);
        gl.BlitFramebuffer(
            sx,
            sy,
            sx + w,
            sy + h,
            dx,
            dy,
            dx + w,
            dy + h,
            ffi::COLOR_BUFFER_BIT,
            ffi::NEAREST,
        );
        self.stats.passes += 1;
    }

    /// The backdrop for `crop` made from `old`'s: its pyramid's texels `plan` keeps copied, each of its parts made
    /// again from its own pixels of the framebuffer, and the whole filtered as after a whole capture (when the recipe
    /// filters it; otherwise the pyramid is the backdrop).
    pub(super) unsafe fn reblur(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
        src_fbo: GLuint,
        old: CachedBackdrop,
        crop: Crop,
        plan: &Reblur,
    ) -> Option<(Target, Option<Target>)> {
        let factor = 1i32 << self.plan(p).levels.len().min(16);
        let (new_t, old_t) = (Texels::of(crop, factor), Texels::of(old.crop, factor));
        let at = (new_t.x0, new_t.y0);
        let old_pyramid = match old.pyramid {
            Some(pyramid) => {
                self.free.push(old.target);
                pyramid
            }
            None => old.target,
        };
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        let pyramid = if old.crop == crop {
            old_pyramid
        } else {
            let Some(pyramid) = self.take_target(gl, (new_t.x1 - new_t.x0, new_t.y1 - new_t.y0))
            else {
                self.free.push(old_pyramid);
                return None;
            };
            if let Some(kept) = plan.kept {
                self.copy_texels(gl, &old_pyramid, (old_t.x0, old_t.y0), &pyramid, at, kept);
            }
            self.free.push(old_pyramid);
            pyramid
        };
        for &part in &plan.parts {
            let Some(made) = self.pyramid(gl, p, src_fbo, part.crop(factor), true) else {
                self.free.push(pyramid);
                return None;
            };
            self.copy_texels(gl, &made, (part.x0, part.y0), &pyramid, at, part);
            self.free.push(made);
        }
        if !self.filters(p) {
            self.stats.reblurs += 1;
            return Some((pyramid, None));
        }
        let Some(out) = self.filtered(gl, p, &pyramid) else {
            self.free.push(pyramid);
            return None;
        };
        self.stats.reblurs += 1;
        Some((out, Some(pyramid)))
    }

    /// Composite one region. The target framebuffer must be bound as draw framebuffer.
    ///
    /// # Safety
    /// Must be called with the renderer's context current (inside `GlesFrame::with_context`).
    pub unsafe fn draw(&mut self, gl: &ffi::Gles2, p: &DrawParams<'_>) -> Result<(), String> {
        if !self.timing || self.gpu_timers {
            return self.draw_now(gl, p);
        }
        gl.Finish();
        let start = std::time::Instant::now();
        let result = self.draw_now(gl, p);
        gl.Finish();
        self.glass_time.total += start.elapsed();
        result
    }

    pub(super) unsafe fn draw_now(
        &mut self,
        gl: &ffi::Gles2,
        p: &DrawParams<'_>,
    ) -> Result<(), String> {
        if self.progs.is_none() {
            return Err(self
                .failure
                .clone()
                .unwrap_or_else(|| "material pipeline not initialised".into()));
        }
        self.tick += 1;
        let saved = SavedState::capture(gl);
        let src_fbo = saved.draw_fbo as GLuint;
        let key = backdrop_key(p.recipe.key, p.output_px, p.scale, p.stage, p.element);
        let crop = self.crop(p);
        let hit = self
            .cache
            .iter()
            .position(|c| c.key == key && c.generation == p.generation && c.crop.contains(&crop));
        let idx = match hit {
            Some(i) => {
                self.stats.backdrop_hits += 1;
                i
            }
            None if matches!(p.stage, MaterialStage::Composite { .. }) => {
                saved.restore(gl);
                let msg = "the backdrop captured below the windows is missing".to_string();
                self.failure = Some(msg.clone());
                return Err(msg);
            }
            None => {
                self.stats.backdrop_misses += 1;
                let old = self
                    .cache
                    .iter()
                    .position(|c| c.key == key)
                    .map(|i| self.cache.swap_remove(i));
                let reblur = old.as_ref().and_then(|old| self.reblur_plan(p, old, crop));
                let blurring = (self.timing && !self.gpu_timers).then(std::time::Instant::now);
                self.timer_begin(gl, true);
                let processed = match (old, reblur) {
                    (Some(old), Some(plan)) => self.reblur(gl, p, src_fbo, old, crop, &plan),
                    (old, _) => {
                        if let Some(old) = old {
                            self.free.extend(old.targets());
                        }
                        self.process_backdrop(gl, p, src_fbo, crop)
                    }
                };
                self.timer_end(gl);
                if let Some(blurring) = blurring {
                    gl.Finish();
                    self.glass_time.blur += blurring.elapsed();
                }
                let Some((target, pyramid)) = processed else {
                    saved.restore(gl);
                    let msg = "backdrop processing failed (framebuffer incomplete?)".to_string();
                    self.failure = Some(msg.clone());
                    return Err(msg);
                };
                if self.cache.len() >= 8 {
                    // a capture is only redrawn with its scene below, so evicting it would leave its
                    // composite without a backdrop: evict the other backdrops first
                    let oldest = (0..self.cache.len())
                        .min_by_key(|i| (self.cache[*i].key.4.is_some(), self.cache[*i].last_used))
                        .unwrap();
                    let old = self.cache.swap_remove(oldest);
                    self.free.extend(old.targets());
                }
                self.cache.push(CachedBackdrop {
                    key,
                    generation: p.generation,
                    target,
                    pyramid,
                    crop,
                    last_used: self.tick,
                });
                self.cache.len() - 1
            }
        };
        self.cache[idx].last_used = self.tick;
        if matches!(p.stage, MaterialStage::Capture { .. }) {
            saved.restore(gl);
            return Ok(());
        }
        let backdrop_tex = self.cache[idx].target.tex;
        // the capture before the filter, which a blur ramp shows where its weight is low: the backdrop itself when the
        // recipe does not filter
        let capture_tex = self.cache[idx]
            .pyramid
            .as_ref()
            .map_or(backdrop_tex, |p| p.tex);
        let held = self.cache[idx].crop;
        self.timer_begin(gl, false);

        let progs = self.progs.as_ref().unwrap();
        let mut region: RegionParams = bytemuck::Zeroable::zeroed();
        region.rect = [
            p.rect.loc.x as f32,
            p.rect.loc.y as f32,
            p.rect.size.w as f32,
            p.rect.size.h as f32,
        ];
        region.shape = [p.corner.0, p.corner.1, p.opacity, 1.0];
        // the backdrop covers the part of the framebuffer it was blurred from: its origin and size
        region.info = [0.0, 1.0, held.x as f32, held.y as f32];
        region.sizes = [
            p.output_px.0 as f32,
            p.output_px.1 as f32,
            held.w as f32,
            held.h as f32,
        ];
        region.tint = [p.tint[0], p.tint[1], p.tint[2], 1.0];
        region.set_fills(&p.recipe.fills);
        // the ramp runs along the region's own rectangle, also where its mask is a window's outline
        region.set_blur_ramp(p.blur_ramp, p.area.loc.y as f32, p.area.size.h as f32);

        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, saved.draw_fbo as GLuint);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, saved.read_fbo as GLuint);
        gl.Viewport(
            saved.viewport[0],
            saved.viewport[1],
            saved.viewport[2],
            saved.viewport[3],
        );
        if saved.scissor {
            gl.Enable(ffi::SCISSOR_TEST);
        }
        gl.Enable(ffi::BLEND);
        gl.BlendFuncSeparate(
            ffi::SRC_ALPHA,
            ffi::ONE_MINUS_SRC_ALPHA,
            ffi::ZERO,
            ffi::ONE,
        );
        gl.UseProgram(progs.material);
        Self::upload_ubo(
            gl,
            self.ubo_region,
            BIND_REGION,
            bytemuck::bytes_of(&region),
        );
        gl.ActiveTexture(ffi::TEXTURE1);
        gl.BindTexture(ffi::TEXTURE_2D, capture_tex);
        gl.Uniform1i(progs.material_capture, 1);
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, backdrop_tex);
        gl.Uniform1i(progs.material_backdrop, 0);
        // a pixel more for the mask's antialiased edge, where the mask is the region's own; a region cut to a window
        // ends at its own rectangle
        let grow = i32::from(p.area == p.rect);
        let bounds = Rectangle::new(
            (p.area.loc.x - grow, p.area.loc.y - grow).into(),
            (p.area.size.w + 2 * grow, p.area.size.h + 2 * grow).into(),
        );
        for d in p.damage {
            let Some(q) = d.intersection(bounds) else {
                continue;
            };
            self.place(
                gl,
                &p.projection,
                [
                    q.loc.x as f32,
                    q.loc.y as f32,
                    q.size.w as f32,
                    q.size.h as f32,
                ],
            );
            gl.DrawArrays(ffi::TRIANGLE_STRIP, 0, 4);
            self.stats.composites += 1;
        }
        self.timer_end(gl);
        saved.restore(gl);
        Ok(())
    }

    /// Start timing a pass on the GPU (the benchmark with timer queries only); `blur` for a backdrop blur.
    pub(super) unsafe fn timer_begin(&mut self, gl: &ffi::Gles2, blur: bool) {
        if !(self.timing && self.gpu_timers) || self.timers.open.is_some() {
            return;
        }
        let q = self.timers.free.pop().unwrap_or_else(|| {
            let mut q = 0;
            gl.GenQueries(1, &mut q);
            q
        });
        gl.BeginQuery(TIME_ELAPSED, q);
        self.timers.open = Some((q, blur));
    }

    /// End the pass timed by [`MaterialGl::timer_begin`].
    pub(super) unsafe fn timer_end(&mut self, gl: &ffi::Gles2) {
        if let Some(open) = self.timers.open.take() {
            gl.EndQuery(TIME_ELAPSED);
            self.timers.pending.push(open);
        }
    }

    /// Once the frame's GPU work finished: add what the timed passes took to [`MaterialGl::glass_time`].
    ///
    /// # Safety
    /// Must be called with the renderer's context current.
    pub unsafe fn collect_pass_times(&mut self, gl: &ffi::Gles2) {
        self.timer_end(gl);
        for (q, blur) in std::mem::take(&mut self.timers.pending) {
            let mut ns: GLuint = 0;
            gl.GetQueryObjectuiv(q, ffi::QUERY_RESULT, &mut ns);
            let took = std::time::Duration::from_nanos(u64::from(ns));
            if blur {
                self.glass_time.blur += took;
            }
            self.glass_time.total += took;
            self.timers.free.push(q);
        }
    }

    /// Release all GL objects. Call with the renderer's context current.
    pub fn destroy(&mut self, renderer: &mut GlesRenderer) {
        let mut free = std::mem::take(&mut self.free);
        free.extend(self.glass.targets());
        let cache = std::mem::take(&mut self.cache);
        let progs = self.progs.take();
        let bufs = [self.ubo_pass, self.ubo_region, self.ubo_place];
        let _ = renderer.with_context(|gl| unsafe {
            for t in free
                .into_iter()
                .chain(cache.into_iter().flat_map(CachedBackdrop::targets))
            {
                gl.DeleteFramebuffers(1, &t.fbo);
                gl.DeleteTextures(1, &t.tex);
            }
            if let Some(p) = progs {
                for prog in [p.downsample, p.blur, p.color, p.material] {
                    gl.DeleteProgram(prog);
                }
            }
            gl.DeleteBuffers(3, bufs.as_ptr());
            for q in self
                .timers
                .free
                .drain(..)
                .chain(self.timers.pending.drain(..).map(|(q, _)| q))
            {
                gl.DeleteQueries(1, &q);
            }
            self.glass.destroy(gl);
        });
    }
}
