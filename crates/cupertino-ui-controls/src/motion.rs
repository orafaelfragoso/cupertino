//! The transitions a control paints by itself: a value that moves from where it was shown to where it is going over a
//! duration, timed from the first paint after it was told to move. While it moves, its paint asks for the next frame,
//! so the window draws each step at the display's pace and stops asking once it has arrived.
//!
//! The model is pure ([`Tween::value_at`]); [`Tween::sample`] reads the paint's clock. With Reduce Motion a tween is
//! always where it is going.

use std::cell::Cell;

use cupertino_ui_core::PaintCx;
use cupertino_ui_platform::Timestamp;

/// How a tween's progress runs from 0 to 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Curve {
    /// Quick at first and slowing into its end: a thumb or a fill that is let go.
    #[default]
    EaseOut,
    /// Slow at both ends: a thumb carried from one end of its track to the other.
    EaseInOut,
}

impl Curve {
    fn at(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Curve::EaseOut => 1.0 - (1.0 - t).powi(3),
            Curve::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// A value moving from one place to another. Opaque.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Tween {
    from: f64,
    to: f64,
    /// How long it waits before it moves, and how long it moves, in nanoseconds.
    delay: u64,
    duration: u64,
    curve: Curve,
    /// When it was told to move: the first paint after that.
    start: Cell<Option<Timestamp>>,
    /// Where the last paint showed it.
    shown: Cell<f64>,
}

impl Tween {
    /// A tween resting at `value`.
    pub fn at(value: f64) -> Tween {
        Tween {
            from: value,
            to: value,
            shown: Cell::new(value),
            ..Tween::default()
        }
    }

    /// Where it is going.
    pub fn target(&self) -> f64 {
        self.to
    }

    /// Moves from where it was last shown to `to` over `duration` seconds along `curve`, after waiting `delay` seconds;
    /// a tween already going to `to` goes on as it was.
    pub fn go(&mut self, to: f64, duration: f64, delay: f64, curve: Curve) {
        if to == self.to {
            return;
        }
        self.from = self.shown.get();
        self.to = to;
        self.duration = nanos(duration);
        self.delay = nanos(delay);
        self.curve = curve;
        self.start.set(None);
    }

    /// Stands at `value` at once.
    pub fn jump(&mut self, value: f64) {
        *self = Tween::at(value);
    }

    /// Where it is `elapsed` nanoseconds after it started moving, and whether it is still on its way.
    pub fn value_at(&self, elapsed: u64) -> (f64, bool) {
        let moving = elapsed.saturating_sub(self.delay);
        if self.from == self.to || self.duration == 0 || moving >= self.duration {
            return (self.to, false);
        }
        let t = moving as f64 / self.duration as f64;
        (self.from + (self.to - self.from) * self.curve.at(t), true)
    }

    /// Where it is at the paint's time, asking for the next frame while it moves; under Reduce Motion, where it is
    /// going.
    pub fn sample(&self, cx: &mut PaintCx<'_>) -> f64 {
        if cx.env().theme.motion.reduce {
            self.shown.set(self.to);
            return self.to;
        }
        let now = cx.now();
        let start = self.start.get().unwrap_or_else(|| {
            self.start.set(Some(now));
            now
        });
        let (value, moving) = self.value_at(now.0.saturating_sub(start.0));
        if moving {
            cx.request_animation_frame();
        }
        self.shown.set(value);
        value
    }
}

fn nanos(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1e9) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn a_tween_starts_where_it_was_shown_runs_over_its_duration_and_settles() {
        let mut t = Tween::at(0.0);
        assert_eq!(t.value_at(0), (0.0, false), "at rest it stays");
        t.go(1.0, 0.2, 0.0, Curve::EaseOut);
        assert_eq!(t.value_at(0), (0.0, true), "it starts where it was");
        let (mid, moving) = t.value_at(100 * MS);
        assert!(
            moving && mid > 0.5 && mid < 1.0,
            "past halfway at half time: {mid}"
        );
        let mut last = 0.0;
        for ms in (0..200).step_by(10) {
            let (v, moving) = t.value_at(ms * MS);
            assert!(moving && v >= last, "forward only");
            last = v;
        }
        assert_eq!(
            t.value_at(200 * MS),
            (1.0, false),
            "settled at its duration"
        );
        assert_eq!(t.value_at(10_000 * MS), (1.0, false));
    }

    #[test]
    fn a_delayed_tween_waits_then_moves() {
        let mut t = Tween::at(1.0);
        t.go(0.0, 0.2, 0.2, Curve::EaseInOut);
        assert_eq!(t.value_at(150 * MS), (1.0, true), "still waiting");
        let (v, moving) = t.value_at(300 * MS);
        assert!(
            moving && (v - 0.5).abs() < 1e-9,
            "halfway, slow at both ends: {v}"
        );
        assert_eq!(t.value_at(400 * MS), (0.0, false));
    }

    #[test]
    fn retargeting_starts_from_where_it_was_shown_and_the_same_target_goes_on() {
        let mut t = Tween::at(0.0);
        t.go(1.0, 0.2, 0.0, Curve::EaseOut);
        t.shown.set(0.4);
        t.start.set(Some(Timestamp(5)));
        t.go(1.0, 0.2, 0.0, Curve::EaseOut);
        assert_eq!(t.start.get(), Some(Timestamp(5)), "going on as it was");
        t.go(0.0, 0.2, 0.0, Curve::EaseOut);
        assert_eq!(t.value_at(0), (0.4, true), "back from where it was shown");
        assert_eq!(t.start.get(), None, "timed from the next paint");
        t.jump(0.7);
        assert_eq!(t.value_at(0), (0.7, false));
    }
}
