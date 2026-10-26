//! Scroll momentum after a fling, modelled on the design UI toolkit's momentum calculators.
//!
//! [`QuarticMomentum`] is the default model; [`Momentum`] is the alternative
//! per-millisecond exponential model described below.
//!
//! * the default deceleration factor is 0.998 per millisecond on both axes
//! * destination: v_ms = v / 1000; if |v_ms| > 0.01 then
//!   destination = origin + v_ms·f/(1−f), clamped to [min, max]
//! * position after a duration converts seconds to *integer milliseconds* (t·1000 → long)
//!
//! per-millisecond geometric decay x(n) = v_ms·f·(1−fⁿ)/(1−f), consistent with the
//! verified closed-form destination. The stop threshold (here |v|<0.01 pt/ms) is.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Momentum {
    pub origin: f64,
    pub velocity_pt_per_s: f64,
    pub factor_per_ms: f64,
    pub min: f64,
    pub max: f64,
}

impl Momentum {
    pub const REFERENCE_DECELERATION: f64 = 0.998;

    pub fn new(origin: f64, velocity_pt_per_s: f64, min: f64, max: f64) -> Self {
        Self { origin, velocity_pt_per_s, factor_per_ms: Self::REFERENCE_DECELERATION, min, max }
    }

    fn v_ms(&self) -> f64 {
        self.velocity_pt_per_s / 1000.0
    }

    /// Unclamped resting displacement (formula).
    pub fn displacement(&self) -> f64 {
        let v = self.v_ms();
        if v.abs() > 0.01 { v * self.factor_per_ms / (1.0 - self.factor_per_ms) } else { 0.0 }
    }

    pub fn destination(&self) -> f64 {
        (self.origin + self.displacement()).clamp(self.min, self.max)
    }

    /// Position after `t` seconds, evaluated at integer milliseconds like the design toolkit.
    pub fn position(&self, t: f64) -> f64 {
        let n = (t * 1000.0).max(0.0) as i64 as f64;
        let f = self.factor_per_ms;
        let d = self.v_ms() * f * (1.0 - f.powf(n)) / (1.0 - f);
        (self.origin + d).clamp(self.min, self.max)
    }

    /// Time (s) until velocity falls below `threshold_pt_per_ms` (threshold 0.01).
    pub fn duration_until_stop(&self, threshold_pt_per_ms: f64) -> f64 {
        let v = self.v_ms().abs();
        if v <= threshold_pt_per_ms {
            return 0.0;
        }
        ((threshold_pt_per_ms / v).ln() / self.factor_per_ms.ln()).ceil() / 1000.0
    }
}

/// Default scroll momentum: quartic-position model.
///
/// constants and structure: momentum constant `C = 1000`, destination
/// `D = sign(v)·C·(|v|/(4C))^(4/3)`, duration `T = (|D|/C)^(1/4)`, velocity `v(t) = 4C·(T−t)³`.
/// (one interpretive link): position `x(t) = x0 + sign·C·(T⁴ − (T−t)⁴), which is the
/// integral of the verified velocity and ends exactly at `x0 + D`.
///
/// The alternative per-millisecond model of the same toolkit is [`Momentum`] (selected by a user
/// preference whose default selects this model).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuarticMomentum {
    pub origin: f64,
    pub velocity_pt_per_s: f64,
    pub constant: f64,
}

impl QuarticMomentum {
    /// the momentum constant.
    pub const CONSTANT: f64 = 1000.0;
    pub const DESTINATION_EXPONENT: f64 = 4.0 / 3.0;
    pub const DURATION_EXPONENT: f64 = 0.25;

    pub fn new(origin: f64, velocity_pt_per_s: f64) -> Self {
        Self { origin, velocity_pt_per_s, constant: Self::CONSTANT }
    }

    pub fn with_constant(origin: f64, velocity_pt_per_s: f64, constant: f64) -> Self {
        Self { origin, velocity_pt_per_s, constant }
    }

    fn sign(&self) -> f64 {
        if self.velocity_pt_per_s < 0.0 { -1.0 } else { 1.0 }
    }

    /// Signed resting displacement `D`.
    pub fn displacement(&self) -> f64 {
        let v = self.velocity_pt_per_s.abs();
        if v == 0.0 || self.constant <= 0.0 {
            return 0.0;
        }
        self.sign() * self.constant * (v / (4.0 * self.constant)).powf(Self::DESTINATION_EXPONENT)
    }

    pub fn destination(&self) -> f64 {
        self.origin + self.displacement()
    }

    /// Coasting duration `T = (|D|/C)^(1/4)` in seconds.
    pub fn duration(&self) -> f64 {
        if self.constant <= 0.0 {
            return 0.0;
        }
        (self.displacement().abs() / self.constant).powf(Self::DURATION_EXPONENT)
    }

    pub fn position(&self, t: f64) -> f64 {
        let big_t = self.duration();
        let t = t.clamp(0.0, big_t);
        self.origin + self.sign() * self.constant * (big_t.powi(4) - (big_t - t).powi(4))
    }

    /// Signed velocity in pt/s; zero outside `[0, T]`.
    pub fn velocity(&self, t: f64) -> f64 {
        let big_t = self.duration();
        if t < 0.0 || t >= big_t {
            return 0.0;
        }
        self.sign() * 4.0 * self.constant * (big_t - t).powi(3)
    }

    /// First time at which the coasting position reaches `bound` (the position is monotonic), or
    /// `None` when the fling stops before it.
    pub fn time_to_reach(&self, bound: f64) -> Option<f64> {
        let d = bound - self.origin;
        if d == 0.0 {
            return Some(0.0);
        }
        if d.signum() != self.sign() || d.abs() > self.displacement().abs() {
            return None;
        }
        let big_t = self.duration();
        let frac = 1.0 - d.abs() / (self.constant * big_t.powi(4));
        Some(big_t - big_t * frac.max(0.0).powf(0.25))
    }
}

/// Momentum model selector: the default quartic model or the per-millisecond exponential one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MomentumModel {
    Quartic(QuarticMomentum),
    Exponential(Momentum),
}

impl MomentumModel {
    /// The default model for a fling starting at `origin`.
    pub fn default_for(origin: f64, velocity_pt_per_s: f64) -> Self {
        Self::Quartic(QuarticMomentum::new(origin, velocity_pt_per_s))
    }

    pub fn exponential(origin: f64, velocity_pt_per_s: f64) -> Self {
        Self::Exponential(Momentum::new(origin, velocity_pt_per_s, f64::MIN, f64::MAX))
    }

    pub fn position(&self, t: f64) -> f64 {
        match self {
            Self::Quartic(m) => m.position(t),
            Self::Exponential(m) => m.position(t),
        }
    }

    pub fn velocity(&self, t: f64) -> f64 {
        match self {
            Self::Quartic(m) => m.velocity(t),
            Self::Exponential(m) => {
                let n = (t * 1000.0).max(0.0).trunc();
                let v = m.velocity_pt_per_s * m.factor_per_ms.powf(n);
                if v.abs() / 1000.0 > 0.01 { v } else { 0.0 }
            }
        }
    }

    pub fn duration(&self) -> f64 {
        match self {
            Self::Quartic(m) => m.duration(),
            Self::Exponential(m) => m.duration_until_stop(0.01),
        }
    }

    pub fn destination(&self) -> f64 {
        match self {
            Self::Quartic(m) => m.destination(),
            Self::Exponential(m) => m.destination(),
        }
    }
}

/// Two-axis quartic momentum with a shared duration (the duration uses the vector length
/// `L = hypot(Dx, Dy)`; per-axis distribution is each axis travels its own `D` over the
/// shared `T` with the same quartic ease-out profile).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuarticMomentum2D {
    pub origin: [f64; 2],
    pub displacement: [f64; 2],
    pub constant: f64,
}

impl QuarticMomentum2D {
    pub fn new(origin: [f64; 2], velocity: [f64; 2]) -> Self {
        Self::with_constant(origin, velocity, QuarticMomentum::CONSTANT)
    }

    pub fn with_constant(origin: [f64; 2], velocity: [f64; 2], constant: f64) -> Self {
        let dx = QuarticMomentum::with_constant(0.0, velocity[0], constant).displacement();
        let dy = QuarticMomentum::with_constant(0.0, velocity[1], constant).displacement();
        Self { origin, displacement: [dx, dy], constant }
    }

    pub fn duration(&self) -> f64 {
        if self.constant <= 0.0 {
            return 0.0;
        }
        (self.displacement[0].hypot(self.displacement[1]) / self.constant).powf(0.25)
    }

    pub fn destination(&self) -> [f64; 2] {
        [self.origin[0] + self.displacement[0], self.origin[1] + self.displacement[1]]
    }

    fn progress(&self, t: f64) -> f64 {
        let big_t = self.duration();
        if big_t <= 0.0 {
            return 1.0;
        }
        let r = (big_t - t.clamp(0.0, big_t)) / big_t;
        1.0 - r.powi(4)
    }

    pub fn position(&self, t: f64) -> [f64; 2] {
        let p = self.progress(t);
        [self.origin[0] + self.displacement[0] * p, self.origin[1] + self.displacement[1] * p]
    }

    pub fn velocity(&self, t: f64) -> [f64; 2] {
        let big_t = self.duration();
        if big_t <= 0.0 || t < 0.0 || t >= big_t {
            return [0.0, 0.0];
        }
        let r = (big_t - t) / big_t;
        let dp = 4.0 * r.powi(3) / big_t;
        [self.displacement[0] * dp, self.displacement[1] * dp]
    }
}

/// approximate elastic overscroll mapping, see [`crate::rubber_band::rubber_band`].
pub fn rubber_band(offset: f64, dimension: f64, c: f64) -> f64 {
    crate::rubber_band::rubber_band(offset, dimension, c)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_form_matches_sum() {
        let m = Momentum::new(0.0, 2000.0, -1e9, 1e9);
        // 2 pt/ms * 0.998/0.002 = 998 pt
        assert!((m.displacement() - 998.0).abs() < 1e-9);
        let far = m.position(60.0);
        assert!((far - 998.0).abs() < 1e-6);
        let mut acc = 0.0;
        let mut v = 2.0;
        for _ in 0..250 {
            v *= 0.998;
            acc += v;
        }
        assert!((m.position(0.25) - acc).abs() < 1e-9);
    }
    #[test]
    fn clamps_and_threshold() {
        let m = Momentum::new(100.0, 5.0, 0.0, 500.0); // 0.005 pt/ms < 0.01 → no movement
        assert_eq!(m.destination(), 100.0);
        let m = Momentum::new(100.0, -10000.0, 0.0, 500.0);
        assert_eq!(m.destination(), 0.0);
    }

    #[test]
    fn quartic_destination_and_duration() {
        let m = QuarticMomentum::new(10.0, 4000.0);
        // D = C·(v/4C)^(4/3) = 1000 for v = 4000
        assert!((m.displacement() - 1000.0).abs() < 1e-9);
        assert!((m.duration() - 1.0).abs() < 1e-12);
        assert!((m.position(m.duration()) - 1010.0).abs() < 1e-9);
        assert!((m.position(99.0) - 1010.0).abs() < 1e-9);
        assert_eq!(m.position(0.0), 10.0);
        let n = QuarticMomentum::new(0.0, -4000.0);
        assert!((n.destination() + 1000.0).abs() < 1e-9);
    }

    #[test]
    fn quartic_velocity_is_derivative_and_matches_initial() {
        let m = QuarticMomentum::new(0.0, 2500.0);
        assert!((m.velocity(0.0) - 2500.0).abs() < 1e-6);
        for t in [0.05, 0.2, 0.4] {
            let h = 1e-6;
            let num = (m.position(t + h) - m.position(t - h)) / (2.0 * h);
            assert!((num - m.velocity(t)).abs() < 1e-3);
        }
        assert_eq!(m.velocity(m.duration() + 0.1), 0.0);
        assert_eq!(QuarticMomentum::new(5.0, 0.0).duration(), 0.0);
        assert_eq!(QuarticMomentum::new(5.0, 0.0).destination(), 5.0);
    }

    #[test]
    fn quartic_time_to_reach_inverts_position() {
        let m = QuarticMomentum::new(0.0, 3000.0);
        let t = m.time_to_reach(200.0).unwrap();
        assert!((m.position(t) - 200.0).abs() < 1e-6);
        assert!(m.time_to_reach(1e6).is_none());
        assert!(m.time_to_reach(-5.0).is_none());
        assert_eq!(m.time_to_reach(0.0), Some(0.0));
    }

    #[test]
    fn quartic_2d_shares_duration() {
        let m = QuarticMomentum2D::new([0.0, 0.0], [3000.0, 0.0]);
        let one = QuarticMomentum::new(0.0, 3000.0);
        assert!((m.duration() - one.duration()).abs() < 1e-12);
        assert!((m.destination()[0] - one.destination()).abs() < 1e-9);
        let d = QuarticMomentum2D::new([0.0, 0.0], [3000.0, 3000.0]);
        let end = d.position(d.duration());
        assert!((end[0] - d.displacement[0]).abs() < 1e-9 && (end[0] - end[1]).abs() < 1e-9);
        assert!(d.velocity(0.0)[0] > 0.0);
        assert_eq!(d.velocity(d.duration() + 1.0), [0.0, 0.0]);
    }

    #[test]
    fn model_selector_defaults_to_quartic() {
        assert!(matches!(MomentumModel::default_for(0.0, 1000.0), MomentumModel::Quartic(_)));
        let e = MomentumModel::exponential(0.0, 2000.0);
        assert!((e.destination() - 998.0).abs() < 1e-6);
        assert!(e.duration() > 0.0 && e.velocity(0.0) > 0.0);
        assert_eq!(e.velocity(e.duration() + 5.0), 0.0);
        let q = MomentumModel::default_for(0.0, 1000.0);
        assert!(q.position(q.duration()) > 0.0 && q.destination() > 0.0);
    }
}
