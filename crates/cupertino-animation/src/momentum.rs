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
        Self {
            origin,
            velocity_pt_per_s,
            factor_per_ms: Self::REFERENCE_DECELERATION,
            min,
            max,
        }
    }

    fn v_ms(&self) -> f64 {
        self.velocity_pt_per_s / 1000.0
    }

    /// Unclamped resting displacement (formula).
    pub fn displacement(&self) -> f64 {
        let v = self.v_ms();
        if v.abs() > 0.01 {
            v * self.factor_per_ms / (1.0 - self.factor_per_ms)
        } else {
            0.0
        }
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
        Self {
            origin,
            velocity_pt_per_s,
            constant: Self::CONSTANT,
        }
    }

    pub fn with_constant(origin: f64, velocity_pt_per_s: f64, constant: f64) -> Self {
        Self {
            origin,
            velocity_pt_per_s,
            constant,
        }
    }

    fn sign(&self) -> f64 {
        if self.velocity_pt_per_s < 0.0 {
            -1.0
        } else {
            1.0
        }
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
                if v.abs() / 1000.0 > 0.01 {
                    v
                } else {
                    0.0
                }
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
        Self {
            origin,
            displacement: [dx, dy],
            constant,
        }
    }

    pub fn duration(&self) -> f64 {
        if self.constant <= 0.0 {
            return 0.0;
        }
        (self.displacement[0].hypot(self.displacement[1]) / self.constant).powf(0.25)
    }

    pub fn destination(&self) -> [f64; 2] {
        [
            self.origin[0] + self.displacement[0],
            self.origin[1] + self.displacement[1],
        ]
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
        [
            self.origin[0] + self.displacement[0] * p,
            self.origin[1] + self.displacement[1] * p,
        ]
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
