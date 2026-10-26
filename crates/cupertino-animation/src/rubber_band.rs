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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_symmetry() {
        assert_eq!(rubber_band(0.0, 800.0, 0.55), 0.0);
        for x in [1.0, 10.0, 250.0, 5000.0] {
            assert_eq!(rubber_band(-x, 800.0, 0.55), -rubber_band(x, 800.0, 0.55));
        }
    }

    #[test]
    fn known_value() {
        let v = rubber_band(100.0, 100.0, 0.55);
        assert!((v - 100.0 * (1.0 - 1.0 / 1.55)).abs() < 1e-12);
    }

    #[test]
    fn monotonic_and_bounded() {
        let d = 600.0;
        let mut last = 0.0;
        for i in 1..2000 {
            let v = rubber_band(i as f64 * 5.0, d, 0.55);
            assert!(v > last && v < d);
            last = v;
        }
    }

    #[test]
    fn initial_slope_is_coefficient() {
        for c in [0.2, 0.55, 1.0] {
            let h = 1e-6;
            let num = rubber_band(h, 500.0, c) / h;
            assert!((num - c).abs() < 1e-5);
            assert!((slope(0.0, 500.0, c) - c).abs() < 1e-12);
        }
    }

    #[test]
    fn slope_matches_numeric_derivative() {
        for x in [0.0, 13.0, 200.0, 900.0] {
            let h = 1e-5;
            let num =
                (rubber_band(x + h, 400.0, 0.55) - rubber_band(x - h, 400.0, 0.55)) / (2.0 * h);
            assert!((num - slope(x, 400.0, 0.55)).abs() < 1e-6);
        }
    }

    #[test]
    fn inverse_roundtrip() {
        let rb = RubberBand::new(720.0);
        for x in [-900.0, -3.0, 0.0, 1.0, 47.0, 1234.0] {
            assert!((rb.inverse(rb.map(x)) - x).abs() < 1e-6 * (1.0 + x.abs()));
        }
        assert!(rb.inverse(720.0).is_infinite());
    }

    #[test]
    fn configurable_coefficient_is_stiffer_when_larger() {
        let soft = rubber_band(100.0, 500.0, 0.3);
        let firm = rubber_band(100.0, 500.0, 0.9);
        assert!(soft < firm);
    }

    #[test]
    fn apply_leaves_inside_unchanged() {
        let rb = RubberBand::new(600.0);
        assert_eq!(rb.apply(50.0, 0.0, 100.0), 50.0);
        assert_eq!(rb.apply(0.0, 0.0, 100.0), 0.0);
        assert!(rb.apply(-40.0, 0.0, 100.0) < 0.0 && rb.apply(-40.0, 0.0, 100.0) > -40.0);
        assert!(rb.apply(160.0, 0.0, 100.0) > 100.0 && rb.apply(160.0, 0.0, 100.0) < 160.0);
    }

    #[test]
    fn degenerate_dimension() {
        assert_eq!(rubber_band(10.0, 0.0, 0.55), 0.0);
        assert_eq!(slope(10.0, 0.0, 0.55), 0.0);
    }

    #[test]
    fn momentum_alias_matches() {
        assert_eq!(
            crate::momentum::rubber_band(33.0, 400.0, 0.55),
            rubber_band(33.0, 400.0, 0.55)
        );
    }

    #[test]
    fn elastic_defaults_and_decay_rate() {
        let e = ElasticMotion::default();
        assert_eq!(
            (e.amplitude, e.period, e.stiffness, e.velocity_max),
            (0.31, 1.6, 20.0, 20000.0)
        );
        assert!((e.decay_rate() - 12.5).abs() < 1e-12);
    }

    #[test]
    fn elastic_delta_formula() {
        let e = ElasticMotion::default();
        assert_eq!(e.delta(40.0, 0.0, 0.0), 40.0);
        let t = 0.1;
        let want = (40.0 + 300.0 * t * 0.31) * (-t * 20.0 / 1.6_f64).exp();
        assert!((e.delta(40.0, 300.0, t) - want).abs() < 1e-12);
        let clamped = e.delta(0.0, 1e9, t);
        assert!((clamped - 20000.0 * t * 0.31 * (-t * 12.5_f64).exp()).abs() < 1e-9);
    }

    #[test]
    fn elastic_velocity_is_derivative_and_settles() {
        let e = ElasticMotion::default();
        for t in [0.05, 0.3] {
            let h = 1e-6;
            let num = (e.delta(10.0, 500.0, t + h) - e.delta(10.0, 500.0, t - h)) / (2.0 * h);
            assert!((num - e.velocity(10.0, 500.0, t)).abs() < 1e-3);
        }
        let s = e.settle_time(100.0, 0.0);
        assert!(s > 0.2 && s < 1.0);
        assert!(e.delta(100.0, 0.0, s + 0.05).abs() < 0.5);
        assert_eq!(e.settle_time(0.1, 0.0), 0.01);
    }

    #[test]
    fn filter_defaults() {
        let c = RubberBandFilterConfig::default();
        assert_eq!(
            (
                c.stiffness,
                c.direction_lock_stretch_ratio,
                c.min_delta_before_stretch
            ),
            (20.0, 1.0, 10.0)
        );
    }

    #[test]
    fn filter_gates_and_locks_direction() {
        let mut f = RubberBandFilter::default();
        assert_eq!(f.add_delta([4.0, 1.0]), [0.0, 0.0]);
        assert!(!f.is_stretching());
        let out = f.add_delta([7.0, 2.0]);
        assert_eq!(out, [11.0, 0.0]);
        assert!(f.is_stretching());
        assert_eq!(f.add_delta([1.0, 5.0]), [0.0, 5.0]);
        f.reset();
        assert!(!f.is_stretching());
    }
}
