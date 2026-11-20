use super::*;

impl<R: GlyphRasterizer> Painter<'_, R> {
    /// The last node painted into the frame whose opaque fill covers all of `d` with nothing over it but what
    /// comes after it: no group above it and no anti-aliased edge inside `d`. What is painted before it there is
    /// hidden.
    pub(super) fn occluder(
        &mut self,
        tree: &Tree<'_, '_>,
        d: &Bounds,
        whole: &Bounds,
    ) -> Option<usize> {
        let mut nodes: Vec<usize> = (0..tree.nodes.len()).collect();
        nodes.sort_unstable_by_key(|&i| std::cmp::Reverse(tree.order[i]));
        nodes.into_iter().find(|&i| {
            let node = &tree.nodes[i];
            let opaque = node
                .ops
                .iter()
                .any(|op| matches!(op, PaintOp::Fill { paint: Paint::Solid(c), .. } if c.a >= 1.0));
            if !opaque || tree.grouped[i] {
                return false;
            }
            // the node is reached only when it and every ancestor touch the rectangle
            let mut at = Some(i);
            while let Some(n) = at {
                if !Bounds::of_rect(&tree.nodes[n].bounds, self.scale).touches(d) {
                    return false;
                }
                at = tree.parent[n];
            }
            let extent = tree.extents[i];
            if extent.intersect(d) != *d {
                return false;
            }
            let chain = tree.clip_chain(i);
            let Some(area) = self.own_area(tree, i, &chain, whole, d) else {
                return false;
            };
            if self.direct(tree, i, &chain, true) {
                return false;
            }
            self.solid(node, &chain, &area)
                .is_some_and(|s| s.color[3] == 255 && s.core.iter().any(|c| c.intersect(d) == *d))
        })
    }

    /// The span path's analysis of a node painted through a scratch image covering `area` (see `spans`).
    pub(super) fn solid(
        &mut self,
        node: &PaintNode,
        chain: &[&PaintNode],
        area: &Bounds,
    ) -> Option<spans::Solid> {
        let solid_fill = |op: &PaintOp| {
            matches!(
                op,
                PaintOp::Fill {
                    shape: Shape::Rect(_) | Shape::RoundedRect { .. },
                    paint: Paint::Solid(_)
                }
            )
        };
        if !node.ops.iter().any(solid_fill) {
            return None;
        }
        let mut glyphs = vec![None; node.ops.len()];
        for (i, op) in node.ops.iter().enumerate() {
            if let PaintOp::Glyphs { origin, run, color } = op {
                let mut covered: Option<Bounds> = None;
                self.place_glyphs(node, origin, run, *color, |x, y, g| {
                    let b = Bounds {
                        x0: x,
                        y0: y,
                        x1: x.saturating_add(g.width as i32),
                        y1: y.saturating_add(g.height as i32),
                    };
                    covered = Some(covered.map_or(b, |c| c.union(&b)));
                })
                .ok()?;
                glyphs[i] = Some(covered.unwrap_or(Bounds {
                    x0: 0,
                    y0: 0,
                    x1: 0,
                    y1: 0,
                }));
            }
        }
        spans::solid(node, chain, self.scale, (area.x0, area.y0), area, &glyphs)
    }

    /// The pixels of `rect` a node's own operations can change where `limit` is needed, under the clips of `chain`.
    pub(super) fn own_area(
        &self,
        tree: &Tree<'_, '_>,
        i: usize,
        chain: &[&PaintNode],
        rect: &Bounds,
        limit: &Bounds,
    ) -> Option<Bounds> {
        let extent = tree.extents[i];
        if extent.is_empty() {
            return None;
        }
        let halo = node_support(&tree.nodes[i], self.scale);
        let mut area = extent
            .outset(halo)
            .intersect(&limit.outset(halo))
            .intersect(rect);
        for owner in chain {
            if let Some(clip) = &owner.clip {
                area = area.intersect(&geometry::clip_bounds(clip, &owner.transform, self.scale));
            }
        }
        (!area.is_empty()).then_some(area)
    }

    /// Whether a node's operations are painted straight into `t`: no clip cuts them and they only draw. Into the
    /// frame, only glyphs are: each of their pixels is blended on its own and the damage only picks which, while a fill
    /// cut by the damage would be rasterized with its paths cut there, and differ from the frame drawn whole.
    pub(super) fn direct(
        &self,
        tree: &Tree<'_, '_>,
        i: usize,
        chain: &[&PaintNode],
        frame: bool,
    ) -> bool {
        let node = &tree.nodes[i];
        let reach = tree.extents[i].outset(node_support(node, self.scale));
        // a clip that lets through, in whole pixels, all the node can reach cuts nothing of it, whatever the damage
        chain.iter().all(|owner| {
            owner
                .clip
                .as_ref()
                .is_none_or(|clip| geometry::clip_holds(clip, &owner.transform, self.scale, &reach))
        }) && node.ops.iter().all(|op| match op {
            PaintOp::Glyphs { .. } => true,
            PaintOp::Fill { .. } | PaintOp::Stroke { .. } | PaintOp::Image { .. } => !frame,
            _ => false,
        })
    }

    pub(super) fn subtree<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n, '_>,
        i: usize,
        chain: &mut Vec<&'n PaintNode>,
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        if !Bounds::of_rect(&node.bounds, self.scale).touches(&t.limit) {
            return Ok(());
        }
        if let Some(o) = t.hidden.filter(|&o| tree.order[i] < tree.order[o]) {
            self.stats.hidden += 1;
            if tree.last[i] < tree.order[o] {
                return Ok(());
            }
            // an ancestor of the node that hides it: its own operations are hidden, its clip still applies
            return self.children(t, tree, i, chain);
        }
        if let Some(group) = &node.group {
            return self.group(t, tree, i, group, chain);
        }
        self.own(t, tree, i, chain)?;
        if let Some(Some(view)) = self.views.get(i).filter(|_| t.frame) {
            return self.composite(t, tree, i, chain, view);
        }
        self.children(t, tree, i, chain)
    }

    /// Lays a kept scroll layer over the frame where its node's clip shows it. The pixels every clip above it and its
    /// own let through whole come straight from the image; at a clip's edge, where it covers part of a pixel, the
    /// content is painted in place as a frame without layers paints it, so the edge is cut node by node.
    pub(super) fn composite<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n, '_>,
        i: usize,
        chain: &mut Vec<&'n PaintNode>,
        view: &View<'_>,
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        let mut area = view.image.viewport.intersect(&t.limit);
        let mut whole = vec![area];
        for owner in chain.iter().copied().chain(std::iter::once(node)) {
            let Some(clip) = &owner.clip else { continue };
            match geometry::clip_cover(clip, &owner.transform, self.scale) {
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
        if area.is_empty() {
            return Ok(());
        }
        let whole: Vec<Bounds> = whole
            .iter()
            .map(|w| w.intersect(&area))
            .filter(|w| !w.is_empty())
            .collect();
        // where the frame is still only to be set to one colour (the background under the layer), the layer is laid
        // over that colour as it is written, in one pass: an empty run of it writes the colour, an opaque one is copied
        let known: Vec<(Bounds, [u8; 4])> = whole
            .iter()
            .flat_map(|piece| self.pending.take_set(piece))
            .collect();
        self.pending.flush(t.pixmap, t.origin, &area);
        for piece in &whole {
            let set: Vec<Bounds> = known.iter().map(|(k, _)| k.intersect(piece)).collect();
            for rest in spans::subtract(*piece, &set) {
                self.lay_rows(t, view, &rest, None);
            }
        }
        for (part, color) in &known {
            self.stats
                .scroll_on_background
                .add(u64::from(part.width()) * u64::from(part.height()) * 4);
            self.lay_rows(t, view, part, Some(*color));
        }
        for rim in spans::subtract(area, &whole) {
            let mut edge = Target {
                pixmap: &mut *t.pixmap,
                origin: t.origin,
                rect: t.rect,
                space: t.space,
                limit: rim,
                clips: Vec::new(),
                hidden: None,
                holes: Vec::new(),
                covering: Vec::new(),
                missing: Vec::new(),
                frame: true,
            };
            self.children(&mut edge, tree, i, chain)?;
            self.pending.flush(edge.pixmap, edge.origin, &rim);
        }
        Ok(())
    }

    /// Lays the window pixels `area` of a kept scroll layer over the target, or, given `under`, over that colour in
    /// place of what the target holds there.
    pub(super) fn lay_rows(
        &mut self,
        t: &mut Target<'_, '_>,
        view: &View<'_>,
        area: &Bounds,
        under: Option<[u8; 4]>,
    ) {
        if area.is_empty() {
            return;
        }
        let image = view.image;
        let height = image.pixels.height() as i32;
        self.stats
            .scroll_composite
            .add(u64::from(area.width()) * u64::from(area.height()) * 4);
        let content = area.offset(view.shift.0, view.shift.1);
        for (y0, y1, row) in layers::ring_pieces(content.y0, content.y1, height) {
            let part = Bounds {
                x0: content.x0 - image.x,
                y0: row,
                x1: content.x1 - image.x,
                y1: row + (y1 - y0),
            };
            let at = (area.x0 - t.origin.0, y0 - view.shift.1 - t.origin.1);
            match under {
                Some(color) => over_color_part(t.pixmap, at, image.pixels.as_ref(), &part, color),
                None => source_over_part(t.pixmap, at, image.pixels.as_ref(), &part, 1.0),
            }
        }
    }

    /// Paints the children under the node's own clip, which cuts descendants but not the node's operations.
    pub(super) fn children<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n, '_>,
        i: usize,
        chain: &mut Vec<&'n PaintNode>,
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        let clipped = node.clip.is_some();
        if clipped {
            chain.push(node);
        }
        let done = tree
            .children_of(i)
            .iter()
            .try_for_each(|&child| self.subtree(t, tree, child, chain));
        if clipped {
            chain.pop();
        }
        done
    }

    /// Paints a node and its subtree into a layer of the subtree's size, applies the effects, clips the result by
    /// the ancestors' clips and composites it once.
    pub(super) fn group<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n, '_>,
        i: usize,
        group: &crate::Group,
        chain: &[&'n PaintNode],
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        // the whole group, whatever the damage: its descendants are rasterized into the same pixels and its blurs and
        // shadows read the same ones in every frame
        let halo = node_support(node, self.scale);
        let area = bounded(
            Bounds::of_rect(&node.bounds, self.scale)
                .intersect(&t.space)
                .intersect(&self.window),
            t,
            halo,
        );
        // laid over at no opacity, the group changes no pixel (a panel's first frame as it fades in), so nothing of it
        // is painted
        if area.is_empty()
            || area.intersect(&t.limit).is_empty()
            || opacity_factor(group.opacity) == 0
        {
            return Ok(());
        }
        // a kept group laid over at its opacity where it is now: its subtree is not painted again
        let at = (node.transform.0[4], node.transform.0[5]);
        let content = (t.frame
            && self.groups.is_some()
            && matches!(node.layer, Some(Layer::Opacity | Layer::Transform)))
        .then(|| group_content(tree, i, at));
        if let Some(content) = &content {
            if let Some(image) = self.groups.as_mut().and_then(|g| g.remove(&node.id)) {
                if image.shows(self.scale, at, content, &area) {
                    self.stats.groups_kept += 1;
                    let laid = self.lay_group(t, &image.pixels, &area, group.opacity, chain);
                    if let Some(groups) = self.groups.as_mut() {
                        groups.insert(node.id, image);
                    }
                    return laid;
                }
                self.group_room += image.bytes();
            }
        }
        self.stats.groups_painted += 1;
        let mut layer = self.scratch(&area)?;
        {
            let mut pm = layer.as_mut();
            let mut inner = Target {
                pixmap: &mut pm,
                origin: (area.x0, area.y0),
                rect: area,
                space: area,
                limit: area,
                clips: Vec::new(),
                hidden: None,
                holes: Vec::new(),
                covering: Vec::new(),
                missing: Vec::new(),
                frame: false,
            };
            self.own(&mut inner, tree, i, &[])?;
            self.children(&mut inner, tree, i, &mut Vec::new())?;
        }
        for effect in &group.effects {
            effects::apply(&mut layer, effect, self.scale, (area.x0, area.y0));
        }
        let bytes = u64::from(area.width()) * u64::from(area.height()) * 4;
        if let Some(content) = content.filter(|_| bytes <= self.group_room) {
            // kept for the frames to come, within the renderer's cap
            self.group_room -= bytes;
            let laid = self.lay_group(t, &layer, &area, group.opacity, chain);
            let image = GroupImage {
                pixels: layer,
                scale: self.scale,
                area,
                at,
                content,
            };
            if let Some(groups) = self.groups.as_mut() {
                groups.insert(node.id, image);
            }
            return laid;
        }
        if let Some(mask) = self.chain_mask(chain, &area, &[])? {
            self.mask_layer(&mut layer, &mask, &area);
        }
        self.lay_over(t, layer, &area, group.opacity, Site::Group)
    }

    /// Multiplies a group's image covering `area` by the clips' `mask`.
    pub(super) fn mask_layer(&mut self, layer: &mut Pixmap, mask: &Mask, area: &Bounds) {
        self.stats
            .mask_apply
            .add(u64::from(area.width()) * u64::from(area.height()) * 4);
        // a pixel the clip covers fully keeps its value (div255(c · 255) is c), and runs of them are passed over
        for (pixels, cover) in layer
            .data_mut()
            .chunks_mut(4 * 16)
            .zip(mask.data().chunks(16))
        {
            if cover.iter().fold(255, |all, &m| all & m) == 255 {
                continue;
            }
            // scaling by 255 and by 0 is exact, so no pixel needs a branch of its own
            for (px, &m) in pixels.as_chunks_mut::<4>().0.iter_mut().zip(cover) {
                *px = effects::mul_px(u32::from_le_bytes(*px), u32::from(m)).to_le_bytes();
            }
        }
    }

    /// Lays a group's image covering `area` over the target at `opacity`, under the clips of `chain`, leaving the
    /// image as it is.
    pub(super) fn lay_group(
        &mut self,
        t: &mut Target<'_, '_>,
        image: &Pixmap,
        area: &Bounds,
        opacity: f32,
        chain: &[&PaintNode],
    ) -> Result<(), RenderError> {
        if let Some(mask) = self.chain_mask(chain, area, &[])? {
            let mut masked = image.clone();
            self.mask_layer(&mut masked, &mask, area);
            return self.lay_over(t, masked, area, opacity, Site::Group);
        }
        let visible = area.intersect(&t.limit);
        if visible.is_empty() {
            return Ok(());
        }
        if t.frame {
            self.pending.flush(t.pixmap, t.origin, &visible);
        }
        self.stats
            .group_composite
            .add(u64::from(visible.width()) * u64::from(visible.height()) * 4);
        source_over_part(
            t.pixmap,
            (visible.x0 - t.origin.0, visible.y0 - t.origin.1),
            image.as_ref(),
            &visible.offset(-area.x0, -area.y0),
            opacity,
        );
        Ok(())
    }

    /// Paints one node's own operations into a scratch image of just the pixels they can reach, under the
    /// ancestors' clips, and lays that over the target. A node that nothing clips or cuts, whose operations only draw,
    /// is painted straight into the target instead: source-over is associative, so the result is the same. A node of
    /// solid rectangles under rectangular clips writes the pixels its fills cover fully as rows and keeps the scratch
    /// image only for the rest (see `spans`).
    pub(super) fn own(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_, '_>,
        i: usize,
        chain: &[&PaintNode],
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        if self.own_area(tree, i, chain, &t.rect, &t.limit).is_none() {
            return Ok(());
        }
        // the pixels the node is painted into are the same whatever the damage, so its paths are rasterized the same
        // way and cut in the same places in every frame; the damage only picks which of them are kept
        let Some(area) = self.own_area(tree, i, chain, &t.space, &t.space) else {
            return Ok(());
        };
        if t.space == EVERYWHERE
            && u64::from(area.width()) * u64::from(area.height()) > layers::LAYER_SCRATCH_PIXELS
        {
            // too large for one scratch image: painted in squares fixed in the content, the same in every frame
            let tile = layers::LAYER_TILE;
            let needed = area.intersect(&t.limit.outset(node_support(node, self.scale)));
            for ty in needed.y0.div_euclid(tile)..=(needed.y1 - 1).div_euclid(tile) {
                for tx in needed.x0.div_euclid(tile)..=(needed.x1 - 1).div_euclid(tile) {
                    let square = Bounds {
                        x0: tx * tile,
                        y0: ty * tile,
                        x1: (tx + 1) * tile,
                        y1: (ty + 1) * tile,
                    };
                    let piece = area.intersect(&square);
                    if !piece.is_empty() {
                        self.own_in(t, tree, i, chain, piece)?;
                    }
                }
            }
            return Ok(());
        }
        self.own_in(t, tree, i, chain, area)
    }

    /// [`Self::own`] into the scratch image `area`.
    pub(super) fn own_in(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_, '_>,
        i: usize,
        chain: &[&PaintNode],
        area: Bounds,
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        let extent = tree.extents[i];
        self.stats.nodes_painted += 1;
        let halo = node_support(node, self.scale);
        let reach = Reach { extent, halo };
        if self.straight_image(t, tree, i, chain, &area) {
            return Ok(());
        }
        // in a scroll layer, whose targets are the rows being painted, a fill straight into the target would be cut at
        // their edges: only glyphs are, as in the frame
        if self.direct(tree, i, chain, t.frame || t.space == EVERYWHERE) {
            let needed = extent.outset(halo).intersect(&t.rect).intersect(&t.limit);
            if t.frame {
                self.pending.flush(t.pixmap, t.origin, &needed);
            }
            let mut inner = Target {
                pixmap: &mut *t.pixmap,
                origin: t.origin,
                rect: t.rect,
                space: t.space,
                limit: needed,
                clips: Vec::new(),
                hidden: None,
                holes: Vec::new(),
                covering: Vec::new(),
                missing: Vec::new(),
                frame: false,
            };
            let mut pos = 0;
            return self.run(&mut inner, node, reach, &mut pos);
        }
        // a node of solid fills is painted in the same rectangles on both paths, so they give the same bytes: the
        // pixels its fills cover fully, which the span path writes as rows, and the rest
        let Some(solid) = self.solid(node, chain, &area) else {
            return self.through_scratch(t, node, chain, reach, &area, None);
        };
        // the rim: what the fills reach outside the pixels they cover fully, with those pixels left out of them. Each
        // of its rectangles is rasterized into a scratch image of its own, the same in every frame, and only when the
        // damage needs some of it
        for piece in spans::subtract(solid.reach, &solid.core) {
            if piece.intersect(&t.limit).is_empty() {
                continue;
            }
            let rim = [piece];
            self.through_scratch(t, node, chain, reach, &piece, Some((&solid, &rim)))?;
        }
        for core in &solid.core {
            let shown = core.intersect(&t.limit);
            if shown.is_empty() {
                continue;
            }
            if !self.fast {
                self.through_scratch(t, node, chain, reach, core, None)?;
                continue;
            }
            self.stats
                .spans
                .add(u64::from(shown.width()) * u64::from(shown.height()) * 4);
            if t.frame {
                self.pending.over(t.pixmap, shown, solid.color);
            } else {
                spans::cover(
                    t.pixmap,
                    &shown.offset(-t.origin.0, -t.origin.1),
                    solid.color,
                );
            }
        }
        Ok(())
    }

    /// Paints a node's operations into a scratch image covering `area` and lays it over the target. With `cut`, the
    /// pixels of its first list are left out of the image and only those of its second list are laid over.
    pub(super) fn through_scratch(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        chain: &[&PaintNode],
        reach: Reach,
        area: &Bounds,
        cut: Option<(&spans::Solid, &[Bounds])>,
    ) -> Result<(), RenderError> {
        let (holes, covering) = cut.map_or_else(
            || (Vec::new(), Vec::new()),
            |(solid, _)| (solid.core.clone(), solid.covering.clone()),
        );
        // a rim's rectangle needs only the glyphs whose rasters reach it
        let needed = area.intersect(&t.limit);
        let missing = cut.map_or_else(Vec::new, |(solid, _)| {
            solid
                .glyphs
                .iter()
                .map(|g| g.is_some_and(|g| g.intersect(&needed).is_empty()))
                .collect()
        });
        let mut scratch = self.scratch(area)?;
        {
            let mut pm = scratch.as_mut();
            let mut inner = Target {
                pixmap: &mut pm,
                origin: (area.x0, area.y0),
                rect: *area,
                space: *area,
                // only the pixels kept are needed: glyphs are blended there alone, which changes no other pixel
                limit: area.intersect(&t.limit),
                clips: self.chain_mask(chain, area, &holes)?.into_iter().collect(),
                hidden: None,
                holes,
                covering,
                missing,
                frame: false,
            };
            let mut pos = 0;
            self.run(&mut inner, node, reach, &mut pos)?;
        }
        let Some((_, rim)) = cut else {
            return self.lay_over(t, scratch, area, 1.0, Site::Node);
        };
        for piece in rim {
            let visible = piece.intersect(&t.limit);
            if visible.is_empty() {
                continue;
            }
            self.stats
                .node_composite
                .add(u64::from(visible.width()) * u64::from(visible.height()) * 4);
            if t.frame {
                self.pending.flush(t.pixmap, t.origin, &visible);
            }
            source_over_part(
                t.pixmap,
                (visible.x0 - t.origin.0, visible.y0 - t.origin.1),
                scratch.as_ref(),
                &visible.offset(-area.x0, -area.y0),
                1.0,
            );
        }
        Ok(())
    }

    pub(super) fn scratch(&mut self, area: &Bounds) -> Result<Pixmap, RenderError> {
        let pixels = u64::from(area.width()) * u64::from(area.height());
        if pixels > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        self.stats.scratch_images.add(pixels * 4);
        Pixmap::new(area.width(), area.height()).ok_or(RenderError::ResourceLimit)
    }

    /// The coverage of every clip on the chain, as a mask of just `area`, leaving out `holes` (window pixels, see
    /// `holed`); `None` when nothing is clipped.
    pub(super) fn chain_mask(
        &mut self,
        chain: &[&PaintNode],
        area: &Bounds,
        holes: &[Bounds],
    ) -> Result<Option<Mask>, RenderError> {
        let mut mask: Option<Mask> = None;
        for owner in chain {
            let Some(clip) = &owner.clip else { continue };
            let m = match mask {
                Some(ref mut m) => m,
                None => {
                    self.stats
                        .masks
                        .add(u64::from(area.width()) * u64::from(area.height()));
                    mask.insert(full_mask(area.width(), area.height())?)
                }
            };
            if let Shape::Path { .. } = clip {
                // the rasterizer skips the holes, whose pixels are not used
                let ts = geometry::to_target(&owner.transform, self.scale, (area.x0, area.y0));
                let (path, ts) = holed(geometry::path(clip), ts, holes, (area.x0, area.y0));
                match path {
                    Some(path) => m.intersect_path(&path, geometry::fill_rule(clip), true, ts),
                    None => m.clear(),
                }
            } else {
                let window = geometry::to_target(&owner.transform, self.scale, (0, 0));
                clip_by(m, clip, window, (area.x0, area.y0));
            }
        }
        Ok(mask)
    }

    /// Composites `image`, which covers `area`, onto the part of the target whose result is needed.
    pub(super) fn lay_over(
        &mut self,
        t: &mut Target<'_, '_>,
        image: Pixmap,
        area: &Bounds,
        opacity: f32,
        site: Site,
    ) -> Result<(), RenderError> {
        let visible = area.intersect(&t.limit);
        if visible.is_empty() {
            return Ok(());
        }
        if t.frame {
            self.pending.flush(t.pixmap, t.origin, &visible);
        }
        let shown = if visible == *area {
            image
        } else {
            let crop = tiny_skia::IntRect::from_xywh(
                visible.x0 - area.x0,
                visible.y0 - area.y0,
                visible.width(),
                visible.height(),
            )
            .ok_or(RenderError::ResourceLimit)?;
            self.stats
                .crop
                .add(u64::from(visible.width()) * u64::from(visible.height()) * 4);
            image.clone_rect(crop).ok_or(RenderError::ResourceLimit)?
        };
        let moved = u64::from(visible.width()) * u64::from(visible.height()) * 4;
        match site {
            Site::Node => self.stats.node_composite.add(moved),
            Site::Group => self.stats.group_composite.add(moved),
        }
        source_over(
            t.pixmap,
            (visible.x0 - t.origin.0, visible.y0 - t.origin.1),
            shown.as_ref(),
            opacity,
        );
        Ok(())
    }

    pub(super) fn run(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        reach: Reach,
        pos: &mut usize,
    ) -> Result<(), RenderError> {
        while let Some(op) = node.ops.get(*pos) {
            *pos += 1;
            match op {
                PaintOp::Fill { shape, paint } => {
                    self.stats.fills += 1;
                    let holed = t.covering.get(*pos - 1).copied().unwrap_or(false);
                    self.fill(t, node, shape, paint, holed)
                }
                PaintOp::Stroke {
                    shape,
                    width,
                    paint,
                } => {
                    self.stats.strokes += 1;
                    self.stroke(t, node, shape, *width, paint)
                }
                PaintOp::Glyphs { origin, run, color } => {
                    if !t.missing.get(*pos - 1).copied().unwrap_or(false) {
                        self.glyph_run(t, node, origin, run, *color)?
                    }
                }
                PaintOp::Image {
                    image,
                    src,
                    dst,
                    sampling,
                } => {
                    self.stats.images += 1;
                    self.image(t, node, image.id, src, dst, *sampling)?
                }
                PaintOp::PushClip(shape) => self.push_clip(t, node, shape)?,
                PaintOp::PopClip => {
                    t.clips.pop();
                }
                PaintOp::PushLayer { opacity, effects } => {
                    self.layer(t, node, reach, pos, *opacity, effects)?
                }
                PaintOp::PopLayer => return Ok(()),
            }
        }
        Ok(())
    }

    pub(super) fn ts(&self, t: &Target<'_, '_>, node: &PaintNode) -> tiny_skia::Transform {
        geometry::to_target(&node.transform, self.scale, t.origin)
    }

    /// Node coordinates to window pixels.
    pub(super) fn window_ts(&self, node: &PaintNode) -> tiny_skia::Transform {
        geometry::to_target(&node.transform, self.scale, (0, 0))
    }

    pub(super) fn push_clip(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        shape: &Shape,
    ) -> Result<(), RenderError> {
        let (w, h) = (t.pixmap.width(), t.pixmap.height());
        self.stats.masks.add(u64::from(w) * u64::from(h));
        let mut mask = match t.mask() {
            Some(m) => m.clone(),
            None => full_mask(w, h)?,
        };
        let window = geometry::to_target(&node.transform, self.scale, (0, 0));
        clip_by(&mut mask, shape, window, t.origin);
        t.clips.push(mask);
        Ok(())
    }

    /// Fills `shape`; when `holed`, without the target's holes.
    pub(super) fn fill(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        shape: &Shape,
        paint: &Paint,
        holed: bool,
    ) {
        let holes: Vec<Bounds> = if holed { t.holes.clone() } else { Vec::new() };
        if !matches!(shape, Shape::Path { .. }) {
            if let Some(outline) = raster::Outline::new(shape, self.window_ts(node), None) {
                self.lay(t, node, &outline, paint, &holes);
            }
            return;
        }
        let (path, ts) = self::holed(geometry::path(shape), self.ts(t, node), &holes, t.origin);
        let Some(path) = path else { return };
        if !matches!(paint, Paint::Solid(_)) {
            return self.lay_path(t, node, &path, geometry::fill_rule(shape), ts, paint);
        }
        let Some(paint) = skia_paint(paint) else {
            return;
        };
        t.pixmap.fill_path(
            &path,
            &paint,
            geometry::fill_rule(shape),
            ts,
            t.clips.last(),
        );
    }

    /// Lays the gradient `paint` through the coverage of `path` (placed by `ts` on the target) and the target's clip.
    pub(super) fn lay_path(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        path: &tiny_skia::Path,
        rule: tiny_skia::FillRule,
        ts: tiny_skia::Transform,
        paint: &Paint,
    ) {
        let (w, h) = (t.pixmap.width(), t.pixmap.height());
        let Some(mut cover) = Mask::new(w, h) else {
            return;
        };
        cover.fill_path(path, rule, true, ts);
        if let Some(clip) = t.clips.last() {
            for (c, m) in cover.data_mut().iter_mut().zip(clip.data()) {
                *c = div255(u32::from(*c) * u32::from(*m)) as u8;
            }
        }
        let Some(placed) = path.clone().transform(ts) else {
            return;
        };
        let b = placed.bounds();
        let reach = Bounds {
            x0: b.left().floor() as i32 - 1 + t.origin.0,
            y0: b.top().floor() as i32 - 1 + t.origin.1,
            x1: b.right().ceil() as i32 + 1 + t.origin.0,
            y1: b.bottom().ceil() as i32 + 1 + t.origin.1,
        };
        let area = t.limit.intersect(&t.rect).intersect(&reach);
        let Some(gradient) = raster::Gradient::new(paint, self.window_ts(node)) else {
            return;
        };
        if area.is_empty() {
            return;
        }
        let data = gradient.shade(&area, dither::BY_DEFAULT);
        let shade = raster::Shade::Pixels { data: &data, area };
        raster::fill_masked(t.pixmap, t.origin, &area, &shade, &cover);
    }

    pub(super) fn stroke(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        shape: &Shape,
        width: f64,
        paint: &Paint,
    ) {
        if !matches!(shape, Shape::Path { .. }) {
            if let Some(outline) = raster::Outline::new(shape, self.window_ts(node), Some(width)) {
                self.lay(t, node, &outline, paint, &[]);
            }
            return;
        }
        let Some(path) = geometry::path(shape) else {
            return;
        };
        let stroke = Stroke {
            width: width as f32,
            ..Stroke::default()
        };
        let ts = self.ts(t, node);
        if !matches!(paint, Paint::Solid(_)) {
            // the stroker's outline, filled
            let scale = ts.get_scale();
            let Some(outline) = path.stroke(&stroke, scale.0.max(scale.1)) else {
                return;
            };
            return self.lay_path(t, node, &outline, tiny_skia::FillRule::Winding, ts, paint);
        }
        let Some(paint) = skia_paint(paint) else {
            return;
        };
        t.pixmap
            .stroke_path(&path, &paint, &stroke, ts, t.clips.last());
    }

    /// Lays `paint` through the coverage of `outline` over the target, where its result is needed.
    pub(super) fn lay(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        outline: &raster::Outline,
        paint: &Paint,
        holes: &[Bounds],
    ) {
        let area = t.limit.intersect(&t.rect).intersect(&outline.bounds);
        if area.is_empty() {
            return;
        }
        let mask = t.clips.last();
        match paint {
            Paint::Solid(c) => {
                let Some(color) = raster::premultiplied(*c) else {
                    return;
                };
                let shade = raster::Shade::Solid(color);
                raster::fill(t.pixmap, t.origin, &area, outline, &shade, mask, holes);
            }
            _ => {
                let Some(gradient) = raster::Gradient::new(paint, self.window_ts(node)) else {
                    return;
                };
                let data = gradient.shade(&area, dither::BY_DEFAULT);
                let shade = raster::Shade::Pixels { data: &data, area };
                raster::fill(t.pixmap, t.origin, &area, outline, &shade, mask, holes);
            }
        }
    }

    pub(super) fn image(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        id: crate::ResourceId,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
    ) -> Result<(), RenderError> {
        let resources = self.resources;
        let image = &resources.images[&id];
        if u64::from(image.width) * u64::from(image.height) > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let Some(Placement {
            crop,
            place,
            ts,
            whole,
        }) = self.placement(t, node, image, src, dst)
        else {
            return Ok(());
        };
        if let Some(at) = whole {
            let mask = t.clips.last().map(Mask::data);
            // premultiplied pixels are read where they are, with no copy kept
            if image.format == PixelFormat::Rgba8Premultiplied {
                lay_over(
                    t.pixmap,
                    mask,
                    &image.pixels,
                    image.stride as usize,
                    crop,
                    at,
                );
            } else {
                let whole = self.images.get(id, image, self.config.max_pixels)?;
                lay_over(
                    t.pixmap,
                    mask,
                    whole.data(),
                    whole.width() as usize * 4,
                    crop,
                    at,
                );
            }
            return Ok(());
        }
        let whole = self.images.get(id, image, self.config.max_pixels)?;
        let Some(part) = whole.clone_rect(crop) else {
            return Ok(());
        };
        let quality = match sampling {
            Sampling::Nearest => FilterQuality::Nearest,
            Sampling::Linear => FilterQuality::Bilinear,
        };
        let paint = tiny_skia::Paint {
            shader: Pattern::new(part.as_ref(), SpreadMode::Pad, quality, 1.0, place),
            ..tiny_skia::Paint::default()
        };
        let Some(path) = geometry::path(&Shape::Rect(*dst)) else {
            return Ok(());
        };
        t.pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            ts,
            t.clips.last(),
        );
        Ok(())
    }

    /// Where `src` of `image` drawn into `dst` (node coordinates) lands in the target: the source pixels it reads, how
    /// they are placed and the node's transform to the target; `None` when it draws nothing.
    pub(super) fn placement(
        &self,
        t: &Target<'_, '_>,
        node: &PaintNode,
        image: &crate::ImageResource,
        src: &Rect,
        dst: &Rect,
    ) -> Option<Placement> {
        if src.width() <= 0.0 || src.height() <= 0.0 || dst.width() <= 0.0 || dst.height() <= 0.0 {
            return None;
        }
        let (x0, y0) = (src.min_x().floor().max(0.0), src.min_y().floor().max(0.0));
        let (x1, y1) = (
            src.max_x().ceil().min(f64::from(image.width)),
            src.max_y().ceil().min(f64::from(image.height)),
        );
        let crop = tiny_skia::IntRect::from_xywh(
            x0 as i32,
            y0 as i32,
            (x1 - x0) as u32,
            (y1 - y0) as u32,
        )?;
        let (sx, sy) = (dst.width() / src.width(), dst.height() / src.height());
        let place = tiny_skia::Transform::from_row(
            sx as f32,
            0.0,
            0.0,
            sy as f32,
            (dst.min_x() + (x0 - src.min_x()) * sx) as f32,
            (dst.min_y() + (y0 - src.min_y()) * sy) as f32,
        );
        let mut ts = self.ts(t, node);
        let mut whole = None;
        // an image drawn one source pixel per device pixel lands on whole pixels, so it is copied, not resampled; the
        // pixel is chosen in window pixels, so every piece of the window it is drawn into puts it in the same place
        let device = ts.pre_concat(place);
        if device.kx == 0.0
            && device.ky == 0.0
            && (device.sx - 1.0).abs() < 1e-4
            && (device.sy - 1.0).abs() < 1e-4
        {
            let window = self.window_ts(node).pre_concat(place);
            let x = whole_pixel(window.tx) - t.origin.0 as f32;
            let y = whole_pixel(window.ty) - t.origin.1 as f32;
            ts = ts.post_translate(x - device.tx, y - device.ty);
            // whole source pixels filling a destination on whole pixels are laid over directly, with the same result
            if x0 == src.min_x()
                && y0 == src.min_y()
                && x1 == src.max_x()
                && y1 == src.max_y()
                && on_pixels(&ts, dst, (x, y), crop)
            {
                whole = Some((x as i32, y as i32));
            }
        }
        Some(Placement {
            crop,
            place,
            ts,
            whole,
        })
    }

    /// Paints a node of the frame whose one operation lays premultiplied pixels one to a device pixel, and which
    /// nothing clips, straight from the image's pixels: its scratch image would hold just those pixels, so they are
    /// laid over the frame as that image would be, with the same bytes, and where the frame is still only to be set
    /// to one colour they are laid over that colour as it is written. Says whether it painted the node; one it does
    /// not take goes through a scratch image as ever.
    pub(super) fn straight_image(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_, '_>,
        i: usize,
        chain: &[&PaintNode],
        area: &Bounds,
    ) -> bool {
        let node = &tree.nodes[i];
        let [PaintOp::Image {
            image: resource,
            src,
            dst,
            ..
        }] = node.ops.as_slice()
        else {
            return false;
        };
        if !self.fast || !t.frame || !t.clips.is_empty() || !t.holes.is_empty() {
            return false;
        }
        let reach = tree.extents[i].outset(node_support(node, self.scale));
        let unclipped = chain.iter().all(|owner| {
            owner
                .clip
                .as_ref()
                .is_none_or(|clip| geometry::clip_holds(clip, &owner.transform, self.scale, &reach))
        });
        let resources = self.resources;
        let Some(image) = resources.images.get(&resource.id) else {
            return false;
        };
        if !unclipped
            || image.format != PixelFormat::Rgba8Premultiplied
            || image.stride != image.width.saturating_mul(4)
            || u64::from(image.width) * u64::from(image.height) > self.config.max_pixels
        {
            return false;
        }
        let Some(Placement {
            crop,
            whole: Some(at),
            ..
        }) = self.placement(t, node, image, src, dst)
        else {
            return false;
        };
        let Some(pixels) = PixmapRef::from_bytes(&image.pixels, image.width, image.height) else {
            return false;
        };
        self.stats.images += 1;
        // the image's pixels in window pixels, and those of them the scratch image would lay over
        let (left, top) = (at.0 + t.origin.0, at.1 + t.origin.1);
        let laid = Bounds {
            x0: left,
            y0: top,
            x1: left + crop.width() as i32,
            y1: top + crop.height() as i32,
        };
        let visible = area.intersect(&t.limit);
        let shown = laid.intersect(&visible);
        let known = if shown.is_empty() {
            Vec::new()
        } else {
            self.pending.take_set(&shown)
        };
        self.pending.flush(t.pixmap, t.origin, &visible);
        self.stats
            .node_composite
            .add(u64::from(shown.width()) * u64::from(shown.height()) * 4);
        let to_image = |b: &Bounds| b.offset(crop.x() - left, crop.y() - top);
        let to_target = |b: &Bounds| (b.x0 - t.origin.0, b.y0 - t.origin.1);
        let set: Vec<Bounds> = known.iter().map(|(k, _)| *k).collect();
        for rest in spans::subtract(shown, &set) {
            if !rest.is_empty() {
                source_over_part(t.pixmap, to_target(&rest), pixels, &to_image(&rest), 1.0);
            }
        }
        for (part, color) in &known {
            over_color_part(t.pixmap, to_target(part), pixels, &to_image(part), *color);
        }
        true
    }

    pub(super) fn glyph_run(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        origin: &cupertino_geometry::Point,
        run: &cupertino_ui_text::GlyphRun,
        color: Color,
    ) -> Result<(), RenderError> {
        self.stats.glyphs += run.glyphs.len() as u32;
        let mask = t.clips.last().map(Mask::data);
        let limit = t.limit.offset(-t.origin.0, -t.origin.1);
        let (pixmap, at) = (&mut *t.pixmap, t.origin);
        self.place_glyphs(node, origin, run, color, |x, y, glyph| {
            blend_glyph(pixmap, mask, &limit, x - at.0, y - at.1, glyph, color);
        })
    }

    /// Hands each glyph of a run with a raster to `each`, with the window pixel of its raster's top-left corner.
    pub(super) fn place_glyphs(
        &mut self,
        node: &PaintNode,
        origin: &cupertino_geometry::Point,
        run: &cupertino_ui_text::GlyphRun,
        color: Color,
        mut each: impl FnMut(i32, i32, &RasterGlyph),
    ) -> Result<(), RenderError> {
        let [a, b, c, d, e, f] = node.transform.0;
        let size_px = f64::from(run.size) * self.scale * a.hypot(b);
        if size_px > MAX_GLYPH_PX {
            return Err(RenderError::ResourceLimit);
        }
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
                self.config.font_smoothing,
                [color.r, color.g, color.b],
            );
            let Some(glyph) = self.glyphs.get(key, self.rasterizer)? else {
                continue;
            };
            let x = ix as i32 + glyph.bearing_x;
            let y = py.round() as i32 - glyph.bearing_y;
            each(x, y, &glyph);
        }
        Ok(())
    }

    pub(super) fn layer(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        reach: Reach,
        pos: &mut usize,
        opacity: f32,
        effects: &[crate::Effect],
    ) -> Result<(), RenderError> {
        // the layer's whole reach, whatever the damage, as for a group
        let area = bounded(
            reach
                .extent
                .outset(reach.halo)
                .intersect(&t.space)
                .intersect(&self.window),
            t,
            reach.halo,
        );
        if area.is_empty() || area.intersect(&t.limit.outset(reach.halo)).is_empty() {
            skip_layer(node, pos);
            return Ok(());
        }
        if u64::from(area.width()) * u64::from(area.height()) > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let moved = u64::from(area.width()) * u64::from(area.height()) * 4;
        self.stats.layers.add(moved);
        let mut layer =
            Pixmap::new(area.width(), area.height()).ok_or(RenderError::ResourceLimit)?;
        {
            let mut pm = layer.as_mut();
            let mut inner = Target {
                pixmap: &mut pm,
                origin: (area.x0, area.y0),
                rect: area,
                space: area,
                limit: area,
                clips: Vec::new(),
                hidden: None,
                holes: Vec::new(),
                covering: Vec::new(),
                missing: Vec::new(),
                frame: false,
            };
            self.run(&mut inner, node, reach, pos)?;
        }
        for effect in effects {
            effects::apply(&mut layer, effect, self.scale, (area.x0, area.y0));
        }
        let paint = PixmapPaint {
            opacity,
            ..PixmapPaint::default()
        };
        self.stats.layer_composite.add(moved);
        t.pixmap.draw_pixmap(
            area.x0 - t.origin.0,
            area.y0 - t.origin.1,
            layer.as_ref(),
            &paint,
            tiny_skia::Transform::identity(),
            t.clips.last(),
        );
        Ok(())
    }
}
