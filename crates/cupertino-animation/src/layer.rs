//! One property of a compositor layer, animated on the compositor's clock.
//!
//! A property rests at a value or moves towards a target along a timed curve or a spring. A new animation retargets
//! from the value and the velocity shown at its start, so the motion stays continuous in both: a spring takes the
//! velocity as its initial velocity, and a timed curve adds a correction that starts at the velocity's difference from
//! the curve's own and fades out by the curve's end (a cubic `c(t) = Δv · t · (1 − t/D)²`, zero in value at both ends
//! and in slope at the end). Time is an absolute timestamp in seconds, so the same motion is sampled alike at any rate.

use crate::spring::{Spring, SpringParams};
use crate::timing::CubicBezier;

/// The curve of one layer animation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayerCurve {
    /// A cubic Bézier timing curve over `duration` seconds.
    Timed { duration: f64, bezier: CubicBezier },
    /// A damped spring: `response` is the period of the undamped oscillation in seconds, `damping` the damping ratio
    /// (1 critically damped, below 1 bouncing).
    Spring { response: f64, damping: f64 },
}

/// Distance below which a spring counts as at its target, in the property's units.
const SPRING_REST: f64 = 1e-3;

#[derive(Clone, Copy, Debug)]
enum Motion {
    Rest,
    Timed {
        from: f64,
        to: f64,
        start: f64,
        duration: f64,
        bezier: CubicBezier,
        /// The velocity shown at the start minus the curve's own initial velocity, faded out over the curve.
        dv: f64,
    },
    Spring {
        from: f64,
        to: f64,
        start: f64,
        /// The unit step response (no initial velocity) and the unit impulse response share these parameters.
        step: Spring,
        impulse: Spring,
        v0: f64,
        settle: f64,
    },
}

/// A property of a layer: its value at any time, its target and its velocity.
#[derive(Clone, Copy, Debug)]
pub struct LayerProperty {
    motion: Motion,
    target: f64,
}

impl LayerProperty {
    /// A property resting at `value`.
    pub fn new(value: f64) -> Self {
        Self {
            motion: Motion::Rest,
            target: value,
        }
    }

    /// The value the property moves towards, or rests at.
    pub fn target(&self) -> f64 {
        self.target
    }

    /// Whether an animation runs at `now`; one that has not started yet counts as running.
    pub fn is_animating(&self, now: f64) -> bool {
        self.end().is_some_and(|end| now < end)
    }

    /// When the running animation ends, `None` at rest.
    pub fn end(&self) -> Option<f64> {
        match self.motion {
            Motion::Rest => None,
            Motion::Timed {
                start, duration, ..
            } => Some(start + duration.max(0.0)),
            Motion::Spring { start, settle, .. } => Some(start + settle),
        }
    }

    /// The value shown at `now`.
    pub fn value(&self, now: f64) -> f64 {
        match self.motion {
            Motion::Rest => self.target,
            Motion::Timed {
                from,
                to,
                start,
                duration,
                bezier,
                dv,
            } => {
                if duration <= 0.0 || now >= start + duration {
                    return to;
                }
                let t = (now - start).max(0.0);
                let x = t / duration;
                from + (to - from) * bezier.eval(x) + dv * t * (1.0 - x) * (1.0 - x)
            }
            Motion::Spring {
                from,
                to,
                start,
                step,
                impulse,
                v0,
                settle,
            } => {
                let t = now - start;
                if t >= settle {
                    return to;
                }
                from + (to - from) * step.value(t) + v0 * (impulse.value(t) - step.value(t))
            }
        }
    }

    /// The velocity shown at `now`, in units per second.
    pub fn velocity(&self, now: f64) -> f64 {
        match self.motion {
            Motion::Rest => 0.0,
            Motion::Timed {
                start, duration, ..
            } => {
                if duration <= 0.0 || now >= start + duration || now < start {
                    return 0.0;
                }
                // the Bézier's slope by a central difference inside the curve
                let h = 1e-6;
                let a = (now - h).max(start);
                let b = (now + h).min(start + duration);
                (self.value(b) - self.value(a)) / (b - a)
            }
            Motion::Spring {
                from,
                to,
                start,
                step,
                impulse,
                v0,
                settle,
            } => {
                let t = now - start;
                if t >= settle || t < 0.0 {
                    return 0.0;
                }
                (to - from) * step.velocity(t) + v0 * (impulse.velocity(t) - step.velocity(t))
            }
        }
    }

    /// Jump to `value` and rest there.
    pub fn set(&mut self, value: f64) {
        self.motion = Motion::Rest;
        self.target = value;
    }

    /// Stop at the value shown at `now`.
    pub fn cancel(&mut self, now: f64) {
        let v = self.value(now);
        self.set(v);
    }

    /// Start moving to `to` along `curve` at `start`, from the value and velocity shown then. `velocity` is the
    /// initial velocity (units per second) used when the property is at rest; a running animation keeps its own. A
    /// timed curve from rest with no velocity (0) follows its own slope.
    pub fn animate(&mut self, to: f64, curve: LayerCurve, start: f64, velocity: f64) {
        let from = self.value(start);
        let moving = self.is_animating(start);
        let v = if moving {
            self.velocity(start)
        } else {
            velocity
        };
        self.target = to;
        self.motion = match curve {
            LayerCurve::Timed { duration, bezier } => {
                let own = if duration > 0.0 {
                    let h = 1e-6;
                    (to - from) * (bezier.eval(h) - bezier.eval(0.0)) / (h * duration)
                } else {
                    0.0
                };
                Motion::Timed {
                    from,
                    to,
                    start,
                    duration,
                    bezier,
                    dv: if duration > 0.0 && (moving || v != 0.0) {
                        v - own
                    } else {
                        0.0
                    },
                }
            }
            LayerCurve::Spring { response, damping } => {
                let params = SpringParams {
                    allows_overdamping: true,
                    ..SpringParams::from_ratio(response.max(1e-3), damping.max(1e-3))
                };
                let step = Spring::new(params);
                let impulse = Spring::new(SpringParams {
                    initial_velocity: 1.0,
                    ..params
                });
                Motion::Spring {
                    from,
                    to,
                    start,
                    step,
                    impulse,
                    v0: v,
                    settle: settle_time(from, to, v, &step, &impulse),
                }
            }
        };
    }
}

/// The time from which the spring stays within [`SPRING_REST`] of its target, sampled every millisecond up to a
/// minute: the step and impulse responses share an envelope, so after it the motion never comes back out.
fn settle_time(from: f64, to: f64, v0: f64, step: &Spring, impulse: &Spring) -> f64 {
    let at = |t: f64| from + (to - from) * step.value(t) + v0 * (impulse.value(t) - step.value(t));
    let envelope = (to - from).abs() + v0.abs();
    if envelope < SPRING_REST {
        return 0.0;
    }
    let bound = step
        .duration_for_epsilon(SPRING_REST / envelope)
        .max(impulse.duration_for_epsilon(SPRING_REST / envelope))
        .min(60.0);
    // walk back from the bound to the last time the motion was outside the rest distance
    let mut t = bound;
    while t > 0.0 && (at(t) - to).abs() < SPRING_REST {
        t -= 1e-3;
    }
    (t + 1e-3).min(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EASE: LayerCurve = LayerCurve::Timed {
        duration: 0.3,
        bezier: CubicBezier::EASE_IN_EASE_OUT,
    };
    const SPRING: LayerCurve = LayerCurve::Spring {
        response: 0.4,
        damping: 0.8,
    };

    fn continuous(p: &LayerProperty, before: f64, after: f64, at: f64) {
        assert!(
            (p.value(at) - before).abs() < 1e-6,
            "value {} vs {before}",
            p.value(at)
        );
        assert!(
            (p.velocity(at) - after).abs() < 1e-2 * after.abs().max(1.0),
            "velocity {} vs {after}",
            p.velocity(at)
        );
    }

    #[test]
    fn a_resting_property_jumps_and_a_timed_one_ends_at_its_target() {
        let mut p = LayerProperty::new(4.0);
        assert_eq!(p.value(10.0), 4.0);
        p.animate(10.0, EASE, 1.0, 0.0);
        assert_eq!(p.value(1.0), 4.0);
        assert!(p.is_animating(1.2));
        assert!(
            (p.value(1.15) - 7.0).abs() < 1e-4,
            "half way at half time: {}",
            p.value(1.15)
        );
        assert_eq!(p.value(1.3), 10.0);
        assert!(!p.is_animating(1.3));
        assert_eq!(p.target(), 10.0);
    }

    #[test]
    fn retargeting_keeps_value_and_velocity_for_every_pair_of_curves() {
        for (first, second) in [
            (EASE, EASE),
            (EASE, SPRING),
            (SPRING, EASE),
            (SPRING, SPRING),
        ] {
            let mut p = LayerProperty::new(0.0);
            p.animate(100.0, first, 0.0, 0.0);
            let at = 0.1;
            let (v, dv) = (p.value(at), p.velocity(at));
            assert!(dv.abs() > 10.0, "it moves at the retarget");
            p.animate(-50.0, second, at, 0.0);
            continuous(&p, v, dv, at);
            assert_eq!(p.target(), -50.0);
            let end = p.end().unwrap();
            assert!((p.value(end) + 50.0).abs() < 1e-2);
        }
    }

    #[test]
    fn a_spring_takes_the_given_velocity_from_rest_and_settles() {
        let mut p = LayerProperty::new(0.0);
        p.animate(0.0, SPRING, 2.0, 300.0);
        assert!((p.velocity(2.0) - 300.0).abs() < 1.0);
        assert!(p.value(2.05) > 5.0, "it moves away first");
        let end = p.end().unwrap();
        assert!(end > 2.0 && end < 5.0);
        assert!(p.value(end + 0.01).abs() < 1e-3);
    }

    #[test]
    fn a_timed_correction_fades_out_by_the_end() {
        let mut p = LayerProperty::new(0.0);
        p.animate(0.0, EASE, 0.0, 500.0);
        assert!(p.value(0.05) > 0.0);
        assert_eq!(p.value(0.3), 0.0);
        assert!(p.velocity(0.299).abs() < 5.0);
    }

    #[test]
    fn a_timed_curve_from_rest_follows_its_own_slope() {
        let mut p = LayerProperty::new(0.0);
        let linear = LayerCurve::Timed {
            duration: 1.0,
            bezier: CubicBezier::LINEAR,
        };
        p.animate(100.0, linear, 0.0, 0.0);
        assert!((p.value(0.25) - 25.0).abs() < 1e-9);
        assert!((p.velocity(0.5) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn cancel_freezes_at_the_shown_value() {
        let mut p = LayerProperty::new(0.0);
        p.animate(10.0, EASE, 0.0, 0.0);
        let v = p.value(0.15);
        p.cancel(0.15);
        assert_eq!(p.value(5.0), v);
        assert!(!p.is_animating(0.15));
    }

    #[test]
    fn a_zero_duration_ends_at_once() {
        let mut p = LayerProperty::new(1.0);
        p.animate(
            2.0,
            LayerCurve::Timed {
                duration: 0.0,
                bezier: CubicBezier::LINEAR,
            },
            3.0,
            0.0,
        );
        assert_eq!(p.value(3.0), 2.0);
        assert!(!p.is_animating(3.0));
    }
}
