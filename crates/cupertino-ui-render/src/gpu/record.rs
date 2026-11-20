//! Walking a display list into draws, as the CPU renderer walks it: a node's own operations under its ancestors'
//! clips, cut at the pixels its scratch image would cover; groups and layers into offscreen targets sized as the CPU
//! renderer sizes them, their effects, then laid over through the clips around them. Draws are recorded with their
//! uniforms and instances and executed afterwards ([`crate::gpu::exec`]); glyphs, path masks and images are uploaded while
//! recording.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::{
    Effect, Group, Paint, PaintNode, PaintNodeId, PaintOp, PixelFormat, RenderError, RenderRequest,
    ResourceId, Sampling, Shape,
};
use cupertino_gpu::atlas::Atlas;
use cupertino_gpu::{
    ClipBlock, ClipsBlock, Error as GpuError, FillBlock, Region, Texture, TextureBlock,
    TextureDesc, TextureFormat, TextureUsage,
};
use cupertino_raster_math::blur::{plan_blur, MAX_BLUR_RADIUS};
use cupertino_raster_math::coverage::{coverage, rect_distance, rounded_distance};
use cupertino_raster_math::dither;
use cupertino_raster_math::gradient::{Stop, Stops, MAX_STOPS};
use cupertino_raster_math::placement::whole_pixel;
use cupertino_theme::Color;
use cupertino_ui_text::{GlyphFormat, GlyphKey, GlyphRasterizer, GlyphRun};

use crate::gpu::geometry::{self, Bounds, Outline};
use crate::gpu::{gpu, GlyphMeta, GpuRenderer, ImageTexture, TargetMem, UNIFORM_ALIGN};

/// Largest glyph the renderer rasterizes, in physical pixels per em (the CPU renderer's limit).
const MAX_GLYPH_PX: f64 = 8192.0;

/// Analytic clips one draw takes; more are rasterized into its clip mask.
const MAX_ANALYTIC_CLIPS: usize = 4;

/// Floats of the uniform blocks of the programs that have no block type of their own ([`FillBlock`] and
/// [`TextureBlock`] are the fill and texture programs').
pub(crate) const RESOLVE_FLOATS: usize = 16;
pub(crate) const LEVEL_FLOATS: usize = 4;
pub(crate) const KERNEL_FLOATS: usize = 40;

/// The pixels a kept scroll layer's content may size its scratch images and layers against: all of them, so each
/// follows its node's own extent and not the window's edges, wherever the layer has scrolled the node to.
const EVERYWHERE: Bounds = Bounds {
    x0: -(1 << 28),
    y0: -(1 << 28),
    x1: 1 << 28,
    y1: 1 << 28,
};

/// The most pixels a scratch image or layer of a kept scroll layer's node covers before it is cut to what the
/// pixels being painted need of it (with its effects' reach around them), as a frame cuts it.
const SCRATCH_PIXELS: u64 = 1 << 22;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TargetId(pub usize);

/// The window's own target.
pub(crate) const WINDOW: TargetId = TargetId(0);

/// A texture a draw reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tex {
    Empty,
    Coverage(usize),
    Color(usize),
    Mask(usize),
    Image(ResourceId),
    Target(TargetId),
    /// A texture of the frame's own, too large for an atlas page.
    Own(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Program {
    Fill,
    Clear,
    Texture,
    Halve,
    Gauss,
    Resolve,
}

/// One recorded draw.
#[derive(Clone, Debug)]
pub(crate) enum Cmd {
    /// Instanced quads of the fill, clear or texture program.
    Quads {
        program: Program,
        target: TargetId,
        scissor: Region,
        uniform: (usize, usize),
        textures: [Tex; 3],
        instances: (u32, u32),
    },
    /// The first pass into a new layer, which starts it transparent even when nothing is drawn into it.
    Begin { target: TargetId },
    /// A pass over a whole target of the blur and resolve programs.
    Whole {
        program: Program,
        target: TargetId,
        uniform: (usize, usize),
        textures: [Tex; 2],
    },
}

/// What one frame records.
pub(crate) struct Recording {
    pub cmds: Vec<Cmd>,
    pub uniforms: Vec<u8>,
    pub instances: Vec<f32>,
    /// The textures targets are drawn into, with their size and format: a target finished with in a frame gives its
    /// texture to a later target of the same size and format, so a frame holds only the targets alive at once.
    pub targets: Vec<(TargetMem, (u32, u32), TextureFormat)>,
    /// The texture of each target, by its id.
    pub storage: Vec<usize>,
    /// Textures whose targets are finished with, free for a later target of the frame.
    spare: Vec<usize>,
    /// Each target's size and whether a pass into it ran (a later pass keeps what an earlier drew).
    pub sizes: Vec<(u32, u32)>,
    pub started: Vec<bool>,
    pub own: Vec<cupertino_gpu::Texture>,
    /// The window is drawn whole: its first pass clears it.
    pub full: bool,
    /// Clip masks of this frame, by their key.
    masks: HashMap<u64, Option<MaskRef>>,
}

impl Recording {
    pub(crate) fn new(full: bool) -> Recording {
        Recording {
            cmds: Vec::new(),
            uniforms: Vec::new(),
            instances: Vec::new(),
            targets: vec![(TargetMem::Window, (0, 0), TextureFormat::Rgba8)],
            storage: vec![0],
            spare: Vec::new(),
            sizes: vec![(0, 0)],
            started: vec![false],
            own: Vec::new(),
            full,
            masks: HashMap::new(),
        }
    }

    fn uniform(&mut self, floats: &[f32]) -> (usize, usize) {
        let at = self.uniforms.len().next_multiple_of(UNIFORM_ALIGN);
        self.uniforms.resize(at, 0);
        for f in floats {
            self.uniforms.extend_from_slice(&f.to_le_bytes());
        }
        (at, floats.len() * 4)
    }

    fn quad(&mut self, rect: Bounds, atlas: (f32, f32)) {
        self.instances.extend_from_slice(&[
            rect.x0 as f32,
            rect.y0 as f32,
            rect.x1 as f32,
            rect.y1 as f32,
            atlas.0,
            atlas.1,
            0.0,
            0.0,
        ]);
    }

    /// A new target drawn into the texture `at`.
    fn add_target(&mut self, at: usize, size: (u32, u32)) -> TargetId {
        self.storage.push(at);
        self.sizes.push(size);
        self.started.push(false);
        TargetId(self.storage.len() - 1)
    }

    fn instance_count(&self) -> u32 {
        (self.instances.len() / 8) as u32
    }

    /// The texture target `t` is drawn into.
    pub(crate) fn target_mem(&self, t: TargetId) -> &TargetMem {
        &self.targets[self.storage[t.0]].0
    }

    /// Nothing recorded after this reads or draws into `t`: its texture may serve a later target of the frame.
    fn finish(&mut self, t: TargetId) {
        let at = self.storage[t.0];
        if t != WINDOW && !self.spare.contains(&at) {
            self.spare.push(at);
        }
    }

    /// A target drawn into a kept scroll layer's texture: its passes keep what the texture holds.
    fn add_kept(&mut self, node: PaintNodeId, slot: usize, texture: Texture) -> TargetId {
        let size = texture.size();
        self.targets.push((
            TargetMem::Kept {
                node,
                slot,
                texture,
            },
            size,
            TextureFormat::Rgba8,
        ));
        let t = self.add_target(self.targets.len() - 1, size);
        self.started[t.0] = true;
        t
    }

    /// Gives every target and texture of the frame back.
    pub(crate) fn release<R>(self, r: &mut GpuRenderer<R>) {
        for (t, _, _) in self.targets {
            match t {
                TargetMem::Window => {}
                TargetMem::Pooled(p) => r.pool.release(p, r.frames),
                TargetMem::Own(t) => r.device.destroy_texture(t),
                TargetMem::Kept {
                    node,
                    slot,
                    texture,
                } => match r.kept.get_mut(&node) {
                    Some(k) if k.textures[slot].is_none() => k.textures[slot] = Some(texture),
                    _ => r.device.destroy_texture(texture),
                },
            }
        }
        for t in self.own {
            r.device.destroy_texture(t);
        }
    }
}

/// A clip mask placed: the texture, the window pixels it covers and the texel of its first pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MaskRef {
    tex: Tex,
    rect: Bounds,
    at: (u32, u32),
}

/// One clip in force.
#[derive(Clone, Debug)]
enum ClipItem {
    /// A rectangle or rounded rectangle, by its coverage.
    Shape { outline: Outline, bounds: Bounds },
    /// A rectangle on whole pixels: all or nothing, the scissor alone. Its bounds, a pixel out, size areas as the
    /// other clips' do.
    Aligned { pixels: Bounds, bounds: Bounds },
    /// A path, rasterized into the clip mask.
    Path {
        path: tiny_skia::Path,
        rule: tiny_skia::FillRule,
        ts: tiny_skia::Transform,
        bounds: Bounds,
        key: u64,
    },
    /// A shape with no area: nothing gets through.
    Empty,
}

impl ClipItem {
    fn new(shape: &Shape, transform: &crate::Transform, scale: f64) -> ClipItem {
        let window = geometry::to_target(transform, scale, (0, 0));
        let bounds = geometry::clip_bounds(shape, transform, scale);
        if let Shape::Path { .. } = shape {
            let Some(path) = geometry::path(shape) else {
                return ClipItem::Empty;
            };
            let rule = geometry::fill_rule(shape);
            let mut h = DefaultHasher::new();
            hash_path(&path, &mut h);
            matches!(rule, tiny_skia::FillRule::EvenOdd).hash(&mut h);
            hash_ts(window, &mut h);
            return ClipItem::Path {
                path,
                rule,
                ts: window,
                bounds,
                key: h.finish(),
            };
        }
        let Some(outline) = Outline::new(shape, window, None) else {
            return ClipItem::Empty;
        };
        if let Some(pixels) = aligned(shape, window) {
            return ClipItem::Aligned { pixels, bounds };
        }
        ClipItem::Shape { outline, bounds }
    }

    /// The pixels the clip can let through.
    fn bounds(&self) -> Bounds {
        match self {
            ClipItem::Shape { bounds, .. }
            | ClipItem::Path { bounds, .. }
            | ClipItem::Aligned { bounds, .. } => *bounds,
            ClipItem::Empty => Bounds::EMPTY,
        }
    }
}

/// A rectangle with square corners under a transform that only scales and moves, its edges on whole window pixels:
/// those pixels, whose coverage is all or nothing.
fn aligned(shape: &Shape, ts: tiny_skia::Transform) -> Option<Bounds> {
    let rect = match shape {
        Shape::Rect(r) => r,
        Shape::RoundedRect { rect, radius } if *radius <= 0.0 => rect,
        _ => return None,
    };
    if ts.kx != 0.0 || ts.ky != 0.0 {
        return None;
    }
    let xs = [
        ts.sx * rect.min_x() as f32 + ts.tx,
        ts.sx * rect.max_x() as f32 + ts.tx,
    ];
    let ys = [
        ts.sy * rect.min_y() as f32 + ts.ty,
        ts.sy * rect.max_y() as f32 + ts.ty,
    ];
    if xs
        .iter()
        .chain(&ys)
        .any(|v| v.fract() != 0.0 || v.abs() > 1e7)
    {
        return None;
    }
    Some(Bounds {
        x0: xs[0].min(xs[1]) as i32,
        y0: ys[0].min(ys[1]) as i32,
        x1: xs[0].max(xs[1]) as i32,
        y1: ys[0].max(ys[1]) as i32,
    })
}

fn hash_ts(ts: tiny_skia::Transform, h: &mut DefaultHasher) {
    for v in [ts.sx, ts.ky, ts.kx, ts.sy, ts.tx, ts.ty] {
        v.to_bits().hash(h);
    }
}

fn hash_path(path: &tiny_skia::Path, h: &mut DefaultHasher) {
    for s in path.segments() {
        let points: &[tiny_skia::Point] = match s {
            tiny_skia::PathSegment::MoveTo(p) => {
                0u8.hash(h);
                &[p][..]
            }
            tiny_skia::PathSegment::LineTo(p) => {
                1u8.hash(h);
                &[p][..]
            }
            tiny_skia::PathSegment::QuadTo(a, b) => {
                2u8.hash(h);
                &[a, b][..]
            }
            tiny_skia::PathSegment::CubicTo(a, b, c) => {
                3u8.hash(h);
                &[a, b, c][..]
            }
            tiny_skia::PathSegment::Close => {
                4u8.hash(h);
                &[][..]
            }
        };
        for p in points {
            p.x.to_bits().hash(h);
            p.y.to_bits().hash(h);
        }
    }
}

/// The clips of one draw.
#[derive(Clone, Debug)]
struct ClipDraw {
    /// Window pixels outside are cut: the aligned rectangles.
    scissor: Bounds,
    shapes: Vec<Outline>,
    mask: Option<MaskRef>,
}

impl ClipDraw {
    fn write(&self, c: &mut ClipsBlock) {
        c.info[0] = self.shapes.len() as f32;
        if let Some(m) = &self.mask {
            c.info[1] = 1.0;
            c.mask_rect = [
                m.rect.x0 as f32,
                m.rect.y0 as f32,
                m.rect.x1 as f32,
                m.rect.y1 as f32,
            ];
            c.mask_at = [m.at.0 as f32, m.at.1 as f32, 0.0, 0.0];
        }
        for (clip, o) in c.clips.iter_mut().zip(&self.shapes) {
            *clip = ClipBlock {
                linear: [o.inv[0], o.inv[1], o.inv[2], o.inv[3]],
                moved: [o.inv[4], o.inv[5], o.half[0], o.half[1]],
                corner: [o.corner, o.px, corner_exponent(o.exponent), o.outset],
            };
        }
    }

    fn mask_tex(&self) -> Tex {
        self.mask.map_or(Tex::Empty, |m| m.tex)
    }
}

/// A target being drawn into: which, the window pixel of its first pixel, the window pixels it covers and those
/// whose result is needed.
#[derive(Clone, Copy, Debug)]
struct Ctx {
    target: TargetId,
    origin: (i32, i32),
    rect: Bounds,
    limit: Bounds,
    /// What the window pixel is moved by to key the dither: a kept scroll layer's content is dithered by its own
    /// pixel (the window pixel plus the layer's offset), so it keeps its bytes as the layer moves.
    dither: (i32, i32),
}

impl Ctx {
    /// The dither's move as the programs take it: x + 8 y, each modulo the pattern's 8 pixels.
    fn dither_shift(&self) -> f32 {
        (self.dither.0.rem_euclid(8) + 8 * self.dither.1.rem_euclid(8)) as f32
    }

    fn frame(&self, r: &Recording) -> [f32; 4] {
        let (w, h) = r.sizes[self.target.0];
        [
            w as f32,
            h as f32,
            self.origin.0 as f32,
            self.origin.1 as f32,
        ]
    }

    fn local(&self, b: &Bounds) -> Bounds {
        b.offset(-self.origin.0, -self.origin.1)
    }

    fn scissor(&self, b: &Bounds) -> Region {
        let l = self.local(b);
        let (x0, y0) = (l.x0.max(0), l.y0.max(0));
        Region::new(
            x0 as u32,
            y0 as u32,
            (l.x1 - x0).max(0) as u32,
            (l.y1 - y0).max(0) as u32,
        )
    }
}

/// The forest of a display list, backed by a flat CSR child table in the frame's bump arena.
struct Tree<'n, 'b> {
    nodes: &'n [PaintNode],
    extents: bumpalo::collections::Vec<'b, Bounds>,
    child_start: bumpalo::collections::Vec<'b, u32>,
    child_len: bumpalo::collections::Vec<'b, u32>,
    child_flat: bumpalo::collections::Vec<'b, usize>,
    parents: bumpalo::collections::Vec<'b, Option<usize>>,
    roots: bumpalo::collections::Vec<'b, usize>,
}

impl<'n, 'b> Tree<'n, 'b> {
    fn new(nodes: &'n [PaintNode], scale: f64, bump: &'b bumpalo::Bump) -> Tree<'n, 'b> {
        let len = nodes.len();
        let mut index: rustc_hash::FxHashMap<PaintNodeId, usize> =
            rustc_hash::FxHashMap::with_capacity_and_hasher(len, Default::default());
        for (i, n) in nodes.iter().enumerate() {
            index.insert(n.id, i);
        }
        let mut parents = bumpalo::collections::Vec::with_capacity_in(len, bump);
        parents.resize(len, None);
        let mut child_len = bumpalo::collections::Vec::with_capacity_in(len, bump);
        child_len.resize(len, 0u32);
        let mut roots = bumpalo::collections::Vec::new_in(bump);
        let mut total_children = 0usize;
        for (i, node) in nodes.iter().enumerate() {
            match node.parent.and_then(|p| index.get(&p)) {
                Some(&p) => {
                    parents[i] = Some(p);
                    child_len[p] += 1;
                    total_children += 1;
                }
                None => roots.push(i),
            }
        }
        let mut child_start = bumpalo::collections::Vec::with_capacity_in(len, bump);
        let mut cursor = 0u32;
        for &count in &child_len {
            child_start.push(cursor);
            cursor += count;
        }
        let mut write_at = bumpalo::collections::Vec::with_capacity_in(len, bump);
        write_at.extend_from_slice(&child_start);
        let mut child_flat = bumpalo::collections::Vec::with_capacity_in(total_children, bump);
        child_flat.resize(total_children, 0usize);
        for (i, &p) in parents.iter().enumerate() {
            if let Some(p) = p {
                let slot = write_at[p] as usize;
                child_flat[slot] = i;
                write_at[p] += 1;
            }
        }
        let mut extents = bumpalo::collections::Vec::with_capacity_in(len, bump);
        for n in nodes {
            extents.push(geometry::own_extent(n, scale));
        }
        Tree {
            nodes,
            extents,
            child_start,
            child_len,
            child_flat,
            parents,
            roots,
        }
    }

    #[inline]
    fn children_of(&self, i: usize) -> &[usize] {
        let s = self.child_start[i] as usize;
        &self.child_flat[s..s + self.child_len[i] as usize]
    }
}

/// What a frame does to a kept scroll layer's texture before the window is painted: the layer's node (by index),
/// where its clip shows it (window pixels, the texture's size), how far its content moved since the texture was
/// painted (its pixels laid into the other texture moved back by as much), and the window pixels of the layer to
/// paint into it again.
pub(crate) struct ScrollPlan {
    pub index: usize,
    pub id: PaintNodeId,
    pub viewport: Bounds,
    pub moved: Option<(i32, i32)>,
    pub paint: Vec<Bounds>,
    /// The layer's offset in device pixels: its content is dithered by the window pixel moved by as much.
    pub shift: (i32, i32),
}

/// Records the frame's damaged rectangles `dirty` (window pixels) of `request`, after bringing the kept scroll layers
/// of `plans` up to date.
pub(crate) fn paint<R: GlyphRasterizer>(
    r: &mut GpuRenderer<R>,
    rec: &mut Recording,
    bump: &bumpalo::Bump,
    request: &RenderRequest<'_>,
    dirty: &[Bounds],
    plans: &[ScrollPlan],
) -> Result<(), RenderError> {
    rec.sizes[0] = (request.width, request.height);
    let whole = Bounds::of_size(request.width, request.height);
    let tree = Tree::new(&request.list.nodes, request.scale, bump);
    let mut p = Painter {
        r,
        rec,
        tree: &tree,
        scale: request.scale,
        request,
        whole,
        views: HashMap::new(),
    };
    for plan in plans {
        p.keep(plan)?;
    }
    for &dirty in dirty {
        let ctx = Ctx {
            target: WINDOW,
            origin: (0, 0),
            rect: whole,
            limit: dirty,
            dither: (0, 0),
        };
        if !p.rec.full {
            // the repainted rectangle starts transparent
            p.clear(&ctx, dirty);
        }
        for &root in &tree.roots {
            p.subtree(&ctx, root, &mut Vec::new())?;
        }
    }
    Ok(())
}

/// An image in an atlas: its page, the texel of its first pixel, and the window pixels it covers.
type Placed = (Tex, (u32, u32), Bounds);

/// A glyph in its atlas: its metrics, page and the texel of its first pixel.
type PlacedGlyph = (GlyphMeta, Tex, (u32, u32));

/// A quad: the target pixels it covers and the texel of its first pixel.
type Quad = (Bounds, (f32, f32));

struct Painter<'a, 'n, R> {
    r: &'a mut GpuRenderer<R>,
    rec: &'a mut Recording,
    tree: &'a Tree<'n, 'a>,
    scale: f64,
    request: &'a RenderRequest<'a>,
    whole: Bounds,
    /// The kept scroll layers laid over the window this frame, by their node's index: the target holding the
    /// layer's content as the frame shows it, and where its clip shows it.
    views: HashMap<usize, (TargetId, Bounds)>,
}

/// What a layer laid over at `opacity` is multiplied by, from 0 (it changes nothing) to 255.
fn opacity_factor(opacity: f32) -> u32 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u32
}

/// A colour premultiplied and in bytes, as fills lay it.
fn premultiplied(c: Color) -> Option<[f32; 4]> {
    if ![c.r, c.g, c.b, c.a].iter().all(|v| v.is_finite()) {
        return None;
    }
    Some(
        c.premultiplied()
            .map(|v| ((v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8) as f32),
    )
}

/// A colour premultiplied and rounded to bytes, as glyphs and shadows are tinted.
fn tint(c: Color) -> [f32; 4] {
    c.premultiplied()
        .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32 as f32)
}

/// A paint placed for the fill program: kind, stops, colour and the window pixel to node coordinates.
struct PaintData {
    kind: f32,
    count: f32,
    color: [f32; 4],
    inv: [f32; 6],
    geometry: [f32; 4],
    offsets: [f32; 8],
    colors: [f32; 32],
}

impl PaintData {
    fn solid(color: [f32; 4]) -> PaintData {
        PaintData {
            kind: 0.0,
            count: 0.0,
            color,
            inv: [0.0; 6],
            geometry: [0.0; 4],
            offsets: [0.0; 8],
            colors: [0.0; 32],
        }
    }

    /// `paint` under `window` (node coordinates to window pixels), as the CPU renderer places it; `None` draws
    /// nothing.
    fn new(paint: &Paint, window: tiny_skia::Transform) -> Option<PaintData> {
        let (given, kind, geometry) = match paint {
            Paint::Solid(c) => return premultiplied(*c).map(PaintData::solid),
            Paint::Linear { start, end, stops } => (
                stops,
                1.0,
                [start.x as f32, start.y as f32, end.x as f32, end.y as f32],
            ),
            Paint::Radial {
                center,
                radius,
                stops,
            } => (
                stops,
                2.0,
                [center.x as f32, center.y as f32, *radius as f32, 0.0],
            ),
        };
        if given.is_empty() || given.len() > MAX_STOPS {
            return None;
        }
        let mut last = 0.0f32;
        let mut stops: Vec<Stop> = given
            .iter()
            .map(|s| {
                last = s.offset.clamp(0.0, 1.0).max(last);
                let c = s.color;
                Stop {
                    offset: last,
                    color: [c.r, c.g, c.b, c.a].map(|v| v.clamp(0.0, 1.0)),
                }
            })
            .collect();
        if stops.len() == 1 {
            stops.push(stops[0]);
        }
        let stops = Stops::new(&stops).ok()?;
        let inv = geometry::inverse(window)?;
        let mut data = PaintData {
            kind,
            count: stops.as_slice().len() as f32,
            color: [0.0; 4],
            inv,
            geometry,
            offsets: [0.0; 8],
            colors: [0.0; 32],
        };
        for (i, s) in stops.as_slice().iter().enumerate() {
            data.offsets[i] = s.offset;
            data.colors[i * 4..i * 4 + 4].copy_from_slice(&s.color);
        }
        Some(data)
    }

    fn write(&self, b: &mut FillBlock) {
        b.color = self.color;
        b.paint = [
            self.kind,
            self.count,
            if dither::BY_DEFAULT { 1.0 } else { 0.0 },
            0.0,
        ];
        b.paint_linear = [self.inv[0], self.inv[1], self.inv[2], self.inv[3]];
        b.paint_moved = [self.inv[4], self.inv[5], 0.0, 0.0];
        b.paint_geometry = self.geometry;
        for (i, v) in b.offsets.iter_mut().enumerate() {
            v.copy_from_slice(&self.offsets[i * 4..i * 4 + 4]);
        }
        for (i, v) in b.colors.iter_mut().enumerate() {
            v.copy_from_slice(&self.colors[i * 4..i * 4 + 4]);
        }
    }
}

/// What covers a fill: its shape, or an 8-bit image placed at a texel.
enum Cover {
    Shape(Outline),
    Image { tex: Tex, at: (u32, u32) },
}

mod paint;
use paint::corner_exponent;
