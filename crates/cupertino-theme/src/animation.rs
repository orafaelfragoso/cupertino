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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;
    use cupertino_animation::Spring;

    fn presets() -> AnimationPresets {
        Theme::load().unwrap().animation
    }

    #[test]
    fn timing_functions_match_builtins() {
        let p = presets();
        assert_eq!(p.timing("default"), Some(CubicBezier::DEFAULT));
        assert_eq!(p.timing("linear"), Some(CubicBezier::LINEAR));
        assert_eq!(p.timing("easeIn"), Some(CubicBezier::EASE_IN));
        assert_eq!(p.timing("easeOut"), Some(CubicBezier::EASE_OUT));
        assert_eq!(
            p.timing("easeInEaseOut"),
            Some(CubicBezier::EASE_IN_EASE_OUT)
        );
        assert_eq!(p.timing_names().count(), 5);
    }

    #[test]
    fn reference_default_spring() {
        let p = presets();
        let s = p.spring("reference_default").unwrap();
        assert_eq!(s, SpringParams::default());
        let settle = Spring::new(s).settling_duration();
        assert!((settle - 1.4727).abs() < 1e-3, "{settle}");
    }

    #[test]
    fn perceptual_springs() {
        let p = presets();
        let s = p.spring("window_move_resize_snap").unwrap();
        assert_eq!(s, SpringParams::from_perceptual(0.35, 0.0));
        assert_eq!(
            p.spring_preset("window_move_resize_snap")
                .unwrap()
                .perceptual,
            Some((0.35, 0.0))
        );
        assert!(p.spring("overview_toggle").is_some());
        assert!(p.spring("perceptual_formula").is_none());
        assert_eq!(p.settling_epsilon, 0.001);
    }

    #[test]
    fn scrolling_presets() {
        let p = presets();
        assert_eq!(p.momentum_deceleration_per_ms, 0.998);
        assert_eq!(p.rubber_band_coefficient, 0.55);
        let m = p.momentum(0.0, 2000.0, -1e9, 1e9);
        assert!((m.displacement() - 998.0).abs() < 1e-9);
        assert_eq!(p.rubber_band(800.0), RubberBand::new(800.0));
    }

    #[test]
    fn dock_presets() {
        let d = presets().dock;
        assert_eq!(d.fish_speed, 42.0);
        assert_eq!(d.ramp_while_dragging_ms, 270.0);
        assert_eq!(d.drop_speed, 350.0);
        assert_eq!(d.autohide_delay_s, 0.22);
        assert_eq!(d.autohide_duration_s, 0.22);
        assert_eq!(d.autohide_curve, Easing::CosineEase);
        assert_eq!(d.magnification_span, 3.0);
    }

    #[test]
    fn cosine_ease_is_not_a_bezier() {
        let p = presets();
        assert_eq!(p.easing("cosine_ease"), Some(Easing::CosineEase));
        assert_eq!(
            p.easing("easeOut"),
            Some(Easing::Bezier(CubicBezier::EASE_OUT))
        );
        assert_eq!(p.easing("nope"), None);
        assert!(p.timing("cosine_ease").is_none());
        let c = Easing::CosineEase;
        assert_eq!(c.as_bezier(), None);
        assert_eq!((c.eval(0.0), c.eval(1.0)), (0.0, 1.0));
        assert!((c.eval(0.5) - 0.5).abs() < 1e-12);
        assert!((c.eval(0.25) - (0.5 - 0.5 * (std::f64::consts::PI * 0.25).cos())).abs() < 1e-12);
    }

    #[test]
    fn unknown_autohide_curve_is_an_error() {
        let json =
            std::fs::read_to_string(crate::spec_dir().join(crate::theme::ANIMATION_FILE)).unwrap();
        let bad = json.replace("\"curve\": \"cosine_ease\"", "\"curve\": \"wobble\"");
        assert_ne!(json, bad);
        assert!(AnimationPresets::from_str(&bad).is_err());
    }

    #[test]
    fn minimize_effects() {
        let e = presets().minimize_effects().to_vec();
        assert_eq!(
            e,
            vec![
                MinimizeEffect::WarpMinimize,
                MinimizeEffect::Scale,
                MinimizeEffect::Suck
            ]
        );
    }

    #[test]
    fn rejects_incomplete_documents() {
        assert!(AnimationPresets::from_str("{}").is_err());
        let json =
            std::fs::read_to_string(crate::spec_dir().join(crate::theme::ANIMATION_FILE)).unwrap();
        let bad = json.replace("\"scale\"", "\"wobble\"");
        assert_ne!(json, bad);
        assert!(
            AnimationPresets::from_str(&bad).is_err(),
            "an unknown minimize effect is an error"
        );
        assert!(AnimationPresets::from_str("not json").is_err());
    }
}
