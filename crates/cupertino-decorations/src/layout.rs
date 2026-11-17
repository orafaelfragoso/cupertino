//! Frame layout in logical points.
//!
//! Coordinates are relative to the top-left corner of the decorated frame: the titlebar occupies
//! `y ∈ [0, titlebar_height)` and the client content sits below it. The frame is
//! `content.width × (reserved + content.height)`, where `reserved` is the titlebar height of a
//! standard bar and zero for a unified one, whose strip lies over the content. `unobscured` is the part of the content
//! no chrome covers (the content below a unified strip), where the client's layout starts ([`crate::chrome`]).

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
    /// The part of `content` the title bar or the client's toolbar row does not cover.
    pub unobscured: Rect,
    pub buttons: [ButtonLayout; 3],
    /// Bounding box of the three art boxes: the area whose hover reveals the glyphs of all buttons.
    pub button_group: Rect,
    pub corner_radius: f64,
}

impl FrameLayout {
    /// Layout for a client content size.
    pub fn new(
        content: Size,
        style: TitlebarStyle,
        metrics: &DecorationMetrics,
        corner_radius: f64,
    ) -> Self {
        let tb = metrics.titlebar_height(style);
        let reserved = metrics.titlebar_reserved(style);
        let frame = Rect::new(0.0, 0.0, content.width, reserved + content.height);
        let titlebar = Rect::new(0.0, 0.0, content.width, tb);
        let content_rect = Rect::new(0.0, reserved, content.width, content.height);
        let covered = (tb - reserved).clamp(0.0, content.height);
        let unobscured = Rect::new(
            0.0,
            reserved + covered,
            content.width,
            content.height - covered,
        );
        // the frame view centres the 16 pt cell in the bar; the disc is centred in its cell
        let cy = tb / 2.0;
        let first = metrics.button_leading(style) + metrics.button_cell.0 / 2.0;
        let rtl = metrics.direction == crate::chrome::Direction::RightToLeft;
        let buttons = ButtonKind::ALL.map(|kind| {
            let cx = first + kind.index() as f64 * metrics.button_pitch();
            // right to left: the same distance from the right edge, close outermost
            let cx = if rtl { content.width - cx } else { cx };
            let a = metrics.button_art;
            ButtonLayout {
                kind,
                center: Point::new(cx, cy),
                radius: metrics.button_diameter / 2.0,
                art: Rect::new(cx - a / 2.0, cy - a / 2.0, a, a),
            }
        });
        let group = buttons[0].art.union(&buttons[2].art);
        Self {
            style,
            frame,
            titlebar,
            content: content_rect,
            unobscured,
            buttons,
            button_group: group,
            corner_radius,
        }
    }

    /// Frame layout for a frame size (content size derived by removing the titlebar).
    pub fn for_frame(
        frame: Size,
        style: TitlebarStyle,
        metrics: &DecorationMetrics,
        corner_radius: f64,
    ) -> Self {
        let reserved = metrics.titlebar_reserved(style);
        Self::new(
            Size::new(frame.width, (frame.height - reserved).max(0.0)),
            style,
            metrics,
            corner_radius,
        )
    }

    pub fn button(&self, kind: ButtonKind) -> &ButtonLayout {
        &self.buttons[kind.index()]
    }

    /// Right edge of the button group.
    pub fn group_right(&self) -> f64 {
        self.button_group.max_x()
    }

    /// How far the button group reaches in from the edge it sits at: its right edge left to right, the frame's width
    /// less its left edge right to left. A centred title keeps this far from both edges.
    pub fn group_reach(&self) -> f64 {
        if self.button_group.min_x() > self.frame.width() / 2.0 {
            self.frame.width() - self.button_group.min_x()
        } else {
            self.button_group.max_x()
        }
    }
}

/// Horizontal placement of the title: the left edge and the width available to the (possibly
/// truncated) text, both in points. `None` when no room is left.
///
/// A centred title stays centred: its width is limited so that it never reaches the button group
/// from either side, i.e. to `frame_width − 2·(group_right + gap)`. A leading title starts after the
/// group and may extend to `trailing` points before the right edge.
pub fn title_slot(
    frame_width: f64,
    group_right: f64,
    text_width: f64,
    align: TitleAlignment,
    metrics: &DecorationMetrics,
) -> Option<(f64, f64)> {
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
