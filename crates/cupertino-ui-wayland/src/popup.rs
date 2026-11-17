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
        offset: (0, 0),
        reactive: false,
        parent_size: None,
        grab,
    })
}
