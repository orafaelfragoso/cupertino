//! Frame layout in logical points.
//!
//! Coordinates are relative to the top-left corner of the decorated frame: the titlebar occupies
//! `y ∈ [0, titlebar_height)` and the client content sits below it. The frame is
//! `content.width × (reserved + content.height)`, where `reserved` is the titlebar height of a
//! standard bar and zero for a unified one, whose strip lies over the content.

use crate::metrics::{ButtonKind, DecorationMetrics, TitleAlignment, TitlebarStyle};
use cupertino_geometry::{Point, Rect, Size};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonLayout {
    pub kind: ButtonKind,
    pub center: Point,
    pub radius: f64,
    /// Square art box, the hit area of the button.
    pub art: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameLayout {
    pub style: TitlebarStyle,
    pub frame: Rect,
    pub titlebar: Rect,
    pub content: Rect,
    pub buttons: [ButtonLayout; 3],
    /// Bounding box of the three art boxes: the area whose hover reveals the glyphs of all buttons.
    pub button_group: Rect,
    pub corner_radius: f64,
}

impl FrameLayout {
    /// Layout for a client content size.
    pub fn new(content: Size, style: TitlebarStyle, metrics: &DecorationMetrics, corner_radius: f64) -> Self {
        let tb = metrics.titlebar_height(style);
        let reserved = metrics.titlebar_reserved(style);
        let frame = Rect::new(0.0, 0.0, content.width, reserved + content.height);
        let titlebar = Rect::new(0.0, 0.0, content.width, tb);
        let content_rect = Rect::new(0.0, reserved, content.width, content.height);
        // the frame view centres the 16 pt cell in the bar; the disc is centred in its cell
        let cy = tb / 2.0;
        let first = metrics.button_leading(style) + metrics.button_cell.0 / 2.0;
        let buttons = ButtonKind::ALL.map(|kind| {
            let cx = first + kind.index() as f64 * metrics.button_pitch();
            let a = metrics.button_art;
            ButtonLayout { kind, center: Point::new(cx, cy), radius: metrics.button_diameter / 2.0, art: Rect::new(cx - a / 2.0, cy - a / 2.0, a, a) }
        });
        let group = buttons[0].art.union(&buttons[2].art);
        Self { style, frame, titlebar, content: content_rect, buttons, button_group: group, corner_radius }
    }

    /// Frame layout for a frame size (content size derived by removing the titlebar).
    pub fn for_frame(frame: Size, style: TitlebarStyle, metrics: &DecorationMetrics, corner_radius: f64) -> Self {
        let reserved = metrics.titlebar_reserved(style);
        Self::new(Size::new(frame.width, (frame.height - reserved).max(0.0)), style, metrics, corner_radius)
    }

    pub fn button(&self, kind: ButtonKind) -> &ButtonLayout {
        &self.buttons[kind.index()]
    }

    /// Right edge of the button group.
    pub fn group_right(&self) -> f64 {
        self.button_group.max_x()
    }
}

/// Horizontal placement of the title: the left edge and the width available to the (possibly
/// truncated) text, both in points. `None` when no room is left.
///
/// A centred title stays centred: its width is limited so that it never reaches the button group
/// from either side, i.e. to `frame_width − 2·(group_right + gap)`. A leading title starts after the
/// group and may extend to `trailing` points before the right edge.
pub fn title_slot(frame_width: f64, group_right: f64, text_width: f64, align: TitleAlignment, metrics: &DecorationMetrics) -> Option<(f64, f64)> {
    let reserve = group_right + metrics.title_gap;
    match align {
        TitleAlignment::Center => {
            let max = frame_width - 2.0 * reserve;
            if max <= 0.0 {
                return None;
            }
            let w = text_width.min(max);
            Some(((frame_width - w) / 2.0, max))
        }
        TitleAlignment::Leading => {
            let max = frame_width - reserve - metrics.title_trailing;
            if max <= 0.0 {
                return None;
            }
            Some((reserve, max))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(style: TitlebarStyle) -> FrameLayout {
        FrameLayout::new(Size::new(800.0, 600.0), style, &DecorationMetrics::default(), 12.0)
    }

    #[test]
    fn frame_stacks_titlebar_over_content() {
        let l = layout(TitlebarStyle::Standard);
        assert_eq!(l.titlebar, Rect::new(0.0, 0.0, 800.0, 28.0));
        assert_eq!(l.content, Rect::new(0.0, 28.0, 800.0, 600.0));
        assert_eq!(l.frame, Rect::new(0.0, 0.0, 800.0, 628.0));
        let u = layout(TitlebarStyle::Unified);
        assert_eq!(u.titlebar.height(), 52.0);
        assert_eq!(u.content, Rect::new(0.0, 0.0, 800.0, 600.0), "the content runs under a unified bar");
        assert_eq!(u.frame, Rect::new(0.0, 0.0, 800.0, 600.0));
    }

    #[test]
    fn buttons_are_ordered_evenly_and_vertically_centred() {
        for style in [TitlebarStyle::Standard, TitlebarStyle::Unified] {
            let l = layout(style);
            let m = DecorationMetrics::default();
            for w in l.buttons.windows(2) {
                assert!((w[1].center.x - w[0].center.x - m.button_pitch()).abs() < 1e-12);
                assert_eq!(w[0].center.y, w[1].center.y);
                assert!(!w[0].art.intersects(&w[1].art), "art boxes do not overlap");
            }
            assert_eq!(l.buttons[0].center.y, l.titlebar.height() / 2.0);
            assert_eq!(l.buttons[0].radius, 6.0);
            assert_eq!(l.buttons[0].art.width(), 16.0);
            assert_eq!(l.buttons[0].kind, ButtonKind::Close);
            assert_eq!(l.buttons[2].kind, ButtonKind::Zoom);
            assert!(l.titlebar.contains(l.buttons[2].center));
        }
    }

    #[test]
    fn first_button_sits_at_the_leading_inset_of_its_style() {
        // cell x 7 / 19 plus half the 14 pt cell; the unified bar centres the 16 pt cell in 52 pt
        assert_eq!(layout(TitlebarStyle::Standard).buttons[0].center, Point::new(14.0, 14.0));
        let u = layout(TitlebarStyle::Unified);
        assert_eq!(u.buttons[0].center, Point::new(26.0, 26.0));
        assert_eq!(u.buttons[2].center.x, 66.0);
    }

    #[test]
    fn group_covers_all_art_boxes() {
        let l = layout(TitlebarStyle::Standard);
        for b in &l.buttons {
            assert_eq!(l.button_group.union(&b.art), l.button_group);
        }
        assert_eq!(l.group_right(), l.buttons[2].art.max_x());
    }

    #[test]
    fn for_frame_inverts_new() {
        let a = layout(TitlebarStyle::Standard);
        let b = FrameLayout::for_frame(a.frame.size, TitlebarStyle::Standard, &DecorationMetrics::default(), 12.0);
        assert_eq!(a, b);
        assert_eq!(FrameLayout::for_frame(Size::new(100.0, 10.0), TitlebarStyle::Standard, &DecorationMetrics::default(), 12.0).content.height(), 0.0);
    }

    #[test]
    fn centred_title_stays_centred_and_clear_of_the_buttons() {
        let m = DecorationMetrics::default();
        let l = layout(TitlebarStyle::Standard);
        let (x, max) = title_slot(800.0, l.group_right(), 120.0, TitleAlignment::Center, &m).unwrap();
        assert_eq!(x, 340.0);
        assert!(x > l.group_right() + m.title_gap);
        assert_eq!(max, 800.0 - 2.0 * (l.group_right() + m.title_gap));
        let (x2, _) = title_slot(800.0, l.group_right(), 10_000.0, TitleAlignment::Center, &m).unwrap();
        assert_eq!(x2, l.group_right() + m.title_gap, "truncated title spans the whole free width");
        assert!(title_slot(100.0, l.group_right(), 50.0, TitleAlignment::Center, &m).is_none());
    }

    #[test]
    fn leading_title_starts_after_the_buttons() {
        let m = DecorationMetrics::default();
        let l = layout(TitlebarStyle::Unified);
        let (x, max) = title_slot(800.0, l.group_right(), 120.0, TitleAlignment::Leading, &m).unwrap();
        assert_eq!(x, l.group_right() + m.title_gap);
        assert_eq!(max, 800.0 - x - m.title_trailing);
    }
}
