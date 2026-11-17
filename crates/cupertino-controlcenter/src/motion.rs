//! The opening and closing of a detail view, with injected time (seconds on any monotonic clock).
//!
//! The tile grows into the open view on a critically damped spring and shrinks back the same way; the other tiles fade
//! out and in, and the tile's own content gives way to the view's. With Reduce Motion nothing moves: the view and the
//! grid cross-fade in place. A reversal starts from where the last motion was, so a quick open and close never jumps.

use crate::model::Detail;

/// Response of the spring, in seconds: the time of one undamped oscillation.
pub const RESPONSE: f64 = 0.35;
/// The spring counts as settled after this long; it is then within a fraction of a point of the end.
pub const SETTLE: f64 = 0.6;
/// Length of the cross-fade that replaces the motion with Reduce Motion, linear.
pub const FADE: f64 = 0.2;

/// How far a critically damped spring of [`RESPONSE`] released from 0 towards 1 has come after `t` seconds:
/// `1 - (1 + wt) e^(-wt)` with `w = 2 pi / RESPONSE`. It never overshoots.
pub fn spring(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= SETTLE {
        return 1.0;
    }
    let wt = std::f64::consts::TAU / RESPONSE * t;
    1.0 - (1.0 + wt) * (-wt).exp()
}

/// A detail view opening or closing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Expansion {
    pub detail: Detail,
    /// Towards the open view; false while it closes.
    pub opening: bool,
    /// When the motion started and the progress it started from.
    since: f64,
    from: f64,
    /// Cross-fade in place instead of moving.
    pub reduce_motion: bool,
}

impl Expansion {
    /// `detail` starts to open at `now` from the grid.
    pub fn open(detail: Detail, now: f64, reduce_motion: bool) -> Self {
        Self {
            detail,
            opening: true,
            since: now,
            from: 0.0,
            reduce_motion,
        }
    }

    /// Turns towards opening (`opening`) or closing at `now`, from the progress it has reached.
    pub fn turn(&mut self, opening: bool, now: f64) {
        if opening == self.opening {
            return;
        }
        self.from = self.progress(now);
        self.opening = opening;
        self.since = now;
    }

    /// How far along the motion is at `now`, 0 to 1.
    fn eased(&self, now: f64) -> f64 {
        let t = now - self.since;
        if self.reduce_motion {
            (t / FADE).clamp(0.0, 1.0)
        } else {
            spring(t)
        }
    }

    /// Progress at `now`, 0 (the grid) to 1 (the open view).
    pub fn progress(&self, now: f64) -> f64 {
        let eased = self.eased(now);
        if self.opening {
            self.from + (1.0 - self.from) * eased
        } else {
            self.from * (1.0 - eased)
        }
    }

    /// Whether the motion has ended at `now`.
    pub fn settled(&self, now: f64) -> bool {
        self.eased(now) >= 1.0
    }

    /// Whether the view has closed completely at `now`, so the grid shows alone.
    pub fn closed(&self, now: f64) -> bool {
        !self.opening && self.settled(now)
    }
}

/// How visible the parts of the panel are at `progress` of a detail's opening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Visibility {
    /// The tiles other than the one that opens.
    pub others: f64,
    /// The opening tile's own content.
    pub tile_content: f64,
    /// The open view's content.
    pub detail_content: f64,
}

/// Share of the motion over which the other tiles fade out, so they are gone before the growing tile covers them: the
/// glass lies under all of the window's content, so a tile cannot hide the content of a tile it passes over.
pub const OTHERS_FADE: f64 = 0.25;

/// The other tiles fade over the first [`OTHERS_FADE`] of the motion; the tile's content is gone by halfway and the
/// view's comes in after it.
pub fn visibility(progress: f64) -> Visibility {
    let p = progress.clamp(0.0, 1.0);
    Visibility {
        others: (1.0 - p / OTHERS_FADE).max(0.0),
        tile_content: (1.0 - 2.0 * p).max(0.0),
        detail_content: (2.0 * p - 1.0).max(0.0),
    }
}
