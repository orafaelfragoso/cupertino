//! Cosine ease of the dock's float animations.
//!
//! every animated dock property is either linear or eased with
//! `0.5 − 0.5·cos(π·t/duration); the magnification amount and the auto-hide offset use the cosine
//! ease. An interrupted animation restarts from the current value towards the new target.

use cupertino_animation::CubicBezier;

/// `0.5 − 0.5·cos(π·x)` for `x` clamped to `[0, 1].
pub fn cosine_ease(x: f64) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * x.clamp(0.0, 1.0)).cos()
}

/// Cubic Bézier within 3·10⁻⁴ of [`cosine_ease`] over the whole range (approximate fit, used where
/// a [`CubicBezier`] is required by the animation layer).
pub const COSINE_EASE_BEZIER: CubicBezier = CubicBezier::new(0.364, 0.0, 0.636, 1.0);

/// A scalar that eases from its current value to a target with [`cosine_ease`].
#[derive(Clone, Copy, Debug)]
pub struct EasedValue {
    from: f64,
    to: f64,
    start: f64,
    duration: f64,
}

impl EasedValue {
    pub fn new(value: f64) -> Self {
        Self {
            from: value,
            to: value,
            start: 0.0,
            duration: 0.0,
        }
    }

    pub fn value(&self, now: f64) -> f64 {
        if self.duration <= 0.0 {
            return self.to;
        }
        let x = ((now - self.start) / self.duration).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * cosine_ease(x)
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.duration > 0.0 && now < self.start + self.duration
    }

    pub fn target(&self) -> f64 {
        self.to
    }

    /// Start (or restart) towards `to` over `duration` seconds from the current value.
    pub fn animate_to(&mut self, to: f64, duration: f64, now: f64) {
        self.from = self.value(now);
        self.to = to;
        self.start = now;
        self.duration = duration.max(0.0);
    }

    pub fn set_immediately(&mut self, v: f64) {
        *self = Self::new(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_ease_properties() {
        assert_eq!(cosine_ease(0.0), 0.0);
        assert_eq!(cosine_ease(1.0), 1.0);
        assert!((cosine_ease(0.5) - 0.5).abs() < 1e-15);
        assert!(
            (cosine_ease(0.25) - (0.5 - 0.5 * (std::f64::consts::FRAC_PI_4).cos())).abs() < 1e-15
        );
        let mut prev = 0.0;
        for k in 1..=100 {
            let v = cosine_ease(k as f64 / 100.0);
            assert!(v > prev);
            prev = v;
        }
        assert_eq!(cosine_ease(-1.0), 0.0);
        assert_eq!(cosine_ease(2.0), 1.0);
    }

    #[test]
    fn bezier_fit_is_close() {
        let mut worst: f64 = 0.0;
        for k in 0..=200 {
            let x = k as f64 / 200.0;
            worst = worst.max((COSINE_EASE_BEZIER.eval(x) - cosine_ease(x)).abs());
        }
        assert!(worst < 5e-4, "worst {worst}");
    }

    #[test]
    fn restart_continues_from_current_value() {
        let mut v = EasedValue::new(0.0);
        v.animate_to(1.0, 1.0, 0.0);
        let mid = v.value(0.5);
        assert!((mid - 0.5).abs() < 1e-12);
        assert!(v.is_animating(0.5));
        v.animate_to(0.0, 0.2, 0.5);
        assert!((v.value(0.5) - mid).abs() < 1e-12);
        assert!(v.value(0.6) < mid);
        assert_eq!(v.value(0.7), 0.0);
        assert!(!v.is_animating(0.7));
    }
}
