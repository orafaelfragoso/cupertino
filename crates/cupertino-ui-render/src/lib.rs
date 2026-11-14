//! CupertinoKit rendering. Core paints each window into a [`DisplayList`] of [`PaintNode`]s in logical points, with
//! materials and animations already resolved to plain paint values. A [`UiRenderer`] repaints the damaged part into
//! a frame it owns and the window system presents that frame. Effects such as blur sample only the window's own
//! layers, never a capture of the screen.

mod cpu;
mod effects;
mod geometry;
mod glyph_cache;
mod image_cache;
mod raster;
mod spans;
mod stats;
mod validate;

pub use cpu::{CpuConfig, CpuRenderer};
/// The colour [`Paint::Solid`] and the other paints take.
pub use cupertino_theme::Color;
pub use stats::{RenderStats, Tally};

use cupertino_geometry::{Point, Rect};
use cupertino_ui_text::{GlyphRun, TextError};
use std::collections::HashMap;
use std::sync::Arc;

/// A paint node's identity across frames; core maps each of its node ids to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaintNodeId(pub u64);

/// Bumped by core whenever a paint node's operations change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaintRevision(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId(pub u64);

/// A resource as one paint operation uses it; a different revision in [`PaintResources`] is an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceRef {
    pub id: ResourceId,
    pub revision: u64,
}

/// A 2D affine transform `[a, b, c, d, e, f]`: `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform(pub [f64; 6]);

impl Transform {
    pub const IDENTITY: Transform = Transform([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathVerb {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CubicTo(Point, Point, Point),
    Close,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FillRule {
    #[default]
    NonZero,
    EvenOdd,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect(Rect),
    RoundedRect {
        rect: Rect,
        radius: f64,
    },
    Path {
        verbs: Vec<PathVerb>,
        fill_rule: FillRule,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

/// Source colour; gradients interpolate straight-alpha stops in encoded sRGB and premultiply the result, once per pixel
/// at its centre. A gradient has at most eight stops: one with more is refused and draws nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Color),
    Linear {
        start: Point,
        end: Point,
        stops: Vec<GradientStop>,
    },
    Radial {
        center: Point,
        radius: f64,
        stops: Vec<GradientStop>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sampling {
    Nearest,
    #[default]
    Linear,
}

/// An effect on an isolated layer, computed from the layer's own content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    Blur {
        radius: f64,
    },
    DropShadow {
        offset: Point,
        blur: f64,
        color: Color,
    },
    InnerShadow {
        offset: Point,
        blur: f64,
        color: Color,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaintOp {
    Fill {
        shape: Shape,
        paint: Paint,
    },
    Stroke {
        shape: Shape,
        width: f64,
        paint: Paint,
    },
    /// `origin` is the run's baseline start.
    Glyphs {
        origin: Point,
        run: GlyphRun,
        color: Color,
    },
    Image {
        image: ResourceRef,
        src: Rect,
        dst: Rect,
        sampling: Sampling,
    },
    PushClip(Shape),
    PopClip,
    /// Starts an isolated group: `opacity` applies once to the whole group, after `effects`.
    PushLayer {
        opacity: f32,
        effects: Vec<Effect>,
    },
    PopLayer,
}

/// Composites a node and its whole subtree as one isolated layer: `opacity` applies once to the result, after
/// `effects`.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub opacity: f32,
    pub effects: Vec<Effect>,
}

/// What one core node painted.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintNode {
    pub id: PaintNodeId,
    /// The node whose subtree this one belongs to. A parent comes before its children in the list, so the list is a
    /// preorder walk of the forest; a parent that is not an earlier node of the list is an error. The renderer
    /// paints a node, then its children, and applies each ancestor's `clip` and `group` to the subtree.
    pub parent: Option<PaintNodeId>,
    pub revision: PaintRevision,
    /// Applies to `clip` and `ops`, which are in the node's own coordinates.
    pub transform: Transform,
    /// The clip this node puts on its descendants, in node coordinates; the node's own ops are not cut by it.
    pub clip: Option<Shape>,
    /// Paints this node and its subtree into one layer, composited once, under the ancestors' clips.
    pub group: Option<Group>,
    /// Everything the node and its descendants may touch, effects included, in window points. The renderer skips a
    /// subtree that misses the damage and sizes a group's layer from it; it works out the node's own extent itself.
    pub bounds: Rect,
    /// Where the node takes pointer input, in window points; never its shadow or focus ring.
    pub hit: Rect,
    pub ops: Vec<PaintOp>,
}

/// One window's paint, back to front, in logical points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayList {
    pub nodes: Vec<PaintNode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// Premultiplied RGBA, one byte per component, in memory order.
    Rgba8Premultiplied,
    /// Straight-alpha RGBA, normalised when the image is added.
    Rgba8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageResource {
    pub revision: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub pixels: Arc<[u8]>,
}

/// The images the operations of a display list refer to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaintResources {
    pub images: HashMap<ResourceId, ImageResource>,
}

/// What must be repainted, in logical points.
#[derive(Clone, Debug, PartialEq)]
pub enum DamageRegion {
    Full,
    Rects(Vec<Rect>),
}

/// One repaint of a window.
#[derive(Clone, Copy, Debug)]
pub struct RenderRequest<'a> {
    pub list: &'a DisplayList,
    pub resources: &'a PaintResources,
    /// Physical pixels per logical point.
    pub scale: f64,
    pub width: u32,
    pub height: u32,
    pub damage: &'a DamageRegion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RenderFrameId(pub u64);

/// A rectangle of whole physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// A frame of the CPU renderer: its whole canonical image and the pixels this frame changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CpuFrame {
    pub id: RenderFrameId,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// Always [`PixelFormat::Rgba8Premultiplied`].
    pub format: PixelFormat,
    pub pixels: Vec<u8>,
    pub damage: Vec<PixelRect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    MissingResource(ResourceId),
    StaleResource(ResourceId),
    /// Clip or layer scopes do not balance.
    UnbalancedScope(PaintNodeId),
    NonFiniteValue(PaintNodeId),
    InvalidScale,
    /// A target with no pixels.
    InvalidSize,
    /// A parent that is not an earlier node of the list.
    UnknownParent(PaintNodeId),
    /// Two nodes with the same id.
    DuplicateNode(PaintNodeId),
    /// A node deeper than 256 levels.
    TooDeep(PaintNodeId),
    /// An image buffer shorter than its stride and height, or a stride shorter than a row.
    InvalidImage(ResourceId),
    Glyph(TextError),
    /// An image, layer or target exceeds the renderer's allocation limits.
    ResourceLimit,
    UnknownFrame(RenderFrameId),
}

/// Checks `request` as every renderer does before it draws anything: the scale, the size, the tree, balanced scopes,
/// finite values and the resources the operations name.
pub fn check_request(request: &RenderRequest<'_>) -> Result<(), RenderError> {
    validate::check(
        request.list,
        request.resources,
        request.scale,
        request.width,
        request.height,
    )
}

/// The rectangles every renderer repaints for `rects` damaged on a `width` by `height` target at `scale`: each grown
/// to whole pixels with a pixel of margin, cut to the target, and those that touch merged, sorted top to bottom.
pub fn damage_rects(rects: &[Rect], scale: f64, width: u32, height: u32) -> Vec<PixelRect> {
    let whole = geometry::Bounds::of_size(width, height);
    geometry::merge(
        rects
            .iter()
            .map(|r| geometry::Bounds::of_rect(r, scale).intersect(&whole))
            .collect(),
    )
    .iter()
    .map(|d| PixelRect {
        x: d.x0 as u32,
        y: d.y0 as u32,
        width: d.width(),
        height: d.height(),
    })
    .collect()
}

pub trait UiRenderer {
    type Frame;

    /// Repaints the damaged region into the renderer's own output. On error no partial frame is exposed and the
    /// next request repaints in full.
    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError>;
    /// Borrows a frame for presentation; only the latest frame id is valid.
    fn frame(&self, id: RenderFrameId) -> Result<&Self::Frame, RenderError>;
}
