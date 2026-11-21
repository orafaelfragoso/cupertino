//! Cosine ease of the dock's float animations.
//!
//! every animated dock property is either linear or eased with
//! `0.5 − 0.5·cos(π·t/duration); the magnification amount and the auto-hide offset use the cosine
//! ease. An interrupted animation restarts from the current value towards the new target.

/// `0.5 − 0.5·cos(π·x)` for `x` clamped to `[0, 1].
pub fn cosine_ease(x: f64) -> f64 {
    0.5 - 0.5 * (std::f64::consts::PI * x.clamp(0.0, 1.0)).cos()
}

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
