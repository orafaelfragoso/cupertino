//! Walking a display list into draws, as the CPU renderer walks it: a node's own operations under its ancestors'
//! clips, cut at the pixels its scratch image would cover; groups and layers into offscreen targets sized as the CPU
//! renderer sizes them, their effects, then laid over through the clips around them. Draws are recorded with their
//! uniforms and instances and executed afterwards ([`crate::exec`]); glyphs, path masks and images are uploaded while
//! recording.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use cupertino_gpu::atlas::Atlas;
use cupertino_gpu::{Error as GpuError, Region, TextureDesc, TextureFormat, TextureUsage};
use cupertino_raster_math::blur::{plan_blur, MAX_BLUR_RADIUS};
use cupertino_raster_math::coverage::{coverage, rect_distance, rounded_distance};
use cupertino_raster_math::dither;
use cupertino_raster_math::gradient::{Stop, Stops, MAX_STOPS};
use cupertino_theme::Color;
use cupertino_ui_render::{
    Effect, Group, Paint, PaintNode, PaintNodeId, PaintOp, PixelFormat, RenderError, RenderRequest,
    ResourceId, Sampling, Shape,
};
use cupertino_ui_text::{GlyphFormat, GlyphKey, GlyphRasterizer, GlyphRun};

use crate::geometry::{self, Bounds, Outline};
use crate::{gpu, GlyphMeta, GpuRenderer, ImageTexture, TargetMem, UNIFORM_ALIGN};

/// Largest glyph the renderer rasterizes, in physical pixels per em (the CPU renderer's limit).
const MAX_GLYPH_PX: f64 = 8192.0;

/// Analytic clips one draw takes; more are rasterized into its clip mask.
const MAX_ANALYTIC_CLIPS: usize = 4;

/// Floats of each program's uniform block.
pub(crate) const FILL_FLOATS: usize = 136;
pub(crate) const TEXTURE_FLOATS: usize = 96;
pub(crate) const RESOLVE_FLOATS: usize = 16;
pub(crate) const LEVEL_FLOATS: usize = 4;
pub(crate) const KERNEL_FLOATS: usize = 40;

/// Where a clip block starts in each program's uniforms.
const FILL_CLIPS: usize = 76;
const TEXTURE_CLIPS: usize = 36;

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
    pub targets: Vec<TargetMem>,
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
            targets: vec![TargetMem::Window],
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

    fn instance_count(&self) -> u32 {
        (self.instances.len() / 8) as u32
    }

    /// Gives every target and texture of the frame back.
    pub(crate) fn release<R>(self, r: &mut GpuRenderer<R>) {
        for t in self.targets {
            match t {
                TargetMem::Window => {}
                TargetMem::Pooled(p) => r.pool.release(p, r.frames),
                TargetMem::Own(t) => r.device.destroy_texture(t),
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
    fn new(shape: &Shape, transform: &cupertino_ui_render::Transform, scale: f64) -> ClipItem {
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
    fn write(&self, u: &mut [f32], base: usize) {
        u[base] = self.shapes.len() as f32;
        if let Some(m) = &self.mask {
            u[base + 1] = 1.0;
            u[base + 4..base + 8].copy_from_slice(&[
                m.rect.x0 as f32,
                m.rect.y0 as f32,
                m.rect.x1 as f32,
                m.rect.y1 as f32,
            ]);
            u[base + 8] = m.at.0 as f32;
            u[base + 9] = m.at.1 as f32;
        }
        for (i, o) in self.shapes.iter().enumerate() {
            let at = base + 12 + i * 12;
            u[at..at + 4].copy_from_slice(&o.inv[..4]);
            u[at + 4..at + 8].copy_from_slice(&[o.inv[4], o.inv[5], o.half[0], o.half[1]]);
            u[at + 8] = o.corner;
            u[at + 9] = o.px;
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
}

impl Ctx {
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
        Region::new(l.x0 as u32, l.y0 as u32, l.width(), l.height())
    }
}

/// The forest of a display list.
struct Tree<'n> {
    nodes: &'n [PaintNode],
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
                Some(&p) => children[p].push(i),
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

/// Records the frame's damaged rectangle `dirty` (window pixels) of `request`.
pub(crate) fn paint<R: GlyphRasterizer>(
    r: &mut GpuRenderer<R>,
    rec: &mut Recording,
    request: &RenderRequest<'_>,
    dirty: &[Bounds],
) -> Result<(), RenderError> {
    rec.sizes[0] = (request.width, request.height);
    let whole = Bounds::of_size(request.width, request.height);
    let tree = Tree::new(&request.list.nodes, request.scale);
    for &dirty in dirty {
        let ctx = Ctx {
            target: WINDOW,
            origin: (0, 0),
            rect: whole,
            limit: dirty,
        };
        if !rec.full {
            // the repainted rectangle starts transparent
            let mut u = [0.0f32; FILL_FLOATS];
            u[..4].copy_from_slice(&ctx.frame(rec));
            u[4] = 1.0;
            u[7] = 1.0;
            u[8] = -(dirty.x0 as f32 + dirty.x1 as f32) / 2.0;
            u[9] = -(dirty.y0 as f32 + dirty.y1 as f32) / 2.0;
            u[10] = dirty.width() as f32 / 2.0 + 1.0;
            u[11] = dirty.height() as f32 / 2.0 + 1.0;
            u[13] = 1.0;
            let uniform = rec.uniform(&u);
            let first = rec.instance_count();
            rec.quad(dirty, (0.0, 0.0));
            rec.cmds.push(Cmd::Quads {
                program: Program::Clear,
                target: WINDOW,
                scissor: ctx.scissor(&dirty),
                uniform,
                textures: [Tex::Empty; 3],
                instances: (first, 1),
            });
        }
        let mut p = Painter {
            r,
            rec,
            tree: &tree,
            scale: request.scale,
            request,
            whole,
        };
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
    tree: &'a Tree<'n>,
    scale: f64,
    request: &'a RenderRequest<'a>,
    whole: Bounds,
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

    fn write(&self, u: &mut [f32]) {
        u[16..20].copy_from_slice(&self.color);
        u[20] = self.kind;
        u[21] = self.count;
        u[22] = if dither::BY_DEFAULT { 1.0 } else { 0.0 };
        u[24..28].copy_from_slice(&self.inv[..4]);
        u[28] = self.inv[4];
        u[29] = self.inv[5];
        u[32..36].copy_from_slice(&self.geometry);
        u[36..44].copy_from_slice(&self.offsets);
        u[44..76].copy_from_slice(&self.colors);
    }
}

/// What covers a fill: its shape, or an 8-bit image placed at a texel.
enum Cover {
    Shape(Outline),
    Image { tex: Tex, at: (u32, u32) },
}

impl<R: GlyphRasterizer> Painter<'_, '_, R> {
    fn device(&self) -> &cupertino_gpu::Device {
        &self.r.device
    }

    fn subtree(
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
        self.children(ctx, i, chain)
    }

    fn children(
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
        let done = self.tree.children[i]
            .iter()
            .try_for_each(|&child| self.subtree(ctx, child, chain));
        if clipped {
            chain.pop();
        }
        done
    }

    /// The pixels a node's own operations can change where `limit` is needed, under the clips of `chain`.
    fn own_area(
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

    fn group(
        &mut self,
        ctx: &Ctx,
        i: usize,
        group: &Group,
        chain: &[ClipItem],
    ) -> Result<(), RenderError> {
        let node = &self.tree.nodes[i];
        let area = Bounds::of_rect(&node.bounds, self.scale).intersect(&ctx.rect);
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
        };
        self.own(&inner, i, &[])?;
        self.children(&inner, i, &mut Vec::new())?;
        let mut layer = layer;
        for effect in &group.effects {
            layer = self.effect(layer, area, effect)?;
        }
        let visible = area.intersect(&ctx.limit);
        self.lay_layer(ctx, layer, area, visible, k, chain)
    }

    fn own(&mut self, ctx: &Ctx, i: usize, chain: &[ClipItem]) -> Result<(), RenderError> {
        if self.own_area(i, chain, &ctx.rect, &ctx.limit).is_none() {
            return Ok(());
        }
        let Some(area) = self.own_area(i, chain, &ctx.rect, &ctx.rect) else {
            return Ok(());
        };
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
    fn run(
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
    fn layer(
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
        };
        self.run(&inner, i, area, area, &mut Vec::new(), pos)?;
        let mut layer = layer;
        for effect in effects {
            layer = self.effect(layer, area, effect)?;
        }
        let k = opacity_factor(opacity);
        if k == 0 {
            return Ok(());
        }
        self.lay_layer(ctx, layer, area, area.intersect(&needed), k, clips)
    }

    /// A new offscreen target covering the window pixels `area`, transparent.
    fn target(&mut self, area: Bounds) -> Result<TargetId, RenderError> {
        let target = self.target_of((area.width(), area.height()), TextureFormat::Rgba8)?;
        self.rec.cmds.push(Cmd::Begin { target });
        Ok(target)
    }

    fn target_of(
        &mut self,
        size: (u32, u32),
        format: TextureFormat,
    ) -> Result<TargetId, RenderError> {
        let device = std::rc::Rc::clone(&self.r.device);
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
        self.r.stats.layers += 1;
        self.rec.targets.push(mem);
        self.rec.sizes.push(size);
        self.rec.started.push(false);
        Ok(TargetId(self.rec.targets.len() - 1))
    }

    /// The format of the blur's levels: floats where the device renders to them (the blur's sums kept as exact as
    /// the CPU keeps them), then half floats, then bytes.
    fn level_format(&self) -> TextureFormat {
        let info = self.device().info();
        if info.float_targets {
            TextureFormat::Rgba32Float
        } else if info.half_float_targets {
            TextureFormat::Rgba16Float
        } else {
            TextureFormat::Rgba8
        }
    }

    /// Applies `effect` to the layer `layer` covering `area`: the layer that holds the result.
    fn effect(
        &mut self,
        layer: TargetId,
        area: Bounds,
        effect: &Effect,
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
        Ok(out)
    }

    /// Lays the layer `layer` (covering `area`) over `ctx` where `visible`, at opacity `k`, through `clips`.
    fn lay_layer(
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
        let mut u = [0.0f32; TEXTURE_FLOATS];
        u[..4].copy_from_slice(&ctx.frame(self.rec));
        u[4] = 1.0;
        u[5] = k as f32;
        u[12] = area.x0 as f32;
        u[13] = area.y0 as f32;
        c.write(&mut u, TEXTURE_CLIPS);
        let local = ctx.local(&shown);
        self.push_texture(
            ctx,
            &u,
            shown,
            [Tex::Target(layer), c.mask_tex(), Tex::Empty],
            &[(local, (0.0, 0.0))],
        );
        Ok(())
    }

    /// Lays the image of a stroke, drawn in its colour at `at` of `tex` and covering `rect`, under `clips`.
    fn lay_stroke(
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
        let mut u = [0.0f32; TEXTURE_FLOATS];
        u[..4].copy_from_slice(&ctx.frame(self.rec));
        u[8..12].copy_from_slice(&[255.0; 4]);
        c.write(&mut u, TEXTURE_CLIPS);
        let atlas = (
            (at.0 as i32 + shown.x0 - rect.x0) as f32,
            (at.1 as i32 + shown.y0 - rect.y0) as f32,
        );
        self.push_texture(
            ctx,
            &u,
            shown,
            [tex, c.mask_tex(), Tex::Empty],
            &[(ctx.local(&shown), atlas)],
        );
        self.r.stats.draws += 1;
        Ok(())
    }

    fn push_texture(
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
    fn clip_draw(&mut self, clips: &[ClipItem]) -> Result<Option<ClipDraw>, RenderError> {
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
    fn clip_mask(&mut self, clips: &[&ClipItem]) -> Result<Option<MaskRef>, RenderError> {
        let mut h = DefaultHasher::new();
        let mut rect = self.whole;
        for clip in clips {
            rect = rect.intersect(&clip.bounds());
            match clip {
                ClipItem::Path { key, .. } => key.hash(&mut h),
                ClipItem::Shape { outline, .. } => {
                    for v in outline
                        .inv
                        .iter()
                        .chain(&outline.half)
                        .chain([&outline.corner, &outline.px])
                    {
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
    fn upload_coverage(
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
    fn flush(&mut self) -> Result<(), RenderError> {
        self.r.execute(self.rec).map_err(gpu)?;
        self.r.frames += 1;
        self.rec.masks.clear();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn fill(
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
        let mut u = [0.0f32; FILL_FLOATS];
        u[..4].copy_from_slice(&ctx.frame(self.rec));
        let (coverage_tex, atlas) = match &cover {
            Cover::Shape(o) => {
                u[4..8].copy_from_slice(&o.inv[..4]);
                u[8..12].copy_from_slice(&[o.inv[4], o.inv[5], o.half[0], o.half[1]]);
                u[12] = o.corner;
                u[13] = o.px;
                u[14] = o.stroke.unwrap_or(0.0);
                (Tex::Empty, (0.0, 0.0))
            }
            Cover::Image { tex, at } => {
                u[15] = 1.0;
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
        data.write(&mut u);
        c.write(&mut u, FILL_CLIPS);
        let uniform = self.rec.uniform(&u);
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
    fn path_cover(
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
    fn glyphs(
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
                    let mut u = [0.0f32; FILL_FLOATS];
                    u[..4].copy_from_slice(&ctx.frame(self.rec));
                    u[15] = 1.0;
                    PaintData::solid(tinted).write(&mut u);
                    clip.write(&mut u, FILL_CLIPS);
                    let uniform = self.rec.uniform(&u);
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
                    let mut u = [0.0f32; TEXTURE_FLOATS];
                    u[..4].copy_from_slice(&ctx.frame(self.rec));
                    u[8..12].copy_from_slice(&tinted);
                    clip.write(&mut u, TEXTURE_CLIPS);
                    self.push_texture(ctx, &u, limit, [tex, clip.mask_tex(), Tex::Empty], &quads);
                }
            }
            self.r.stats.draws += 1;
        }
        Ok(())
    }

    /// A glyph's raster placed in its atlas: its metrics, page and texel; `None` for a glyph with no pixels.
    fn glyph(&mut self, key: GlyphKey) -> Result<Option<PlacedGlyph>, RenderError> {
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
    fn image(
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
        let device = ts.pre_concat(place);
        if device.kx == 0.0
            && device.ky == 0.0
            && (device.sx - 1.0).abs() < 1e-4
            && (device.sy - 1.0).abs() < 1e-4
        {
            ts = ts.post_translate(device.tx.round() - device.tx, device.ty.round() - device.ty);
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
        let mut u = [0.0f32; TEXTURE_FLOATS];
        u[..4].copy_from_slice(&ctx.frame(self.rec));
        u[4] = 2.0;
        u[5] = 255.0;
        u[6] = if sampling == Sampling::Linear {
            1.0
        } else {
            0.0
        };
        u[16..20].copy_from_slice(&[x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
        // window pixel to the crop's texels, then to the image's
        u[20..24].copy_from_slice(&inv[..4]);
        u[24] = inv[4] + x0 as f32;
        u[25] = inv[5] + y0 as f32;
        let mut cover = Tex::Empty;
        if let Some(edges) = aligned(&Shape::Rect(*dst), ts) {
            // edges on whole pixels: every pixel inside is covered
            let inside = edges.intersect(&shown);
            if inside.is_empty() {
                return Ok(());
            }
            c.write(&mut u, TEXTURE_CLIPS);
            let local = ctx.local(&inside);
            self.push_texture(
                ctx,
                &u,
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
            u[7] = 1.0;
            u[28..32].copy_from_slice(&[
                rect.x0 as f32,
                rect.y0 as f32,
                rect.x1 as f32,
                rect.y1 as f32,
            ]);
            u[32] = at.0 as f32;
            u[33] = at.1 as f32;
        } else {
            return Ok(());
        }
        c.write(&mut u, TEXTURE_CLIPS);
        let local = ctx.local(&shown);
        self.push_texture(
            ctx,
            &u,
            shown,
            [Tex::Image(id), c.mask_tex(), cover],
            &[(local, (0.0, 0.0))],
        );
        Ok(())
    }

    /// The image `id` as a premultiplied texture of the renderer's, uploaded when new or changed.
    fn upload_image(&mut self, id: ResourceId) -> Result<(), RenderError> {
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
enum Kind {
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
            rounded_distance(p, o.half, o.corner, 2.0)
        } else {
            rect_distance(p, o.half)
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
