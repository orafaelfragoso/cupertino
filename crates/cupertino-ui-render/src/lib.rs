//! CupertinoKit rendering. Core paints each window into a [`DisplayList`] in logical points; a [`UiRenderer`] draws
//! the damaged part of it into a [`PixelTarget`] at the window's scale. Effects such as materials are drawn from the
//! window's own content and a local recipe, never from a capture of the screen.

use cupertino_geometry::{Point, Rect};
use cupertino_theme::Color;
use cupertino_ui_text::GlyphRun;

/// The core node a run of display items was painted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PaintNodeId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ImageId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradientStop {
    pub offset: f32,
    pub color: Color,
}

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

/// How an image fills its rectangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ImageFit {
    #[default]
    Fit,
    Fill,
    Cover,
}

/// The materials a window may draw behind its content; each has a deterministic local fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Material {
    WindowBackground,
    Sidebar,
    Menu,
    Popover,
    Header,
    Sheet,
    ToolTip,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DisplayItem {
    /// Starts the items of one core node: what it painted may spill outside `hit`, which is where it takes input.
    Node {
        id: PaintNodeId,
        bounds: Rect,
        hit: Rect,
    },
    PushClip {
        rect: Rect,
        radius: f64,
    },
    PopClip,
    PushLayer {
        opacity: f32,
    },
    PopLayer,
    Rect {
        rect: Rect,
        radius: f64,
        paint: Paint,
    },
    Stroke {
        rect: Rect,
        radius: f64,
        width: f64,
        paint: Paint,
    },
    Shadow {
        rect: Rect,
        radius: f64,
        blur: f64,
        offset: Point,
        color: Color,
    },
    Glyphs {
        origin: Point,
        run: GlyphRun,
        color: Color,
    },
    Image {
        id: ImageId,
        rect: Rect,
        fit: ImageFit,
    },
    Material {
        rect: Rect,
        radius: f64,
        material: Material,
    },
}

/// One window's paint, in logical points.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayList {
    pub items: Vec<DisplayItem>,
}

/// The parts of a window, in logical points, whose pixels must be redrawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Damage {
    pub rects: Vec<Rect>,
}

/// Premultiplied ARGB8888 pixels, little-endian, as Wayland's `argb8888`.
pub struct PixelTarget<'a> {
    pub pixels: &'a mut [u8],
    pub width: u32,
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// Physical pixels per logical point.
    pub scale: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    TargetTooSmall,
    MissingImage(ImageId),
}

pub trait UiRenderer {
    /// Draws the parts of `list` inside `damage` into `target`, leaving every other pixel as it was.
    fn render(
        &mut self,
        list: &DisplayList,
        damage: &Damage,
        target: &mut PixelTarget<'_>,
    ) -> Result<(), RenderError>;
}
