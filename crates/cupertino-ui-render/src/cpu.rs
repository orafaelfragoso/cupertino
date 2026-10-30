use crate::effects::{self, div255};
use crate::geometry::{self, Bounds};
use crate::glyph_cache::GlyphCache;
use crate::image_cache::ImageCache;
use crate::stats::RenderStats;
use crate::validate;
use crate::{
    CpuFrame, DamageRegion, Paint, PaintNode, PaintNodeId, PaintOp, PixelFormat, PixelRect,
    RenderError, RenderFrameId, RenderRequest, Sampling, Shape, UiRenderer,
};
use cupertino_geometry::Rect;
use cupertino_theme::Color;
use cupertino_ui_text::{GlyphFormat, GlyphKey, GlyphRasterizer, RasterGlyph};
use std::collections::HashMap;
use tiny_skia::{
    FilterQuality, GradientStop, LinearGradient, Mask, Pattern, Pixmap, PixmapMut, PixmapPaint,
    Point, RadialGradient, SpreadMode, Stroke,
};

/// Largest glyph the renderer rasterizes, in physical pixels per em.
const MAX_GLYPH_PX: f64 = 8192.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuConfig {
    /// The most pixels the target or one layer may have.
    pub max_pixels: u64,
    pub glyph_cache_bytes: usize,
    pub image_cache_bytes: usize,
    /// Darkens glyph stems by the fill colour; on by default, off only when the user turns font smoothing off.
    pub font_smoothing: bool,
}

/// Paints display lists into a premultiplied RGBA image it keeps between frames, repainting only the damage.
pub struct CpuRenderer<R> {
    config: CpuConfig,
    rasterizer: R,
    glyphs: GlyphCache,
    images: ImageCache,
    frame: CpuFrame,
    scale: f64,
    /// The image holds the last frame's complete contents.
    valid: bool,
    next_id: u64,
    stats: RenderStats,
}

impl<R: GlyphRasterizer> CpuRenderer<R> {
    pub fn new(config: CpuConfig, rasterizer: R) -> CpuRenderer<R> {
        CpuRenderer {
            glyphs: GlyphCache::new(config.glyph_cache_bytes),
            images: ImageCache::new(config.image_cache_bytes),
            frame: CpuFrame {
                id: RenderFrameId(0),
                width: 0,
                height: 0,
                stride: 0,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Vec::new(),
                damage: Vec::new(),
            },
            config,
            rasterizer,
            scale: 1.0,
            valid: false,
            next_id: 1,
            stats: RenderStats::default(),
        }
    }

    /// What the last [`render`](UiRenderer::render) call did, counted; empty when the request was refused before painting.
    pub fn stats(&self) -> &RenderStats {
        &self.stats
    }

    fn prepare(&mut self, request: &RenderRequest<'_>) -> Result<bool, RenderError> {
        validate::check(
            request.list,
            request.resources,
            request.scale,
            request.width,
            request.height,
        )?;
        let stride = request
            .width
            .checked_mul(4)
            .ok_or(RenderError::ResourceLimit)?;
        let len = u64::from(stride) * u64::from(request.height);
        if u64::from(request.width) * u64::from(request.height) > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        let resized = request.width != self.frame.width
            || request.height != self.frame.height
            || request.scale != self.scale
            || !self.valid;
        let full = resized || matches!(request.damage, DamageRegion::Full);
        self.valid = false;
        if full {
            let len = usize::try_from(len).map_err(|_| RenderError::ResourceLimit)?;
            self.frame.pixels.clear();
            self.frame
                .pixels
                .try_reserve_exact(len)
                .map_err(|_| RenderError::ResourceLimit)?;
            self.frame.pixels.resize(len, 0);
            self.frame.width = request.width;
            self.frame.height = request.height;
            self.frame.stride = stride;
            self.scale = request.scale;
        }
        Ok(full)
    }

    fn paint(&mut self, request: &RenderRequest<'_>, dirty: &[Bounds]) -> Result<(), RenderError> {
        let (width, height, scale) = (request.width, request.height, request.scale);
        let whole = Bounds::of_size(width, height);
        let mut base = PixmapMut::from_bytes(&mut self.frame.pixels, width, height)
            .ok_or(RenderError::InvalidSize)?;
        let tree = Tree::new(&request.list.nodes, scale);
        let mut painter = Painter {
            stats: &mut self.stats,
            config: &self.config,
            scale,
            resources: request.resources,
            glyphs: &mut self.glyphs,
            images: &mut self.images,
            rasterizer: &mut self.rasterizer,
        };
        for d in dirty {
            clear(&mut base, d);
            painter
                .stats
                .clear
                .add(u64::from(d.width()) * u64::from(d.height()) * 4);
            let mut target = Target {
                pixmap: &mut base,
                origin: (0, 0),
                rect: whole,
                limit: *d,
                clips: Vec::new(),
            };
            for &root in &tree.roots {
                painter.subtree(&mut target, &tree, root, &mut Vec::new())?;
            }
        }
        Ok(())
    }
}

impl<R: GlyphRasterizer> UiRenderer for CpuRenderer<R> {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        self.stats = RenderStats::default();
        let full = self.prepare(request)?;
        let (width, height, scale) = (request.width, request.height, request.scale);
        let whole = Bounds::of_size(width, height);
        let dirty = match (full, request.damage) {
            (false, DamageRegion::Rects(rects)) => geometry::merge(
                rects
                    .iter()
                    .map(|r| Bounds::of_rect(r, scale).intersect(&whole))
                    .collect(),
            ),
            _ => vec![whole],
        };
        self.stats = RenderStats {
            dirty_rects: dirty.len() as u32,
            dirty_pixels: dirty
                .iter()
                .map(|d| u64::from(d.width()) * u64::from(d.height()))
                .sum(),
            frame_bytes: self.frame.pixels.len() as u64,
            ..RenderStats::default()
        };
        let painted = self.paint(request, &dirty);
        self.stats.glyph_rasterize = self.glyphs.take_filled();
        self.stats.image_decode = self.images.take_filled();
        painted?;
        self.glyphs.trim();
        self.images.trim();
        self.stats.retained_glyphs = self.glyphs.retained();
        self.stats.retained_images = self.images.retained();
        self.frame.damage = dirty
            .iter()
            .map(|d| PixelRect {
                x: d.x0 as u32,
                y: d.y0 as u32,
                width: d.width(),
                height: d.height(),
            })
            .collect();
        self.frame.id = RenderFrameId(self.next_id);
        self.next_id += 1;
        self.valid = true;
        Ok(self.frame.id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        if self.valid && id == self.frame.id {
            Ok(&self.frame)
        } else {
            Err(RenderError::UnknownFrame(id))
        }
    }
}

fn clear(base: &mut PixmapMut<'_>, d: &Bounds) {
    let stride = base.width() as usize * 4;
    for y in d.y0..d.y1 {
        let row = y as usize * stride;
        base.data_mut()[row + d.x0 as usize * 4..row + d.x1 as usize * 4].fill(0);
    }
}

/// One pixmap being painted and where it sits in the window's pixels.
struct Target<'a, 'b> {
    pixmap: &'a mut PixmapMut<'b>,
    origin: (i32, i32),
    /// The window pixels the pixmap covers.
    rect: Bounds,
    /// The window pixels whose result is needed.
    limit: Bounds,
    clips: Vec<Mask>,
}

impl Target<'_, '_> {
    fn mask(&self) -> Option<&Mask> {
        self.clips.last()
    }
}

/// Which composite a layer image is laid over the target by, for counting.
#[derive(Clone, Copy)]
enum Site {
    Node,
    Group,
}

/// How far a node's own operations reach: their extent and what its layers' effects add around it.
#[derive(Clone, Copy)]
struct Reach {
    extent: Bounds,
    halo: i32,
}

/// The forest of a display list: each node's children and the roots, in list order.
struct Tree<'n> {
    nodes: &'n [PaintNode],
    /// Each node's own extent in window pixels.
    extents: Vec<Bounds>,
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

impl<'n> Tree<'n> {
    fn new(nodes: &'n [PaintNode], scale: f64) -> Tree<'n> {
        let index: HashMap<PaintNodeId, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let mut children = vec![Vec::new(); nodes.len()];
        let mut roots = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            match node.parent.and_then(|p| index.get(&p)) {
                Some(&parent) => children[parent].push(i),
                None => roots.push(i),
            }
        }
        Tree {
            nodes,
            extents: nodes
                .iter()
                .map(|n| geometry::own_extent(n, scale))
                .collect(),
            children,
            roots,
        }
    }
}

struct Painter<'a, R> {
    stats: &'a mut RenderStats,
    config: &'a CpuConfig,
    scale: f64,
    resources: &'a crate::PaintResources,
    glyphs: &'a mut GlyphCache,
    images: &'a mut ImageCache,
    rasterizer: &'a mut R,
}

impl<R: GlyphRasterizer> Painter<'_, R> {
    fn subtree<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n>,
        i: usize,
        chain: &mut Vec<&'n PaintNode>,
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        if !Bounds::of_rect(&node.bounds, self.scale).touches(&t.limit) {
            return Ok(());
        }
        if let Some(group) = &node.group {
            return self.group(t, tree, i, group, chain);
        }
        self.own(t, tree, i, chain)?;
        self.children(t, tree, i, chain)
    }

    /// Paints the children under the node's own clip, which cuts descendants but not the node's operations.
    fn children<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n>,
        i: usize,
        chain: &mut Vec<&'n PaintNode>,
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        let clipped = node.clip.is_some();
        if clipped {
            chain.push(node);
        }
        let done = tree.children[i]
            .iter()
            .try_for_each(|&child| self.subtree(t, tree, child, chain));
        if clipped {
            chain.pop();
        }
        done
    }

    /// Paints a node and its subtree into a layer of the subtree's size, applies the effects, clips the result by
    /// the ancestors' clips and composites it once.
    fn group<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n>,
        i: usize,
        group: &crate::Group,
        chain: &[&'n PaintNode],
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        let halo = effects::support(&group.effects, self.scale)
            .saturating_add(node_support(node, self.scale));
        let area = Bounds::of_rect(&node.bounds, self.scale)
            .intersect(&t.limit.outset(halo))
            .intersect(&t.rect);
        if area.is_empty() {
            return Ok(());
        }
        self.stats.groups_painted += 1;
        let mut layer = self.scratch(&area)?;
        {
            let mut pm = layer.as_mut();
            let mut inner = Target {
                pixmap: &mut pm,
                origin: (area.x0, area.y0),
                rect: area,
                limit: area,
                clips: Vec::new(),
            };
            self.own(&mut inner, tree, i, &[])?;
            self.children(&mut inner, tree, i, &mut Vec::new())?;
        }
        for effect in &group.effects {
            effects::apply(&mut layer, effect, self.scale);
        }
        if let Some(mask) = self.chain_mask(chain, &area)? {
            self.stats
                .mask_apply
                .add(u64::from(area.width()) * u64::from(area.height()) * 4);
            for (px, m) in layer
                .data_mut()
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(mask.data())
            {
                for c in px.iter_mut() {
                    *c = div255(u32::from(*c) * u32::from(*m)) as u8;
                }
            }
        }
        self.lay_over(t, layer, &area, group.opacity, Site::Group)
    }

    /// Paints one node's own operations into a scratch image of just the pixels they can reach, under the
    /// ancestors' clips, and lays that over the target. A node that nothing clips or cuts, whose operations only draw,
    /// is painted straight into the target instead: source-over is associative, so the result is the same.
    fn own(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_>,
        i: usize,
        chain: &[&PaintNode],
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        let extent = tree.extents[i];
        if extent.is_empty() {
            return Ok(());
        }
        let halo = node_support(node, self.scale);
        let mut area = extent
            .outset(halo)
            .intersect(&t.limit.outset(halo))
            .intersect(&t.rect);
        for owner in chain {
            if let Some(clip) = &owner.clip {
                area = area.intersect(&geometry::clip_bounds(clip, &owner.transform, self.scale));
            }
        }
        if area.is_empty() {
            return Ok(());
        }
        self.stats.nodes_painted += 1;
        let reach = extent.outset(halo).intersect(&t.rect);
        let direct = chain.iter().all(|owner| owner.clip.is_none())
            && reach.intersect(&t.limit) == reach
            && node.ops.iter().all(|op| {
                matches!(
                    op,
                    PaintOp::Fill { .. }
                        | PaintOp::Stroke { .. }
                        | PaintOp::Glyphs { .. }
                        | PaintOp::Image { .. }
                )
            });
        if direct {
            let mut inner = Target {
                pixmap: &mut *t.pixmap,
                origin: t.origin,
                rect: t.rect,
                limit: reach,
                clips: Vec::new(),
            };
            let mut pos = 0;
            return self.run(&mut inner, node, Reach { extent, halo }, &mut pos);
        }
        let mut scratch = self.scratch(&area)?;
        {
            let mut pm = scratch.as_mut();
            let mut inner = Target {
                pixmap: &mut pm,
                origin: (area.x0, area.y0),
                rect: area,
                limit: area,
                clips: self.chain_mask(chain, &area)?.into_iter().collect(),
            };
            let mut pos = 0;
            self.run(&mut inner, node, Reach { extent, halo }, &mut pos)?;
        }
        self.lay_over(t, scratch, &area, 1.0, Site::Node)
    }

    fn scratch(&mut self, area: &Bounds) -> Result<Pixmap, RenderError> {
        let pixels = u64::from(area.width()) * u64::from(area.height());
        if pixels > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        self.stats.scratch_images.add(pixels * 4);
        Pixmap::new(area.width(), area.height()).ok_or(RenderError::ResourceLimit)
    }

    /// The coverage of every clip on the chain, as a mask of just `area`; `None` when nothing is clipped.
    fn chain_mask(
        &mut self,
        chain: &[&PaintNode],
        area: &Bounds,
    ) -> Result<Option<Mask>, RenderError> {
        let mut mask: Option<Mask> = None;
        for owner in chain {
            let Some(clip) = &owner.clip else { continue };
            let ts = geometry::to_target(&owner.transform, self.scale, (area.x0, area.y0));
            let path = geometry::path(clip);
            let rule = geometry::fill_rule(clip);
            match (mask.as_mut(), path) {
                (None, path) => {
                    self.stats
                        .masks
                        .add(u64::from(area.width()) * u64::from(area.height()));
                    let mut m =
                        Mask::new(area.width(), area.height()).ok_or(RenderError::ResourceLimit)?;
                    if let Some(path) = path {
                        m.fill_path(&path, rule, true, ts);
                    }
                    mask = Some(m);
                }
                (Some(m), Some(path)) => m.intersect_path(&path, rule, true, ts),
                (Some(m), None) => m.clear(),
            }
        }
        Ok(mask)
    }

    /// Composites `image`, which covers `area`, onto the part of the target whose result is needed.
    fn lay_over(
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
            (
                (visible.x0 - t.origin.0) as usize,
                (visible.y0 - t.origin.1) as usize,
            ),
            &shown,
            opacity,
        );
        Ok(())
    }

    fn run(
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
                    self.fill(t, node, shape, paint)
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
                    self.glyph_run(t, node, origin, run, *color)?
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

    fn ts(&self, t: &Target<'_, '_>, node: &PaintNode) -> tiny_skia::Transform {
        geometry::to_target(&node.transform, self.scale, t.origin)
    }

    fn push_clip(
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
        match geometry::path(shape) {
            Some(path) => {
                mask.intersect_path(&path, geometry::fill_rule(shape), true, self.ts(t, node))
            }
            None => mask.clear(),
        }
        t.clips.push(mask);
        Ok(())
    }

    fn fill(&self, t: &mut Target<'_, '_>, node: &PaintNode, shape: &Shape, paint: &Paint) {
        let (Some(path), Some(paint)) = (geometry::path(shape), skia_paint(paint)) else {
            return;
        };
        let ts = self.ts(t, node);
        t.pixmap.fill_path(
            &path,
            &paint,
            geometry::fill_rule(shape),
            ts,
            t.clips.last(),
        );
    }

    fn stroke(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        shape: &Shape,
        width: f64,
        paint: &Paint,
    ) {
        let (Some(path), Some(paint)) = (geometry::path(shape), skia_paint(paint)) else {
            return;
        };
        let stroke = Stroke {
            width: width as f32,
            ..Stroke::default()
        };
        let ts = self.ts(t, node);
        t.pixmap
            .stroke_path(&path, &paint, &stroke, ts, t.clips.last());
    }

    fn image(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        id: crate::ResourceId,
        src: &Rect,
        dst: &Rect,
        sampling: Sampling,
    ) -> Result<(), RenderError> {
        let image = &self.resources.images[&id];
        let whole = self.images.get(id, image, self.config.max_pixels)?;
        if src.width() <= 0.0 || src.height() <= 0.0 || dst.width() <= 0.0 || dst.height() <= 0.0 {
            return Ok(());
        }
        let (x0, y0) = (src.min_x().floor().max(0.0), src.min_y().floor().max(0.0));
        let (x1, y1) = (
            src.max_x().ceil().min(f64::from(image.width)),
            src.max_y().ceil().min(f64::from(image.height)),
        );
        let Some(crop) =
            tiny_skia::IntRect::from_xywh(x0 as i32, y0 as i32, (x1 - x0) as u32, (y1 - y0) as u32)
        else {
            return Ok(());
        };
        let Some(part) = whole.clone_rect(crop) else {
            return Ok(());
        };
        let (sx, sy) = (dst.width() / src.width(), dst.height() / src.height());
        let place = tiny_skia::Transform::from_row(
            sx as f32,
            0.0,
            0.0,
            sy as f32,
            (dst.min_x() + (x0 - src.min_x()) * sx) as f32,
            (dst.min_y() + (y0 - src.min_y()) * sy) as f32,
        );
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
        let ts = self.ts(t, node);
        t.pixmap.fill_path(
            &path,
            &paint,
            tiny_skia::FillRule::Winding,
            ts,
            t.clips.last(),
        );
        Ok(())
    }

    fn glyph_run(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        origin: &cupertino_geometry::Point,
        run: &cupertino_ui_text::GlyphRun,
        color: Color,
    ) -> Result<(), RenderError> {
        let [a, b, c, d, e, f] = node.transform.0;
        let size_px = f64::from(run.size) * self.scale * a.hypot(b);
        if size_px > MAX_GLYPH_PX {
            return Err(RenderError::ResourceLimit);
        }
        self.stats.glyphs += run.glyphs.len() as u32;
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
            let x = ix as i32 + glyph.bearing_x - t.origin.0;
            let y = py.round() as i32 - glyph.bearing_y - t.origin.1;
            blend_glyph(
                t.pixmap,
                t.clips.last().map(Mask::data),
                x,
                y,
                &glyph,
                color,
            );
        }
        Ok(())
    }

    fn layer(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        reach: Reach,
        pos: &mut usize,
        opacity: f32,
        effects: &[crate::Effect],
    ) -> Result<(), RenderError> {
        let area = reach
            .extent
            .outset(reach.halo)
            .intersect(&t.limit.outset(reach.halo))
            .intersect(&t.rect);
        if area.is_empty() {
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
                limit: area,
                clips: Vec::new(),
            };
            self.run(&mut inner, node, reach, pos)?;
        }
        for effect in effects {
            effects::apply(&mut layer, effect, self.scale);
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

fn node_support(node: &PaintNode, scale: f64) -> i32 {
    node.ops
        .iter()
        .map(|op| match op {
            PaintOp::PushLayer { effects, .. } => effects::support(effects, scale),
            _ => 0,
        })
        .fold(0, i32::saturating_add)
}

/// Moves `pos` past the `PopLayer` that closes the layer just opened.
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

fn full_mask(w: u32, h: u32) -> Result<Mask, RenderError> {
    let mut mask = Mask::new(w, h).ok_or(RenderError::ResourceLimit)?;
    mask.data_mut().fill(255);
    Ok(mask)
}

fn skia_color(c: Color) -> Option<tiny_skia::Color> {
    let q = |v: f32| v.clamp(0.0, 1.0);
    tiny_skia::Color::from_rgba(q(c.r), q(c.g), q(c.b), q(c.a))
}

fn stops(stops: &[crate::GradientStop]) -> Option<Vec<GradientStop>> {
    stops
        .iter()
        .map(|s| {
            Some(GradientStop::new(
                s.offset.clamp(0.0, 1.0),
                skia_color(s.color)?,
            ))
        })
        .collect()
}

fn skia_paint(paint: &Paint) -> Option<tiny_skia::Paint<'static>> {
    let shader = match paint {
        Paint::Solid(c) => tiny_skia::Shader::SolidColor(skia_color(*c)?),
        Paint::Linear {
            start,
            end,
            stops: s,
        } => LinearGradient::new(
            Point::from_xy(start.x as f32, start.y as f32),
            Point::from_xy(end.x as f32, end.y as f32),
            stops(s)?,
            SpreadMode::Pad,
            tiny_skia::Transform::identity(),
        )?,
        Paint::Radial {
            center,
            radius,
            stops: s,
        } => {
            let c = Point::from_xy(center.x as f32, center.y as f32);
            RadialGradient::new(
                c,
                c,
                *radius as f32,
                stops(s)?,
                SpreadMode::Pad,
                tiny_skia::Transform::identity(),
            )?
        }
    };
    Some(tiny_skia::Paint {
        shader,
        anti_alias: true,
        ..tiny_skia::Paint::default()
    })
}

/// Source-over of the premultiplied `src`, scaled by `opacity`, onto the pixmap with its top-left pixel at `at`, in
/// 8-bit integers; `src` lies inside the pixmap.
fn source_over(dst: &mut PixmapMut<'_>, at: (usize, usize), src: &Pixmap, opacity: f32) {
    let k = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    let stride = dst.width() as usize * 4;
    let row = src.width() as usize * 4;
    let dst = dst.data_mut();
    for (y, s) in src.data().chunks_exact(row).enumerate() {
        let d = &mut dst[(at.1 + y) * stride + at.0 * 4..][..row];
        for (s, d) in s.as_chunks::<4>().0.iter().zip(d.as_chunks_mut::<4>().0) {
            let s = if k == 255 {
                s.map(u32::from)
            } else {
                s.map(|c| div255(u32::from(c) * k))
            };
            match s[3] {
                0 if s == [0; 4] => {}
                255 => *d = s.map(|c| c as u8),
                a => {
                    let keep = 255 - a;
                    for i in 0..4 {
                        d[i] = (s[i] + div255(u32::from(d[i]) * keep)).min(255) as u8;
                    }
                }
            }
        }
    }
}

/// Source-over of a glyph image at (`x`, `y`) in the pixmap, through the clip mask.
fn blend_glyph(
    pm: &mut PixmapMut<'_>,
    mask: Option<&[u8]>,
    x: i32,
    y: i32,
    g: &RasterGlyph,
    color: Color,
) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let tint = color
        .premultiplied()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32);
    let stride = pm.width() as usize * 4;
    let bytes_per = match g.format {
        GlyphFormat::Coverage => 1,
        GlyphFormat::ColorPremultiplied => 4,
    };
    for gy in 0..g.height as i32 {
        let py = y + gy;
        if py < 0 || py >= h {
            continue;
        }
        for gx in 0..g.width as i32 {
            let px = x + gx;
            if px < 0 || px >= w {
                continue;
            }
            let at = gy as usize * g.stride as usize + gx as usize * bytes_per;
            let clip = mask.map_or(255, |m| {
                u32::from(m[py as usize * w as usize + px as usize])
            });
            let src: [u32; 4] = match g.format {
                GlyphFormat::Coverage => {
                    let cov = div255(u32::from(g.pixels[at]) * clip);
                    [0, 1, 2, 3].map(|i| div255(tint[i] * cov))
                }
                GlyphFormat::ColorPremultiplied => {
                    let s = &g.pixels[at..at + 4];
                    let k = div255(tint[3] * clip);
                    [0, 1, 2, 3].map(|i| div255(u32::from(s[i]) * k))
                }
            };
            if src[3] == 0 && src[0] == 0 && src[1] == 0 && src[2] == 0 {
                continue;
            }
            let out = &mut pm.data_mut()[py as usize * stride + px as usize * 4..][..4];
            let keep = 255 - src[3];
            for i in 0..4 {
                out[i] = (src[i] + div255(u32::from(out[i]) * keep)).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pixmap of premultiplied pixels drawn from `seed`, with every alpha from transparent to opaque.
    fn noise(width: u32, height: u32, seed: u64) -> Pixmap {
        let mut state = seed;
        let mut next = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        let mut pixmap = Pixmap::new(width, height).unwrap();
        for px in pixmap.data_mut().as_chunks_mut::<4>().0 {
            let alpha = match next() % 4 {
                0 => 0,
                1 => 255,
                _ => next() % 256,
            };
            let mut channel = || (next() % (alpha + 1)) as u8;
            *px = [channel(), channel(), channel(), alpha as u8];
        }
        pixmap
    }

    #[test]
    fn the_integer_composite_matches_the_float_one_within_one_step() {
        for (seed, opacity) in [(1, 1.0), (2, 1.0), (3, 0.5), (4, 0.3), (5, 0.0), (6, 0.9)] {
            let src = noise(37, 23, seed);
            let dst = noise(64, 48, seed + 100);
            let mut integer = dst.clone();
            source_over(&mut integer.as_mut(), (11, 7), &src, opacity);
            let mut float = dst.clone();
            float.draw_pixmap(
                11,
                7,
                src.as_ref(),
                &PixmapPaint {
                    opacity,
                    ..PixmapPaint::default()
                },
                tiny_skia::Transform::identity(),
                None,
            );
            let delta = integer
                .data()
                .iter()
                .zip(float.data())
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap();
            assert!(delta <= 1, "opacity {opacity}: {delta} steps apart");
        }
    }
}
