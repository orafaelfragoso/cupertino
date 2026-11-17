//! Animation presets loaded from the theme data (`animations.json`), converted into `cupertino-animation` types, and
//! the design's timing-function table ([`TimingPreset`]).

use crate::error::{Error, Result};
use cupertino_animation::cosine_ease;
use cupertino_animation::{CubicBezier, Curve, Momentum, RubberBand, Spring, SpringParams};
use serde::Deserialize;
use std::collections::BTreeMap;

/// An easing referenced by name: a cubic bézier of the timing table or the cosine S-curve (`0.5 − 0.5·cos(π·x)`), which is
/// not a bézier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Easing {
    Bezier(CubicBezier),
    CosineEase,
}

impl Easing {
    /// Eased progress for linear progress `x ∈ [0, 1]`.
    pub fn eval(&self, x: f64) -> f64 {
        match self {
            Self::Bezier(b) => b.eval(x),
            Self::CosineEase => cosine_ease(x),
        }
    }

    pub fn as_bezier(&self) -> Option<CubicBezier> {
        match self {
            Self::Bezier(b) => Some(*b),
            Self::CosineEase => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SpringPreset {
    pub params: SpringParams,
    /// `(perceptual duration, bounce)` when the preset is defined that way.
    pub perceptual: Option<(f64, f64)>,
}

/// Dock-related timing values.
#[derive(Clone, Debug)]
pub struct DockAnimation {
    /// Fish-speed factor of the magnification ramp: `trunc(ln(|Δ|·0.5 + 1)·fish_speed + 1)` ms.
    pub fish_speed: f64,
    pub ramp_while_dragging_ms: f64,
    pub drop_speed: f64,
    pub autohide_delay_s: f64,
    pub autohide_duration_s: f64,
    pub autohide_curve: Easing,
    /// Half-width of the magnification profile in tile widths.
    pub magnification_span: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinimizeEffect {
    WarpMinimize,
    Scale,
    Suck,
}

#[derive(Clone, Debug)]
pub struct AnimationPresets {
    timing: BTreeMap<String, CubicBezier>,
    cosine_ease: bool,
    springs: BTreeMap<String, SpringPreset>,
    pub settling_epsilon: f64,
    pub momentum_deceleration_per_ms: f64,
    pub rubber_band_coefficient: f64,
    pub dock: DockAnimation,
    minimize_effects: Vec<MinimizeEffect>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawSpring {
    Physical {
        mass: f64,
        stiffness: f64,
        damping: f64,
        initial_velocity: f64,
    },
    Perceptual {
        perceptual_duration: f64,
        bounce: f64,
    },
}

#[derive(Deserialize)]
struct RawScrolling {
    momentum_deceleration_per_ms: f64,
    rubber_band_coefficient: f64,
}

#[derive(Deserialize)]
struct RawAutohide {
    delay_s: f64,
    duration_s: f64,
    curve: String,
}

#[derive(Deserialize)]
struct RawDock {
    fish_speed: f64,
    ramp_while_dragging_ms: f64,
    drop_speed: f64,
    autohide: RawAutohide,
    magnification_span: f64,
}

#[derive(Deserialize)]
struct RawAnimations {
    timing_functions: BTreeMap<String, [f64; 4]>,
    cosine_ease: bool,
    springs: BTreeMap<String, RawSpring>,
    settling_epsilon: f64,
    scrolling: RawScrolling,
    dock: RawDock,
    minimize_effects: Vec<String>,
}

impl AnimationPresets {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(json: &str) -> Result<Self> {
        let raw: RawAnimations = serde_json::from_str(json)?;
        let timing: BTreeMap<String, CubicBezier> = raw
            .timing_functions
            .into_iter()
            .map(|(k, b)| (k, CubicBezier::new(b[0], b[1], b[2], b[3])))
            .collect();
        let springs = raw
            .springs
            .into_iter()
            .map(|(name, s)| {
                let preset = match s {
                    RawSpring::Physical {
                        mass,
                        stiffness,
                        damping,
                        initial_velocity,
                    } => SpringPreset {
                        params: SpringParams {
                            mass,
                            stiffness,
                            damping,
                            initial_velocity,
                            allows_overdamping: false,
                        },
                        perceptual: None,
                    },
                    RawSpring::Perceptual {
                        perceptual_duration,
                        bounce,
                    } => SpringPreset {
                        params: SpringParams::from_perceptual(perceptual_duration, bounce),
                        perceptual: Some((perceptual_duration, bounce)),
                    },
                };
                (name, preset)
            })
            .collect();
        let curve_name = raw.dock.autohide.curve.as_str();
        let autohide_curve = if curve_name == "cosine_ease" {
            if !raw.cosine_ease {
                return Err(Error::NotFound("cosine_ease".into()));
            }
            Easing::CosineEase
        } else {
            Easing::Bezier(*timing.get(curve_name).ok_or_else(|| {
                Error::Invalid(format!(
                    "unknown easing {curve_name} in dock.autohide.curve"
                ))
            })?)
        };
        let dock = DockAnimation {
            fish_speed: raw.dock.fish_speed,
            ramp_while_dragging_ms: raw.dock.ramp_while_dragging_ms,
            drop_speed: raw.dock.drop_speed,
            autohide_delay_s: raw.dock.autohide.delay_s,
            autohide_duration_s: raw.dock.autohide.duration_s,
            autohide_curve,
            magnification_span: raw.dock.magnification_span,
        };
        let minimize_effects = raw
            .minimize_effects
            .iter()
            .map(|s| match s.as_str() {
                "warp" => Ok(MinimizeEffect::WarpMinimize),
                "scale" => Ok(MinimizeEffect::Scale),
                "suck" => Ok(MinimizeEffect::Suck),
                other => Err(Error::Invalid(format!("unknown minimize effect {other}"))),
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            timing,
            cosine_ease: raw.cosine_ease,
            springs,
            settling_epsilon: raw.settling_epsilon,
            momentum_deceleration_per_ms: raw.scrolling.momentum_deceleration_per_ms,
            rubber_band_coefficient: raw.scrolling.rubber_band_coefficient,
            dock,
            minimize_effects,
        })
    }

    pub fn timing_names(&self) -> impl Iterator<Item = &str> {
        self.timing.keys().map(String::as_str)
    }

    pub fn spring_names(&self) -> impl Iterator<Item = &str> {
        self.springs.keys().map(String::as_str)
    }

    /// Timing function by name (`default`, `linear`).
    pub fn timing(&self, name: &str) -> Option<CubicBezier> {
        self.timing.get(name).copied()
    }

    /// Easing by name: the timing functions plus `cosine_ease`.
    pub fn easing(&self, name: &str) -> Option<Easing> {
        if name == "cosine_ease" {
            return self.cosine_ease.then_some(Easing::CosineEase);
        }
        self.timing(name).map(Easing::Bezier)
    }

    /// Spring by name: `reference_default`, `window_move_resize_snap`, `overview_toggle`.
    pub fn spring(&self, name: &str) -> Option<SpringParams> {
        self.springs.get(name).map(|s| s.params)
    }

    pub fn spring_preset(&self, name: &str) -> Option<&SpringPreset> {
        self.springs.get(name)
    }

    pub fn momentum(&self, origin: f64, velocity_pt_per_s: f64, min: f64, max: f64) -> Momentum {
        Momentum {
            factor_per_ms: self.momentum_deceleration_per_ms,
            ..Momentum::new(origin, velocity_pt_per_s, min, max)
        }
    }

    pub fn rubber_band(&self, dimension: f64) -> RubberBand {
        RubberBand::with_coefficient(dimension, self.rubber_band_coefficient)
    }

    pub fn minimize_effects(&self) -> &[MinimizeEffect] {
        &self.minimize_effects
    }
}

/// The default timing curve: a gentle start and a long settle (the table's `ease` entry).
pub const EASE_CURVE: CubicBezier = CubicBezier::new(0.25, 0.1, 0.25, 1.0);
/// Constant speed from start to end (the table's `linear` entry).
pub const LINEAR_CURVE: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);
/// Starts slowly and ends at full speed (the table's `ease-in` entry).
pub const EASE_IN_CURVE: CubicBezier = CubicBezier::new(0.42, 0.0, 1.0, 1.0);
/// Starts at full speed and slows to a stop (the table's `ease-out` entry).
pub const EASE_OUT_CURVE: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);
/// Starts and ends slowly, symmetric about the midpoint (the table's `ease-in-out` entry).
pub const EASE_IN_OUT_CURVE: CubicBezier = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
/// The design's default spring, used where a motion names a spring without parameters (the table's `spring` entry).
pub const DEFAULT_SPRING: SpringParams = SpringParams {
    mass: 1.0,
    stiffness: 100.0,
    damping: 10.0,
    initial_velocity: 0.0,
    allows_overdamping: false,
};

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
            Self::Ease => Some(EASE_CURVE),
            Self::Linear => Some(LINEAR_CURVE),
            Self::EaseIn => Some(EASE_IN_CURVE),
            Self::EaseOut => Some(EASE_OUT_CURVE),
            Self::EaseInOut => Some(EASE_IN_OUT_CURVE),
            Self::Spring => None,
        }
    }

    pub fn spring(self) -> Option<SpringParams> {
        (self == Self::Spring).then_some(DEFAULT_SPRING)
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
            None => Spring::new(DEFAULT_SPRING).value(elapsed),
        }
    }

    pub fn curve(self, duration: f64) -> Curve {
        match self.bezier() {
            Some(bezier) => Curve::Timed { duration, bezier },
            None => Curve::Spring(DEFAULT_SPRING),
        }
    }
}
