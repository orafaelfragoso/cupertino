//! Hit-testing of the decorated frame.
//!
//! Points are in frame-local logical points (see [`crate::layout`]); negative coordinates and
//! coordinates beyond the frame are outside the frame geometry and can still hit a resize zone.
//! Priority: window control buttons, resize zones, titlebar (drag), content.

use crate::layout::FrameLayout;
use crate::metrics::{ButtonKind, DecorationMetrics, DoubleClickAction, TitlebarStyle};
use cupertino_geometry::{Point, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    None,
    Content,
    /// Titlebar background: drags the window, a double click runs the configured action.
    Titlebar,
    Button(ButtonKind),
    Resize(ResizeEdge),
}

/// Band around the frame in which the pointer resizes: from `outset` points outside to `inset`
/// points inside the frame geometry (4 and 3 pt).
pub fn resize_band(frame: Rect, metrics: &DecorationMetrics) -> (Rect, Rect) {
    (
        frame.outset(metrics.resize_outset),
        frame.outset(-metrics.resize_inset),
    )
}

/// The rectangle in which the compositor must deliver pointer input for the frame to be
/// hit-tested, i.e. the frame grown by the resize outset.
pub fn input_region(layout: &FrameLayout, metrics: &DecorationMetrics) -> Rect {
    layout.frame.outset(metrics.resize_outset)
}

/// The resize zone of the frame `f` under `p`, if any: the band of [`resize_band`], with the corner zones running
/// along the edges from each corner (12 pt, the bottom-right one 15 pt). Server-decorated and
/// client-decorated windows share it, measured from their frame. The part outside the frame exists for titled windows
/// only (`exterior`); an untitled window has the inside band alone.
pub fn resize_hit(
    f: Rect,
    metrics: &DecorationMetrics,
    p: Point,
    exterior: bool,
) -> Option<ResizeEdge> {
    let (outer, inner) = resize_band(f, metrics);
    let outer = if exterior { outer } else { f };
    if !outer.contains(p) || inner.contains(p) {
        return None;
    }
    // corner zones run along the edges from the corner; the bottom-right one is longer
    let (cx, cy) = (
        metrics.resize_corner.min(f.width() / 2.0),
        metrics.resize_corner.min(f.height() / 2.0),
    );
    let (sx, sy) = (
        metrics.resize_corner_bottom_right.min(f.width() / 2.0),
        metrics.resize_corner_bottom_right.min(f.height() / 2.0),
    );
    let (on_left, on_right) = (p.x < inner.min_x(), p.x >= inner.max_x());
    let (on_top, on_bottom) = (p.y < inner.min_y(), p.y >= inner.max_y());
    let left = on_left || ((on_top || on_bottom) && p.x < f.min_x() + cx);
    let right = !left
        && (on_right || (on_top && p.x >= f.max_x() - cx) || (on_bottom && p.x >= f.max_x() - sx));
    let top = on_top || ((on_left || on_right) && p.y < f.min_y() + cy);
    let bottom = !top
        && (on_bottom || (on_left && p.y >= f.max_y() - cy) || (on_right && p.y >= f.max_y() - sy));
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (true, _, _, true) => ResizeEdge::BottomLeft,
        (true, _, _, _) => ResizeEdge::Left,
        (_, true, true, _) => ResizeEdge::TopRight,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (_, true, _, _) => ResizeEdge::Right,
        (_, _, true, _) => ResizeEdge::Top,
        _ => ResizeEdge::Bottom,
    })
}

/// What lies under `p`. `resizable = false` disables the resize zones.
pub fn hit_test(
    layout: &FrameLayout,
    metrics: &DecorationMetrics,
    p: Point,
    resizable: bool,
) -> Hit {
    for b in &layout.buttons {
        if b.art.contains(p) {
            return Hit::Button(b.kind);
        }
    }
    if resizable {
        // a decorated frame has a title bar: the band reaches outside
        if let Some(e) = resize_hit(layout.frame, metrics, p, true) {
            return Hit::Resize(e);
        }
    }
    // the strip of a unified bar belongs to the client's toolbar, which moves the window itself
    if layout.style == TitlebarStyle::Standard && layout.titlebar.contains(p) {
        return Hit::Titlebar;
    }
    if layout.content.contains(p) {
        return Hit::Content;
    }
    Hit::None
}

/// Whether the pointer is over the button group (hovering it reveals the glyphs of all three).
pub fn in_button_group(layout: &FrameLayout, p: Point) -> bool {
    layout.button_group.contains(p)
}

/// Action for a double click that hit `hit`.
pub fn double_click(hit: Hit, action: DoubleClickAction) -> DoubleClickAction {
    match hit {
        Hit::Titlebar => action,
        _ => DoubleClickAction::None,
    }
}

impl ResizeEdge {
    /// Edge bits for a resize request: (left, right, top, bottom).
    pub fn sides(self) -> (bool, bool, bool, bool) {
        use ResizeEdge::*;
        (
            matches!(self, Left | TopLeft | BottomLeft),
            matches!(self, Right | TopRight | BottomRight),
            matches!(self, Top | TopLeft | TopRight),
            matches!(self, Bottom | BottomLeft | BottomRight),
        )
    }
}
