//! Easing curves and the progress animator shared by all effects.

use cupertino_animation::{CubicBezier, Spring, SpringParams};

/// Easing of a normalised progress value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ease {
    Linear,
    /// `0.5 − 0.5·cos(π·x): as the easing of both warp-minimize phases and the funnel
    /// minimize progress, of the dock's animated properties
    /// and, as `sin²(π·x/2), of the window transforms and frame changes.
    Cosine,
    Bezier(CubicBezier),
}

impl Ease {
    pub fn apply(self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Ease::Linear => x,
            Ease::Cosine => cosine(x),
            Ease::Bezier(b) => b.eval(x),
        }
    }
}

/// `0.5 − 0.5·cos(π·x)` for `x ∈ [0, 1]`.
pub fn cosine(x: f64) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * x).cos()
}

/// How a transition advances.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Driver {
    Timed { duration: f64, ease: Ease },
    Spring(SpringParams),
}

impl Driver {
    pub fn spring_perceptual(duration: f64, bounce: f64) -> Self {
        Driver::Spring(SpringParams::from_perceptual(duration, bounce))
    }

    /// Critically damped spring parameterised by response (period of the undamped oscillation) and
    /// damping ratio.
    pub fn spring_response(response: f64, damping_ratio: f64) -> Self {
        Driver::Spring(SpringParams::from_ratio(response, damping_ratio))
    }

    /// Seconds until the driver is within `1e-3` of its target for good.
    pub fn settle_time(&self) -> f64 {
        match *self {
            Driver::Timed { duration, .. } => duration.max(0.0),
            Driver::Spring(p) => {
                let s = Spring::new(p);
                let mut last_outside = 0.0;
                let mut t = 0.0;
                while t < 20.0 {
                    if (1.0 - s.value(t)).abs() >= 1e-3 {
                        last_outside = t;
                    }
                    t += 0.002;
                }
                last_outside + 0.002
            }
        }
    }
}

/// Progress in `[0, 1]` (springs may overshoot slightly when underdamped) that can run forward or
/// backward and be redirected at any time: the new run starts from the current value and, for
/// springs, keeps the current velocity.
#[derive(Clone, Copy, Debug)]
pub struct ProgressAnimator {
    driver: Driver,
    from: f64,
    to: f64,
    start: f64,
    duration: f64,
    spring: Option<Spring>,
}

impl ProgressAnimator {
    pub fn new(driver: Driver, initial: f64) -> Self {
        Self {
            driver,
            from: initial,
            to: initial,
            start: 0.0,
            duration: 0.0,
            spring: None,
        }
    }

    pub fn driver(&self) -> Driver {
        self.driver
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    pub fn value(&self, now: f64) -> f64 {
        let t = now - self.start;
        match (self.driver, self.spring) {
            (_, _) if self.duration <= 0.0 => self.to,
            (Driver::Timed { ease, .. }, _) => {
                self.from + (self.to - self.from) * ease.apply(t / self.duration)
            }
            (Driver::Spring(_), Some(s)) => self.from + (self.to - self.from) * s.value(t),
            (Driver::Spring(_), None) => self.to,
        }
    }

    /// Rate of change in progress units per second.
    pub fn velocity(&self, now: f64) -> f64 {
        let t = now - self.start;
        if self.duration <= 0.0 || t < 0.0 {
            return 0.0;
        }
        match (self.driver, self.spring) {
            (Driver::Spring(_), Some(s)) => (self.to - self.from) * s.velocity(t),
            _ => {
                let h = 1e-5;
                (self.value(now + h) - self.value(now)) / h
            }
        }
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.duration > 0.0 && now < self.start + self.duration
    }

    /// Run towards `to` from the current value. Timed drivers scale their duration by the fraction
    /// of the full range still to travel, so reversing halfway takes half the time (approximate).
    pub fn animate_to(&mut self, to: f64, now: f64) {
        let from = self.value(now);
        let vel = self.velocity(now);
        let dist = (to - from).abs();
        self.from = from;
        self.to = to;
        self.start = now;
        match self.driver {
            Driver::Timed { duration, .. } => {
                self.duration = duration * dist.min(1.0);
                self.spring = None;
            }
            Driver::Spring(mut p) => {
                if dist < 1e-9 {
                    self.duration = 0.0;
                    self.spring = None;
                } else {
                    p.initial_velocity = vel / (to - from);
                    self.spring = Some(Spring::new(p));
                    self.duration = Driver::Spring(p).settle_time();
                }
            }
        }
    }

    pub fn set(&mut self, value: f64) {
        self.from = value;
        self.to = value;
        self.duration = 0.0;
        self.spring = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_is_symmetric_and_monotonic() {
        assert_eq!(cosine(0.0), 0.0);
        assert_eq!(cosine(1.0), 1.0);
        assert!((cosine(0.3) + cosine(0.7) - 1.0).abs() < 1e-12);
        let mut prev = -1.0;
        for k in 0..=100 {
            let v = Ease::Cosine.apply(k as f64 / 100.0);
            assert!(v >= prev);
            prev = v;
        }
    }

    #[test]
    fn timed_run_and_reversal_are_continuous() {
        let mut a = ProgressAnimator::new(
            Driver::Timed {
                duration: 0.5,
                ease: Ease::Cosine,
            },
            0.0,
        );
        a.animate_to(1.0, 10.0);
        assert!(a.is_animating(10.1));
        let mid = a.value(10.2);
        assert!(mid > 0.0 && mid < 1.0);
        a.animate_to(0.0, 10.2);
        assert!((a.value(10.2) - mid).abs() < 1e-12);
        assert!(a.value(10.3) < mid);
        assert_eq!(a.value(11.0), 0.0);
        assert!(!a.is_animating(11.0));
    }

    #[test]
    fn reversal_takes_proportional_time() {
        let mut a = ProgressAnimator::new(
            Driver::Timed {
                duration: 1.0,
                ease: Ease::Linear,
            },
            0.0,
        );
        a.animate_to(1.0, 0.0);
        a.animate_to(0.0, 0.25);
        assert!(a.is_animating(0.45));
        assert!(!a.is_animating(0.5 + 1e-9));
    }

    #[test]
    fn spring_keeps_velocity_when_retargeted() {
        let mut a = ProgressAnimator::new(Driver::spring_response(0.3, 1.0), 0.0);
        a.animate_to(1.0, 0.0);
        let (p, v) = (a.value(0.1), a.velocity(0.1));
        assert!(v > 0.0);
        a.animate_to(0.0, 0.1);
        assert!((a.value(0.1) - p).abs() < 1e-9);
        assert!(
            (a.velocity(0.1) - v).abs() < 0.02 * v.abs() + 1e-6,
            "{} vs {}",
            a.velocity(0.1),
            v
        );
        assert!(a.value(5.0).abs() < 1e-3);
    }

    #[test]
    fn critical_spring_does_not_overshoot() {
        let mut a = ProgressAnimator::new(Driver::spring_response(0.3, 1.0), 0.0);
        a.animate_to(1.0, 0.0);
        let mut prev = 0.0;
        for k in 0..400 {
            let v = a.value(k as f64 * 0.005);
            assert!(v >= prev - 1e-12 && v <= 1.0 + 1e-9);
            prev = v;
        }
    }

    #[test]
    fn settle_time_of_the_show_desktop_spring() {
        let t = Driver::spring_response(0.3, 1.0).settle_time();
        assert!(t > 0.3 && t < 1.0, "{t}");
    }
}
