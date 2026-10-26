//! Window kind of a new window and where a dialog opens.
//!
//! The design resolves a window's open / close animation in this order:
//! an explicit behaviour, then an application-modal window (behaviour 7: grows in, fades out), then
//! a window showing a document, then the utility style (fades out), else none. Wayland carries one
//! of these signals, modality (xdg-dialog-v1 `set_modal`); the utility role, documents and alerts
//! have none, so the server passes `utility: false` and [`window_kind`] never picks
//! [`WindowKind::Document`] or [`WindowKind::Alert`]. A modal
//! window with a parent becomes a sheet when [`crate::FocusConfig::place_modal_children_as_sheets`]
//! is on; the model decides that when it maps the window.

use cupertino_effects::WindowKind;

use crate::geometry::{Rect, Size};

/// Offset of a modal panel opened while another modal panel of the application is up, from that
/// panel's top-left corner, in points ((66, -66) in a y-up space).
pub const MODAL_CASCADE: (f64, f64) = (66.0, 66.0);

/// Window kind for the protocol facts of a new window, in the design's precedence: modality
/// before the utility role.
pub fn window_kind(modal: bool, utility: bool) -> WindowKind {
    if modal {
        WindowKind::AppModal
    } else if utility {
        WindowKind::Utility
    } else {
        WindowKind::Plain
    }
}

/// Frame of a sheet of `size` on `parent`: centred horizontally and snapped to whole points,
/// its top at the bottom of the parent's title bar of `titlebar` points
/// (for a window without a toolbar).
pub fn sheet_frame(parent: Rect, size: Size, titlebar: f64) -> Rect {
    Rect::new(parent.x + (0.5 * (parent.w - size.w)).round(), parent.y + titlebar, size.w, size.h)
}

/// Frame of a modal panel of `size` without a parent, the design's window centring on the usable area:
/// centred horizontally, its top a quarter of the free height below the usable top, snapped to whole
/// points. With `previous` (the top-left corner of the application's modal panel
/// that is already up) it opens [`MODAL_CASCADE`] right of and below it instead, kept inside
/// `screen`.
pub fn modal_panel_frame(usable: Rect, screen: Rect, size: Size, previous: Option<(f64, f64)>) -> Rect {
    match previous {
        Some((x, y)) => {
            let x = (x + MODAL_CASCADE.0).min(screen.right() - size.w);
            let y = (y + MODAL_CASCADE.1).min(screen.bottom() - size.h);
            Rect::new(x, y, size.w, size.h)
        }
        None => {
            let (dx, dy) = (usable.w - size.w, usable.h - size.h);
            Rect::new((usable.x + 0.5 * dx).round(), (usable.y + 0.25 * dy).round(), size.w, size.h)
        }
    }
}

/// Frame of a child window that is not a sheet: centred on its parent and kept inside the usable
/// area by its top-left corner. approximate: the design's placement of non-sheet child windows
/// is not known (a panel centres on the screen, but a child window there is placed by
/// its application).
pub fn child_frame(parent: Rect, size: Size, usable: Rect) -> Rect {
    let x = (parent.cx() - size.w / 2.0).round().min(usable.right() - size.w).max(usable.x);
    let y = (parent.cy() - size.h / 2.0).round().min(usable.bottom() - size.h).max(usable.y);
    Rect::new(x, y, size.w, size.h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USABLE: Rect = Rect::new(0.0, 24.0, 1920.0, 1056.0);
    const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

    #[test]
    fn modality_wins_over_the_utility_role() {
        assert_eq!(window_kind(false, false), WindowKind::Plain);
        assert_eq!(window_kind(false, true), WindowKind::Utility);
        assert_eq!(window_kind(true, false), WindowKind::AppModal);
        assert_eq!(window_kind(true, true), WindowKind::AppModal);
    }

    #[test]
    fn a_sheet_hangs_centred_from_the_parent_title_bar() {
        let parent = Rect::new(100.0, 50.0, 801.0, 600.0);
        assert_eq!(sheet_frame(parent, Size::new(400.0, 200.0), 28.0), Rect::new(301.0, 78.0, 400.0, 200.0));
    }

    #[test]
    fn a_modal_panel_sits_a_quarter_of_the_free_height_below_the_top() {
        let f = modal_panel_frame(USABLE, SCREEN, Size::new(260.0, 155.0), None);
        // dx = 1660, dy = 901: x = 830, y = 24 + 225.25
        assert_eq!(f, Rect::new(830.0, 249.0, 260.0, 155.0));
    }

    #[test]
    fn a_second_modal_panel_cascades_and_stays_on_screen() {
        let size = Size::new(260.0, 155.0);
        assert_eq!(modal_panel_frame(USABLE, SCREEN, size, Some((830.0, 249.0))), Rect::new(896.0, 315.0, 260.0, 155.0));
        assert_eq!(modal_panel_frame(USABLE, SCREEN, size, Some((1700.0, 900.0))), Rect::new(1660.0, 925.0, 260.0, 155.0));
    }

    #[test]
    fn a_child_window_centres_on_its_parent_inside_the_usable_area() {
        let parent = Rect::new(200.0, 100.0, 800.0, 600.0);
        assert_eq!(child_frame(parent, Size::new(300.0, 200.0), USABLE), Rect::new(450.0, 300.0, 300.0, 200.0));
        let high = Rect::new(-300.0, 0.0, 400.0, 100.0);
        assert_eq!(child_frame(high, Size::new(300.0, 200.0), USABLE), Rect::new(0.0, 24.0, 300.0, 200.0));
    }
}
