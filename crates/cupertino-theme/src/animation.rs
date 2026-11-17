//! Animation presets loaded from the theme data (`animations.json`), converted into `cupertino-animation` types.

use crate::error::{Error, Result};
use cupertino_animation::cosine_ease;
use cupertino_animation::{CubicBezier, Momentum, RubberBand, SpringParams};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

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

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_str(&text)
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
