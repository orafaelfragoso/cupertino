//! The motion of the notification centre's list: sliding in from the right edge and out again as one piece, a cleared
//! card leaving, and the cards below it rising into the gap.
//!
//! Every motion is a catalogue motion that began at a time, from where it was and at the speed it had, so the view can
//! draw any frame from the model alone and a reversal (opening while it closes) goes on from where it is, without a
//! jump.

use cupertino_kit::motion::{Animated, SpringPreset, Timing};

/// How far the list is from its place while it is out of the output, in points: past the card, its margin and its
/// shadow.
pub const SLIDE_DISTANCE: f64 = 370.0;
/// The list is at rest within this distance of where it is going, in points, and slower than [`REST_SPEED`].
const REST_DISTANCE: f64 = 0.25;
/// The slowest the list moves while it is not yet at rest, in points per second.
const REST_SPEED: f64 = 4.0;

/// The share of its size a leaving card has lost when it is gone.
pub const LEAVE_SHRINK: f64 = 0.06;
/// The blur radius of a card that is gone, in points.
pub const LEAVE_BLUR: f64 = 10.0;

/// How the list slides in and out.
fn slide_timing() -> Timing {
    Timing::from(SpringPreset::NOTIFICATION_LIST_SLIDE)
}

/// How the cards below a cleared one rise into its gap.
fn rise_timing() -> Timing {
    Timing::from(SpringPreset::NOTIFICATION_RISE)
}

/// Where the list is between its place (0) and out of the output ([`SLIDE_DISTANCE`]).
#[derive(Clone, Debug)]
pub struct Slide(Animated<f64>);

impl Default for Slide {
    /// The list out of the output, at rest.
    fn default() -> Self {
        Slide(Animated::new(SLIDE_DISTANCE))
    }
}

impl Slide {
    /// Whether the list has come to rest by `now`.
    pub fn at_rest(&self, now: f64) -> bool {
        let target = self.0.target();
        !self.0.is_animating(now)
            || ((self.0.value(now) - target).abs() < REST_DISTANCE
                && self.0.velocity(now).abs() < REST_SPEED)
    }

    /// How far the list is from its place at `now`, in points: 0 in place, [`SLIDE_DISTANCE`] out.
    pub fn offset(&self, now: f64) -> f64 {
        if self.at_rest(now) {
            return self.0.target();
        }
        self.0.value(now).clamp(0.0, SLIDE_DISTANCE)
    }

    /// Whether the list is out of the output and at rest at `now`.
    pub fn is_out(&self, now: f64) -> bool {
        self.0.target() == SLIDE_DISTANCE && self.at_rest(now)
    }

    /// Go to `target` from where the list is at `now`, at the speed it has; at once when `instant` (Reduce Motion).
    pub fn go_to(&mut self, target: f64, now: f64, instant: bool) {
        if instant {
            self.0.set_immediately(target);
        } else if self.0.target() != target {
            self.0.animate_to(target, slide_timing(), now);
        }
    }
}

/// How far a card cleared at `start` has left at `now`: 0 as it begins, 1 when it is gone.
pub fn leave_progress(start: f64, now: f64) -> f64 {
    let timing = Timing::NOTIFICATION_CARD_LEAVE;
    timing.value((now - start).clamp(0.0, timing.duration()))
}

/// How long a cleared card takes to blur, shrink and fade away, in seconds.
pub fn leave_duration() -> f64 {
    Timing::NOTIFICATION_CARD_LEAVE.duration()
}

/// Cards cleared together, waiting for their leaving to end.
#[derive(Clone, Debug, PartialEq)]
pub struct Removal {
    pub ids: Vec<u32>,
    pub start: f64,
    /// Where the gap they leave begins in the list, and how tall it is.
    pub gap: Gap,
}

/// The room a card (or a group) takes in the list: from `top`, `height` tall, in the list's own points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap {
    pub top: f64,
    pub height: f64,
}

/// The cards below a gap, rising into it since `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rise {
    pub start: f64,
    pub gap: Gap,
}

impl Rise {
    /// How far below its place a card that lies from `y` down is at `now`, in points.
    pub fn shift(&self, y: f64, now: f64) -> f64 {
        if y < self.gap.top - 0.5 || self.done(now) {
            return 0.0;
        }
        (self.gap.height * (1.0 - rise_timing().value(now - self.start))).max(0.0)
    }

    /// Whether the cards have settled by `now`.
    pub fn done(&self, now: f64) -> bool {
        (now - self.start).partial_cmp(&rise_timing().duration()) != Some(std::cmp::Ordering::Less)
    }
}
