use crate::effects::{self, div255};
use crate::geometry::{self, Bounds};
use crate::glyph_cache::GlyphCache;
use crate::image_cache::ImageCache;
use crate::spans;
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
    /// Takes the span path and skips what an opaque node hides; off only to check both give the same pixels.
    fast: bool,
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
            fast: true,
        }
    }

    /// The renderer that paints every node the general way, to compare the fast paths against.
    #[cfg(test)]
    pub(crate) fn without_fast_paths(mut self) -> CpuRenderer<R> {
        self.fast = false;
        self
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
            fast: self.fast,
            pending: spans::Pending::default(),
        };
        for d in dirty {
            let hidden = if self.fast {
                painter.occluder(&tree, d, &whole)
            } else {
                None
            };
            // an opaque node over the whole rectangle writes every pixel of it
            if hidden.is_none() {
                if self.fast {
                    painter.pending.set(*d, [0; 4]);
                } else {
                    clear(&mut base, d);
                }
                painter
                    .stats
                    .clear
                    .add(u64::from(d.width()) * u64::from(d.height()) * 4);
            }
            let mut target = Target {
                pixmap: &mut base,
                origin: (0, 0),
                rect: whole,
                limit: *d,
                clips: Vec::new(),
                hidden,
                holes: Vec::new(),
                covering: Vec::new(),
                missing: Vec::new(),
                frame: true,
            };
            for &root in &tree.roots {
                painter.subtree(&mut target, &tree, root, &mut Vec::new())?;
            }
            painter.pending.flush_all(&mut base, (0, 0));
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
    /// The node whose opaque fill covers all of `limit`: what is painted before it is not painted.
    hidden: Option<usize>,
    /// Window pixels the node's fills and clips cover fully, which the span path writes itself: the fills that cover
    /// them (by operation index, in `covering`) and the clips leave them out of the pixmap (see `holed`).
    holes: Vec<Bounds>,
    covering: Vec<bool>,
    /// Glyph runs, by operation index, whose rasters miss the pixels `limit` needs: they are not drawn.
    missing: Vec<bool>,
    /// The frame itself, whose span writes wait in the painter's `pending` until something else paints there.
    frame: bool,
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
    parent: Vec<Option<usize>>,
    /// Each node's place in the order nodes are painted, and the last place in its subtree.
    order: Vec<usize>,
    last: Vec<usize>,
    /// The node or an ancestor is painted as a group.
    grouped: Vec<bool>,
}

impl<'n> Tree<'n> {
    fn new(nodes: &'n [PaintNode], scale: f64) -> Tree<'n> {
        let index: HashMap<PaintNodeId, usize> =
            nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
        let mut children = vec![Vec::new(); nodes.len()];
        let mut roots = Vec::new();
        let mut parent = vec![None; nodes.len()];
        for (i, node) in nodes.iter().enumerate() {
            match node.parent.and_then(|p| index.get(&p)) {
                Some(&p) => {
                    children[p].push(i);
                    parent[i] = Some(p);
                }
                None => roots.push(i),
            }
        }
        let (mut order, mut last) = (vec![0; nodes.len()], vec![0; nodes.len()]);
        let mut grouped = vec![false; nodes.len()];
        let mut next = 0;
        // (node, children done): a node is placed on the way down and closed on the way back up
        let mut stack: Vec<(usize, bool)> = roots.iter().rev().map(|&r| (r, false)).collect();
        while let Some((i, done)) = stack.pop() {
            if done {
                last[i] = next - 1;
                continue;
            }
            order[i] = next;
            next += 1;
            grouped[i] = nodes[i].group.is_some() || parent[i].is_some_and(|p| grouped[p]);
            stack.push((i, true));
            stack.extend(children[i].iter().rev().map(|&c| (c, false)));
        }
        Tree {
            nodes,
            extents: nodes
                .iter()
                .map(|n| geometry::own_extent(n, scale))
                .collect(),
            children,
            roots,
            parent,
            order,
            last,
            grouped,
        }
    }

    /// The ancestors of `i` that clip their descendants, outermost first.
    fn clip_chain(&self, i: usize) -> Vec<&'n PaintNode> {
        let mut chain = Vec::new();
        let mut at = self.parent[i];
        while let Some(p) = at {
            if self.nodes[p].clip.is_some() {
                chain.push(&self.nodes[p]);
            }
            at = self.parent[p];
        }
        chain.reverse();
        chain
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
    fast: bool,
    /// Span writes to the frame not made yet.
    pending: spans::Pending,
}

impl<R: GlyphRasterizer> Painter<'_, R> {
    /// The last node painted into the frame whose opaque fill covers all of `d` with nothing over it but what
    /// comes after it: no group above it and no anti-aliased edge inside `d`. What is painted before it there is
    /// hidden.
    fn occluder(&mut self, tree: &Tree<'_>, d: &Bounds, whole: &Bounds) -> Option<usize> {
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
    fn solid(
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
    fn own_area(
        &self,
        tree: &Tree<'_>,
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
    fn direct(&self, tree: &Tree<'_>, i: usize, chain: &[&PaintNode], frame: bool) -> bool {
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
        // the whole group, whatever the damage: its descendants are rasterized into the same pixels and its blurs and
        // shadows read the same ones in every frame
        let area = Bounds::of_rect(&node.bounds, self.scale).intersect(&t.rect);
        if area.is_empty() || area.intersect(&t.limit).is_empty() {
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
            effects::apply(&mut layer, effect, self.scale);
        }
        if let Some(mask) = self.chain_mask(chain, &area, &[])? {
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
        self.lay_over(t, layer, &area, group.opacity, Site::Group)
    }

    /// Paints one node's own operations into a scratch image of just the pixels they can reach, under the
    /// ancestors' clips, and lays that over the target. A node that nothing clips or cuts, whose operations only draw,
    /// is painted straight into the target instead: source-over is associative, so the result is the same. A node of
    /// solid rectangles under rectangular clips writes the pixels its fills cover fully as rows and keeps the scratch
    /// image only for the rest (see `spans`).
    fn own(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_>,
        i: usize,
        chain: &[&PaintNode],
    ) -> Result<(), RenderError> {
        let node = &tree.nodes[i];
        let extent = tree.extents[i];
        if self.own_area(tree, i, chain, &t.rect, &t.limit).is_none() {
            return Ok(());
        }
        // the pixels the node is painted into are the same whatever the damage, so its paths are rasterized the same
        // way and cut in the same places in every frame; the damage only picks which of them are kept
        let Some(area) = self.own_area(tree, i, chain, &t.rect, &t.rect) else {
            return Ok(());
        };
        self.stats.nodes_painted += 1;
        let halo = node_support(node, self.scale);
        let reach = Reach { extent, halo };
        if self.direct(tree, i, chain, t.frame) {
            let needed = extent.outset(halo).intersect(&t.rect).intersect(&t.limit);
            if t.frame {
                self.pending.flush(t.pixmap, t.origin, &needed);
            }
            let mut inner = Target {
                pixmap: &mut *t.pixmap,
                origin: t.origin,
                rect: t.rect,
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
    fn through_scratch(
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
                &scratch,
                &visible.offset(-area.x0, -area.y0),
                1.0,
            );
        }
        Ok(())
    }

    fn scratch(&mut self, area: &Bounds) -> Result<Pixmap, RenderError> {
        let pixels = u64::from(area.width()) * u64::from(area.height());
        if pixels > self.config.max_pixels {
            return Err(RenderError::ResourceLimit);
        }
        self.stats.scratch_images.add(pixels * 4);
        Pixmap::new(area.width(), area.height()).ok_or(RenderError::ResourceLimit)
    }

    /// The coverage of every clip on the chain, as a mask of just `area`, leaving out `holes` (window pixels, see
    /// `holed`); `None` when nothing is clipped.
    fn chain_mask(
        &mut self,
        chain: &[&PaintNode],
        area: &Bounds,
        holes: &[Bounds],
    ) -> Result<Option<Mask>, RenderError> {
        let mut mask: Option<Mask> = None;
        for owner in chain {
            let Some(clip) = &owner.clip else { continue };
            let ts = geometry::to_target(&owner.transform, self.scale, (area.x0, area.y0));
            let (path, ts) = holed(geometry::path(clip), ts, holes, (area.x0, area.y0));
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

    /// Fills `shape`; when `holed`, without the target's holes.
    fn fill(
        &self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        shape: &Shape,
        paint: &Paint,
        holed: bool,
    ) {
        let holes: &[Bounds] = if holed { &t.holes } else { &[] };
        let (path, ts) = self::holed(geometry::path(shape), self.ts(t, node), holes, t.origin);
        let (Some(path), Some(paint)) = (path, skia_paint(paint)) else {
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
        let mut ts = self.ts(t, node);
        // an image drawn one source pixel per device pixel lands on whole pixels, so it is copied, not resampled
        let device = ts.pre_concat(place);
        if device.kx == 0.0
            && device.ky == 0.0
            && (device.sx - 1.0).abs() < 1e-4
            && (device.sy - 1.0).abs() < 1e-4
        {
            ts = ts.post_translate(device.tx.round() - device.tx, device.ty.round() - device.ty);
        }
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
        self.stats.glyphs += run.glyphs.len() as u32;
        let mask = t.clips.last().map(Mask::data);
        let limit = t.limit.offset(-t.origin.0, -t.origin.1);
        let (pixmap, at) = (&mut *t.pixmap, t.origin);
        self.place_glyphs(node, origin, run, color, |x, y, glyph| {
            blend_glyph(pixmap, mask, &limit, x - at.0, y - at.1, glyph, color);
        })
    }

    /// Hands each glyph of a run with a raster to `each`, with the window pixel of its raster's top-left corner.
    fn place_glyphs(
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

    fn layer(
        &mut self,
        t: &mut Target<'_, '_>,
        node: &PaintNode,
        reach: Reach,
        pos: &mut usize,
        opacity: f32,
        effects: &[crate::Effect],
    ) -> Result<(), RenderError> {
        // the layer's whole reach, whatever the damage, as for a group
        let area = reach.extent.outset(reach.halo).intersect(&t.rect);
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

/// `path` placed by `ts` with `holes` (window pixels; the target's top-left at `origin`) cut out of it, and the
/// transform to fill the result with. A hole is a contour of the opposite direction one pixel inside its rectangle, on
/// whole pixels: under the non-zero rule the rasterizer then skips its pixels, and as its edges lie on pixel
/// boundaries clear of every edge of the path, every pixel outside the rectangle is covered exactly as before.
fn holed(
    path: Option<tiny_skia::Path>,
    ts: tiny_skia::Transform,
    holes: &[Bounds],
    origin: (i32, i32),
) -> (Option<tiny_skia::Path>, tiny_skia::Transform) {
    if holes.is_empty() {
        return (path, ts);
    }
    let Some(placed) = path.and_then(|p| p.transform(ts)) else {
        return (None, ts);
    };
    let mut b = tiny_skia::PathBuilder::new();
    b.push_path(&placed);
    for h in holes {
        let h = h.outset(-1).offset(-origin.0, -origin.1);
        if h.is_empty() {
            continue;
        }
        let (x0, y0, x1, y1) = (h.x0 as f32, h.y0 as f32, h.x1 as f32, h.y1 as f32);
        b.move_to(x0, y0);
        b.line_to(x0, y1);
        b.line_to(x1, y1);
        b.line_to(x1, y0);
        b.close();
    }
    (b.finish(), tiny_skia::Transform::identity())
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

pub(crate) fn skia_paint(paint: &Paint) -> Option<tiny_skia::Paint<'static>> {
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
                0.0,
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
/// 8-bit integers. Whatever of `src` falls outside the pixmap is not drawn.
fn source_over(dst: &mut PixmapMut<'_>, at: (i32, i32), src: &Pixmap, opacity: f32) {
    let whole = Bounds::of_size(src.width(), src.height());
    source_over_part(dst, at, src, &whole, opacity);
}

/// [`source_over`] of the pixels `part` of `src` (in its own pixels), with the part's top-left pixel at `at`.
fn source_over_part(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: &Pixmap,
    part: &Bounds,
    opacity: f32,
) {
    let k = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
    let stride = dst.width() as usize * 4;
    let part = part.intersect(&Bounds::of_size(src.width(), src.height()));
    if part.is_empty() {
        return;
    }
    let src_stride = src.width() as usize * 4;
    let (from, row) = (part.x0 as usize * 4, part.width() as usize * 4);
    let (Ok(x), Ok(top)) = (usize::try_from(at.0), usize::try_from(at.1)) else {
        debug_assert!(false, "the layer starts before the target");
        return;
    };
    let fits = x.checked_mul(4).and_then(|left| left.checked_add(row));
    debug_assert!(
        fits.is_some_and(|right| right <= stride),
        "the layer is wider than the target"
    );
    if fits.is_none_or(|right| right > stride) {
        return;
    }
    let dst = dst.data_mut();
    let lines = src
        .data()
        .chunks_exact(src_stride)
        .skip(part.y0 as usize)
        .take(part.height() as usize)
        .map(|line| &line[from..from + row]);
    for (y, s) in lines.enumerate() {
        let start = top
            .checked_add(y)
            .and_then(|line| line.checked_mul(stride))
            .and_then(|line| line.checked_add(x * 4));
        let Some(d) = start.and_then(|start| dst.get_mut(start..start.checked_add(row)?)) else {
            debug_assert!(false, "the layer is taller than the target");
            return;
        };
        // most of a layer is often empty: runs of empty pixels are passed over, and nothing is scaled or laid for them
        for (s, d) in s.chunks(4 * 16).zip(d.chunks_mut(4 * 16)) {
            if empty(s) {
                continue;
            }
            over_run(s, d, k);
        }
    }
}

/// Whether every byte of `bytes` is zero, read a word at a time and without an early exit, so it vectorizes.
#[inline]
fn empty(bytes: &[u8]) -> bool {
    let (words, rest) = bytes.as_chunks::<8>();
    words.iter().fold(0, |any, w| any | u64::from_ne_bytes(*w)) == 0 && rest.iter().all(|&b| b == 0)
}

/// [`source_over_part`] of one run of pixels of `s` onto `d` at opacity `k` (0..=255), without a branch per pixel so
/// that it vectorizes: an empty source pixel leaves `d` as it is and an opaque one replaces it (scaling by 255 and by 0
/// is exact), the bytes the cases taken apart would give.
fn over_run(s: &[u8], d: &mut [u8], k: u32) {
    let pairs = s.as_chunks::<4>().0.iter().zip(d.as_chunks_mut::<4>().0);
    if k == 255 {
        for (s, d) in pairs {
            *d = effects::over_px(u32::from_le_bytes(*s), u32::from_le_bytes(*d)).to_le_bytes();
        }
    } else {
        for (s, d) in pairs {
            let s = effects::mul_px(u32::from_le_bytes(*s), k);
            *d = effects::over_px(s, u32::from_le_bytes(*d)).to_le_bytes();
        }
    }
}

/// Source-over of a glyph image at (`x`, `y`) in the pixmap, through the clip mask and inside `limit` (pixmap
/// pixels): a raster can reach past the box its node's extent allows.
fn blend_glyph(
    pm: &mut PixmapMut<'_>,
    mask: Option<&[u8]>,
    limit: &Bounds,
    x: i32,
    y: i32,
    g: &RasterGlyph,
    color: Color,
) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, x1) = (limit.x0.max(0), limit.x1.min(w));
    let (y0, y1) = (limit.y0.max(0), limit.y1.min(h));
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
        if py < y0 || py >= y1 {
            continue;
        }
        for gx in 0..g.width as i32 {
            let px = x + gx;
            if px < x0 || px >= x1 {
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
    use crate::{
        DisplayList, GradientStop, Group, ImageResource, PaintResources, PaintRevision, ResourceId,
        ResourceRef, Transform,
    };
    use cupertino_ui_text::{FontId, GlyphId, GlyphRun, PositionedGlyph};
    use std::sync::Arc;

    /// Glyph rasters of half-covered boxes; glyph 7's are three em wide, past the box a node's extent allows a glyph.
    struct NoGlyphs;

    impl GlyphRasterizer for NoGlyphs {
        fn rasterize(
            &mut self,
            key: &GlyphKey,
        ) -> Result<Option<RasterGlyph>, cupertino_ui_text::TextError> {
            let side = (key.size_q / 64).max(1);
            let width = if key.glyph == GlyphId(7) {
                side * 3
            } else {
                side / 2 + 1
            };
            let pixels: Vec<u8> = (0..width * side)
                .map(|i| (i * 37 % 256) as u8 / 2 + key.subpixel_x * 16)
                .collect();
            Ok(Some(RasterGlyph {
                format: GlyphFormat::Coverage,
                width,
                height: side,
                stride: width,
                bearing_x: -1,
                bearing_y: side as i32 - 2,
                pixels: Arc::from(pixels),
            }))
        }
    }

    /// The image the scenes draw.
    const PICTURE: ResourceId = ResourceId(1);

    fn resources() -> PaintResources {
        let pixels: Vec<u8> = (0..8 * 8 * 4).map(|i| (i * 29 % 256) as u8).collect();
        PaintResources {
            images: [(
                PICTURE,
                ImageResource {
                    revision: 1,
                    width: 8,
                    height: 8,
                    stride: 32,
                    format: crate::PixelFormat::Rgba8,
                    pixels: Arc::from(pixels),
                },
            )]
            .into(),
        }
    }

    /// A small deterministic generator.
    struct Draw(u64);

    impl Draw {
        fn next(&mut self) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }

        /// In `[lo, hi)`, on a grid of `1 / steps`; `steps` 0 for any value.
        fn value(&mut self, lo: f64, hi: f64, steps: u32) -> f64 {
            let unit = f64::from(self.next()) / f64::from(u32::MAX);
            let v = lo + (hi - lo) * unit;
            if steps == 0 {
                v
            } else {
                (v * f64::from(steps)).floor() / f64::from(steps)
            }
        }

        fn pick(&mut self, n: u32) -> u32 {
            self.next() % n
        }

        fn color(&mut self) -> Color {
            let a = match self.pick(5) {
                0 | 1 => 1.0,
                2 => 0.03,
                3 => 0.5,
                _ => self.value(0.0, 1.0, 0) as f32,
            };
            let mut c = || self.value(0.0, 1.0, 0) as f32;
            Color::new(c(), c(), c(), a)
        }

        fn rect(&mut self, w: f64, h: f64, steps: u32) -> Rect {
            let (x, y) = (self.value(-10.0, w, steps), self.value(-10.0, h, steps));
            Rect::new(
                x,
                y,
                self.value(0.5, w / 1.5, steps),
                self.value(0.5, h / 1.5, steps),
            )
        }
    }

    const SCENE: (f64, f64) = (120.0, 90.0);

    fn plain(id: u64, parent: Option<u64>, transform: (f64, f64), ops: Vec<PaintOp>) -> PaintNode {
        PaintNode {
            id: PaintNodeId(id),
            parent: parent.map(PaintNodeId),
            revision: PaintRevision(1),
            transform: Transform([1.0, 0.0, 0.0, 1.0, transform.0, transform.1]),
            clip: None,
            group: None,
            bounds: Rect::new(-20.0, -20.0, SCENE.0 + 40.0, SCENE.1 + 40.0),
            hit: Rect::new(0.0, 0.0, 0.0, 0.0),
            ops,
        }
    }

    /// A window of the kind the fast paths are for: an opaque background, panes that clip, an opaque page over one of
    /// them, and translucent and opaque rectangles and rounded rectangles at any offset, with nodes the fast paths
    /// leave alone (strokes, paths, groups, rounded clips) among them.
    fn scene(seed: u64) -> DisplayList {
        let mut draw = Draw(seed);
        let (w, h) = SCENE;
        let steps = [0, 1, 2, 4, 8];
        let grid = |draw: &mut Draw| steps[draw.pick(5) as usize];
        let mut nodes = vec![plain(
            1,
            None,
            (0.0, 0.0),
            vec![PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, w, h)),
                paint: Paint::Solid(Color::new(0.9, 0.9, 0.9, 1.0)),
            }],
        )];
        let mut parents = vec![1];
        for id in 2..5 {
            let g = grid(&mut draw);
            let origin = (draw.value(0.0, w / 3.0, g), draw.value(0.0, h / 3.0, g));
            let clip = draw.rect(w, h, g);
            let parent = parents[draw.pick(parents.len() as u32) as usize];
            let mut pane = plain(id, Some(parent), origin, Vec::new());
            pane.clip = Some(if draw.pick(5) == 0 {
                Shape::RoundedRect {
                    rect: clip,
                    radius: 4.0,
                }
            } else {
                Shape::Rect(clip)
            });
            nodes.push(pane);
            parents.push(id);
        }
        // the page: an opaque rectangle over a whole pane
        let g = grid(&mut draw);
        nodes.push(plain(
            5,
            Some(2),
            (0.0, 0.0),
            vec![PaintOp::Fill {
                shape: Shape::Rect(Rect::new(
                    draw.value(-5.0, 10.0, g),
                    draw.value(-5.0, 10.0, g),
                    w,
                    h,
                )),
                paint: Paint::Solid(Color::new(0.2, 0.25, 0.3, 1.0)),
            }],
        ));
        parents.push(5);
        for id in 6..40 {
            let g = grid(&mut draw);
            let origin = (draw.value(-5.0, w / 2.0, g), draw.value(-5.0, h / 2.0, g));
            let ops = (0..1 + draw.pick(4))
                .flat_map(|_| {
                    let rect = draw.rect(w / 2.0, h / 2.0, g);
                    let paint = Paint::Solid(draw.color());
                    let op = match draw.pick(17) {
                        12 => PaintOp::Glyphs {
                            origin: rect.origin,
                            run: GlyphRun {
                                font: FontId(1),
                                size: draw.value(4.0, 14.0, 2) as f32,
                                glyphs: (0..1 + draw.pick(6))
                                    .map(|i| PositionedGlyph {
                                        id: GlyphId(draw.pick(8)),
                                        x: i as f32 * draw.value(2.0, 7.0, 4) as f32,
                                        y: 0.0,
                                        cluster: 0,
                                    })
                                    .collect(),
                            },
                            color: draw.color(),
                        },
                        13 => PaintOp::Image {
                            image: ResourceRef {
                                id: PICTURE,
                                revision: 1,
                            },
                            src: Rect::new(0.0, 0.0, 8.0, 8.0),
                            dst: Rect::new(rect.min_x(), rect.min_y(), 8.0, 8.0),
                            sampling: crate::Sampling::Linear,
                        },
                        14 => PaintOp::Fill {
                            shape: Shape::Rect(rect),
                            paint: Paint::Linear {
                                start: rect.origin,
                                end: cupertino_geometry::Point::new(rect.max_x(), rect.max_y()),
                                stops: vec![
                                    GradientStop {
                                        offset: 0.0,
                                        color: draw.color(),
                                    },
                                    GradientStop {
                                        offset: 1.0,
                                        color: draw.color(),
                                    },
                                ],
                            },
                        },
                        15 => {
                            let inner = draw.rect(w / 2.0, h / 2.0, g);
                            return vec![
                                PaintOp::PushClip(Shape::Rect(rect)),
                                PaintOp::Fill {
                                    shape: Shape::Rect(inner),
                                    paint,
                                },
                                PaintOp::PopClip,
                            ];
                        }
                        0 => PaintOp::Stroke {
                            shape: Shape::Rect(rect),
                            width: 1.0,
                            paint,
                        },
                        1 => PaintOp::Fill {
                            shape: Shape::Path {
                                verbs: vec![
                                    crate::PathVerb::MoveTo(rect.origin),
                                    crate::PathVerb::LineTo(cupertino_geometry::Point::new(
                                        rect.max_x(),
                                        rect.max_y(),
                                    )),
                                    crate::PathVerb::LineTo(cupertino_geometry::Point::new(
                                        rect.min_x(),
                                        rect.max_y(),
                                    )),
                                    crate::PathVerb::Close,
                                ],
                                fill_rule: crate::FillRule::NonZero,
                            },
                            paint,
                        },
                        2..=6 => PaintOp::Fill {
                            shape: Shape::RoundedRect {
                                rect,
                                radius: draw.value(0.0, 12.0, g),
                            },
                            paint,
                        },
                        _ => PaintOp::Fill {
                            shape: Shape::Rect(rect),
                            paint,
                        },
                    };
                    vec![op]
                })
                .collect();
            let parent = parents[draw.pick(parents.len() as u32) as usize];
            let mut leaf = plain(id, Some(parent), origin, ops);
            if draw.pick(10) == 0 {
                leaf.group = Some(Group {
                    opacity: 0.7,
                    effects: Vec::new(),
                });
            }
            nodes.push(leaf);
            parents.push(id);
        }
        DisplayList { nodes }
    }

    /// Renders the same frames with and without the fast paths and compares every byte; returns the fast renderer's
    /// counters summed over the frames.
    fn compare(seed: u64, scale: f64) -> (u64, u32) {
        compare_list(seed, scale, scene(seed))
    }

    /// [`compare`] on the frames of `list`.
    fn compare_list(seed: u64, scale: f64, mut list: DisplayList) -> (u64, u32) {
        let none = resources();
        let config = CpuConfig {
            max_pixels: 1 << 22,
            glyph_cache_bytes: 1 << 16,
            image_cache_bytes: 1 << 16,
            font_smoothing: true,
        };
        let mut fast = CpuRenderer::new(config, NoGlyphs);
        let mut slow = CpuRenderer::new(config, NoGlyphs).without_fast_paths();
        let mut draw = Draw(seed ^ 0x5eed);
        let (width, height) = (
            (SCENE.0 * scale).ceil() as u32,
            (SCENE.1 * scale).ceil() as u32,
        );
        let (mut spans, mut hidden) = (0, 0);
        for frame in 0..8 {
            let damage = if frame == 0 {
                DamageRegion::Full
            } else {
                // something changes, and the frame repaints a few rectangles of any size and offset
                let i = 5 + draw.pick(list.nodes.len() as u32 - 5) as usize;
                for op in &mut list.nodes[i].ops {
                    if let PaintOp::Fill { paint, .. } = op {
                        *paint = Paint::Solid(draw.color());
                    }
                }
                let g = [0, 1, 4][draw.pick(3) as usize];
                DamageRegion::Rects(
                    (0..1 + draw.pick(3))
                        .map(|_| {
                            let r = draw.rect(SCENE.0, SCENE.1, g);
                            if draw.pick(2) == 0 {
                                // a control's worth
                                Rect::new(r.min_x(), r.min_y(), r.width() / 4.0, r.height() / 4.0)
                            } else {
                                r
                            }
                        })
                        .collect(),
                )
            };
            let request = RenderRequest {
                list: &list,
                resources: &none,
                scale,
                width,
                height,
                damage: &damage,
            };
            let a = fast.render(&request).unwrap();
            let b = slow.render(&request).unwrap();
            spans += fast.stats().spans.count;
            hidden += fast.stats().hidden;
            let (a, b) = (fast.frame(a).unwrap(), slow.frame(b).unwrap());
            if let Some(at) = (0..a.pixels.len()).find(|&i| a.pixels[i] != b.pixels[i]) {
                let px = at / 4;
                let (x, y) = (px as u32 % width, px as u32 / width);
                panic!(
                    "seed {seed}, scale {scale}, frame {frame}: pixel ({x}, {y}) is {:?} on the fast path and {:?} on the general one",
                    &a.pixels[px * 4..px * 4 + 4],
                    &b.pixels[px * 4..px * 4 + 4]
                );
            }
        }
        (spans, hidden)
    }

    #[test]
    fn the_fast_paths_paint_the_same_bytes_as_the_general_one() {
        let (mut spans, mut hidden) = (0, 0);
        for seed in 0..40 {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let (s, h) = compare(seed, scale);
                spans += s;
                hidden += h;
            }
        }
        assert!(spans > 1000, "the span path ran {spans} times");
        assert!(hidden > 20, "nodes hidden {hidden} times");
    }

    /// [`scene`] made harder for the fast paths: nodes scaled, flipped or turned, fills that are all but opaque,
    /// layers inside a node's operations, groups with effects, and more opaque pages, one of them rounded and one
    /// under a group.
    fn harsh(seed: u64) -> DisplayList {
        let mut list = scene(seed);
        let mut draw = Draw(seed ^ 0x0bad_5eed);
        let (w, h) = SCENE;
        let near = [0.996, 0.998, 0.999, 0.9999, 254.4 / 255.0, 254.6 / 255.0];
        let mut shadowed = false;
        for node in &mut list.nodes[1..] {
            let [_, _, _, _, e, f] = node.transform.0;
            node.transform = match draw.pick(8) {
                0 => {
                    let s = [0.5, 0.75, 1.3, 2.0];
                    Transform([
                        s[draw.pick(4) as usize],
                        0.0,
                        0.0,
                        s[draw.pick(4) as usize],
                        e,
                        f,
                    ])
                }
                1 => Transform([0.96, 0.28, -0.28, 0.96, e, f]),
                2 => Transform([-1.0, 0.0, 0.0, 1.0, e + w / 2.0, f]),
                _ => node.transform,
            };
            for op in &mut node.ops {
                if let PaintOp::Fill {
                    paint: Paint::Solid(c),
                    ..
                } = op
                {
                    if draw.pick(3) == 0 {
                        c.a = near[draw.pick(near.len() as u32) as usize];
                    }
                }
            }
            match draw.pick(12) {
                0 => {
                    let at = draw.pick(node.ops.len() as u32 + 1) as usize;
                    let rect = draw.rect(w / 2.0, h / 2.0, 2);
                    let layer = [
                        PaintOp::PushLayer {
                            opacity: 0.6,
                            effects: Vec::new(),
                        },
                        PaintOp::Fill {
                            shape: Shape::Rect(rect),
                            paint: Paint::Solid(draw.color()),
                        },
                        PaintOp::PopLayer,
                    ];
                    node.ops.splice(at..at, layer);
                }
                // a shadow is slow to paint unoptimized: one group in a scene has one
                1 if node.group.is_none() && !shadowed => {
                    shadowed = true;
                    node.group = Some(Group {
                        opacity: [1.0, 0.8][draw.pick(2) as usize],
                        effects: vec![crate::Effect::DropShadow {
                            offset: cupertino_geometry::Point::new(1.0, 2.0),
                            blur: 3.0,
                            color: Color::new(0.0, 0.0, 0.0, 0.4),
                        }],
                    });
                }
                _ => {}
            }
        }
        // more opaque pages: a rounded one, and one inside a group
        let parents: Vec<u64> = list.nodes.iter().map(|n| n.id.0).collect();
        for id in 100..104 {
            let parent = parents[draw.pick(parents.len() as u32) as usize];
            let g = [0, 2, 4][draw.pick(3) as usize];
            let rect = Rect::new(
                draw.value(-5.0, 20.0, g),
                draw.value(-5.0, 20.0, g),
                draw.value(30.0, w, g),
                draw.value(30.0, h, g),
            );
            let shape = if id == 100 {
                Shape::RoundedRect { rect, radius: 7.0 }
            } else {
                Shape::Rect(rect)
            };
            let mut ops = vec![PaintOp::Fill {
                shape,
                paint: Paint::Solid(Color::new(0.3, 0.6, 0.1, 1.0)),
            }];
            if id == 102 {
                // an opaque detail in a corner, and over the page a fill that is all but opaque: the page hides
                // nothing
                ops.insert(
                    0,
                    PaintOp::Fill {
                        shape: Shape::Rect(Rect::new(rect.min_x(), rect.min_y(), 2.0, 2.0)),
                        paint: Paint::Solid(Color::new(1.0, 1.0, 1.0, 1.0)),
                    },
                );
                ops[1] = PaintOp::Fill {
                    shape: Shape::Rect(rect),
                    paint: Paint::Solid(Color::new(0.3, 0.6, 0.1, 254.4 / 255.0)),
                };
            }
            let mut page = plain(id, Some(parent), (0.0, 0.0), ops);
            if id == 101 {
                page.group = Some(Group {
                    opacity: 0.9,
                    effects: Vec::new(),
                });
            }
            // a page is the last child of its parent so that the list stays a preorder walk
            let at = list
                .nodes
                .iter()
                .rposition(|n| {
                    let mut up = Some(n.id);
                    while let Some(p) = up {
                        if p.0 == parent {
                            return true;
                        }
                        up = list.nodes.iter().find(|m| m.id == p).and_then(|m| m.parent);
                    }
                    false
                })
                .unwrap();
            list.nodes.insert(at + 1, page);
        }
        // last, over everything: a pane that clips, painted through a gradient so that its pixels are written before
        // its children's rows, and translucent panels stacked on it, some of two fills, at any offset
        let g = [0, 2, 4][draw.pick(3) as usize];
        let mut stack = plain(
            200,
            None,
            (draw.value(-4.0, 8.0, g), draw.value(-4.0, 8.0, g)),
            vec![PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, w * 0.8, h * 0.8)),
                paint: Paint::Linear {
                    start: cupertino_geometry::Point::new(0.0, 0.0),
                    end: cupertino_geometry::Point::new(w, h),
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: Color::new(0.1, 0.5, 0.9, 0.7),
                        },
                        GradientStop {
                            offset: 1.0,
                            color: Color::new(0.9, 0.4, 0.1, 1.0),
                        },
                    ],
                },
            }],
        );
        stack.clip = Some(Shape::Rect(Rect::new(
            draw.value(0.0, 6.0, g),
            draw.value(0.0, 6.0, g),
            w * 0.75,
            h * 0.75,
        )));
        list.nodes.push(stack);
        for id in 201..201 + 2 + draw.pick(4) as u64 {
            let ops = (0..1 + draw.pick(2))
                .map(|_| {
                    let rect = Rect::new(
                        draw.value(0.0, w / 3.0, g),
                        draw.value(0.0, h / 3.0, g),
                        draw.value(w / 4.0, w / 1.5, g),
                        draw.value(h / 4.0, h / 1.5, g),
                    );
                    let c = |draw: &mut Draw| draw.value(0.0, 1.0, 0) as f32;
                    let color = Color::new(
                        c(&mut draw),
                        c(&mut draw),
                        c(&mut draw),
                        [0.05, 0.3, 0.6, 0.97][draw.pick(4) as usize],
                    );
                    PaintOp::Fill {
                        shape: if draw.pick(2) == 0 {
                            Shape::Rect(rect)
                        } else {
                            Shape::RoundedRect {
                                rect,
                                radius: draw.value(0.0, 8.0, g),
                            }
                        },
                        paint: Paint::Solid(color),
                    }
                })
                .collect();
            // a control's label, picture or ring on its own fill
            let mut ops: Vec<PaintOp> = ops;
            let PaintOp::Fill {
                shape: Shape::Rect(inside) | Shape::RoundedRect { rect: inside, .. },
                ..
            } = ops[0].clone()
            else {
                unreachable!()
            };
            let spot = cupertino_geometry::Point::new(
                inside.min_x() + inside.width() * draw.value(0.2, 0.6, 0),
                inside.min_y() + inside.height() * draw.value(0.4, 0.8, 0),
            );
            match draw.pick(4) {
                0 => ops.push(PaintOp::Glyphs {
                    origin: spot,
                    run: GlyphRun {
                        font: FontId(1),
                        size: draw.value(6.0, 13.0, 2) as f32,
                        glyphs: (0..1 + draw.pick(4))
                            .map(|i| PositionedGlyph {
                                id: GlyphId(draw.pick(8)),
                                x: i as f32 * 4.5,
                                y: 0.0,
                                cluster: 0,
                            })
                            .collect(),
                    },
                    color: draw.color(),
                }),
                1 => ops.push(PaintOp::Image {
                    image: ResourceRef {
                        id: PICTURE,
                        revision: 1,
                    },
                    src: Rect::new(0.0, 0.0, 8.0, 8.0),
                    dst: Rect::new(spot.x, spot.y - 6.0, 6.0, 6.0),
                    sampling: crate::Sampling::Linear,
                }),
                2 => ops.push(PaintOp::Stroke {
                    shape: Shape::Rect(Rect::new(spot.x - 4.0, spot.y - 4.0, 8.0, 5.0)),
                    width: draw.value(1.0, 4.0, 2),
                    paint: Paint::Solid(draw.color()),
                }),
                _ => {}
            }
            let at = (draw.value(-2.0, 4.0, g), draw.value(-2.0, 4.0, g));
            list.nodes.push(plain(id, Some(200), at, ops));
        }
        list
    }

    #[test]
    fn the_fast_paths_paint_the_same_bytes_on_turned_and_scaled_nodes_layers_and_near_opaque_fills()
    {
        let (mut spans, mut hidden) = (0, 0);
        for seed in 0..16 {
            for scale in [1.0, 1.25, 1.75, 2.0] {
                let (s, h) = compare_list(seed, scale, harsh(seed));
                spans += s;
                hidden += h;
            }
        }
        assert!(spans > 300, "the span path ran {spans} times");
        assert!(hidden > 10, "nodes hidden {hidden} times");
    }

    /// A window background, a pane that clips, an opaque page over it and a translucent rounded group on the page, all
    /// larger than the damage: the shape of a settings window.
    fn form(scale: f64) -> (DisplayList, u32, u32) {
        let (w, h) = (300.0, 200.0);
        let fill = |shape: Shape, c: Color| PaintOp::Fill {
            shape,
            paint: Paint::Solid(c),
        };
        let mut pane = plain(2, Some(1), (100.5, 0.0), Vec::new());
        pane.clip = Some(Shape::Rect(Rect::new(0.0, 0.0, 199.5, h)));
        let list = DisplayList {
            nodes: vec![
                plain(
                    1,
                    None,
                    (0.0, 0.0),
                    vec![fill(
                        Shape::Rect(Rect::new(0.0, 0.0, w, h)),
                        Color::new(0.9, 0.9, 0.9, 1.0),
                    )],
                ),
                pane,
                plain(
                    3,
                    Some(2),
                    (100.5, 0.0),
                    vec![fill(
                        Shape::Rect(Rect::new(0.0, 0.0, 199.5, h)),
                        Color::new(0.95, 0.95, 0.95, 1.0),
                    )],
                ),
                plain(
                    4,
                    Some(3),
                    (120.25, 20.0),
                    vec![fill(
                        Shape::RoundedRect {
                            rect: Rect::new(0.0, 0.0, 160.0, 150.0),
                            radius: 10.0,
                        },
                        Color::new(0.0, 0.0, 0.0, 0.05),
                    )],
                ),
            ],
        };
        let size = |v: f64| (v * scale).ceil() as u32;
        (list, size(w), size(h))
    }

    #[test]
    fn a_page_over_the_window_background_hides_it_and_is_written_as_rows() {
        let none = PaintResources::default();
        let config = CpuConfig {
            max_pixels: 1 << 22,
            glyph_cache_bytes: 1 << 16,
            image_cache_bytes: 1 << 16,
            font_smoothing: true,
        };
        for scale in [1.0, 1.5, 2.0] {
            let (list, width, height) = form(scale);
            let mut fast = CpuRenderer::new(config, NoGlyphs);
            let mut slow = CpuRenderer::new(config, NoGlyphs).without_fast_paths();
            for damage in [
                DamageRegion::Full,
                DamageRegion::Rects(vec![Rect::new(130.0, 40.0, 100.0, 80.0)]),
            ] {
                let request = RenderRequest {
                    list: &list,
                    resources: &none,
                    scale,
                    width,
                    height,
                    damage: &damage,
                };
                let (a, b) = (
                    fast.render(&request).unwrap(),
                    slow.render(&request).unwrap(),
                );
                assert_eq!(
                    fast.frame(a).unwrap().pixels,
                    slow.frame(b).unwrap().pixels,
                    "scale {scale}, {damage:?}"
                );
            }
            // the damage lies inside the page: the background is hidden, the page and the group's middle are rows,
            // and only the group's rim goes through a scratch image
            let stats = *fast.stats();
            let general = *slow.stats();
            assert_eq!(
                stats.hidden, 2,
                "scale {scale}: the window and the pane, above the page"
            );
            assert_eq!(
                stats.spans.count, 2,
                "scale {scale}: the page and the group"
            );
            assert_eq!(
                stats.clear.count, 0,
                "scale {scale}: the page writes every pixel"
            );
            assert_eq!(general.scratch_images.count, 3);
            assert!(
                stats.node_composite.bytes * 4 < general.node_composite.bytes / 4,
                "scale {scale}: {} bytes composited, {} on the general path",
                stats.node_composite.bytes,
                general.node_composite.bytes
            );
        }
    }

    /// Draws `list`, changed frame by frame, over the frame before with damage that covers the change and more
    /// rectangles of any size and offset, and checks each frame byte for byte against the full frame of the same list,
    /// with every pixel that changed inside the damage the frame reports. Both renderers take the same paths (`fast`
    /// or not), so only the damage differs.
    fn incremental_matches_full(seed: u64, scale: f64, mut list: DisplayList, fast: bool) {
        let none = resources();
        let config = CpuConfig {
            max_pixels: 1 << 22,
            glyph_cache_bytes: 1 << 16,
            image_cache_bytes: 1 << 16,
            font_smoothing: true,
        };
        let make = || {
            let r = CpuRenderer::new(config, NoGlyphs);
            if fast {
                r
            } else {
                r.without_fast_paths()
            }
        };
        let (mut step, mut whole) = (make(), make());
        let mut draw = Draw(seed ^ 0xd1a7);
        let (width, height) = (
            (SCENE.0 * scale).ceil() as u32,
            (SCENE.1 * scale).ceil() as u32,
        );
        let pixels = (width * height) as usize;
        let render = |r: &mut CpuRenderer<NoGlyphs>, list: &DisplayList, damage: &DamageRegion| {
            let request = RenderRequest {
                list,
                resources: &none,
                scale,
                width,
                height,
                damage,
            };
            let id = r.render(&request).unwrap();
            r.frame(id).unwrap().clone()
        };
        let at = |p: usize| (p as u32 % width, p as u32 / width);
        let mut before = render(&mut step, &list, &DamageRegion::Full);
        for frame in 1..10 {
            // every other frame changes one node's colours; the others repaint what did not change
            if frame % 2 == 0 {
                let i = 1 + draw.pick(list.nodes.len() as u32 - 1) as usize;
                for op in &mut list.nodes[i].ops {
                    if let PaintOp::Fill { paint, .. } = op {
                        *paint = Paint::Solid(draw.color());
                    }
                }
            }
            let full = render(&mut whole, &list, &DamageRegion::Full);
            // the pixels the change moved, and rectangles of any size and offset besides, in points
            let moved = (0..pixels)
                .filter(|&p| full.pixels[4 * p..4 * p + 4] != before.pixels[4 * p..4 * p + 4])
                .map(at)
                .fold(None, |b: Option<(u32, u32, u32, u32)>, (x, y)| {
                    Some(b.map_or((x, y, x, y), |(x0, y0, x1, y1)| {
                        (x0.min(x), y0.min(y), x1.max(x), y1.max(y))
                    }))
                });
            let g = [0, 1, 2, 4, 8][draw.pick(5) as usize];
            let mut rects: Vec<Rect> = (0..1 + draw.pick(4))
                .map(|_| {
                    let r = draw.rect(SCENE.0, SCENE.1, g);
                    match draw.pick(3) {
                        // a caret's or an underline's worth
                        0 => Rect::new(r.min_x(), r.min_y(), r.width() / 12.0, r.height() / 12.0),
                        1 => Rect::new(r.min_x(), r.min_y(), r.width() / 3.0, r.height() / 3.0),
                        _ => r,
                    }
                })
                .collect();
            if let Some((x0, y0, x1, y1)) = moved {
                rects.push(Rect::from_ltrb(
                    f64::from(x0) / scale,
                    f64::from(y0) / scale,
                    f64::from(x1 + 1) / scale,
                    f64::from(y1 + 1) / scale,
                ));
            }
            let damage = DamageRegion::Rects(rects);
            let after = render(&mut step, &list, &damage);
            if let Some(p) = (0..pixels)
                .find(|&p| after.pixels[4 * p..4 * p + 4] != full.pixels[4 * p..4 * p + 4])
            {
                panic!(
                    "seed {seed}, scale {scale}, fast {fast}, frame {frame}: pixel {:?} is {:?} drawn over the frame \
                     before under {damage:?} and {:?} drawn whole",
                    at(p),
                    &after.pixels[4 * p..4 * p + 4],
                    &full.pixels[4 * p..4 * p + 4],
                );
            }
            for p in 0..pixels {
                let (x, y) = at(p);
                let reported = after
                    .damage
                    .iter()
                    .any(|d| x >= d.x && x < d.x + d.width && y >= d.y && y < d.y + d.height);
                assert!(
                    reported || after.pixels[4 * p..4 * p + 4] == before.pixels[4 * p..4 * p + 4],
                    "seed {seed}, scale {scale}, frame {frame}: pixel ({x}, {y}) changed outside the damage reported"
                );
            }
            before = after;
        }
    }

    #[test]
    fn a_frame_drawn_over_the_last_has_the_bytes_of_one_drawn_whole() {
        for seed in 0..24 {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                for fast in [true, false] {
                    incremental_matches_full(seed, scale, scene(seed), fast);
                    incremental_matches_full(seed, scale, harsh(seed), fast);
                }
            }
        }
    }

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
