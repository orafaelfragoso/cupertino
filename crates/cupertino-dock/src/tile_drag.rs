//! The design's drag-reorder rules around the model ([`crate::Dock::begin_drag`]): the press
//! that turns into a drag, the timings of the gap, the drop and the removal, and the puff shown
//! where a dragged tile was removed.

use cupertino_animation::CubicBezier;
use cupertino_wm::geometry::Point;

use crate::tiles::TileId;

/// A press that moves more than this on either axis from where it began is a drag
/// (`5.0 < |dx|` or `5.0 < |dy| in the tile click handler ).
pub const DRAG_THRESHOLD: f64 = 5.0;

/// How long a press that stays in place waits before the tile's menu opens, in seconds
/// (the `popup_delay` preference, 0.5 when unset or not positive).
pub const HOLD_MENU_DELAY: f64 = 0.5;

/// Seconds a neighbour takes to slide to its new place when the gap moves
/// (270 ms, cosine ease).
pub const SLIDE_S: f64 = 0.270;

/// Seconds a dropped tile takes to settle into its slot (350 ms,
/// cosine ease).
pub const SETTLE_S: f64 = 0.350;

/// A drag cannot remove its tile before this many seconds have passed (the
/// one-shot timer of 500000000 ns that waits for).
pub const REMOVE_DELAY_S: f64 = 0.5;

/// Opacity of the dragged icon while a drop would remove it.
pub const REMOVING_OPACITY: f64 = 0.35;

/// Seconds the icon fades to and from [`REMOVING_OPACITY`] and the "Remove" label fades in and out,
/// on the ease-in-ease-out timing function (the function is as designed; the
/// duration is the layer framework's default transaction duration, 0.25 s).
pub const LOOK_S: f64 = 0.25;

/// The fade of a removed tile's icon and label from opacity 1 to 0: this
/// timing function (..24) over the default transaction duration at animation
/// speed 0.75, `0.25 / 0.75` seconds (derived from the default duration).
pub const REMOVE_FADE_CURVE: CubicBezier = CubicBezier::new(0.22363, 0.002, 0.5534, 0.9529);
pub const REMOVE_FADE_S: f64 = 0.25 / 0.75;

/// Duration of the removal puff in seconds: that of the icon's fade. The puff itself is not drawn
/// by the Dock process (no sprite or timing in it) and its look is approximate.
pub const POOF_DURATION: f64 = REMOVE_FADE_S;

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
        Self { id, origin, started: now, dragging: false, held: false }
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
        if self.dragging || ((p.x - self.origin.x).abs() <= DRAG_THRESHOLD && (p.y - self.origin.y).abs() <= DRAG_THRESHOLD) {
            return false;
        }
        self.dragging = true;
        true
    }

    pub fn is_dragging(&self) -> bool {
        self.dragging
    }
}

/// The puff where a removed tile vanished, from `started` for [`POOF_DURATION`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Poof {
    pub centre: Point,
    pub started: f64,
}

impl Poof {
    /// How far the puff is, 0 at its start to 1 at its end; `None` once it is over.
    pub fn progress(&self, now: f64) -> Option<f64> {
        let p = (now - self.started) / POOF_DURATION;
        (0.0..1.0).contains(&p).then_some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_becomes_a_drag_once_past_the_threshold_on_either_axis_and_stays_one() {
        let mut press = TilePress::new(TileId(3), Point::new(100.0, 40.0), 0.0);
        assert!(!press.moved(Point::new(100.0 + DRAG_THRESHOLD, 40.0 - DRAG_THRESHOLD)), "the threshold itself is still a click");
        assert!(!press.is_dragging());
        assert!(press.moved(Point::new(100.0, 40.0 + DRAG_THRESHOLD + 0.5)));
        assert!(press.is_dragging());
        assert!(!press.moved(Point::new(300.0, 40.0)), "the drag begins once");
        assert!(!press.moved(Point::new(100.0, 40.0)) && press.is_dragging(), "coming back does not undo it");
    }

    #[test]
    fn a_press_held_in_place_opens_the_menu_once_and_never_after_a_drag() {
        let mut press = TilePress::new(TileId(3), Point::new(100.0, 40.0), 10.0);
        assert_eq!(press.hold_due(10.0), Some(HOLD_MENU_DELAY));
        assert!(!press.held(10.0 + HOLD_MENU_DELAY - 0.01));
        assert!(press.held(10.0 + HOLD_MENU_DELAY));
        assert!(!press.held(11.0), "once");
        assert_eq!(press.hold_due(11.0), None);
        let mut dragged = TilePress::new(TileId(3), Point::new(100.0, 40.0), 10.0);
        assert!(dragged.moved(Point::new(110.0, 40.0)));
        assert!(!dragged.held(11.0), "a press that began to drag has no menu to open");
    }

    #[test]
    fn the_puff_runs_for_its_duration_and_then_is_over() {
        let poof = Poof { centre: Point::new(10.0, 10.0), started: 2.0 };
        assert_eq!(poof.progress(2.0), Some(0.0));
        assert!((poof.progress(2.0 + POOF_DURATION / 2.0).unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(poof.progress(2.0 + POOF_DURATION), None);
        assert_eq!(poof.progress(1.0), None, "before it started there is nothing");
    }
}
