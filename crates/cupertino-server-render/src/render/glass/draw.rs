use super::*;

impl MaterialGl {
    /// `src` blurred with a Gaussian of `sigma_pt` points at `px_per_pt` texels per point, into a new target; opaque as a
    /// backdrop, or keeping the alpha of `src` with `keep_alpha`.
    unsafe fn blurred(
        &mut self,
        gl: &ffi::Gles2,
        src: (GLuint, (i32, i32)),
        sigma_pt: f32,
        px_per_pt: f32,
        keep_alpha: bool,
    ) -> Option<Target> {
        let _blur =
            tracing::trace_span!("server.blur", width = src.1 .0, height = src.1 .1).entered();
        let alpha = if keep_alpha { PASS_KEEP_ALPHA } else { 0 };
        let (ds, ds_loc, bl, bl_loc) = {
            let progs = self.progs.as_ref()?;
            (
                progs.downsample,
                progs.src_loc[0],
                progs.blur,
                progs.src_loc[1],
            )
        };
        let plan = plan_blur(src.1 .0 as u32, src.1 .1 as u32, 1.0, sigma_pt, px_per_pt);
        let (mut tex, mut size) = src;
        let mut owned: Option<Target> = None;
        for lvl in &plan.levels {
            let next_size = (lvl.0 as i32, lvl.1 as i32);
            let next = self.take_target(gl, next_size)?;
            let mut params = PassParams::new(
                (size.0 as u32, size.1 as u32),
                (next_size.0 as u32, next_size.1 as u32),
            );
            params.flags[1] = alpha;
            self.pass(gl, ds, ds_loc, tex, &next, &params);
            (tex, size) = (next.tex, next.size);
            if let Some(o) = owned.replace(next) {
                self.free.push(o);
            }
        }
        let mut params = PassParams::new(
            (size.0 as u32, size.1 as u32),
            (size.0 as u32, size.1 as u32),
        );
        params.set_weights(&plan.weights);
        params.flags[1] = alpha;
        let radius = plan.weights.len() as i32 - 1;
        let tmp = self.take_target(gl, size)?;
        let out = self.take_target(gl, size)?;
        params.dir_radius = [1, 0, radius, 0];
        self.pass(gl, bl, bl_loc, tex, &tmp, &params);
        params.dir_radius = [0, 1, radius, 0];
        self.pass(gl, bl, bl_loc, tmp.tex, &out, &params);
        self.free.push(tmp);
        if let Some(o) = owned {
            self.free.push(o);
        }
        Some(out)
    }

    /// The texture `src` of `size` texels blurred by a Gaussian of `sigma_pt` points at `px_per_pt` texels a point, as
    /// [`blurred`](Self::blurred) makes it, and stretched back to `size` into the texture `dst`: a layer's content blur
    /// (`crate::render::content_blur`). The GL state is as it was after it; false when the passes are unavailable.
    pub(crate) unsafe fn blur_texture(
        &mut self,
        gl: &ffi::Gles2,
        src: GLuint,
        dst: GLuint,
        size: (i32, i32),
        sigma_pt: f32,
        px_per_pt: f32,
    ) -> bool {
        if self.progs.is_none() {
            return false;
        }
        let saved = SavedState::capture(gl);
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        // a texture made as a render target has no sampling state of its own yet: without it the passes read nothing
        gl.ActiveTexture(ffi::TEXTURE0);
        gl.BindTexture(ffi::TEXTURE_2D, src);
        for (name, value) in [
            (ffi::TEXTURE_MIN_FILTER, ffi::LINEAR),
            (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR),
            (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE),
            (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE),
        ] {
            gl.TexParameteri(ffi::TEXTURE_2D, name, value as i32);
        }
        let done = match self.blurred(gl, (src, size), sigma_pt, px_per_pt, true) {
            Some(out) => {
                let mut fbo = 0;
                gl.GenFramebuffers(1, &mut fbo);
                gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, fbo);
                gl.FramebufferTexture2D(
                    ffi::DRAW_FRAMEBUFFER,
                    ffi::COLOR_ATTACHMENT0,
                    ffi::TEXTURE_2D,
                    dst,
                    0,
                );
                gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, out.fbo);
                gl.BlitFramebuffer(
                    0,
                    0,
                    out.size.0,
                    out.size.1,
                    0,
                    0,
                    size.0,
                    size.1,
                    ffi::COLOR_BUFFER_BIT,
                    ffi::LINEAR,
                );
                gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                gl.DeleteFramebuffers(1, &fbo);
                self.stats.passes += 1;
                self.free.push(out);
                true
            }
            None => false,
        };
        saved.restore(gl);
        done
    }

    /// Capture framebuffer rectangle `fb` of `src_fbo` at the recipe's capture scale and blur it into the copies (one per distinct
    /// deviation of its shapes' blur), the blur fill and the edge bleed's blur `glass` reads. With `within`, a framebuffer rectangle, the capture outside it repeats its edge pixels.
    unsafe fn process_glass(
        &mut self,
        gl: &ffi::Gles2,
        src_fbo: GLuint,
        fb: [i32; 4],
        within: Option<[i32; 4]>,
        glass: &Container,
        scale: f32,
    ) -> Option<(Vec<Target>, [usize; GLASS_SLOTS])> {
        gl.Disable(ffi::SCISSOR_TEST);
        gl.Disable(ffi::BLEND);
        let size = capture_size(fb, glass.params.capture_scale);
        let capture = self.take_target(gl, size)?;
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, src_fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, capture.fbo);
        gl.BlitFramebuffer(
            fb[0],
            fb[1],
            fb[0] + fb[2],
            fb[1] + fb[3],
            0,
            0,
            size.0,
            size.1,
            ffi::COLOR_BUFFER_BIT,
            ffi::LINEAR,
        );
        self.stats.passes += 1;
        if let Some(within) = within {
            for (src, dst) in edge_blits(fb, size, within) {
                gl.BlitFramebuffer(
                    src[0],
                    src[1],
                    src[2],
                    src[3],
                    dst[0],
                    dst[1],
                    dst[2],
                    dst[3],
                    ffi::COLOR_BUFFER_BIT,
                    ffi::NEAREST,
                );
                self.stats.passes += 1;
            }
        }
        // texels of the capture per point
        let px_per_pt = scale * size.0 as f32 / fb[2] as f32;
        let src = (capture.tex, capture.size);
        let mut targets = vec![capture];
        let mut slots = [usize::MAX; GLASS_SLOTS];
        for (i, (sigma, used)) in glass.levels().into_iter().enumerate() {
            if used {
                targets.push(self.blurred(gl, src, sigma, px_per_pt, false)?);
                slots[i] = targets.len() - 1;
            }
        }
        if let Some(sigma) = glass.fill_sigma() {
            targets.push(self.blurred(gl, src, sigma, px_per_pt, false)?);
            slots[FILL_LEVEL] = targets.len() - 1;
        }
        // the edge bleed reads a copy blurred already where one has its deviation, else one of its own
        match glass.bleed_source() {
            Some(BleedSource::Level(i)) => slots[BLEED_LEVEL] = slots[i],
            Some(BleedSource::Fill) => slots[BLEED_LEVEL] = slots[FILL_LEVEL],
            Some(BleedSource::Own(sigma)) => {
                targets.push(self.blurred(gl, src, sigma, px_per_pt, false)?);
                slots[BLEED_LEVEL] = targets.len() - 1;
            }
            None => {}
        }
        // the bend outside the edge reads the capture itself
        if glass.reads_sharp() {
            slots[SHARP_LEVEL] = 0;
        }
        // a level no shape reads has no weight: any texture will do
        let any = slots
            .iter()
            .copied()
            .find(|s| *s != usize::MAX)
            .unwrap_or(0);
        for s in &mut slots {
            if *s == usize::MAX {
                *s = any;
            }
        }
        // the capture itself is read only through the levels made from it
        if !slots.contains(&0) {
            self.free.push(targets.swap_remove(0));
            let last = targets.len();
            for s in &mut slots {
                if *s == last {
                    *s = 0;
                }
            }
        }
        Some((targets, slots))
    }

    /// Composite one glass container. The target framebuffer must be bound as draw framebuffer.
    ///
    /// # Safety
    /// Must be called with the renderer's context current (inside `GlesFrame::with_context`).
    pub unsafe fn draw_glass(&mut self, gl: &ffi::Gles2, p: &GlassDraw<'_>) -> Result<(), String> {
        if !self.timing || self.gpu_timers {
            return self.draw_glass_now(gl, p);
        }
        gl.Finish();
        let start = std::time::Instant::now();
        let result = self.draw_glass_now(gl, p);
        gl.Finish();
        self.glass_time.total += start.elapsed();
        result
    }

    unsafe fn draw_glass_now(&mut self, gl: &ffi::Gles2, p: &GlassDraw<'_>) -> Result<(), String> {
        if self.progs.is_none() || self.glass.program == 0 {
            return Err(self
                .failure
                .clone()
                .unwrap_or_else(|| "glass pass not initialised".into()));
        }
        self.tick += 1;
        let saved = SavedState::capture(gl);
        let viewport = saved.viewport;
        let Some(fb) = framebuffer_rect(&p.projection, viewport, p.area) else {
            return Ok(());
        };
        let state = p.glass.state(p.generation, p.scale);
        let sig = Sig::new()
            .u(state)
            .u(fb[0] as u64)
            .u(fb[1] as u64)
            .u(fb[2] as u64)
            .u(fb[3] as u64)
            .finish();
        let released = self.glass.cache.begin_draw();
        self.free
            .extend(released.into_iter().flat_map(|c| c.targets));
        let kept = self
            .glass
            .cache
            .get(p.owner, sig)
            .map(|c| c.slots.map(|s| c.targets[s].tex));
        let textures = match kept {
            Some(textures) => {
                self.stats.backdrop_hits += 1;
                textures
            }
            None => {
                self.stats.backdrop_misses += 1;
                // the textures this draw replaces take the new backdrop, rather than textures created anew
                let replaced = self.glass.cache.make_room(p.owner);
                self.free
                    .extend(replaced.into_iter().flat_map(|c| c.targets));
                let blurring = (self.timing && !self.gpu_timers).then(std::time::Instant::now);
                self.timer_begin(gl, true);
                let within = p.glass.within.and_then(|w| {
                    let w = to_physical_rect(w, Scale::from(f64::from(p.scale)));
                    framebuffer_rect(&p.projection, viewport, w)
                });
                let Some((targets, slots)) = self.process_glass(
                    gl,
                    saved.draw_fbo as GLuint,
                    fb,
                    within,
                    &p.glass.container,
                    p.scale,
                ) else {
                    self.timer_end(gl);
                    saved.restore(gl);
                    return Err("glass backdrop processing failed (framebuffer incomplete?)".into());
                };
                self.timer_end(gl);
                if let Some(blurring) = blurring {
                    gl.Finish();
                    self.glass_time.blur += blurring.elapsed();
                }
                if !p.glass.probes.is_empty() {
                    self.read_tones(gl, p, &targets, &slots, viewport, fb);
                }
                let textures = slots.map(|s| targets[s].tex);
                let dropped =
                    self.glass
                        .cache
                        .insert(p.owner, state, sig, CachedGlass { targets, slots });
                self.free
                    .extend(dropped.into_iter().flat_map(|c| c.targets));
                textures
            }
        };

        let uv = capture_uv(&p.projection, viewport, fb);
        let quad = [
            p.area.loc.x as f32,
            p.area.loc.y as f32,
            p.area.size.w as f32,
            p.area.size.h as f32,
        ];
        let target = [viewport[2] as f32, viewport[3] as f32];
        let uniform = GlassUniform::new(&p.glass.container, p.scale, uv, quad, target, p.opacity);
        // the glass of a window meets the window's other backdrops and content on whole pixels, as they are drawn
        let uniform = if p.glass.within.is_some() {
            uniform.on_pixels()
        } else {
            uniform
        };

        self.timer_begin(gl, false);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, saved.draw_fbo as GLuint);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, saved.read_fbo as GLuint);
        gl.Viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
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
        gl.UseProgram(self.glass.program);
        Self::upload_ubo(gl, self.glass.ubo, BIND_GLASS, bytemuck::bytes_of(&uniform));
        for (unit, tex) in textures.iter().enumerate() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(ffi::TEXTURE_2D, *tex);
        }
        for d in p.damage {
            let Some(q) = d.intersection(p.area) else {
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
            self.stats.glass_composites += 1;
        }
        for unit in 2..textures.len() {
            gl.ActiveTexture(ffi::TEXTURE0 + unit as u32);
            gl.BindTexture(ffi::TEXTURE_2D, 0);
        }
        // what dropped textures released goes back to the driver beyond what is kept for reuse
        self.trim_free(gl);
        self.timer_end(gl);
        saved.restore(gl);
        Ok(())
    }

    /// Start reading back the widest blur level `glass` uses at a coarse grid, for the faces of its probes; the
    /// pixels arrive in a pixel buffer without stalling the frame ([`MaterialGl::poll_tones`]).
    unsafe fn read_tones(
        &mut self,
        gl: &ffi::Gles2,
        p: &GlassDraw<'_>,
        targets: &[Target],
        slots: &[usize; GLASS_SLOTS],
        viewport: [i32; 4],
        fb: [i32; 4],
    ) {
        let Some(widest) = p
            .glass
            .container
            .levels()
            .iter()
            .rposition(|(_, used)| *used)
        else {
            return;
        };
        let level = &targets[slots[widest]];
        let grid = (
            level.size.0.clamp(1, TONE_GRID),
            level.size.1.clamp(1, TONE_GRID),
        );
        let kept = self.glass.grids.iter().position(|t| t.size == grid);
        let Some(small) = kept
            .map(|i| self.glass.grids.swap_remove(i))
            .or_else(|| self.alloc_target(gl, grid, false))
        else {
            return;
        };
        gl.Disable(ffi::SCISSOR_TEST);
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, level.fbo);
        gl.BindFramebuffer(ffi::DRAW_FRAMEBUFFER, small.fbo);
        gl.BlitFramebuffer(
            0,
            0,
            level.size.0,
            level.size.1,
            0,
            0,
            grid.0,
            grid.1,
            ffi::COLOR_BUFFER_BIT,
            ffi::LINEAR,
        );
        let mut pbo = 0;
        gl.GenBuffers(1, &mut pbo);
        gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, pbo);
        gl.BufferData(
            ffi::PIXEL_PACK_BUFFER,
            (grid.0 * grid.1 * 4) as isize,
            std::ptr::null(),
            ffi::STREAM_READ,
        );
        gl.BindFramebuffer(ffi::READ_FRAMEBUFFER, small.fbo);
        gl.ReadPixels(
            0,
            0,
            grid.0,
            grid.1,
            ffi::RGBA,
            ffi::UNSIGNED_BYTE,
            std::ptr::null_mut(),
        );
        gl.BindBuffer(ffi::PIXEL_PACK_BUFFER, 0);
        let fence = gl.FenceSync(ffi::SYNC_GPU_COMMANDS_COMPLETE, 0);
        // the copy into the buffer was queued; the small target serves the next readback
        if self.glass.grids.len() < MAX_TONE_READS {
            self.glass.grids.push(small);
        } else {
            self.destroy_target(gl, small);
        }
        let uv = capture_uv(&p.projection, viewport, fb);
        let probes = p
            .glass
            .probes
            .iter()
            .map(|probe| {
                let side = probe.rect[2].min(probe.rect[3]);
                (probe.serial, probe_uv(probe.rect, p.scale, &uv), side)
            })
            .collect();
        if self.glass.reads.len() >= MAX_TONE_READS {
            let old = self.glass.reads.remove(0);
            GlassGl::delete_read(gl, old);
        }
        self.glass.reads.push(ToneRead {
            pbo,
            fence,
            grid,
            mode: p.glass.container.mode,
            probes,
        });
    }

    /// The faces measured since the last call, and whether readbacks are still in flight, after polling them. Call with
    /// the renderer's context current.
    ///
    /// # Safety
    /// Must be called with the renderer's context current.
    pub unsafe fn poll_tones(&mut self, gl: &ffi::Gles2) -> (Vec<ToneReading>, bool) {
        let pending = self.glass.poll_reads(gl);
        (std::mem::take(&mut self.glass.readings), pending)
    }

    /// Whether readbacks of faces are still in flight; the next frames poll them. A test that reads the tones waits for
    /// none before it does, so every face read back by then was measured.
    pub fn tone_reads_in_flight(&self) -> bool {
        !self.glass.reads.is_empty()
    }

    /// Note glass backdrops drawn too large to follow what lies behind them, while a frame is built.
    pub fn note_unadapted(&mut self, serials: impl IntoIterator<Item = u64>) {
        self.glass.unadapted.extend(serials);
    }

    /// The glass backdrops noted too large since the last call.
    pub fn take_unadapted(&mut self) -> Vec<u64> {
        let mut all = std::mem::take(&mut self.glass.unadapted);
        all.sort_unstable();
        all.dedup();
        all
    }

    /// While a frame is built: whether the textures of glass `owner` for `state` are whole, keeping them through the
    /// frame ([`GlassCache::check`]).
    pub fn glass_check(&mut self, owner: u64, state: u64) -> bool {
        self.glass.cache.check(owner, state)
    }
}

/// A glass container's element. It covers the container's sampled area; everything below it in the element list is
/// part of its backdrop. [`GlassElement::finalize`] gives it the signature of the elements below that overlap the area
/// ([`area_generation`]): its commit advances when they changed, so the damage tracker repaints the whole area and the
/// backdrop is captured from a freshly drawn scene.
pub struct GlassElement {
    key: ElementKey,
    id: Id,
    commit: CommitCounter,
    area: Rectangle<f64, Logical>,
    glass: Rc<GlassContainer>,
    shared: SharedMaterial,
    generation: u64,
    opacity: f32,
    scale: f64,
}

impl std::fmt::Debug for GlassElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlassElement")
            .field("area", &self.area)
            .field("generation", &self.generation)
            .finish()
    }
}

impl GlassElement {
    pub fn new(
        key: ElementKey,
        cache: &mut ElementCache,
        area: Rectangle<f64, Logical>,
        glass: Rc<GlassContainer>,
        shared: SharedMaterial,
        opacity: f32,
        scale: f64,
    ) -> Self {
        let mut e = Self {
            key,
            id: Id::new(),
            commit: CommitCounter::default(),
            area,
            glass,
            shared,
            generation: 0,
            opacity,
            scale,
        };
        // the id until `finalize`, which sets the commit: asking with a provisional signature would advance it
        (e.id, e.commit) = cache.peek(key, e.sig(0));
        e
    }

    pub fn area(&self) -> Rectangle<f64, Logical> {
        self.area
    }

    /// The key its id, commit and backdrop record are kept under.
    pub fn key(&self) -> ElementKey {
        self.key
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn sig(&self, generation: u64) -> u64 {
        Sig::new()
            .rect(self.area)
            .f(self.opacity as f64)
            .u(self.glass.key)
            .u(generation)
            .finish()
    }

    /// Set the backdrop signature ([`area_generation`]) and refresh the id and commit counter from the cache. Call
    /// once per frame: the commit also advances when the pass dropped the container's textures while it showed, so the
    /// whole area is repainted for the next capture.
    pub fn finalize(&mut self, generation: u64, cache: &mut ElementCache) {
        self.generation = generation;
        let state = self.glass.state(generation, self.scale as f32);
        let whole = self.shared.borrow_mut().glass_check(self.owner(), state);
        let (id, mut commit) = cache.slot(self.key, self.sig(generation));
        if !whole {
            commit = cache.advance(self.key).unwrap_or(commit);
        }
        self.id = id;
        self.commit = commit;
    }

    fn owner(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.key.hash(&mut h);
        h.finish()
    }
}

impl Element for GlassElement {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.commit
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        Rectangle::from_size(Size::from((1.0, 1.0)))
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        to_physical_rect(self.area, scale)
    }

    fn opaque_regions(&self, _scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::default()
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }
}

impl RenderElement<GlesRenderer> for GlassElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        _src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        _opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        let _glass = tracing::trace_span!("server.glass", rects = damage.len()).entered();
        if damage.is_empty() || !self.shared.borrow().is_ready() {
            return Ok(());
        }
        let abs: Vec<Rectangle<i32, Physical>> = damage
            .iter()
            .map(|d| Rectangle::new(d.loc + dst.loc, d.size))
            .collect();
        // the shapes are output-local points; the element is drawn where its area is
        let params = GlassDraw {
            projection: *frame.projection(),
            glass: &self.glass,
            owner: self.owner(),
            generation: self.generation,
            scale: self.scale as f32,
            area: dst,
            opacity: self.opacity,
            damage: &abs,
        };
        let shared = self.shared.clone();
        let res =
            frame.with_context(move |gl| unsafe { shared.borrow_mut().draw_glass(gl, &params) })?;
        if let Err(e) = res {
            let mut s = self.shared.borrow_mut();
            if s.failure.is_none() {
                s.failure = Some(e.clone());
            }
            tracing::error!(error = %e, "glass pass failed");
        }
        Ok(())
    }

    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}
