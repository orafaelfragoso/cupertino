//! The design's drag-reorder rules around the model ([`crate::dock::Dock::begin_drag`]): the press
//! that turns into a drag, the delays of the menu and the removal, and the puff shown where a
//! dragged tile was removed. The motions of the gap, the drop and the removal are the catalogue's
//! ([`Timing::DOCK_TILE_SLIDE`], [`Timing::DOCK_TILE_SETTLE`], [`Timing::DOCK_REMOVE_LOOK`],
//! [`Timing::DOCK_REMOVE_FADE`]).

use cupertino_kit::motion::Timing;
use cupertino_wm::geometry::Point;

use crate::tiles::TileId;

/// A press that moves more than this on either axis from where it began is a drag
/// (`5.0 < |dx|` or `5.0 < |dy| in the tile click handler ).
pub const DRAG_THRESHOLD: f64 = 5.0;

/// How long a press that stays in place waits before the tile's menu opens, in seconds
/// (the `popup_delay` preference, 0.5 when unset or not positive).
pub const HOLD_MENU_DELAY: f64 = 0.5;

/// A drag cannot remove its tile before this many seconds have passed (the
/// one-shot timer of 500000000 ns that waits for).
pub const REMOVE_DELAY_S: f64 = 0.5;

/// Opacity of the dragged icon while a drop would remove it, which it fades to and from as
/// [`Timing::DOCK_REMOVE_LOOK`] runs.
pub const REMOVING_OPACITY: f64 = 0.35;

/// A left press on a tile that may become a drag. The press stays a click until the pointer is
/// further than [`DRAG_THRESHOLD`] from where it began, on either axis; from then on it is a drag
/// for good, even if the pointer comes back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TilePress {
    pub id: TileId,
    pub origin: Point,
    started: f64,
    dragging: bool,
    held: bool,
}

impl TilePress {
    pub fn new(id: TileId, origin: Point, now: f64) -> Self {
        Self {
            id,
            origin,
            started: now,
            dragging: false,
            held: false,
        }
    }

    /// Seconds until a press that stays in place opens the tile's menu; `None` once it became a
    /// drag or the menu opened.
    pub fn hold_due(&self, now: f64) -> Option<f64> {
        (!self.dragging && !self.held).then(|| (self.started + HOLD_MENU_DELAY - now).max(0.0))
    }

    /// True exactly once: when the press has been held in place for [`HOLD_MENU_DELAY`].
    pub fn held(&mut self, now: f64) -> bool {
        let due = self.hold_due(now) == Some(0.0);
        self.held |= due;
        due
    }

    /// The pointer moved to `p`. True exactly once: when the press turns into a drag.
    pub fn moved(&mut self, p: Point) -> bool {
        if self.dragging
            || ((p.x - self.origin.x).abs() <= DRAG_THRESHOLD
                && (p.y - self.origin.y).abs() <= DRAG_THRESHOLD)
        {
            return false;
        }
        self.dragging = true;
        true
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }
}

/// The puff where a removed tile vanished, from `started` for as long as the icon's fade
/// ([`Timing::DOCK_REMOVE_FADE`]). The puff itself is not drawn by the Dock process (no sprite or
/// timing in it) and its look is approximate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Poof {
    pub centre: Point,
    pub started: f64,
}

impl Poof {
    /// How far the puff is, 0 at its start to 1 at its end; `None` once it is over.
    pub fn progress(&self, now: f64) -> Option<f64> {
        let p = (now - self.started) / Timing::DOCK_REMOVE_FADE.duration();
        (0.0..1.0).contains(&p).then_some(p)
    }
}
