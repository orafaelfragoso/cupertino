use super::*;

impl<R: GlyphRasterizer> Painter<'_, '_, R> {
    pub(super) fn device(&self) -> &cupertino_gpu::Device {
        &self.r.device
    }

    pub(super) fn subtree(
        &mut self,
        ctx: &Ctx,
        i: usize,
        chain: &mut Vec<ClipItem>,
    ) -> Result<(), RenderError> {
        let node = &self.tree.nodes[i];
        if !Bounds::of_rect(&node.bounds, self.scale).touches(&ctx.limit) {
            return Ok(());
        }
        if let Some(group) = &node.group {
            return self.group(ctx, i, group, chain);
        }
        self.own(ctx, i, chain)?;
        if ctx.target == WINDOW {
            if let Some(&(kept, viewport)) = self.views.get(&i) {
                return self.composite(ctx, i, chain, kept, viewport);
            }
        }
        self.children(ctx, i, chain)
    }

    /// The pixels `rect` of `ctx`'s target made transparent.
    pub(super) fn clear(&mut self, ctx: &Ctx, rect: Bounds) {
        let mut b = FillBlock {
            frame: ctx.frame(self.rec),
            shape_linear: [1.0, 0.0, 0.0, 1.0],
            shape_moved: [
                -(rect.x0 as f32 + rect.x1 as f32) / 2.0,
                -(rect.y0 as f32 + rect.y1 as f32) / 2.0,
                rect.width() as f32 / 2.0 + 1.0,
                rect.height() as f32 / 2.0 + 1.0,
            ],
            ..FillBlock::default()
        };
        b.shape[1] = 1.0;
        let uniform = self.rec.uniform(b.floats());
        let first = self.rec.instance_count();
        self.rec.quad(ctx.local(&rect), (0.0, 0.0));
        self.rec.cmds.push(Cmd::Quads {
            program: Program::Clear,
            target: ctx.target,
            scissor: ctx.scissor(&rect),
            uniform,
            textures: [Tex::Empty; 3],
            instances: (first, 1),
        });
    }

    /// Brings a kept scroll layer's texture up to date as `plan` says: the pixels that stay in view moved into the
    /// other texture, then the pixels to paint again cleared and painted, the layer's content in place in the window's
    /// pixels and under none of its clips (those cut it as it is laid over).
    pub(super) fn keep(&mut self, plan: &ScrollPlan) -> Result<(), RenderError> {
        let v = plan.viewport;
        let size = (v.width(), v.height());
        let Some(kept) = self.r.kept.get_mut(&plan.id) else {
            return Ok(());
        };
        let held = kept.current;
        let Some(texture) = kept.textures[held].take() else {
            return Ok(());
        };
        let from = self.rec.add_kept(plan.id, held, texture);
        let at = (v.x0, v.y0);
        let target = match plan.moved {
            None => from,
            Some((dx, dy)) => {
                let other = 1 - held;
                let texture = match self
                    .r
                    .kept
                    .get_mut(&plan.id)
                    .and_then(|k| k.textures[other].take())
                {
                    Some(t) => t,
                    None => {
                        self.r.stats.targets_made += 1;
                        self.r
                            .device
                            .create_texture(&TextureDesc {
                                size,
                                format: TextureFormat::Rgba8,
                                usage: TextureUsage::TARGET,
                            })
                            .map_err(gpu)?
                    }
                };
                let to = self.rec.add_kept(plan.id, other, texture);
                if let Some(k) = self.r.kept.get_mut(&plan.id) {
                    k.current = other;
                }
                self.rec.cmds.push(Cmd::Begin { target: to });
                let ctx = Ctx {
                    target: to,
                    origin: at,
                    rect: v,
                    limit: v,
                    dither: (0, 0),
                };
                let was = v.offset(-dx, -dy);
                self.lay_layer(&ctx, from, was, was.intersect(&v), 255, &[])?;
                to
            }
        };
        for &rect in &plan.paint {
            let rect = rect.intersect(&v);
            if rect.is_empty() {
                continue;
            }
            self.r.stats.scroll_pixels += u64::from(rect.width()) * u64::from(rect.height());
            let ctx = Ctx {
                target,
                origin: at,
                rect: EVERYWHERE,
                limit: rect,
                dither: plan.shift,
            };
            self.clear(&ctx, rect);
            for &child in self.tree.children_of(plan.index) {
                self.subtree(&ctx, child, &mut Vec::new())?;
            }
        }
        self.views.insert(plan.index, (target, v));
        Ok(())
    }

    /// Lays a kept scroll layer over the window where its node's clip shows it. The pixels every clip above it and
    /// its own let through whole come straight from its texture; at a clip's edge, where it covers part of a pixel,
    /// the content is painted in place, so the edge is cut draw by draw as a frame without the layer cuts it.
    pub(super) fn composite(
        &mut self,
        ctx: &Ctx,
        i: usize,
        chain: &mut Vec<ClipItem>,
        kept: TargetId,
        viewport: Bounds,
    ) -> Result<(), RenderError> {
        let mut area = viewport.intersect(&ctx.limit);
        let mut whole = vec![area];
        let mut owner = Some(i);
        while let Some(n) = owner {
            let node = &self.tree.nodes[n];
            if let Some(clip) = &node.clip {
                match geometry::clip_cover(clip, &node.transform, self.scale) {
                    Some((reach, inside)) => {
                        area = area.intersect(&reach);
                        whole = whole
                            .iter()
                            .flat_map(|w| inside.iter().map(move |p| w.intersect(p)))
                            .filter(|w| !w.is_empty())
                            .collect();
                    }
                    None => whole.clear(),
                }
            }
            owner = self.tree.parents[n];
        }
        if area.is_empty() {
            return Ok(());
        }
        let whole: Vec<Bounds> = whole
            .iter()
            .map(|w| w.intersect(&area))
            .filter(|w| !w.is_empty())
            .collect();
        for piece in &whole {
            self.lay_layer(ctx, kept, viewport, *piece, 255, &[])?;
        }
        for rim in geometry::subtract(area, &whole) {
            let edge = Ctx { limit: rim, ..*ctx };
            self.children(&edge, i, chain)?;
        }
        Ok(())
    }

    pub(super) fn children(
        &mut self,
        ctx: &Ctx,
        i: usize,
        chain: &mut Vec<ClipItem>,
    ) -> Result<(), RenderError> {
        let node = &self.tree.nodes[i];
        let clipped = node.clip.is_some();
        if let Some(clip) = &node.clip {
            chain.push(ClipItem::new(clip, &node.transform, self.scale));
        }
        let done = self
            .tree
            .children_of(i)
            .iter()
            .try_for_each(|&child| self.subtree(ctx, child, chain));
        if clipped {
            chain.pop();
        }
        done
    }

    /// The pixels a node's own operations can change where `limit` is needed, under the clips of `chain`.
    pub(super) fn own_area(
        &self,
        i: usize,
        chain: &[ClipItem],
        rect: &Bounds,
        limit: &Bounds,
    ) -> Option<Bounds> {
        let extent = self.tree.extents[i];
        if extent.is_empty() {
            return None;
        }
        let halo = geometry::node_support(&self.tree.nodes[i], self.scale);
        let mut area = extent
            .outset(halo)
            .intersect(&limit.outset(halo))
            .intersect(rect);
        for clip in chain {
            area = area.intersect(&clip.bounds());
        }
        (!area.is_empty()).then_some(area)
    }

    pub(super) fn group(
        &mut self,
        ctx: &Ctx,
        i: usize,
        group: &Group,
        chain: &[ClipItem],
    ) -> Result<(), RenderError> {
        let node = &self.tree.nodes[i];
        let area = bounded(
            Bounds::of_rect(&node.bounds, self.scale).intersect(&ctx.rect),
            ctx,
            geometry::node_support(node, self.scale),
        );
        let k = opacity_factor(group.opacity);
        if area.is_empty() || area.intersect(&ctx.limit).is_empty() || k == 0 {
            return Ok(());
        }
        let layer = self.target(area)?;
        let inner = Ctx {
            target: layer,
            origin: (area.x0, area.y0),
            rect: area,
            limit: area,
            dither: ctx.dither,
        };
        self.own(&inner, i, &[])?;
        self.children(&inner, i, &mut Vec::new())?;
        let mut layer = layer;
        for effect in &group.effects {
            layer = self.effect(layer, area, effect, ctx.dither_shift())?;
        }
        let visible = area.intersect(&ctx.limit);
        let laid = self.lay_layer(ctx, layer, area, visible, k, chain);
        self.rec.finish(layer);
        laid
    }

    pub(super) fn own(
        &mut self,
        ctx: &Ctx,
        i: usize,
        chain: &[ClipItem],
    ) -> Result<(), RenderError> {
        if self.own_area(i, chain, &ctx.rect, &ctx.limit).is_none() {
            return Ok(());
        }
        let Some(area) = self.own_area(i, chain, &ctx.rect, &ctx.rect) else {
            return Ok(());
        };
        let area = bounded(
            area,
            ctx,
            geometry::node_support(&self.tree.nodes[i], self.scale),
        );
        let mut clips = chain.to_vec();
        let mut pos = 0;
        self.run(
            ctx,
            i,
            area,
            area.intersect(&ctx.limit),
            &mut clips,
            &mut pos,
        )
    }

    /// Runs a node's operations into `ctx` from `pos` to its `PopLayer` or end: `area` is the window pixels its
    /// scratch image would cover and `needed` those of them kept.
    pub(super) fn run(
        &mut self,
        ctx: &Ctx,
        i: usize,
        area: Bounds,
        needed: Bounds,
        clips: &mut Vec<ClipItem>,
        pos: &mut usize,
    ) -> Result<(), RenderError> {
        let node = &self.tree.nodes[i];
        while let Some(op) = node.ops.get(*pos) {
            *pos += 1;
            match op {
                PaintOp::Fill { shape, paint } => {
                    self.fill(ctx, node, shape, None, paint, area, needed, clips)?
                }
                PaintOp::Stroke {
                    shape,
                    width,
                    paint,
                } => self.fill(ctx, node, shape, Some(*width), paint, area, needed, clips)?,
                PaintOp::Glyphs { origin, run, color } => {
                    self.glyphs(ctx, node, origin, run, *color, needed, clips)?
                }
                PaintOp::Image {
                    image,
                    src,
                    dst,
                    sampling,
                } => self.image(
                    ctx, node, image.id, src, dst, *sampling, area, needed, clips,
                )?,
                PaintOp::PushClip(shape) => {
                    clips.push(ClipItem::new(shape, &node.transform, self.scale))
                }
                PaintOp::PopClip => {
                    clips.pop();
                }
                PaintOp::PushLayer { opacity, effects } => {
                    self.layer(ctx, i, area, needed, clips, pos, *opacity, effects)?
                }
                PaintOp::PopLayer => return Ok(()),
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn layer(
        &mut self,
        ctx: &Ctx,
        i: usize,
        rect: Bounds,
        needed: Bounds,
        clips: &[ClipItem],
        pos: &mut usize,
        opacity: f32,
        effects: &[Effect],
    ) -> Result<(), RenderError> {
        let node = &self.tree.nodes[i];
        let halo = geometry::node_support(node, self.scale);
        let area = self.tree.extents[i].outset(halo).intersect(&rect);
        if area.is_empty() || area.intersect(&needed.outset(halo)).is_empty() {
            skip_layer(node, pos);
            return Ok(());
        }
        if u64::from(area.width()) * u64::from(area.height()) > self.r.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let layer = self.target(area)?;
        let inner = Ctx {
            target: layer,
            origin: (area.x0, area.y0),
            rect: area,
            limit: area,
            dither: ctx.dither,
        };
        self.run(&inner, i, area, area, &mut Vec::new(), pos)?;
        let mut layer = layer;
        for effect in effects {
            layer = self.effect(layer, area, effect, ctx.dither_shift())?;
        }
        let k = opacity_factor(opacity);
        let laid = if k == 0 {
            Ok(())
        } else {
            self.lay_layer(ctx, layer, area, area.intersect(&needed), k, clips)
        };
        self.rec.finish(layer);
        laid
    }

    /// A new offscreen target covering the window pixels `area`, transparent.
    pub(super) fn target(&mut self, area: Bounds) -> Result<TargetId, RenderError> {
        let target = self.target_of((area.width(), area.height()), TextureFormat::Rgba8)?;
        self.rec.cmds.push(Cmd::Begin { target });
        Ok(target)
    }

    pub(super) fn target_of(
        &mut self,
        size: (u32, u32),
        format: TextureFormat,
    ) -> Result<TargetId, RenderError> {
        self.r.stats.layers += 1;
        let reused = self.rec.spare.iter().position(|&at| {
            let (_, s, f) = self.rec.targets[at];
            (s, f) == (size, format)
        });
        if let Some(i) = reused {
            let at = self.rec.spare.swap_remove(i);
            return Ok(self.rec.add_target(at, size));
        }
        let device = std::rc::Rc::clone(&self.r.device);
        let made_before = self.r.pool.made();
        let mem = match self.r.pool.acquire(&mut &*device, size, format) {
            Ok(p) => TargetMem::Pooled(p),
            Err(GpuError::OverBudget { .. }) => TargetMem::Own(
                device
                    .create_texture(&TextureDesc {
                        size,
                        format,
                        usage: TextureUsage::TARGET,
                    })
                    .map_err(gpu)?,
            ),
            Err(e) => return Err(gpu(e)),
        };
        if matches!(mem, TargetMem::Own(_)) || self.r.pool.made() != made_before {
            self.r.stats.targets_made += 1;
        }
        self.rec.targets.push((mem, size, format));
        Ok(self.rec.add_target(self.rec.targets.len() - 1, size))
    }

    /// The format of the blur's levels: floats where the device renders to them (the blur's sums kept as exact as
    /// the CPU keeps them), then half floats, then bytes.
    pub(super) fn level_format(&self) -> TextureFormat {
        crate::gpu::level_format(self.device().info())
    }

    /// Applies `effect` to the layer `layer` covering `area`: the layer that holds the result.
    pub(super) fn effect(
        &mut self,
        layer: TargetId,
        area: Bounds,
        effect: &Effect,
        dither: f32,
    ) -> Result<TargetId, RenderError> {
        let (mode, sigma, shift, color) = match *effect {
            Effect::Blur { radius } => (
                0.0,
                geometry::sigma_px(radius, self.scale),
                (0.0, 0.0),
                None,
            ),
            Effect::DropShadow {
                offset,
                blur,
                color,
            }
            | Effect::InnerShadow {
                offset,
                blur,
                color,
            } => (
                if matches!(effect, Effect::DropShadow { .. }) {
                    1.0
                } else {
                    2.0
                },
                geometry::sigma_px(blur, self.scale),
                (
                    (offset.x * self.scale).clamp(-1e9, 1e9) as f32,
                    (offset.y * self.scale).clamp(-1e9, 1e9) as f32,
                ),
                Some(color),
            ),
        };
        if mode == 0.0 && sigma <= 0.0 {
            return Ok(layer);
        }
        let (w, h) = (area.width(), area.height());
        let plan = plan_blur(w, h, 1.0, sigma, 1.0);
        let format = self.level_format();
        let mut level = Tex::Target(layer);
        for &(lw, lh) in &plan.levels {
            let next = self.target_of((lw.max(1), lh.max(1)), format)?;
            let uniform = self.rec.uniform(&level_uniform());
            self.rec.cmds.push(Cmd::Whole {
                program: Program::Halve,
                target: next,
                uniform,
                textures: [level, Tex::Empty],
            });
            self.finish_level(level, layer);
            level = Tex::Target(next);
        }
        if plan.weights.len() > 1 {
            let size = plan.final_size(w, h);
            let size = (size.0.max(1), size.1.max(1));
            for direction in [(1, 0), (0, 1)] {
                let next = self.target_of(size, format)?;
                let uniform = self.rec.uniform(&kernel_uniform(&plan.weights, direction));
                self.rec.cmds.push(Cmd::Whole {
                    program: Program::Gauss,
                    target: next,
                    uniform,
                    textures: [level, Tex::Empty],
                });
                self.finish_level(level, layer);
                level = Tex::Target(next);
            }
        }
        let out = self.target(area)?;
        let mut u = [0.0f32; RESOLVE_FLOATS];
        u[0] = mode;
        u[1] = (1u64 << plan.levels.len()) as f32;
        u[2] = shift.0;
        u[3] = shift.1;
        u[4] = if dither::BY_DEFAULT { 1.0 } else { 0.0 };
        u[5] = dither;
        u[8] = area.x0 as f32;
        u[9] = area.y0 as f32;
        if let Some(c) = color {
            u[12..16].copy_from_slice(&tint(c));
        }
        let uniform = self.rec.uniform(&u);
        self.rec.cmds.push(Cmd::Whole {
            program: Program::Resolve,
            target: out,
            uniform,
            textures: [level, Tex::Target(layer)],
        });
        self.finish_level(level, layer);
        self.rec.finish(layer);
        Ok(out)
    }

    /// A level of `layer`'s blur is read for the last time: its texture may serve a later target.
    pub(super) fn finish_level(&mut self, level: Tex, layer: TargetId) {
        if let Tex::Target(t) = level {
            if t != layer {
                self.rec.finish(t);
            }
        }
    }

    /// Lays the layer `layer` (covering `area`) over `ctx` where `visible`, at opacity `k`, through `clips`.
    pub(super) fn lay_layer(
        &mut self,
        ctx: &Ctx,
        layer: TargetId,
        area: Bounds,
        visible: Bounds,
        k: u32,
        clips: &[ClipItem],
    ) -> Result<(), RenderError> {
        let Some(c) = self.clip_draw(clips)? else {
            return Ok(());
        };
        let shown = visible.intersect(&c.scissor);
        if shown.is_empty() {
            return Ok(());
        }
        let mut b = TextureBlock {
            frame: ctx.frame(self.rec),
            mode: [1.0, k as f32, 0.0, 0.0],
            source: [area.x0 as f32, area.y0 as f32, 0.0, 0.0],
            ..TextureBlock::default()
        };
        c.write(&mut b.clips);
        let local = ctx.local(&shown);
        self.push_texture(
            ctx,
            b.floats(),
            shown,
            [Tex::Target(layer), c.mask_tex(), Tex::Empty],
            &[(local, (0.0, 0.0))],
        );
        Ok(())
    }

    /// Lays the image of a stroke, drawn in its colour at `at` of `tex` and covering `rect`, under `clips`.
    pub(super) fn lay_stroke(
        &mut self,
        ctx: &Ctx,
        tex: Tex,
        at: (u32, u32),
        rect: Bounds,
        needed: Bounds,
        clips: &[ClipItem],
    ) -> Result<(), RenderError> {
        let Some(c) = self.clip_draw(clips)? else {
            return Ok(());
        };
        let shown = rect.intersect(&needed).intersect(&c.scissor);
        if shown.is_empty() {
            return Ok(());
        }
        let mut b = TextureBlock {
            frame: ctx.frame(self.rec),
            tint: [255.0; 4],
            ..TextureBlock::default()
        };
        c.write(&mut b.clips);
        let atlas = (
            (at.0 as i32 + shown.x0 - rect.x0) as f32,
            (at.1 as i32 + shown.y0 - rect.y0) as f32,
        );
        self.push_texture(
            ctx,
            b.floats(),
            shown,
            [tex, c.mask_tex(), Tex::Empty],
            &[(ctx.local(&shown), atlas)],
        );
        self.r.stats.draws += 1;
        Ok(())
    }

    pub(super) fn push_texture(
        &mut self,
        ctx: &Ctx,
        u: &[f32],
        scissor: Bounds,
        textures: [Tex; 3],
        quads: &[(Bounds, (f32, f32))],
    ) {
        let uniform = self.rec.uniform(u);
        let first = self.rec.instance_count();
        for (rect, at) in quads {
            self.rec.quad(*rect, *at);
        }
        self.rec.cmds.push(Cmd::Quads {
            program: Program::Texture,
            target: ctx.target,
            scissor: ctx.scissor(&scissor),
            uniform,
            textures,
            instances: (first, quads.len() as u32),
        });
    }

    /// The clips of a draw under `clips`: aligned rectangles as the scissor, up to four shapes analytically, the
    /// rest (and every path) in one clip mask. `None` when nothing gets through.
    pub(super) fn clip_draw(
        &mut self,
        clips: &[ClipItem],
    ) -> Result<Option<ClipDraw>, RenderError> {
        let mut scissor = self.whole;
        let mut shapes = Vec::new();
        let mut masked: Vec<&ClipItem> = Vec::new();
        for clip in clips {
            match clip {
                ClipItem::Empty => return Ok(None),
                ClipItem::Aligned { pixels, .. } => scissor = scissor.intersect(pixels),
                ClipItem::Shape { outline, .. } if shapes.len() < MAX_ANALYTIC_CLIPS => {
                    shapes.push(*outline)
                }
                other => masked.push(other),
            }
        }
        if scissor.is_empty() {
            return Ok(None);
        }
        let mask = if masked.is_empty() {
            None
        } else {
            match self.clip_mask(&masked)? {
                Some(m) => Some(m),
                None => return Ok(None),
            }
        };
        Ok(Some(ClipDraw {
            scissor,
            shapes,
            mask,
        }))
    }

    /// The product of `clips`' coverage as a mask over the pixels all of them can let through, uploaded.
    pub(super) fn clip_mask(
        &mut self,
        clips: &[&ClipItem],
    ) -> Result<Option<MaskRef>, RenderError> {
        let mut h = DefaultHasher::new();
        let mut rect = self.whole;
        for clip in clips {
            rect = rect.intersect(&clip.bounds());
            match clip {
                ClipItem::Path { key, .. } => key.hash(&mut h),
                ClipItem::Shape { outline, .. } => {
                    for v in outline.inv.iter().chain(&outline.half).chain([
                        &outline.corner,
                        &outline.exponent,
                        &outline.outset,
                        &outline.px,
                    ]) {
                        v.to_bits().hash(&mut h);
                    }
                }
                _ => {}
            }
        }
        if rect.is_empty() {
            return Ok(None);
        }
        rect.hash(&mut h);
        let key = h.finish();
        if let Some(m) = self.rec.masks.get(&key) {
            return Ok(*m);
        }
        let mut mask =
            tiny_skia::Mask::new(rect.width(), rect.height()).ok_or(RenderError::ResourceLimit)?;
        mask.data_mut().fill(255);
        for clip in clips {
            match clip {
                ClipItem::Path { path, rule, ts, .. } => {
                    let ts = ts.post_translate(-rect.x0 as f32, -rect.y0 as f32);
                    mask.intersect_path(path, *rule, true, ts);
                }
                ClipItem::Shape { outline, .. } => {
                    clip_by_outline(&mut mask, outline, (rect.x0, rect.y0))
                }
                _ => {}
            }
        }
        let placed =
            self.upload_coverage(Kind::Mask, key, (rect.width(), rect.height()), mask.data())?;
        let m = Some(MaskRef {
            tex: placed.0,
            rect,
            at: placed.1,
        });
        self.rec.masks.insert(key, m);
        Ok(m)
    }

    /// Places `pixels` (8-bit, or RGBA for colour glyphs) in the atlas of `kind` under `key`, or in a texture of the
    /// frame's own when it is larger than a page; flushes the frame's draws when every page is in use.
    pub(super) fn upload_coverage(
        &mut self,
        kind: Kind,
        key: u64,
        size: (u32, u32),
        pixels: &[u8],
    ) -> Result<(Tex, (u32, u32)), RenderError> {
        let frames = self.r.frames;
        let atlas = match kind {
            Kind::Coverage => &mut self.r.coverage,
            Kind::Color => &mut self.r.color,
            Kind::Mask => &mut self.r.masks,
        };
        let tex = |page| match kind {
            Kind::Coverage => Tex::Coverage(page),
            Kind::Color => Tex::Color(page),
            Kind::Mask => Tex::Mask(page),
        };
        if let Some(slot) = atlas.get(key, frames) {
            return Ok((tex(slot.page), (slot.region.x, slot.region.y)));
        }
        let page = atlas.model().config().page_size;
        if size.0 + 2 > page.0 || size.1 + 2 > page.1 {
            let format = match kind {
                Kind::Color => TextureFormat::Rgba8,
                _ => TextureFormat::R8,
            };
            let t = self
                .device()
                .create_texture(&TextureDesc {
                    size,
                    format,
                    usage: TextureUsage::SAMPLED,
                })
                .map_err(gpu)?;
            self.device()
                .write_texture(&t, Region::new(0, 0, size.0, size.1), pixels)
                .map_err(gpu)?;
            self.rec.own.push(t);
            return Ok((Tex::Own(self.rec.own.len() - 1), (0, 0)));
        }
        match kind {
            Kind::Mask => self.r.stats.mask_uploads += 1,
            _ => self.r.stats.glyph_uploads += 1,
        }
        let device = std::rc::Rc::clone(&self.r.device);
        let inserted = insert(atlas_of(self.r, kind), &device, key, size, pixels, frames);
        let slot = match inserted {
            Err(GpuError::OverBudget { .. }) => {
                // every page holds this frame's entries: draw what is recorded, then start again
                self.flush()?;
                let frames = self.r.frames;
                insert(atlas_of(self.r, kind), &device, key, size, pixels, frames)
            }
            other => other,
        }
        .map_err(gpu)?;
        Ok((tex(slot.page), (slot.region.x, slot.region.y)))
    }

    /// Executes what is recorded so far and starts the atlases' next frame.
    pub(super) fn flush(&mut self) -> Result<(), RenderError> {
        self.r.execute(self.rec).map_err(gpu)?;
        self.r.frames += 1;
        self.rec.masks.clear();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn fill(
        &mut self,
        ctx: &Ctx,
        node: &PaintNode,
        shape: &Shape,
        stroke: Option<f64>,
        paint: &Paint,
        area: Bounds,
        needed: Bounds,
        clips: &[ClipItem],
    ) -> Result<(), RenderError> {
        let window = geometry::to_target(&node.transform, self.scale, (0, 0));
        let Some(data) = PaintData::new(paint, window) else {
            return Ok(());
        };
        // a solid stroke along a path is laid as the stroker lays it, a piece at a time over what lies below: drawn
        // in its colour over nothing, then laid over the target as one image
        let solid = match (paint, stroke) {
            (Paint::Solid(c), Some(_)) => Some(*c),
            _ => None,
        };
        let (cover, reach) = if let Shape::Path { .. } = shape {
            let Some(path) = geometry::path(shape) else {
                return Ok(());
            };
            let rule = geometry::fill_rule(shape);
            match self.path_cover(&path, rule, window, stroke, solid, area)? {
                Some((tex, at, rect)) if solid.is_some() => {
                    return self.lay_stroke(ctx, tex, at, rect, needed, clips);
                }
                Some((tex, at, rect)) => (Cover::Image { tex, at }, rect),
                None => return Ok(()),
            }
        } else {
            match Outline::new(shape, window, stroke) {
                Some(o) => (Cover::Shape(o), o.bounds),
                None => return Ok(()),
            }
        };
        let Some(c) = self.clip_draw(clips)? else {
            return Ok(());
        };
        let shown = reach.intersect(&needed).intersect(&c.scissor);
        if shown.is_empty() {
            return Ok(());
        }
        let mut b = FillBlock {
            frame: ctx.frame(self.rec),
            ..FillBlock::default()
        };
        let (coverage_tex, atlas) = match &cover {
            Cover::Shape(o) => {
                b.shape_linear = [o.inv[0], o.inv[1], o.inv[2], o.inv[3]];
                b.shape_moved = [o.inv[4], o.inv[5], o.half[0], o.half[1]];
                b.shape = [o.corner, o.px, o.stroke.unwrap_or(0.0), 0.0];
                b.clips.info[2] = corner_exponent(o.exponent);
                b.outline = [o.outset, 0.0, 0.0, 0.0];
                (Tex::Empty, (0.0, 0.0))
            }
            Cover::Image { tex, at } => {
                b.shape[3] = 1.0;
                // the quad's first pixel is `shown`'s; the texel of the image's first pixel is `at`, at `reach`
                (
                    *tex,
                    (
                        (at.0 as i32 + shown.x0 - reach.x0) as f32,
                        (at.1 as i32 + shown.y0 - reach.y0) as f32,
                    ),
                )
            }
        };
        data.write(&mut b);
        b.paint[3] = ctx.dither_shift();
        c.write(&mut b.clips);
        let uniform = self.rec.uniform(b.floats());
        let first = self.rec.instance_count();
        self.rec.quad(ctx.local(&shown), atlas);
        self.rec.cmds.push(Cmd::Quads {
            program: Program::Fill,
            target: ctx.target,
            scissor: ctx.scissor(&shown),
            uniform,
            textures: [coverage_tex, c.mask_tex(), Tex::Empty],
            instances: (first, 1),
        });
        self.r.stats.draws += 1;
        Ok(())
    }

    /// The coverage of a path (or its stroke of `stroke`) placed by `ts`, rasterized on the CPU over the pixels of
    /// `area` it can touch and uploaded: its texture, the texel of its first pixel and its window pixels.
    pub(super) fn path_cover(
        &mut self,
        path: &tiny_skia::Path,
        rule: tiny_skia::FillRule,
        ts: tiny_skia::Transform,
        stroke: Option<f64>,
        solid: Option<Color>,
        area: Bounds,
    ) -> Result<Option<Placed>, RenderError> {
        let outline;
        let (shape, rule, grow) = match stroke {
            Some(width) if solid.is_none() => {
                let scale = ts.get_scale();
                let s = tiny_skia::Stroke {
                    width: width as f32,
                    ..tiny_skia::Stroke::default()
                };
                let Some(o) = path.stroke(&s, scale.0.max(scale.1)) else {
                    return Ok(None);
                };
                outline = o;
                (&outline, tiny_skia::FillRule::Winding, 0)
            }
            Some(width) => {
                // a mitred join reaches out to the miter limit's half widths
                let scale = ts.get_scale();
                let reach = tiny_skia::Stroke::default().miter_limit.max(1.0);
                (
                    path,
                    rule,
                    (width as f32 * scale.0.max(scale.1) * reach / 2.0).ceil() as i32 + 1,
                )
            }
            None => (path, rule, 0),
        };
        let rect = geometry::path_bounds(shape, ts)
            .outset(grow)
            .intersect(&area);
        if rect.is_empty() {
            return Ok(None);
        }
        let local = ts.post_translate(-rect.x0 as f32, -rect.y0 as f32);
        let mut h = DefaultHasher::new();
        hash_path(shape, &mut h);
        hash_ts(local, &mut h);
        matches!(rule, tiny_skia::FillRule::EvenOdd).hash(&mut h);
        let color = solid.map(|c| [c.r, c.g, c.b, c.a].map(f32::to_bits));
        (stroke.map(f64::to_bits), color, rect.width(), rect.height()).hash(&mut h);
        let key = h.finish();
        let pixels = match (stroke, solid) {
            (Some(width), Some(color)) => {
                // a solid stroke is drawn by the stroker (hairlines included), in its colour
                let mut pm = tiny_skia::Pixmap::new(rect.width(), rect.height())
                    .ok_or(RenderError::ResourceLimit)?;
                let q = |v: f32| v.clamp(0.0, 1.0);
                let Some(color) =
                    tiny_skia::Color::from_rgba(q(color.r), q(color.g), q(color.b), q(color.a))
                else {
                    return Ok(None);
                };
                let paint = tiny_skia::Paint {
                    shader: tiny_skia::Shader::SolidColor(color),
                    anti_alias: true,
                    ..tiny_skia::Paint::default()
                };
                let s = tiny_skia::Stroke {
                    width: width as f32,
                    ..tiny_skia::Stroke::default()
                };
                pm.stroke_path(shape, &paint, &s, local, None);
                let (tex, at) = self.upload_coverage(
                    Kind::Color,
                    key,
                    (rect.width(), rect.height()),
                    pm.data(),
                )?;
                return Ok(Some((tex, at, rect)));
            }
            _ => {
                let mut m = tiny_skia::Mask::new(rect.width(), rect.height())
                    .ok_or(RenderError::ResourceLimit)?;
                m.fill_path(shape, rule, true, local);
                m.data().to_vec()
            }
        };
        let (tex, at) =
            self.upload_coverage(Kind::Mask, key, (rect.width(), rect.height()), &pixels)?;
        Ok(Some((tex, at, rect)))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn glyphs(
        &mut self,
        ctx: &Ctx,
        node: &PaintNode,
        origin: &cupertino_geometry::Point,
        run: &GlyphRun,
        color: Color,
        needed: Bounds,
        clips: &[ClipItem],
    ) -> Result<(), RenderError> {
        let [a, b, c, d, e, f] = node.transform.0;
        let size_px = f64::from(run.size) * self.scale * a.hypot(b);
        if size_px > MAX_GLYPH_PX {
            return Err(RenderError::ResourceLimit);
        }
        let Some(clip) = self.clip_draw(clips)? else {
            return Ok(());
        };
        let limit = needed.intersect(&clip.scissor);
        if limit.is_empty() {
            return Ok(());
        }
        let tinted = tint(color);
        // (texture, quads) runs of one atlas page and format, in order
        let mut batches: Vec<(Tex, GlyphFormat, Vec<Quad>)> = Vec::new();
        for g in &run.glyphs {
            let (lx, ly) = (origin.x + f64::from(g.x), origin.y + f64::from(g.y));
            let px = (a * lx + c * ly + e) * self.scale;
            let py = (b * lx + d * ly + f) * self.scale;
            let ix = px.floor();
            let key = GlyphKey::for_draw(
                run.font,
                g.id,
                size_px,
                px - ix,
                self.r.config.font_smoothing,
                [color.r, color.g, color.b],
            );
            let Some((meta, tex, at)) = self.glyph(key)? else {
                continue;
            };
            let x = ix as i32 + meta.bearing_x;
            let y = py.round() as i32 - meta.bearing_y;
            let rect = Bounds {
                x0: x,
                y0: y,
                x1: x.saturating_add(meta.width as i32),
                y1: y.saturating_add(meta.height as i32),
            };
            let shown = rect.intersect(&limit);
            if shown.is_empty() {
                continue;
            }
            let quad = (ctx.local(&rect), (at.0 as f32, at.1 as f32));
            match batches.last_mut() {
                Some((t, fmt, quads)) if *t == tex && *fmt == meta.format => quads.push(quad),
                _ => batches.push((tex, meta.format, vec![quad])),
            }
        }
        for (tex, format, quads) in batches {
            match format {
                GlyphFormat::Coverage => {
                    let mut b = FillBlock {
                        frame: ctx.frame(self.rec),
                        ..FillBlock::default()
                    };
                    b.shape[3] = 1.0;
                    PaintData::solid(tinted).write(&mut b);
                    clip.write(&mut b.clips);
                    let uniform = self.rec.uniform(b.floats());
                    let first = self.rec.instance_count();
                    for (rect, at) in &quads {
                        self.rec.quad(*rect, *at);
                    }
                    self.rec.cmds.push(Cmd::Quads {
                        program: Program::Fill,
                        target: ctx.target,
                        scissor: ctx.scissor(&limit),
                        uniform,
                        textures: [tex, clip.mask_tex(), Tex::Empty],
                        instances: (first, quads.len() as u32),
                    });
                }
                GlyphFormat::ColorPremultiplied => {
                    let mut b = TextureBlock {
                        frame: ctx.frame(self.rec),
                        tint: tinted,
                        ..TextureBlock::default()
                    };
                    clip.write(&mut b.clips);
                    self.push_texture(
                        ctx,
                        b.floats(),
                        limit,
                        [tex, clip.mask_tex(), Tex::Empty],
                        &quads,
                    );
                }
            }
            self.r.stats.draws += 1;
        }
        Ok(())
    }

    /// A glyph's raster placed in its atlas: its metrics, page and texel; `None` for a glyph with no pixels.
    pub(super) fn glyph(&mut self, key: GlyphKey) -> Result<Option<PlacedGlyph>, RenderError> {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        let id = h.finish();
        let kind_of = |m: &GlyphMeta| match m.format {
            GlyphFormat::Coverage => Kind::Coverage,
            GlyphFormat::ColorPremultiplied => Kind::Color,
        };
        if let Some(meta) = self.r.glyphs.get(&key).copied() {
            let Some(meta) = meta else { return Ok(None) };
            let frames = self.r.frames;
            if let Some(slot) = atlas_of(self.r, kind_of(&meta)).get(id, frames) {
                let tex = match meta.format {
                    GlyphFormat::Coverage => Tex::Coverage(slot.page),
                    GlyphFormat::ColorPremultiplied => Tex::Color(slot.page),
                };
                return Ok(Some((meta, tex, (slot.region.x, slot.region.y))));
            }
        }
        let raster = self
            .r
            .rasterizer
            .rasterize(&key)
            .map_err(RenderError::Glyph)?;
        let Some(g) = raster.filter(|g| g.width > 0 && g.height > 0) else {
            self.r.glyphs.insert(key, None);
            return Ok(None);
        };
        let meta = GlyphMeta {
            format: g.format,
            width: g.width,
            height: g.height,
            bearing_x: g.bearing_x,
            bearing_y: g.bearing_y,
        };
        let bytes_per = match g.format {
            GlyphFormat::Coverage => 1,
            GlyphFormat::ColorPremultiplied => 4,
        };
        let row = (g.width * bytes_per) as usize;
        let mut packed = Vec::with_capacity(row * g.height as usize);
        for y in 0..g.height as usize {
            let at = y * g.stride as usize;
            packed.extend_from_slice(&g.pixels[at..at + row]);
        }
        let (tex, at) = self.upload_coverage(kind_of(&meta), id, (g.width, g.height), &packed)?;
        if !matches!(tex, Tex::Own(_)) {
            self.r.glyphs.insert(key, Some(meta));
        }
        Ok(Some((meta, tex, at)))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn image(
        &mut self,
        ctx: &Ctx,
        node: &PaintNode,
        id: ResourceId,
        src: &cupertino_geometry::Rect,
        dst: &cupertino_geometry::Rect,
        sampling: Sampling,
        area: Bounds,
        needed: Bounds,
        clips: &[ClipItem],
    ) -> Result<(), RenderError> {
        let image = &self.request.resources.images[&id];
        self.upload_image(id)?;
        if src.width() <= 0.0 || src.height() <= 0.0 || dst.width() <= 0.0 || dst.height() <= 0.0 {
            return Ok(());
        }
        let (x0, y0) = (src.min_x().floor().max(0.0), src.min_y().floor().max(0.0));
        let (x1, y1) = (
            src.max_x().ceil().min(f64::from(image.width)),
            src.max_y().ceil().min(f64::from(image.height)),
        );
        if tiny_skia::IntRect::from_xywh(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
            .is_none()
        {
            return Ok(());
        }
        let (sx, sy) = (dst.width() / src.width(), dst.height() / src.height());
        let place = tiny_skia::Transform::from_row(
            sx as f32,
            0.0,
            0.0,
            sy as f32,
            (dst.min_x() + (x0 - src.min_x()) * sx) as f32,
            (dst.min_y() + (y0 - src.min_y()) * sy) as f32,
        );
        let mut ts = geometry::to_target(&node.transform, self.scale, (0, 0));
        // one source pixel per device pixel: on the whole window pixel the CPU renderer puts it on
        let device = ts.pre_concat(place);
        if device.kx == 0.0
            && device.ky == 0.0
            && (device.sx - 1.0).abs() < 1e-4
            && (device.sy - 1.0).abs() < 1e-4
        {
            ts = ts.post_translate(
                whole_pixel(device.tx) - device.tx,
                whole_pixel(device.ty) - device.ty,
            );
        }
        let Some(inv) = geometry::inverse(ts.pre_concat(place)) else {
            return Ok(());
        };
        let Some(rect_path) = geometry::path(&Shape::Rect(*dst)) else {
            return Ok(());
        };
        let reach = geometry::path_bounds(&rect_path, ts).intersect(&area);
        let Some(c) = self.clip_draw(clips)? else {
            return Ok(());
        };
        let shown = reach.intersect(&needed).intersect(&c.scissor);
        if shown.is_empty() {
            return Ok(());
        }
        let linear = if sampling == Sampling::Linear {
            1.0
        } else {
            0.0
        };
        let mut b = TextureBlock {
            frame: ctx.frame(self.rec),
            mode: [2.0, 255.0, linear, 0.0],
            crop: [x0 as f32, y0 as f32, x1 as f32, y1 as f32],
            // window pixel to the crop's texels, then to the image's
            image_linear: [inv[0], inv[1], inv[2], inv[3]],
            image_moved: [inv[4] + x0 as f32, inv[5] + y0 as f32, 0.0, 0.0],
            ..TextureBlock::default()
        };
        let mut cover = Tex::Empty;
        if let Some(edges) = aligned(&Shape::Rect(*dst), ts) {
            // edges on whole pixels: every pixel inside is covered
            let inside = edges.intersect(&shown);
            if inside.is_empty() {
                return Ok(());
            }
            c.write(&mut b.clips);
            let local = ctx.local(&inside);
            self.push_texture(
                ctx,
                b.floats(),
                inside,
                [Tex::Image(id), c.mask_tex(), cover],
                &[(local, (0.0, 0.0))],
            );
            return Ok(());
        }
        if let Some((tex, at, rect)) = self.path_cover(
            &rect_path,
            tiny_skia::FillRule::Winding,
            ts,
            None,
            None,
            area,
        )? {
            cover = tex;
            b.mode[3] = 1.0;
            b.cover_rect = [
                rect.x0 as f32,
                rect.y0 as f32,
                rect.x1 as f32,
                rect.y1 as f32,
            ];
            b.cover_at = [at.0 as f32, at.1 as f32, 0.0, 0.0];
        } else {
            return Ok(());
        }
        c.write(&mut b.clips);
        let local = ctx.local(&shown);
        self.push_texture(
            ctx,
            b.floats(),
            shown,
            [Tex::Image(id), c.mask_tex(), cover],
            &[(local, (0.0, 0.0))],
        );
        Ok(())
    }

    /// The image `id` as a premultiplied texture of the renderer's, uploaded when new or changed.
    pub(super) fn upload_image(&mut self, id: ResourceId) -> Result<(), RenderError> {
        let image = &self.request.resources.images[&id];
        let frames = self.r.frames;
        if let Some(t) = self.r.images.get_mut(&id) {
            if t.revision == image.revision {
                t.used = frames;
                return Ok(());
            }
        }
        if u64::from(image.width) * u64::from(image.height) > self.r.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let _upload = tracing::trace_span!(
            "gpu.upload_image",
            width = image.width,
            height = image.height
        )
        .entered();
        let row = image.width as usize * 4;
        let mut pixels = Vec::with_capacity(row * image.height as usize);
        for y in 0..image.height as usize {
            let src = &image.pixels[y * image.stride as usize..y * image.stride as usize + row];
            match image.format {
                PixelFormat::Rgba8Premultiplied => pixels.extend_from_slice(src),
                PixelFormat::Rgba8 => {
                    for s in src.as_chunks::<4>().0 {
                        let a = u32::from(s[3]);
                        for &c in &s[..3] {
                            pixels.push(div255(u32::from(c) * a) as u8);
                        }
                        pixels.push(s[3]);
                    }
                }
            }
        }
        let texture = self
            .device()
            .create_texture(&TextureDesc {
                size: (image.width, image.height),
                format: TextureFormat::Rgba8,
                usage: TextureUsage::SAMPLED,
            })
            .map_err(gpu)?;
        self.device()
            .write_texture(
                &texture,
                Region::new(0, 0, image.width, image.height),
                &pixels,
            )
            .map_err(gpu)?;
        self.r.stats.image_uploads += 1;
        if let Some(old) = self.r.images.insert(
            id,
            ImageTexture {
                revision: image.revision,
                texture,
                used: frames,
            },
        ) {
            self.r.device.destroy_texture(old.texture);
        }
        Ok(())
    }
}

/// Which atlas an upload goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Coverage,
    Color,
    Mask,
}

fn atlas_of<R>(r: &mut GpuRenderer<R>, kind: Kind) -> &mut Atlas {
    match kind {
        Kind::Coverage => &mut r.coverage,
        Kind::Color => &mut r.color,
        Kind::Mask => &mut r.masks,
    }
}

fn insert(
    atlas: &mut Atlas,
    device: &cupertino_gpu::Device,
    key: u64,
    size: (u32, u32),
    pixels: &[u8],
    frame: u64,
) -> Result<cupertino_gpu::atlas::Slot, GpuError> {
    atlas.insert(device, key, size, pixels, frame)
}

/// Moves `pos` past the `PopLayer` that closes the layer just opened.
/// A scratch image's or layer's `area`: painting a kept scroll layer, where it follows its node's own extent, no
/// larger than [`SCRATCH_PIXELS`], else cut to what `ctx` needs of it with `halo` around, as in the window.
fn bounded(area: Bounds, ctx: &Ctx, halo: i32) -> Bounds {
    if ctx.rect == EVERYWHERE && u64::from(area.width()) * u64::from(area.height()) > SCRATCH_PIXELS
    {
        area.intersect(&ctx.limit.outset(halo))
    } else {
        area
    }
}

fn skip_layer(node: &PaintNode, pos: &mut usize) {
    let mut depth = 1;
    while let Some(op) = node.ops.get(*pos) {
        *pos += 1;
        match op {
            PaintOp::PushLayer { .. } => depth += 1,
            PaintOp::PopLayer => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// A shape's corner exponent as the coverage programs take it: 0 for a quarter circle, else the exponent.
pub(super) fn corner_exponent(exponent: f32) -> f32 {
    if exponent > cupertino_raster_math::coverage::CIRCULAR_EXPONENT {
        exponent
    } else {
        0.0
    }
}

fn div255(v: u32) -> u32 {
    let v = v + 128;
    (v + (v >> 8)) >> 8
}

/// Multiplies `mask` (its first pixel at window pixel `origin`) by the coverage of `outline`, as the CPU renderer's
/// clip does: each pixel's coverage byte, a full pixel kept, an empty one cleared.
fn clip_by_outline(mask: &mut tiny_skia::Mask, o: &Outline, origin: (i32, i32)) {
    let w = mask.width() as i32;
    for (i, m) in mask.data_mut().iter_mut().enumerate() {
        let (x, y) = (origin.0 + i as i32 % w, origin.1 + i as i32 / w);
        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
        let [a, b, c, d, e, f] = o.inv;
        let p = [a * fx + c * fy + e, b * fx + d * fy + f];
        let dist = if o.corner > 0.0 {
            rounded_distance(p, o.half, o.corner, o.exponent) - o.outset
        } else {
            rect_distance(p, [o.half[0] + o.outset, o.half[1] + o.outset])
        };
        let cov = (coverage(dist, o.px) * 255.0 + 0.5) as u8;
        *m = match cov {
            255 => *m,
            0 => 0,
            c => div255(u32::from(*m) * u32::from(c)) as u8,
        };
    }
}

/// The halving's block: its edge rule, an unsigned integer, 1 (transparent past the layer).
fn level_uniform() -> [f32; LEVEL_FLOATS] {
    [f32::from_bits(1), 0.0, 0.0, 0.0]
}

/// The Gaussian pass's block: 36 weights in 9 `vec4`, the direction and radius as integers, the edge rule.
fn kernel_uniform(weights: &[f32], direction: (i32, i32)) -> [f32; KERNEL_FLOATS] {
    let mut u = [0.0f32; KERNEL_FLOATS];
    let radius = (weights.len() - 1).min(MAX_BLUR_RADIUS);
    u[..=radius].copy_from_slice(&weights[..=radius]);
    u[36] = f32::from_bits(direction.0 as u32);
    u[37] = f32::from_bits(direction.1 as u32);
    u[38] = f32::from_bits(radius as u32);
    u[39] = f32::from_bits(1);
    u
}
