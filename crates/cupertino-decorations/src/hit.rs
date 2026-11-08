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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untitled_frame_has_the_inside_band_only() {
        let m = DecorationMetrics::default();
        let f = Rect::new(0.0, 0.0, 400.0, 300.0);
        assert_eq!(
            resize_hit(f, &m, Point::new(402.0, 150.0), true),
            Some(ResizeEdge::Right)
        );
        assert_eq!(
            resize_hit(f, &m, Point::new(402.0, 150.0), false),
            None,
            "outside"
        );
        assert_eq!(
            resize_hit(f, &m, Point::new(398.0, 150.0), false),
            Some(ResizeEdge::Right),
            "inside"
        );
        assert_eq!(
            resize_hit(f, &m, Point::new(396.0, 150.0), false),
            None,
            "past the 3 pt band"
        );
    }
    use crate::metrics::TitlebarStyle;
    use cupertino_geometry::Size;

    fn setup() -> (FrameLayout, DecorationMetrics) {
        let m = DecorationMetrics::default();
        (
            FrameLayout::new(Size::new(800.0, 600.0), TitlebarStyle::Standard, &m, 12.0),
            m,
        )
    }

    fn hit(p: (f64, f64)) -> Hit {
        let (l, m) = setup();
        hit_test(&l, &m, Point::new(p.0, p.1), true)
    }

    #[test]
    fn buttons_hit_at_their_centres_and_art_boxes() {
        let (l, m) = setup();
        for b in &l.buttons {
            assert_eq!(hit_test(&l, &m, b.center, true), Hit::Button(b.kind));
            assert_eq!(
                hit_test(
                    &l,
                    &m,
                    Point::new(b.art.min_x() + 0.5, b.art.min_y() + 0.5),
                    true
                ),
                Hit::Button(b.kind)
            );
        }
        assert_eq!(
            hit((l.buttons[1].art.max_x() + 0.5, 14.0)),
            Hit::Titlebar,
            "gap between art boxes"
        );
    }

    #[test]
    fn titlebar_and_content() {
        assert_eq!(hit((400.0, 14.0)), Hit::Titlebar);
        assert_eq!(hit((400.0, 31.9)), Hit::Titlebar);
        assert_eq!(hit((400.0, 32.0)), Hit::Content);
        assert_eq!(hit((400.0, 300.0)), Hit::Content);
    }

    #[test]
    fn outside_the_input_region_is_nothing() {
        assert_eq!(hit((-5.0, 100.0)), Hit::None);
        assert_eq!(hit((400.0, 700.0)), Hit::None);
        assert_eq!(hit((900.0, 100.0)), Hit::None);
    }

    #[test]
    fn resize_zones_extend_outside_the_geometry() {
        assert_eq!(hit((-3.0, 300.0)), Hit::Resize(ResizeEdge::Left));
        assert_eq!(hit((803.0, 300.0)), Hit::Resize(ResizeEdge::Right));
        assert_eq!(hit((400.0, -3.0)), Hit::Resize(ResizeEdge::Top));
        assert_eq!(hit((400.0, 635.0)), Hit::Resize(ResizeEdge::Bottom));
        assert_eq!(hit((-4.0, 300.0)), Hit::Resize(ResizeEdge::Left));
        assert_eq!(hit((-4.1, 300.0)), Hit::None, "outset is 4 pt");
    }

    #[test]
    fn resize_zones_reach_inside_by_the_inset() {
        assert_eq!(hit((2.9, 300.0)), Hit::Resize(ResizeEdge::Left));
        assert_eq!(hit((3.0, 300.0)), Hit::Content, "inset is 3 pt");
        assert_eq!(hit((400.0, 629.0)), Hit::Resize(ResizeEdge::Bottom));
    }

    #[test]
    fn corners_resolve_to_diagonal_resizes() {
        assert_eq!(hit((-3.0, -3.0)), Hit::Resize(ResizeEdge::TopLeft));
        assert_eq!(hit((803.0, -3.0)), Hit::Resize(ResizeEdge::TopRight));
        assert_eq!(hit((-3.0, 635.0)), Hit::Resize(ResizeEdge::BottomLeft));
        assert_eq!(hit((803.0, 635.0)), Hit::Resize(ResizeEdge::BottomRight));
        assert_eq!(
            hit((11.9, -3.0)),
            Hit::Resize(ResizeEdge::TopLeft),
            "along the edge near the corner"
        );
        assert_eq!(
            hit((12.0, -3.0)),
            Hit::Resize(ResizeEdge::Top),
            "12 pt corner"
        );
        assert_eq!(hit((788.0, -3.0)), Hit::Resize(ResizeEdge::TopRight));
        assert_eq!(hit((787.9, -3.0)), Hit::Resize(ResizeEdge::Top));
        assert_eq!(hit((785.0, 634.0)), Hit::Resize(ResizeEdge::BottomRight));
        assert_eq!(
            hit((784.9, 634.0)),
            Hit::Resize(ResizeEdge::Bottom),
            "15 pt bottom-right corner"
        );
        assert_eq!(hit((801.0, 617.0)), Hit::Resize(ResizeEdge::BottomRight));
        assert_eq!(hit((801.0, 616.9)), Hit::Resize(ResizeEdge::Right));
        assert_eq!(hit((-3.0, 620.0)), Hit::Resize(ResizeEdge::BottomLeft));
        assert_eq!(hit((-3.0, 619.9)), Hit::Resize(ResizeEdge::Left));
    }

    #[test]
    fn buttons_win_over_resize_zones() {
        let (l, m) = setup();
        let b = l.buttons[0];
        assert_eq!(
            hit_test(&l, &m, Point::new(b.art.min_x() + 0.1, b.center.y), true),
            Hit::Button(ButtonKind::Close)
        );
    }

    #[test]
    fn non_resizable_windows_have_no_resize_zones() {
        let (l, m) = setup();
        assert_eq!(hit_test(&l, &m, Point::new(-3.0, 300.0), false), Hit::None);
        assert_eq!(
            hit_test(&l, &m, Point::new(1.0, 300.0), false),
            Hit::Content
        );
    }

    #[test]
    fn input_region_is_the_outset_frame() {
        let (l, m) = setup();
        assert_eq!(input_region(&l, &m), Rect::new(-4.0, -4.0, 808.0, 640.0));
    }

    #[test]
    fn double_click_acts_on_the_titlebar_only() {
        assert_eq!(
            double_click(Hit::Titlebar, DoubleClickAction::Zoom),
            DoubleClickAction::Zoom
        );
        assert_eq!(
            double_click(Hit::Titlebar, DoubleClickAction::Minimize),
            DoubleClickAction::Minimize
        );
        assert_eq!(
            double_click(Hit::Content, DoubleClickAction::Zoom),
            DoubleClickAction::None
        );
        assert_eq!(
            double_click(Hit::Button(ButtonKind::Zoom), DoubleClickAction::Zoom),
            DoubleClickAction::None
        );
    }

    #[test]
    fn button_group_hover() {
        let (l, _) = setup();
        assert!(in_button_group(&l, l.buttons[1].center));
        assert!(
            in_button_group(
                &l,
                Point::new(l.buttons[1].art.max_x() + 1.0, l.buttons[1].center.y)
            ),
            "gap inside the group"
        );
        assert!(!in_button_group(&l, Point::new(400.0, 14.0)));
    }

    #[test]
    fn edge_sides() {
        assert_eq!(ResizeEdge::TopLeft.sides(), (true, false, true, false));
        assert_eq!(ResizeEdge::Bottom.sides(), (false, false, false, true));
    }
}
