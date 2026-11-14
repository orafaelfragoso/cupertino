//! CupertinoKit rendering. Core paints each window into a [`DisplayList`] of [`PaintNode`]s in logical points, with
//! materials and animations already resolved to plain paint values. A [`UiRenderer`] repaints the damaged part into
//! a frame it owns and the window system presents that frame. Effects such as blur sample only the window's own
//! layers, never a capture of the screen.

mod cpu;
mod effects;
mod geometry;
mod glyph_cache;
mod image_cache;
mod layers;
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

/// How a renderer keeps a node's subtree between frames: a layer boundary of the paint tree. A layer never changes what a
/// frame shows, only what the renderer paints again to show it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Layer {
    /// The node scrolls its descendants, which its clip shows moved up and to the left by `offset` (points, a whole
    /// number of device pixels; the descendants' transforms already include it). The renderer keeps what the clip
    /// shows as an image of the content in its own coordinates: a scroll moves the image and paints only what comes into
    /// view. Kept only as [`scroll_layers`] says.
    Scroll { offset: Point },
    /// The node's group is translucent and may change its opacity from frame to frame (a fade): what its subtree paints
    /// does not depend on the opacity, so a renderer that keeps the group's image lays it over at each opacity without
    /// painting the subtree again.
    Opacity,
    /// The node's transform animates, and with it its subtree's (a popover growing out of its anchor), and so may its
    /// group's opacity: what the subtree paints in its own coordinates does not change with either, so a renderer that
    /// keeps the group's image may lay it over where the transform puts it and at the group's opacity. The CPU renderer
    /// lays a kept image over only where it was painted, at another opacity: a moved image does not land its paths and
    /// dithering on the pixels painting them there would, so at any change of the transform it paints the subtree
    /// again.
    Transform,
}

/// The scroll layer each node of `list` is kept in, by index into the list, as every renderer keeps them: a node marked
/// [`Layer::Scroll`] with a clip, a transform that only moves, no group above it and no kept scroll layer above it.
/// Its descendants are kept in it; it is not kept in itself. A frame's damage and a renderer read the same answer, so a
/// node that only moved with its layer's offset is not damaged and is not painted again.
pub fn scroll_layers(list: &DisplayList) -> Vec<Option<usize>> {
    let index: HashMap<PaintNodeId, usize> = list
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect();
    // per node: the kept layer it lies in, and whether a group lies above it
    let mut kept: Vec<Option<usize>> = Vec::with_capacity(list.nodes.len());
    let mut grouped: Vec<bool> = Vec::with_capacity(list.nodes.len());
    for (i, node) in list.nodes.iter().enumerate() {
        let parent = node
            .parent
            .and_then(|p| index.get(&p))
            .copied()
            .filter(|&p| p < i);
        let (above, under_group) = match parent {
            // a scroll node inside a kept layer is not kept itself: its descendants are the outer layer's
            Some(p) => {
                let layer =
                    kept[p].or_else(|| is_kept_scroll(&list.nodes[p], grouped[p]).then_some(p));
                (layer, grouped[p] || list.nodes[p].group.is_some())
            }
            None => (None, false),
        };
        kept.push(above);
        grouped.push(under_group);
    }
    kept
}

/// Whether `node`, with a group above it or not, is a scroll layer a renderer keeps (see [`scroll_layers`]).
fn is_kept_scroll(node: &PaintNode, under_group: bool) -> bool {
    let [a, b, c, d, ..] = node.transform.0;
    matches!(node.layer, Some(Layer::Scroll { offset }) if offset.x.is_finite() && offset.y.is_finite())
        && node.clip.is_some()
        && node.group.is_none()
        && !under_group
        && (a, b, c, d) == (1.0, 0.0, 0.0, 1.0)
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
    /// Whether, and how, the renderer keeps the node's subtree between frames.
    pub layer: Option<Layer>,
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

/// A frame a window presents: its whole canonical image and the pixels this frame changed. The CPU renderer's frames
/// hold their pixels in memory; a frame drawn on a GPU keeps them on its device (`device`) and leaves `pixels`
/// empty, for a presenter on that device to draw from. [`CpuFrame::in_memory`] reads such a frame back where pixels
/// in memory are needed.
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
    /// The image on a GPU device, for a frame whose pixels stay there.
    pub device: Option<DeviceFrameRef>,
}

impl CpuFrame {
    /// The frame with its pixels in memory: itself, or a copy whose pixels were read back from its device.
    pub fn in_memory(&self) -> Result<std::borrow::Cow<'_, CpuFrame>, RenderError> {
        match &self.device {
            Some(on) if self.pixels.is_empty() => {
                let pixels = on.0.read_pixels()?;
                if pixels.len() != self.width as usize * self.height as usize * 4 {
                    return Err(RenderError::InvalidSize);
                }
                Ok(std::borrow::Cow::Owned(CpuFrame {
                    id: self.id,
                    width: self.width,
                    height: self.height,
                    stride: self.width * 4,
                    format: self.format,
                    pixels,
                    damage: self.damage.clone(),
                    device: None,
                }))
            }
            _ => Ok(std::borrow::Cow::Borrowed(self)),
        }
    }
}

/// An image kept on a GPU device: what a presenter on the same device draws from, read back where it must be in memory.
pub trait DeviceFrame {
    /// The image as its own type, for a presenter that knows it.
    fn as_any(&self) -> &dyn std::any::Any;
    /// Its pixels, premultiplied RGBA, rows first to last, without padding.
    fn read_pixels(&self) -> Result<Vec<u8>, RenderError>;
}

/// A frame's image on a device, shared with the renderer that keeps drawing into it. Two are equal when they are the
/// same image.
#[derive(Clone)]
pub struct DeviceFrameRef(pub std::rc::Rc<dyn DeviceFrame>);

impl std::fmt::Debug for DeviceFrameRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeviceFrameRef")
    }
}

impl PartialEq for DeviceFrameRef {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DeviceFrameRef {}

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
    /// Lets go of the layers kept between frames ([`Layer`]), keeping the frame: the window is not seen. The next
    /// frame paints what it shows of them whole. A renderer that keeps none does nothing.
    fn drop_layers(&mut self) {}
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    /// An image on a pretend device: two pixels.
    struct OnDevice;

    impl DeviceFrame for OnDevice {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn read_pixels(&self) -> Result<Vec<u8>, RenderError> {
            Ok(vec![1, 2, 3, 4, 5, 6, 7, 8])
        }
    }

    fn frame(pixels: Vec<u8>, device: Option<DeviceFrameRef>) -> CpuFrame {
        CpuFrame {
            id: RenderFrameId(3),
            width: 2,
            height: 1,
            stride: 8,
            format: PixelFormat::Rgba8Premultiplied,
            pixels,
            damage: vec![PixelRect {
                x: 1,
                y: 0,
                width: 1,
                height: 1,
            }],
            device,
        }
    }

    #[test]
    fn a_frame_on_a_device_is_read_back_and_one_in_memory_is_itself() {
        let on = frame(Vec::new(), Some(DeviceFrameRef(std::rc::Rc::new(OnDevice))));
        let read = on.in_memory().unwrap();
        assert!(matches!(read, std::borrow::Cow::Owned(_)));
        assert_eq!(read.pixels, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(
            (read.stride, read.damage.clone(), read.device.clone()),
            (8, on.damage.clone(), None)
        );
        let kept = frame(vec![9; 8], None);
        assert!(matches!(
            kept.in_memory().unwrap(),
            std::borrow::Cow::Borrowed(_)
        ));
        // a frame is equal to a clone of itself, image and all
        assert_eq!(on.clone(), on);
    }
}
