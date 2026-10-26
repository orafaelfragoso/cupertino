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
            if v < x { lo = t } else { hi = t }
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
    pub const TABLE: [TimingPreset; 6] =
        [Self::Ease, Self::Linear, Self::EaseIn, Self::EaseOut, Self::EaseInOut, Self::Spring];

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
            Some(b) => b.eval(if duration <= 0.0 { 1.0 } else { elapsed / duration }),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_and_linear() {
        for f in [CubicBezier::DEFAULT, CubicBezier::EASE_IN, CubicBezier::EASE_OUT, CubicBezier::EASE_IN_EASE_OUT] {
            assert_eq!(f.eval(0.0), 0.0);
            assert_eq!(f.eval(1.0), 1.0);
        }
        for i in 0..=10 {
            let x = i as f64 / 10.0;
            assert!((CubicBezier::LINEAR.eval(x) - x).abs() < 1e-9);
        }
        // symmetric curve: f(0.5) == 0.5
        assert!((CubicBezier::EASE_IN_EASE_OUT.eval(0.5) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn preset_table_matches_recovered_values() {
        assert_eq!(TimingPreset::TABLE.len(), 6);
        let want = [(0.25, 0.1, 0.25, 1.0), (0.0, 0.0, 1.0, 1.0), (0.42, 0.0, 1.0, 1.0), (0.0, 0.0, 0.58, 1.0), (0.42, 0.0, 0.58, 1.0)];
        for (i, w) in want.iter().enumerate() {
            let b = TimingPreset::from_index(i).unwrap().bezier().unwrap();
            assert_eq!((b.x1, b.y1, b.x2, b.y2), *w);
        }
        let s = TimingPreset::from_index(5).unwrap().spring().unwrap();
        assert_eq!((s.mass, s.stiffness, s.damping), (1.0, 100.0, 10.0));
        assert!(TimingPreset::from_index(6).is_none());
    }

    #[test]
    fn preset_names_and_indices_roundtrip() {
        for p in TimingPreset::TABLE {
            assert_eq!(TimingPreset::from_name(p.name()), Some(p));
            assert_eq!(TimingPreset::from_index(p.index()), Some(p));
        }
        assert!(TimingPreset::from_name("nope").is_none());
    }

    #[test]
    fn preset_sampling() {
        assert!((TimingPreset::Linear.sample(0.25, 1.0) - 0.25).abs() < 1e-9);
        assert_eq!(TimingPreset::Ease.sample(1.0, 1.0), 1.0);
        let sp = TimingPreset::Spring;
        assert_eq!(sp.sample(0.0, 0.0), 0.0);
        let wd = 10.0 * (0.75f64).sqrt();
        let t: f64 = 0.3;
        let want = 1.0 - (-5.0 * t).exp() * ((wd * t).cos() + 0.5 * 10.0 / wd * (wd * t).sin());
        assert!((sp.sample(t, 1.0) - want).abs() < 1e-9);
        assert!(matches!(TimingPreset::EaseIn.curve(0.2), Curve::Timed { .. }));
        assert!(matches!(sp.curve(0.2), Curve::Spring(_)));
    }

    #[test]
    fn cosine_ease_is_symmetric_s_curve() {
        assert_eq!(cosine_ease(0.0), 0.0);
        assert!((cosine_ease(1.0) - 1.0).abs() < 1e-12);
        assert!((cosine_ease(0.5) - 0.5).abs() < 1e-12);
        assert!((cosine_ease(0.25) + cosine_ease(0.75) - 1.0).abs() < 1e-12);
        assert_eq!(cosine_ease(-1.0), 0.0);
    }
}
