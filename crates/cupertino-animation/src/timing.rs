//! Cubic-Bézier timing functions (reference timing function semantics).
//!
//! Built-in control points —: default (0.25,0.1,0.25,1),
//! linear (0,0,1,1), easeIn (0.42,0,1,1), easeOut (0,0,0.58,1), easeInEaseOut (0.42,0,0.58,1).
//! Solver — approximate: Newton–Raphson with bisection fallback (unit-bezier style); the
//! reference solver was not analysed numerically. Differences are expected to be < 1e-6.

use crate::animator::Curve;
use crate::spring::{Spring, SpringParams};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl CubicBezier {
    pub const DEFAULT: Self = Self::new(0.25, 0.1, 0.25, 1.0);
    pub const LINEAR: Self = Self::new(0.0, 0.0, 1.0, 1.0);
    pub const EASE_IN: Self = Self::new(0.42, 0.0, 1.0, 1.0);
    pub const EASE_OUT: Self = Self::new(0.0, 0.0, 0.58, 1.0);
    pub const EASE_IN_EASE_OUT: Self = Self::new(0.42, 0.0, 0.58, 1.0);

    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn coeffs(a1: f64, a2: f64) -> (f64, f64, f64) {
        let c = 3.0 * a1;
        let b = 3.0 * (a2 - a1) - c;
        (1.0 - c - b, b, c)
    }

    fn sample(a: f64, b: f64, c: f64, t: f64) -> f64 {
        ((a * t + b) * t + c) * t
    }

    fn solve_x(&self, x: f64) -> f64 {
        let (ax, bx, cx) = Self::coeffs(self.x1, self.x2);
        let mut t = x;
        for _ in 0..8 {
            let err = Self::sample(ax, bx, cx, t) - x;
            if err.abs() < 1e-9 {
                return t;
            }
            let d = (3.0 * ax * t + 2.0 * bx) * t + cx;
            if d.abs() < 1e-9 {
                break;
            }
            t -= err / d;
        }
        let (mut lo, mut hi) = (0.0, 1.0);
        t = x;
        while hi - lo > 1e-9 {
            let v = Self::sample(ax, bx, cx, t);
            if v < x {
                lo = t
            } else {
                hi = t
            }
            t = 0.5 * (lo + hi);
        }
        t
    }

    /// Eased progress for linear input progress `x ∈ [0,1]`.
    pub fn eval(&self, x: f64) -> f64 {
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        let t = self.solve_x(x);
        let (ay, by, cy) = Self::coeffs(self.y1, self.y2);
        Self::sample(ay, by, cy, t)
    }
}

/// Cosine S-curve `0.5 − 0.5·cos(π·x) used by the dock's float animations.
pub fn cosine_ease(x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    0.5 - 0.5 * (std::f64::consts::PI * x).cos()
}

/// The timing-function table: five cubic béziers and one
/// spring (mass 1, stiffness 100, damping 10). Which transitions use the spring entry is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimingPreset {
    Ease,
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Spring,
}

impl TimingPreset {
    pub const TABLE: [TimingPreset; 6] = [
        Self::Ease,
        Self::Linear,
        Self::EaseIn,
        Self::EaseOut,
        Self::EaseInOut,
        Self::Spring,
    ];

    pub fn from_index(i: usize) -> Option<Self> {
        Self::TABLE.get(i).copied()
    }

    pub fn index(self) -> usize {
        Self::TABLE.iter().position(|p| *p == self).unwrap_or(0)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Ease => "ease",
            Self::Linear => "linear",
            Self::EaseIn => "ease-in",
            Self::EaseOut => "ease-out",
            Self::EaseInOut => "ease-in-out",
            Self::Spring => "spring",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::TABLE.iter().copied().find(|p| p.name() == name)
    }

    pub fn bezier(self) -> Option<CubicBezier> {
        match self {
            Self::Ease => Some(CubicBezier::DEFAULT),
            Self::Linear => Some(CubicBezier::LINEAR),
            Self::EaseIn => Some(CubicBezier::EASE_IN),
            Self::EaseOut => Some(CubicBezier::EASE_OUT),
            Self::EaseInOut => Some(CubicBezier::EASE_IN_EASE_OUT),
            Self::Spring => None,
        }
    }

    pub fn spring(self) -> Option<SpringParams> {
        (self == Self::Spring).then(SpringParams::default)
    }

    /// Eased progress `elapsed / duration`; the spring entry ignores `duration` (it runs in
    /// absolute time and settles by itself).
    pub fn sample(self, elapsed: f64, duration: f64) -> f64 {
        match self.bezier() {
            Some(b) => b.eval(if duration <= 0.0 {
                1.0
            } else {
                elapsed / duration
            }),
            None => Spring::new(SpringParams::default()).value(elapsed),
        }
    }

    pub fn curve(self, duration: f64) -> Curve {
        match self.bezier() {
            Some(bezier) => Curve::Timed { duration, bezier },
            None => Curve::Spring(SpringParams::default()),
        }
    }
}
