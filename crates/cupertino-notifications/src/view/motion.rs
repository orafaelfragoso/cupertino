//! The motion of the notification centre's list: sliding in from the right edge and out again as one piece, a cleared
//! card leaving, and the cards below it rising into the gap.
//!
//! Every motion is a pure function of the time it began, where it began and how fast it was going, so the view can
//! draw any frame from the model alone and a reversal (opening while it closes) goes on from where it is, without a
//! jump.

/// The rate of the slide's critically damped spring, in radians per second: about 0.4 s to settle.
pub const SLIDE_RATE: f64 = 21.0;
/// How far the list is from its place while it is out of the output, in points: past the card, its margin and its
/// shadow.
pub const SLIDE_DISTANCE: f64 = 370.0;
/// The list is at rest within this distance of where it is going, in points, and slower than [`REST_SPEED`].
const REST_DISTANCE: f64 = 0.25;
/// The slowest the list moves while it is not yet at rest, in points per second.
const REST_SPEED: f64 = 4.0;
/// A spring is at rest this long after it began, whatever its start.
const SETTLED_AFTER: f64 = 2.0;

/// How long a cleared card takes to blur, shrink and fade away, in seconds.
pub const LEAVE_DURATION: f64 = 0.13;
/// The share of its size a leaving card has lost when it is gone.
pub const LEAVE_SHRINK: f64 = 0.06;
/// The blur radius of a card that is gone, in points.
pub const LEAVE_BLUR: f64 = 10.0;
/// The rate of the spring the cards below a cleared one rise with, in radians per second.
pub const RISE_RATE: f64 = 21.75;

/// A critically damped spring from `from` (moving at `velocity`) to `target`, `t` seconds after it began.
fn spring(from: f64, velocity: f64, target: f64, rate: f64, t: f64) -> (f64, f64) {
    if !(t < SETTLED_AFTER) {
        return (target, 0.0);
    }
    let away = from - target;
    let carry = velocity + rate * away;
    let decay = (-rate * t).exp();
    (
        target + (away + carry * t) * decay,
        (velocity - rate * carry * t) * decay,
    )
}

/// Where the list is between its place (0) and out of the output ([`SLIDE_DISTANCE`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slide {
    start: f64,
    from: f64,
    velocity: f64,
    target: f64,
}

impl Slide {
    /// The list out of the output, at rest.
    pub const OUT: Slide = Slide {
        start: f64::NEG_INFINITY,
        from: SLIDE_DISTANCE,
        velocity: 0.0,
        target: SLIDE_DISTANCE,
    };

    fn state(&self, now: f64) -> (f64, f64) {
        spring(
            self.from,
            self.velocity,
            self.target,
            SLIDE_RATE,
            now - self.start,
        )
    }

    /// Whether the list has come to rest by `now`.
    pub fn at_rest(&self, now: f64) -> bool {
        let (at, speed) = self.state(now);
        (at - self.target).abs() < REST_DISTANCE && speed.abs() < REST_SPEED
    }

    /// How far the list is from its place at `now`, in points: 0 in place, [`SLIDE_DISTANCE`] out.
    pub fn offset(&self, now: f64) -> f64 {
        if self.at_rest(now) {
            return self.target;
        }
        self.state(now).0.clamp(0.0, SLIDE_DISTANCE)
    }

    /// Whether the list is out of the output and at rest at `now`.
    pub fn is_out(&self, now: f64) -> bool {
        self.target == SLIDE_DISTANCE && self.at_rest(now)
    }

    /// Go to `target` from where the list is at `now`, at the speed it has; at once when `instant` (Reduce Motion).
    pub fn go_to(&mut self, target: f64, now: f64, instant: bool) {
        if instant {
            *self = Slide {
                start: f64::NEG_INFINITY,
                from: target,
                velocity: 0.0,
                target,
            };
        } else if self.target != target {
            let (at, velocity) = self.state(now);
            *self = Slide {
                start: now,
                from: at,
                velocity,
                target,
            };
        }
    }
}

/// How far a card cleared at `start` has left at `now`: 0 as it begins, 1 when it is gone.
pub fn leave_progress(start: f64, now: f64) -> f64 {
    let p = ((now - start) / LEAVE_DURATION).clamp(0.0, 1.0);
    // slow at first and quick at the end, as a card pulled away
    p * p * (3.0 - 2.0 * p)
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
        if y < self.gap.top - 0.5 {
            return 0.0;
        }
        let (at, _) = spring(self.gap.height, 0.0, 0.0, RISE_RATE, now - self.start);
        at.max(0.0)
    }

    /// Whether the cards have settled by `now`.
    pub fn done(&self, now: f64) -> bool {
        !(now - self.start < 1.0)
    }
}
