//! Where an `xdg_popup` goes.

use crate::Error;
use cupertino_geometry::Rect;
use cupertino_wayland_client::{Anchor, ConstraintAdjustment, Gravity, PopupPlacement};

/// A popup covering `rect` (parent-local points) rounded outwards to whole points, its top-left corner at the
/// rectangle's whatever the parent's size; with `grab` it takes the seat's grab. Errors on an empty or non-finite
/// rectangle, which no popup can have.
pub fn placement(rect: &Rect, grab: bool) -> Result<PopupPlacement, Error> {
    let finite = [rect.x(), rect.y(), rect.width(), rect.height()]
        .iter()
        .all(|v| v.is_finite());
    let whole = rect.round_out();
    if !finite || whole.width() <= 0.0 || whole.height() <= 0.0 {
        return Err(Error::InvalidShape);
    }
    Ok(PopupPlacement {
        anchor_rect: whole,
        size: (whole.width() as u32, whole.height() as u32),
        anchor: Anchor::TopLeft,
        gravity: Gravity::BottomRight,
        constraint_adjustment: ConstraintAdjustment::empty(),
        grab,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_popup_covers_the_rect_in_whole_points() {
        let p = placement(&Rect::new(10.4, 24.0, 153.2, 88.5), true).unwrap();
        assert_eq!(p.anchor_rect, Rect::new(10.0, 24.0, 154.0, 89.0));
        assert_eq!(p.size, (154, 89));
        assert_eq!(
            (p.anchor, p.gravity),
            (Anchor::TopLeft, Gravity::BottomRight)
        );
    }

    #[test]
    fn empty_or_non_finite_rects_are_refused() {
        for rect in [
            Rect::new(0.0, 0.0, 0.0, 10.0),
            Rect::new(f64::NAN, 0.0, 10.0, 10.0),
            Rect::new(0.0, 0.0, f64::INFINITY, 10.0),
        ] {
            assert!(matches!(placement(&rect, true), Err(Error::InvalidShape)));
        }
    }
}
