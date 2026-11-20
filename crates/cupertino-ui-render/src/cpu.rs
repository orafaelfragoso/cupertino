use crate::effects::{self, div255};
use crate::geometry::{self, Bounds};
use crate::glyph_cache::GlyphCache;
use crate::image_cache::ImageCache;
use crate::layers::{self, GroupImage, GroupNode, Kept, ScrollImage};
use crate::raster;
use crate::spans;
use crate::stats::RenderStats;
use crate::validate;
use crate::{
    CpuFrame, DamageRegion, Layer, MemoryTarget, Paint, PaintNode, PaintNodeId, PaintOp,
    PixelFormat, PixelRect, RenderError, RenderFrameId, RenderRequest, Sampling, Shape, UiRenderer,
};
use bumpalo::collections::Vec as BumpVec;
use bumpalo::Bump;
use cupertino_geometry::Rect;
use cupertino_raster_math::dither;
use cupertino_raster_math::placement::whole_pixel;
use cupertino_theme::Color;
use cupertino_ui_text::{GlyphFormat, GlyphKey, GlyphRasterizer, RasterGlyph};
use rustc_hash::FxHashMap;
use std::collections::{HashMap, HashSet};
use tiny_skia::{
    FilterQuality, GradientStop, LinearGradient, Mask, Pattern, Pixmap, PixmapMut, PixmapPaint,
    PixmapRef, Point, RadialGradient, SpreadMode, Stroke,
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

/// Paints display lists into a premultiplied RGBA image it keeps between frames, repainting only the damage, or into
/// memory it is lent ([`UiRenderer::render_into`]), repainting the damage and what the memory missed.
pub struct CpuRenderer<R> {
    config: CpuConfig,
    rasterizer: R,
    glyphs: GlyphCache,
    images: ImageCache,
    frame: CpuFrame,
    scale: f64,
    /// The last frame was drawn whole and its layers are good for the next.
    valid: bool,
    /// The renderer's own image holds the last frame; not when it was drawn into lent memory, which lets the image go.
    image: bool,
    next_id: u64,
    stats: RenderStats,
    /// Takes the span path and skips what an opaque node hides; off only to check both give the same pixels.
    fast: bool,
    /// The layers kept from the last frame.
    kept: Kept,
    /// Per-frame scratch arena for the display-list forest and CSR child index table.
    scratch: Bump,
}

/// The pixels a target painted in content coordinates may size its scratch images against: all of them, so a node's
/// scratch image follows its own extent alone.
const EVERYWHERE: Bounds = Bounds {
    x0: -(1 << 28),
    y0: -(1 << 28),
    x1: 1 << 28,
    y1: 1 << 28,
};

/// What a frame does with one scroll layer ([`crate::scroll_layers`]).
struct ScrollPlan {
    /// The layer's node, by index into the list.
    index: usize,
    id: PaintNodeId,
    /// The content's offset in points, and in device pixels.
    offset: cupertino_geometry::Point,
    /// Its image is kept (in [`Kept::scrolls`]); otherwise the frame paints its content where it shows every frame.
    kept: bool,
    /// Content pixels to paint into the kept image.
    paint: Vec<Bounds>,
    /// Window pixels the frame repaints for the layer besides the damage.
    frame: Vec<Bounds>,
}

/// A kept scroll layer as a frame lays it over.
struct View<'a> {
    image: &'a ScrollImage,
    /// The drawn offset, in device pixels.
    shift: (i32, i32),
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
                device: None,
            },
            config,
            rasterizer,
            scale: 1.0,
            valid: false,
            image: false,
            next_id: 1,
            stats: RenderStats::default(),
            fast: true,
            kept: Kept::default(),
            scratch: Bump::new(),
        }
    }

    /// How many layers the renderer keeps, and their bytes.
    pub fn kept_layers(&self) -> (u64, u64) {
        (self.kept.count(), self.kept.bytes())
    }

    /// What the last [`render`](UiRenderer::render) call did, counted; empty when the request was refused before painting.
    pub fn stats(&self) -> &RenderStats {
        &self.stats
    }

    /// Checks the request and, for a frame drawn into the renderer's own image (`own`), sizes its pixels for it:
    /// whether the frame is drawn whole, and whether its pixels were just allocated, all zero (transparent), so a clear
    /// need not write them. A frame drawn into lent memory lets the image go.
    fn prepare(
        &mut self,
        request: &RenderRequest<'_>,
        own: bool,
    ) -> Result<(bool, bool), RenderError> {
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
            || !self.valid
            || (own && !self.image);
        let full = resized || matches!(request.damage, DamageRegion::Full);
        self.valid = false;
        let mut zeroed = false;
        if !own {
            self.frame.pixels = Vec::new();
            self.image = false;
        }
        if full {
            let len = usize::try_from(len).map_err(|_| RenderError::ResourceLimit)?;
            // a frame drawn whole clears or paints over every pixel, so pixels of the right size are kept as they are;
            // pixels of a new size are allocated zeroed, which the system gives without writing them, so the first
            // frame of a window touches only what it paints (the old ones are let go first, not held alongside)
            if own && self.frame.pixels.len() != len {
                self.frame.pixels = Vec::new();
                self.frame.pixels = vec![0; len];
                zeroed = true;
            }
            self.frame.width = request.width;
            self.frame.height = request.height;
            self.frame.stride = stride;
            self.scale = request.scale;
        }
        Ok((full, zeroed))
    }

    /// The bytes of the frame's pixels, the measure of the kept layers' cap, whether they are the renderer's own or
    /// lent.
    fn frame_bytes(&self) -> u64 {
        u64::from(self.frame.stride) * u64::from(self.frame.height)
    }

    /// Paints the `dirty` rectangles into `pixels`, the frame's bytes; `zeroed`: every pixel of the frame is zero
    /// (transparent), so clearing writes nothing.
    #[allow(clippy::too_many_arguments)]
    fn paint(
        &mut self,
        request: &RenderRequest<'_>,
        pixels: &mut [u8],
        dirty: &[Bounds],
        zeroed: bool,
        kept_in: &[Option<usize>],
        plans: &[ScrollPlan],
    ) -> Result<(), RenderError> {
        let (width, height, scale) = (request.width, request.height, request.scale);
        let whole = Bounds::of_size(width, height);
        // what the kept groups may hold more: the cap less the scroll layers and the groups kept already
        let group_room =
            (self.frame_bytes() + layers::LAYER_SLACK_BYTES).saturating_sub(self.kept.bytes());
        let mut base =
            PixmapMut::from_bytes(pixels, width, height).ok_or(RenderError::InvalidSize)?;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.reset();
        let res = (|| {
            let mut tree = Tree::new(&request.list.nodes, scale, &scratch);
            let mut views: Vec<Option<View<'_>>> = Vec::new();
            for plan in plans.iter().filter(|p| p.kept) {
                let Some(image) = self.kept.scrolls.get(&plan.id) else {
                    continue;
                };
                if views.is_empty() {
                    views.resize_with(request.list.nodes.len(), || None);
                }
                views[plan.index] = Some(View {
                    image,
                    shift: device_shift(plan.offset, scale),
                });
                // the layer's content is laid over with it: none of it covers the frame on its own
                for (j, layer) in kept_in.iter().enumerate() {
                    if *layer == Some(plan.index) {
                        tree.grouped[j] = true;
                    }
                }
            }
            let mut painter = Painter {
                stats: &mut self.stats,
                config: &self.config,
                scale,
                resources: request.resources,
                glyphs: &mut self.glyphs,
                images: &mut self.images,
                rasterizer: &mut self.rasterizer,
                fast: self.fast,
                pending: spans::Pending::over_zero(zeroed),
                views: &views,
                groups: Some(&mut self.kept.groups),
                group_room,
                window: whole,
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
                    } else if !zeroed {
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
                    space: whole,
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
        })();
        self.scratch = scratch;
        res
    }
}

impl<R: GlyphRasterizer> CpuRenderer<R> {
    /// What this frame does with each scroll layer, given the damage in window pixels: which pixels of its kept image
    /// it paints (what came into view and what changed) and which window pixels it lays over again (all it shows, when
    /// it moved). Images are made, kept or let go here, within the cap ([`layers::LAYER_SLACK_BYTES`] beyond the
    /// frame's own size).
    fn plan_scrolls(
        &mut self,
        request: &RenderRequest<'_>,
        kept_in: &[Option<usize>],
        full: bool,
        damage: &[Bounds],
    ) -> Vec<ScrollPlan> {
        let scale = request.scale;
        let whole = Bounds::of_size(request.width, request.height);
        let mut layer_nodes: Vec<usize> = kept_in.iter().flatten().copied().collect();
        layer_nodes.sort_unstable();
        layer_nodes.dedup();
        let mut before = std::mem::take(&mut self.kept.scrolls);
        let seen = std::mem::take(&mut self.kept.seen);
        let mut budget = self.frame_bytes() + layers::LAYER_SLACK_BYTES;
        let mut plans = Vec::with_capacity(layer_nodes.len());
        for index in layer_nodes {
            let node = &request.list.nodes[index];
            let (Some(Layer::Scroll { offset }), Some(clip)) = (node.layer, &node.clip) else {
                continue;
            };
            self.kept.seen.insert(node.id);
            let viewport = geometry::clip_bounds(clip, &node.transform, scale).intersect(&whole);
            let shift = device_shift(offset, scale);
            let previous = before.remove(&node.id);
            let mut plan = ScrollPlan {
                index,
                id: node.id,
                offset,
                kept: false,
                paint: Vec::new(),
                frame: Vec::new(),
            };
            if let Some(p) = previous.as_ref().filter(|p| p.viewport != viewport) {
                plan.frame.push(p.viewport);
            }
            if viewport.is_empty() {
                plans.push(plan);
                continue;
            }
            // a layer new to this frame has only new nodes in it, or nodes that came from elsewhere, and those damage
            // what they draw: what the frame showed there is still right
            if previous.is_some() || seen.contains(&node.id) {
                plan.frame.push(viewport);
            }
            let bytes = u64::from(viewport.width()) * u64::from(viewport.height()) * 4;
            if bytes > budget {
                // past the cap: the frame paints what the layer shows every frame, as if it were not one
                plans.push(plan);
                continue;
            }
            let content = viewport.offset(shift.0, shift.1);
            let reused = previous.filter(|p| {
                !full
                    && p.scale == scale
                    && p.viewport == viewport
                    && p.x == content.x0
                    && p.content().touches(&content)
            });
            let image = match reused {
                Some(mut image) => {
                    let rows = image.content();
                    if content.y0 < rows.y0 {
                        plan.paint.push(Bounds {
                            y1: rows.y0,
                            ..content
                        });
                    }
                    if content.y1 > rows.y1 {
                        plan.paint.push(Bounds {
                            y0: rows.y1,
                            ..content
                        });
                    }
                    if image.top == content.y0 {
                        // it stays where it was: only the damage is painted again and laid over
                        plan.frame.clear();
                    }
                    plan.paint.extend(
                        damage
                            .iter()
                            .map(|d| d.intersect(&viewport))
                            .filter(|d| !d.is_empty())
                            .map(|d| d.offset(shift.0, shift.1)),
                    );
                    image.top = content.y0;
                    image
                }
                None => {
                    let Some(pixels) = Pixmap::new(viewport.width(), viewport.height()) else {
                        plans.push(plan);
                        continue;
                    };
                    plan.paint.push(content);
                    ScrollImage {
                        pixels,
                        scale,
                        viewport,
                        x: content.x0,
                        top: content.y0,
                    }
                }
            };
            budget -= bytes;
            plan.kept = true;
            self.kept.scrolls.insert(node.id, image);
            plans.push(plan);
        }
        plans
    }

    /// Lets go of the kept groups whose node is not a kept group of this frame, and of them all when, with the scroll
    /// layers this frame keeps, they no longer fit in the cap.
    fn keep_groups(&mut self, request: &RenderRequest<'_>) {
        if self.kept.groups.is_empty() {
            return;
        }
        let groups: HashSet<PaintNodeId> = request
            .list
            .nodes
            .iter()
            .filter(|n| {
                n.group.is_some() && matches!(n.layer, Some(Layer::Opacity | Layer::Transform))
            })
            .map(|n| n.id)
            .collect();
        self.kept.groups.retain(|id, _| groups.contains(id));
        if self.kept.bytes() > self.frame_bytes() + layers::LAYER_SLACK_BYTES {
            self.kept.groups.clear();
        }
    }

    /// Paints what each plan names into its layer's kept image, in content pixels: the layer's content that reaches
    /// those pixels, moved by the layer's offset, over transparent pixels.
    fn paint_scrolls(
        &mut self,
        request: &RenderRequest<'_>,
        kept_in: &[Option<usize>],
        plans: &[ScrollPlan],
    ) -> Result<(), RenderError> {
        let scale = request.scale;
        for plan in plans.iter().filter(|p| p.kept && !p.paint.is_empty()) {
            let Some(image) = self.kept.scrolls.get_mut(&plan.id) else {
                continue;
            };
            let held = image.content();
            // the pieces of what is to be painted, each pixel once, none grown over pixels kept
            let mut rects: Vec<Bounds> = Vec::new();
            for r in &plan.paint {
                let new = spans::subtract(r.intersect(&held), &rects);
                rects.extend(new.into_iter().filter(|r| !r.is_empty()));
            }
            if rects.is_empty() {
                continue;
            }
            let _layer = tracing::trace_span!("cpu.scroll_layer", rects = rects.len()).entered();
            let content = in_content(request.list, kept_in, plan, scale, &rects);
            let mut scratch = std::mem::take(&mut self.scratch);
            scratch.reset();
            let res = (|| {
                let tree = Tree::new(&content, scale, &scratch);
                let mut painter = Painter {
                    stats: &mut self.stats,
                    config: &self.config,
                    scale,
                    resources: request.resources,
                    glyphs: &mut self.glyphs,
                    images: &mut self.images,
                    rasterizer: &mut self.rasterizer,
                    fast: self.fast,
                    pending: spans::Pending::over_zero(false),
                    views: &[],
                    groups: None,
                    group_room: 0,
                    window: {
                        let shift = device_shift(plan.offset, scale);
                        Bounds::of_size(request.width, request.height).offset(shift.0, shift.1)
                    },
                };
                let (width, height) = (image.pixels.width(), image.pixels.height());
                let stride = width as usize * 4;
                for r in rects {
                    for (y0, y1, row) in layers::ring_pieces(r.y0, r.y1, height as i32) {
                        let rows = (y1 - y0) as usize;
                        let start = row as usize * stride;
                        let bytes = &mut image.pixels.data_mut()[start..start + rows * stride];
                        let mut pm = PixmapMut::from_bytes(bytes, width, rows as u32)
                            .ok_or(RenderError::InvalidSize)?;
                        let piece = Bounds {
                            x0: held.x0,
                            y0,
                            x1: held.x1,
                            y1,
                        };
                        let limit = Bounds { y0, y1, ..r };
                        clear(&mut pm, &limit.offset(-piece.x0, -piece.y0));
                        painter.stats.layer_pixels +=
                            u64::from(limit.width()) * u64::from(limit.height());
                        let mut target = Target {
                            pixmap: &mut pm,
                            origin: (piece.x0, piece.y0),
                            rect: piece,
                            space: EVERYWHERE,
                            limit,
                            clips: Vec::new(),
                            hidden: None,
                            holes: Vec::new(),
                            covering: Vec::new(),
                            missing: Vec::new(),
                            frame: false,
                        };
                        for &root in &tree.roots {
                            painter.subtree(&mut target, &tree, root, &mut Vec::new())?;
                        }
                    }
                }
                Ok(())
            })();
            self.scratch = scratch;
            res?;
        }
        Ok(())
    }
}

/// The subtree of node `i` as a kept group's image holds it, in the coordinates of the node moved from `at` to the
/// origin: the node with its group's effects and not its opacity, then its descendants in paint order.
fn group_content(tree: &Tree<'_, '_>, i: usize, at: (f64, f64)) -> Vec<GroupNode> {
    let mut out = Vec::new();
    let mut stack = vec![i];
    while let Some(j) = stack.pop() {
        let node = &tree.nodes[j];
        let [a, b, c, d, e, f] = node.transform.0;
        out.push(GroupNode {
            id: node.id,
            revision: node.revision,
            transform: [a, b, c, d, e - at.0, f - at.1],
            clip: node.clip.clone(),
            group: match (&node.group, j == i) {
                (Some(g), true) => Some(crate::Group {
                    opacity: 1.0,
                    effects: g.effects.clone(),
                }),
                (g, _) => g.clone(),
            },
            bounds: node.bounds.translate(-at.0, -at.1),
            // the group's own mark says how it may change, not what it shows
            layer: if j == i { None } else { node.layer },
        });
        stack.extend(tree.children_of(j).iter().rev());
    }
    out
}

/// A scroll offset in points as the device pixels it moves the content by.
fn device_shift(offset: cupertino_geometry::Point, scale: f64) -> (i32, i32) {
    let px = |v: f64| {
        (v * scale)
            .round()
            .clamp(-f64::from(1 << 28), f64::from(1 << 28)) as i32
    };
    (px(offset.x), px(offset.y))
}

/// The nodes of `plan`'s layer that reach `rects` (content pixels), moved by its offset into content coordinates, its
/// direct children as roots. A node is taken only with its parent, as painting culls a subtree by its root's bounds.
fn in_content(
    list: &crate::DisplayList,
    kept_in: &[Option<usize>],
    plan: &ScrollPlan,
    scale: f64,
    rects: &[Bounds],
) -> Vec<PaintNode> {
    let layer = list.nodes[plan.index].id;
    let (dx, dy) = (plan.offset.x, plan.offset.y);
    let mut taken: HashSet<PaintNodeId> = HashSet::new();
    // the taken nodes inside a group: a group paints its whole subtree, whatever the pixels asked for, since its blurs
    // and shadows carry what lies outside them into them
    let mut grouped: HashSet<PaintNodeId> = HashSet::new();
    let mut nodes = Vec::new();
    for (node, _) in list
        .nodes
        .iter()
        .zip(kept_in)
        .filter(|(_, l)| **l == Some(plan.index))
    {
        let parent_taken =
            node.parent == Some(layer) || node.parent.is_some_and(|p| taken.contains(&p));
        let in_group = node.parent.is_some_and(|p| grouped.contains(&p));
        let bounds = node.bounds.translate(dx, dy);
        let reach = Bounds::of_rect(&bounds, scale);
        if !parent_taken || !(in_group || rects.iter().any(|r| reach.touches(r))) {
            continue;
        }
        taken.insert(node.id);
        if in_group || node.group.is_some() {
            grouped.insert(node.id);
        }
        let mut moved = node.clone();
        moved.transform.0[4] += dx;
        moved.transform.0[5] += dy;
        moved.bounds = bounds;
        moved.hit = node.hit.translate(dx, dy);
        if moved.parent == Some(layer) {
            moved.parent = None;
        }
        nodes.push(moved);
    }
    nodes
}

impl<R: GlyphRasterizer> UiRenderer for CpuRenderer<R> {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        let _raster = tracing::trace_span!(
            "cpu.raster",
            width = request.width,
            height = request.height,
            nodes = request.list.nodes.len()
        )
        .entered();
        self.stats = RenderStats::default();
        let (full, zeroed) = self.prepare(request, true)?;
        let mut pixels = std::mem::take(&mut self.frame.pixels);
        let drawn = self.draw(request, &mut pixels, full, zeroed, None);
        self.frame.pixels = pixels;
        self.frame.damage = drawn?;
        self.frame.id = RenderFrameId(self.next_id);
        self.next_id += 1;
        self.valid = true;
        self.image = true;
        Ok(self.frame.id)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        if self.valid && self.image && id == self.frame.id {
            Ok(&self.frame)
        } else {
            Err(RenderError::UnknownFrame(id))
        }
    }

    fn drop_layers(&mut self) {
        self.kept.clear();
    }

    fn draws_into_memory(&self) -> bool {
        true
    }

    fn render_into(
        &mut self,
        request: &RenderRequest<'_>,
        target: MemoryTarget<'_>,
    ) -> Result<(RenderFrameId, Vec<PixelRect>), RenderError> {
        let _raster = tracing::trace_span!(
            "cpu.raster",
            width = request.width,
            height = request.height,
            nodes = request.list.nodes.len(),
            lent = true
        )
        .entered();
        self.stats = RenderStats::default();
        let (full, _) = self.prepare(request, false)?;
        let row = request.width as usize * 4;
        let len = row * request.height as usize;
        if target.stride != row || target.pixels.len() < len {
            return Err(RenderError::InvalidSize);
        }
        // what the memory holds is unknown, so nothing is taken to be zero
        let damage = self.draw(
            request,
            &mut target.pixels[..len],
            full,
            false,
            Some(target.missed),
        )?;
        self.frame.damage = damage.clone();
        self.frame.id = RenderFrameId(self.next_id);
        self.next_id += 1;
        self.valid = true;
        Ok((self.frame.id, damage))
    }
}

impl<R: GlyphRasterizer> CpuRenderer<R> {
    /// Draws the frame into `pixels`: the request's damage and what the layers need, all of it when `full`, and for
    /// lent memory (`missed` set) what it missed, all of it when that is unknown. Returns the rectangles that changed
    /// since the last frame.
    fn draw(
        &mut self,
        request: &RenderRequest<'_>,
        pixels: &mut [u8],
        full: bool,
        zeroed: bool,
        missed: Option<Option<&[PixelRect]>>,
    ) -> Result<Vec<PixelRect>, RenderError> {
        let (width, height, scale) = (request.width, request.height, request.scale);
        let whole = Bounds::of_size(width, height);
        let damage: Vec<Bounds> = match (full, request.damage) {
            (false, DamageRegion::Rects(rects)) => rects
                .iter()
                .map(|r| Bounds::of_rect(r, scale).intersect(&whole))
                .filter(|r| !r.is_empty())
                .collect(),
            _ => vec![whole],
        };
        let planning = tracing::trace_span!("cpu.plan").entered();
        let kept_in = crate::scroll_layers(request.list);
        let plans = self.plan_scrolls(request, &kept_in, full, &damage);
        self.keep_groups(request);
        drop(planning);
        let dirty = if full {
            vec![whole]
        } else {
            let mut dirty = damage;
            dirty.extend(plans.iter().flat_map(|p| p.frame.iter().copied()));
            geometry::merge(dirty)
        };
        // lent memory also needs what it missed of the frames shown since it held one
        let painted = match missed {
            None => None,
            Some(_) if full => None,
            Some(None) => Some(vec![whole]),
            Some(Some(rects)) => Some(geometry::merge(
                dirty
                    .iter()
                    .copied()
                    .chain(rects.iter().map(|r| {
                        Bounds {
                            x0: r.x as i32,
                            y0: r.y as i32,
                            x1: r.x.saturating_add(r.width) as i32,
                            y1: r.y.saturating_add(r.height) as i32,
                        }
                        .intersect(&whole)
                    }))
                    .collect(),
            )),
        };
        let painting = painted.as_deref().unwrap_or(&dirty);
        self.stats = RenderStats {
            dirty_rects: painting.len() as u32,
            dirty_pixels: painting
                .iter()
                .map(|d| u64::from(d.width()) * u64::from(d.height()))
                .sum(),
            frame_bytes: self.frame_bytes(),
            ..RenderStats::default()
        };
        let painted = tracing::trace_span!("cpu.scroll_layers")
            .in_scope(|| self.paint_scrolls(request, &kept_in, &plans))
            .and_then(|()| {
                tracing::trace_span!("cpu.paint", rects = painting.len())
                    .in_scope(|| self.paint(request, pixels, painting, zeroed, &kept_in, &plans))
            });
        self.stats.glyph_rasterize = self.glyphs.take_filled();
        self.stats.image_decode = self.images.take_filled();
        if painted.is_err() {
            // a layer may be half painted: the next frame paints every one whole
            self.kept.clear();
        }
        painted?;
        self.stats.retained_layers = crate::Tally {
            count: self.kept.count(),
            bytes: self.kept.bytes(),
        };
        self.glyphs.trim();
        self.images.trim();
        self.stats.retained_glyphs = self.glyphs.retained();
        self.stats.retained_images = self.images.retained();
        Ok(dirty
            .iter()
            .map(|d| PixelRect {
                x: d.x0 as u32,
                y: d.y0 as u32,
                width: d.width(),
                height: d.height(),
            })
            .collect())
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
    /// The pixels scratch images may cover: `rect`, except in a scroll layer, whose scratch images follow each node's
    /// extent alone so a row of content is painted the same whatever part of the layer is painted.
    space: Bounds,
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

/// The forest of a display list: each node's children (flat CSR table in the frame's bump arena) and the roots.
struct Tree<'n, 'b> {
    nodes: &'n [PaintNode],
    /// Each node's own extent in window pixels.
    extents: BumpVec<'b, Bounds>,
    child_start: BumpVec<'b, u32>,
    child_len: BumpVec<'b, u32>,
    child_flat: BumpVec<'b, usize>,
    roots: BumpVec<'b, usize>,
    parent: BumpVec<'b, Option<usize>>,
    /// Each node's place in the order nodes are painted, and the last place in its subtree.
    order: BumpVec<'b, usize>,
    last: BumpVec<'b, usize>,
    /// The node or an ancestor is painted as a group.
    grouped: BumpVec<'b, bool>,
}

impl<'n, 'b> Tree<'n, 'b> {
    fn new(nodes: &'n [PaintNode], scale: f64, bump: &'b Bump) -> Tree<'n, 'b> {
        let len = nodes.len();
        let mut index: FxHashMap<PaintNodeId, usize> =
            FxHashMap::with_capacity_and_hasher(len, Default::default());
        for (i, n) in nodes.iter().enumerate() {
            index.insert(n.id, i);
        }
        let mut parent = BumpVec::with_capacity_in(len, bump);
        parent.resize(len, None);
        let mut child_len = BumpVec::with_capacity_in(len, bump);
        child_len.resize(len, 0u32);
        let mut roots = BumpVec::new_in(bump);
        let mut total_children = 0usize;
        for (i, node) in nodes.iter().enumerate() {
            match node.parent.and_then(|p| index.get(&p)) {
                Some(&p) => {
                    parent[i] = Some(p);
                    child_len[p] += 1;
                    total_children += 1;
                }
                None => roots.push(i),
            }
        }
        let mut child_start = BumpVec::with_capacity_in(len, bump);
        let mut cursor = 0u32;
        for &count in &child_len {
            child_start.push(cursor);
            cursor += count;
        }
        let mut write_at = BumpVec::with_capacity_in(len, bump);
        write_at.extend_from_slice(&child_start);
        let mut child_flat = BumpVec::with_capacity_in(total_children, bump);
        child_flat.resize(total_children, 0usize);
        for (i, &p) in parent.iter().enumerate() {
            if let Some(p) = p {
                let slot = write_at[p] as usize;
                child_flat[slot] = i;
                write_at[p] += 1;
            }
        }
        let mut order = BumpVec::with_capacity_in(len, bump);
        order.resize(len, 0usize);
        let mut last = BumpVec::with_capacity_in(len, bump);
        last.resize(len, 0usize);
        let mut grouped = BumpVec::with_capacity_in(len, bump);
        grouped.resize(len, false);
        let mut next = 0;
        // (node, children done): a node is placed on the way down and closed on the way back up
        let mut stack: BumpVec<'b, (usize, bool)> = BumpVec::with_capacity_in(len.min(256), bump);
        for &r in roots.iter().rev() {
            stack.push((r, false));
        }
        while let Some((i, done)) = stack.pop() {
            if done {
                last[i] = next - 1;
                continue;
            }
            order[i] = next;
            next += 1;
            grouped[i] = nodes[i].group.is_some() || parent[i].is_some_and(|p| grouped[p]);
            stack.push((i, true));
            let s = child_start[i] as usize;
            let e = s + child_len[i] as usize;
            for &c in child_flat[s..e].iter().rev() {
                stack.push((c, false));
            }
        }
        let mut extents = BumpVec::with_capacity_in(len, bump);
        for n in nodes {
            extents.push(geometry::own_extent(n, scale));
        }
        Tree {
            nodes,
            extents,
            child_start,
            child_len,
            child_flat,
            roots,
            parent,
            order,
            last,
            grouped,
        }
    }

    #[inline]
    fn children_of(&self, i: usize) -> &[usize] {
        let s = self.child_start[i] as usize;
        &self.child_flat[s..s + self.child_len[i] as usize]
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
    /// The kept scroll layers the frame lays over, by node index (empty when there are none, and inside a layer).
    views: &'a [Option<View<'a>>],
    /// The kept groups, by node, when the painter paints the frame; and the bytes more of them may hold.
    groups: Option<&'a mut HashMap<PaintNodeId, GroupImage>>,
    group_room: u64,
    /// The window's pixels where the painter paints, which bound what a group or a layer of effects reads, as
    /// painting in place does: in a scroll layer's image, the window moved by the layer's offset.
    window: Bounds,
}

mod helpers;
mod paint;
pub(crate) use helpers::skia_paint;
use helpers::*;
