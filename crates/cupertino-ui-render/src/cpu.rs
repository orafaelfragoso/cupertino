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
use cupertino_geometry::Rect;
use cupertino_raster_math::dither;
use cupertino_raster_math::placement::whole_pixel;
use cupertino_theme::Color;
use cupertino_ui_text::{GlyphFormat, GlyphKey, GlyphRasterizer, RasterGlyph};
use std::collections::HashMap;
use std::collections::HashSet;
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
        let mut tree = Tree::new(&request.list.nodes, scale);
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
            let tree = Tree::new(&content, scale);
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
        }
        Ok(())
    }
}

/// The subtree of node `i` as a kept group's image holds it, in the coordinates of the node moved from `at` to the
/// origin: the node with its group's effects and not its opacity, then its descendants in paint order.
fn group_content(tree: &Tree<'_>, i: usize, at: (f64, f64)) -> Vec<GroupNode> {
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
        stack.extend(tree.children[j].iter().rev());
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
    /// The kept scroll layers the frame lays over, by node index (empty when there are none, and inside a layer).
    views: &'a [Option<View<'a>>],
    /// The kept groups, by node, when the painter paints the frame; and the bytes more of them may hold.
    groups: Option<&'a mut HashMap<PaintNodeId, GroupImage>>,
    group_room: u64,
    /// The window's pixels where the painter paints, which bound what a group or a layer of effects reads, as
    /// painting in place does: in a scroll layer's image, the window moved by the layer's offset.
    window: Bounds,
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
        if let Some(Some(view)) = self.views.get(i).filter(|_| t.frame) {
            return self.composite(t, tree, i, chain, view);
        }
        self.children(t, tree, i, chain)
    }

    /// Lays a kept scroll layer over the frame where its node's clip shows it. The pixels every clip above it and its
    /// own let through whole come straight from the image; at a clip's edge, where it covers part of a pixel, the
    /// content is painted in place as a frame without layers paints it, so the edge is cut node by node.
    fn composite<'n>(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'n>,
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
    fn lay_rows(
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
    fn mask_layer(&mut self, layer: &mut Pixmap, mask: &Mask, area: &Bounds) {
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
    fn lay_group(
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
    fn own(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_>,
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
    fn own_in(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_>,
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
            shown.as_ref(),
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

    /// Node coordinates to window pixels.
    fn window_ts(&self, node: &PaintNode) -> tiny_skia::Transform {
        geometry::to_target(&node.transform, self.scale, (0, 0))
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
        let window = geometry::to_target(&node.transform, self.scale, (0, 0));
        clip_by(&mut mask, shape, window, t.origin);
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
    fn lay_path(
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

    fn stroke(
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
    fn lay(
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

    fn image(
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
    fn placement(
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
    fn straight_image(
        &mut self,
        t: &mut Target<'_, '_>,
        tree: &Tree<'_>,
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

/// A group's or an operation layer's `area`: in a scroll layer, where it follows its own bounds, no larger than one
/// scratch image allows, else cut to what the target needs of it (with `halo` around) as in the frame.
fn bounded(area: Bounds, t: &Target<'_, '_>, halo: i32) -> Bounds {
    if t.space == EVERYWHERE
        && u64::from(area.width()) * u64::from(area.height()) > layers::LAYER_SCRATCH_PIXELS
    {
        area.intersect(&t.rect.outset(halo))
    } else {
        area
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

/// Multiplies `mask`, whose top-left pixel is window pixel `origin`, by the coverage of the clip `shape` placed by
/// `window` (node coordinates to window pixels).
fn clip_by(mask: &mut Mask, shape: &Shape, window: tiny_skia::Transform, origin: (i32, i32)) {
    if let Shape::Path { .. } = shape {
        let ts = window.post_translate(-origin.0 as f32, -origin.1 as f32);
        match geometry::path(shape) {
            Some(path) => mask.intersect_path(&path, geometry::fill_rule(shape), true, ts),
            None => mask.clear(),
        }
        return;
    }
    match raster::Outline::new(shape, window, None) {
        Some(outline) => outline.clip(mask, origin),
        None => mask.clear(),
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
fn source_over(dst: &mut PixmapMut<'_>, at: (i32, i32), src: PixmapRef<'_>, opacity: f32) {
    let whole = Bounds::of_size(src.width(), src.height());
    source_over_part(dst, at, src, &whole, opacity);
}

/// [`source_over`] of the pixels `part` of `src` (in its own pixels), with the part's top-left pixel at `at`.
/// What a layer laid over at `opacity` is multiplied by, from 0 (it changes nothing) to 255.
fn opacity_factor(opacity: f32) -> u32 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u32
}

fn source_over_part(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: PixmapRef<'_>,
    part: &Bounds,
    opacity: f32,
) {
    let k = opacity_factor(opacity);
    // most of a layer is often empty: runs of empty pixels are passed over, and nothing is scaled or laid for them; an
    // opaque run laid over whole replaces what it covers
    lay_part(dst, at, src, part, |s, d| {
        if empty(s) {
            return;
        }
        if k == 255 && opaque(s) {
            d.copy_from_slice(s);
            return;
        }
        over_run(s, d, k);
    });
}

/// The pixels `part` of `src` laid over `color` (premultiplied) and written to the pixmap with the part's top-left pixel
/// at `at`, whatever the pixmap held there: the bytes of `color` written first and `src` laid over it with
/// [`source_over_part`], in one pass.
fn over_color_part(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: PixmapRef<'_>,
    part: &Bounds,
    color: [u8; 4],
) {
    let c = u32::from_le_bytes(color);
    lay_part(dst, at, src, part, |s, d| {
        if empty(s) {
            for d in d.as_chunks_mut::<4>().0 {
                *d = color;
            }
        } else if opaque(s) {
            d.copy_from_slice(s);
        } else {
            for (s, d) in s.as_chunks::<4>().0.iter().zip(d.as_chunks_mut::<4>().0) {
                *d = effects::over_px(u32::from_le_bytes(*s), c).to_le_bytes();
            }
        }
    });
}

/// Calls `run` on each run of up to 16 pixels of the pixels `part` of `src` and the pixels of the pixmap they land on,
/// with the part's top-left pixel at `at`. Whatever of `src` falls outside the pixmap is not drawn.
fn lay_part(
    dst: &mut PixmapMut<'_>,
    at: (i32, i32),
    src: PixmapRef<'_>,
    part: &Bounds,
    mut run: impl FnMut(&[u8], &mut [u8]),
) {
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
        for (s, d) in s.chunks(4 * 16).zip(d.chunks_mut(4 * 16)) {
            run(s, d);
        }
    }
}

/// Whether every pixel of `bytes` is opaque, without an early exit, so it vectorizes.
#[inline]
fn opaque(bytes: &[u8]) -> bool {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .fold(255, |all, p| all & p[3])
        == 255
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
/// Where an image operation lands: the source pixels it reads, how they are placed in node coordinates, the node's
/// transform to the target (moved onto whole pixels for an image drawn one source pixel per device pixel), and, when
/// whole source pixels fill a destination on whole pixels, the target pixel their top-left corner falls on.
struct Placement {
    crop: tiny_skia::IntRect,
    place: tiny_skia::Transform,
    ts: tiny_skia::Transform,
    whole: Option<(i32, i32)>,
}

/// Whether `dst` under `ts` covers exactly the device pixels an image cut to `crop` covers at `at`.
fn on_pixels(
    ts: &tiny_skia::Transform,
    dst: &Rect,
    at: (f32, f32),
    crop: tiny_skia::IntRect,
) -> bool {
    const SLACK: f32 = 1e-3;
    if ts.kx != 0.0 || ts.ky != 0.0 {
        return false;
    }
    let edge =
        |v: f64, scale: f32, shift: f32, want: f32| (v as f32 * scale + shift - want).abs() < SLACK;
    edge(dst.min_x(), ts.sx, ts.tx, at.0)
        && edge(dst.max_x(), ts.sx, ts.tx, at.0 + crop.width() as f32)
        && edge(dst.min_y(), ts.sy, ts.ty, at.1)
        && edge(dst.max_y(), ts.sy, ts.ty, at.1 + crop.height() as f32)
}

/// `v` rounded to the nearest whole number, a half to the even one, for `v` in 0 to 2^22: adding 2^23 leaves no
/// fraction bits, so the addition itself rounds. The same result as `f32::round_ties_even`, with no call to make.
#[inline(always)]
fn to_even(v: f32) -> f32 {
    const SHIFT: f32 = 8_388_608.0;
    (v + SHIFT) - SHIFT
}

/// Lays the `crop` of a premultiplied image, its rows `stride` bytes apart in `image`, over the pixmap with its top-left
/// corner on pixel `at`, one source pixel per pixel, under the clip `mask`. The arithmetic is the one a pattern fill of the same image computes, in the same
/// order and precision, so the pixels are the same; a direct loop skips the fill's per-pixel sampling.
fn lay_over(
    pm: &mut PixmapMut<'_>,
    mask: Option<&[u8]>,
    image: &[u8],
    stride: usize,
    crop: tiny_skia::IntRect,
    at: (i32, i32),
) {
    const UNIT: f32 = 1.0 / 255.0;
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, x1) = (at.0.max(0), (at.0 + crop.width() as i32).min(w));
    let (y0, y1) = (at.1.max(0), (at.1 + crop.height() as i32).min(h));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let (dw, n) = (w as usize, (x1 - x0) as usize);
    let sx = (x0 - at.0 + crop.x()) as usize;
    let (out, _) = pm.data_mut().as_chunks_mut::<4>();
    for y in y0..y1 {
        let sy = (y - at.1 + crop.y()) as usize;
        let row = y as usize * dw + x0 as usize;
        let from = image[sy * stride + sx * 4..][..n * 4].as_chunks::<4>().0;
        let to = &mut out[row..][..n];
        let covers = mask.map(|m| &m[row..][..n]);
        for (i, (s, d)) in from.iter().zip(to.iter_mut()).enumerate() {
            let cover = covers.map_or(255, |m| m[i]);
            if cover == 0 || u32::from_ne_bytes(*s) == 0 {
                continue;
            }
            // opaque, or over nothing: the sum is the source pixel itself
            if cover == 255 && (s[3] == 255 || u32::from_ne_bytes(*d) == 0) {
                *d = *s;
                continue;
            }
            let mut v = s.map(|b| f32::from(b) * UNIT);
            if cover != 255 {
                let c = f32::from(cover) / 255.0;
                v = v.map(|value| value * c);
            }
            let keep = 1.0 - v[3];
            for (b, value) in d.iter_mut().zip(v) {
                let o = f32::from(*b) * UNIT * keep + value;
                *b = to_even(o.clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
}

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
