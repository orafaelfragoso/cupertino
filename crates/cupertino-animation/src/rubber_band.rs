//! Elastic overscroll ("rubber band") mapping.
//!
//! This module uses the commonly
//! documented elastic curve
//!
//! ```text
//! f(x) = d · (1 − 1 / (x·c/d + 1))
//! ```
//!
//! where `x` is the raw overscroll distance, `d` the viewport dimension along the scroll axis and
//! `c` a stiffness coefficient. The coefficient is configurable; the default 0.55 is approximate
//! and must be confirmed by recordings.
//!
//! Properties: f(0) = 0, f'(0) = c, f is monotonic and bounded by `d`, odd-symmetric around zero.

pub const DEFAULT_COEFFICIENT: f64 = 0.55;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubberBand {
    pub dimension: f64,
    pub coefficient: f64,
}

impl RubberBand {
    pub fn new(dimension: f64) -> Self {
        Self {
            dimension,
            coefficient: DEFAULT_COEFFICIENT,
        }
    }

    pub fn with_coefficient(dimension: f64, coefficient: f64) -> Self {
        Self {
            dimension,
            coefficient,
        }
    }

    /// Displayed overscroll for a raw overscroll `offset` (signed).
    pub fn map(&self, offset: f64) -> f64 {
        rubber_band(offset, self.dimension, self.coefficient)
    }

    /// Raw offset that produces displayed overscroll `shown` (inverse of [`Self::map`]).
    /// Returns ±infinity when |shown| ≥ dimension.
    pub fn inverse(&self, shown: f64) -> f64 {
        inverse(shown, self.dimension, self.coefficient)
    }

    /// d(shown)/d(raw) at raw offset `offset`.
    pub fn slope(&self, offset: f64) -> f64 {
        slope(offset, self.dimension, self.coefficient)
    }

    /// Position after applying the elastic mapping outside `[min, max]`; unchanged inside.
    pub fn apply(&self, position: f64, min: f64, max: f64) -> f64 {
        apply(position, min, max, self.dimension, self.coefficient)
    }
}

/// `d·(1 − 1/(|x|·c/d + 1))` with the sign of `offset`. Zero for a non-positive `dimension`.
pub fn rubber_band(offset: f64, dimension: f64, c: f64) -> f64 {
    if dimension <= 0.0 || offset == 0.0 {
        return 0.0;
    }
    let x = offset.abs();
    offset.signum() * (1.0 - 1.0 / (x * c / dimension + 1.0)) * dimension
}

/// Inverse of [`rubber_band`]: x = d·(1/(1 − y/d) − 1)/c.
pub fn inverse(shown: f64, dimension: f64, c: f64) -> f64 {
    if dimension <= 0.0 || c <= 0.0 || shown == 0.0 {
        return 0.0;
    }
    let y = shown.abs() / dimension;
    if y >= 1.0 {
        return shown.signum() * f64::INFINITY;
    }
    shown.signum() * dimension * (1.0 / (1.0 - y) - 1.0) / c
}

/// Derivative of [`rubber_band`] with respect to the raw offset: c / (|x|·c/d + 1)².
pub fn slope(offset: f64, dimension: f64, c: f64) -> f64 {
    if dimension <= 0.0 {
        return 0.0;
    }
    let u = offset.abs() * c / dimension + 1.0;
    c / (u * u)
}

/// Clamp-with-overscroll: inside `[min, max]` the position is unchanged, outside it the excess is
/// mapped through the elastic curve.
pub fn apply(position: f64, min: f64, max: f64, dimension: f64, c: f64) -> f64 {
    if position < min {
        min + rubber_band(position - min, dimension, c)
    } else if position > max {
        max + rubber_band(position - max, dimension, c)
    } else {
        position
    }
}

/// Time-domain elastic motion used for the overshoot and return of a fling that reaches the edge
/// of the scrollable range.
///
/// constants and formula: `x(t) = (x0 + clamp(v, ±v_max)·t·A) · exp(−t·k / P) with
/// amplitude `A = 0.31`, period `P = 1.6`, stiffness `k = 20`, `v_max = 20000`; the decay rate is
/// `k / P = 12.5 s⁻¹`. The "settled" threshold (|x| < 0.5 pt) and the bisection used to find the
/// start of the return are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElasticMotion {
    pub amplitude: f64,
    pub period: f64,
    pub stiffness: f64,
    pub velocity_max: f64,
    pub settle_threshold: f64,
}

impl Default for ElasticMotion {
    fn default() -> Self {
        Self {
            amplitude: 0.31,
            period: 1.6,
            stiffness: 20.0,
            velocity_max: 20000.0,
            settle_threshold: 0.5,
        }
    }
}

impl ElasticMotion {
    pub fn decay_rate(&self) -> f64 {
        self.stiffness / self.period
    }

    /// Displacement from the edge after `t` seconds, for an initial displacement `origin` and
    /// initial velocity `velocity` (both signed, away from the edge positive).
    pub fn delta(&self, origin: f64, velocity: f64, t: f64) -> f64 {
        let t = t.max(0.0);
        let v = velocity.clamp(-self.velocity_max, self.velocity_max);
        (origin + v * t * self.amplitude) * (-t * self.decay_rate()).exp()
    }

    /// Velocity (d delta / dt) at time `t`.
    pub fn velocity(&self, origin: f64, velocity: f64, t: f64) -> f64 {
        let t = t.max(0.0);
        let v = velocity.clamp(-self.velocity_max, self.velocity_max);
        let k = self.decay_rate();
        let e = (-t * k).exp();
        v * self.amplitude * e - k * (origin + v * t * self.amplitude) * e
    }

    /// Time after which the displacement stays below the settle threshold (search: step
    /// 10 ms to a 10 s cap, then bisection to 0.01 s).
    pub fn settle_time(&self, origin: f64, velocity: f64) -> f64 {
        let settled = |t: f64| self.delta(origin, velocity, t).abs() < self.settle_threshold;
        let mut t = 0.0;
        let mut last_unsettled = 0.0;
        while t < 10.0 {
            if !settled(t) {
                last_unsettled = t;
            }
            t += 0.01;
        }
        (last_unsettled + 0.01).min(10.0)
    }
}

/// Defaults of the stateful rubber-band delta filter.
///
/// defaults: stiffness 20, direction-lock stretch ratio 1.0, minimum delta before stretch
/// 10. semantics: direction lock + minimum-delta gate; the transfer function is not
/// reduced to a closed form, so `stiffness` is only carried as the elastic stiffness for
/// [`ElasticMotion`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RubberBandFilterConfig {
    pub stiffness: f64,
    pub direction_lock_stretch_ratio: f64,
    pub min_delta_before_stretch: f64,
}

impl Default for RubberBandFilterConfig {
    fn default() -> Self {
        Self {
            stiffness: 20.0,
            direction_lock_stretch_ratio: 1.0,
            min_delta_before_stretch: 10.0,
        }
    }
}

/// Gates and axis-locks overscroll deltas before they stretch the content (semantics of
/// the verified defaults).
#[derive(Clone, Copy, Debug, Default)]
pub struct RubberBandFilter {
    pub config: RubberBandFilterConfig,
    pending: [f64; 2],
    stretching: bool,
}

impl RubberBandFilter {
    pub fn new(config: RubberBandFilterConfig) -> Self {
        Self {
            config,
            pending: [0.0; 2],
            stretching: false,
        }
    }

    pub fn is_stretching(&self) -> bool {
        self.stretching
    }

    pub fn reset(&mut self) {
        self.pending = [0.0; 2];
        self.stretching = false;
    }

    /// Feed a raw two-axis overscroll delta; returns the delta allowed to stretch the content.
    pub fn add_delta(&mut self, delta: [f64; 2]) -> [f64; 2] {
        let ratio = self.config.direction_lock_stretch_ratio;
        let locked = if delta[0].abs() * ratio >= delta[1].abs() {
            [delta[0], 0.0]
        } else {
            [0.0, delta[1]]
        };
        if self.stretching {
            return locked;
        }
        self.pending[0] += locked[0];
        self.pending[1] += locked[1];
        if self.pending[0].hypot(self.pending[1]) >= self.config.min_delta_before_stretch {
            self.stretching = true;
            let out = self.pending;
            self.pending = [0.0; 2];
            return out;
        }
        [0.0, 0.0]
    }
}
