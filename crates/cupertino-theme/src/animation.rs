//! The motion catalogue: the design's named easings ([`Easing`]), springs ([`SpringPreset`]) and motions
//! ([`Timing`]), the presets loaded from the theme data (`animations.json`) and the design's timing-function table
//! ([`TimingPreset`]). A motion is named, never built from numbers outside this crate; a crate that evaluates its
//! numbers or sends them to the compositor reads them out.

use crate::error::{Error, Result};
use cupertino_animation::cosine_ease;
use cupertino_animation::{
    CubicBezier, Curve, LayerCurve, Momentum, RubberBand, Spring, SpringParams,
};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The cubic Bézier the catalogue gives for the cosine ease where a Bézier is required (the compositor's curves and
/// [`Curve`]): within 3·10⁻⁴ of the cosine everywhere.
const COSINE_EASE_FIT: CubicBezier = CubicBezier::new(0.364, 0.0, 0.636, 1.0);

/// A timing function of the design catalogue: a cubic Bézier of the timing table or of a named motion, or the cosine
/// S-curve (`0.5 − 0.5·cos(π·x)`), which is not a Bézier.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Easing(Shape);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Bezier(CubicBezier),
    Cosine,
}

impl Easing {
    /// The table's `ease` entry: a gentle start and a long settle.
    pub const EASE: Easing = Easing(Shape::Bezier(EASE_CURVE));
    /// The table's `linear` entry: constant speed from start to end.
    pub const LINEAR: Easing = Easing(Shape::Bezier(LINEAR_CURVE));
    /// The table's `ease-in` entry: starts slowly and ends at full speed.
    pub const EASE_IN: Easing = Easing(Shape::Bezier(EASE_IN_CURVE));
    /// The table's `ease-out` entry: starts at full speed and slows to a stop.
    pub const EASE_OUT: Easing = Easing(Shape::Bezier(EASE_OUT_CURVE));
    /// The table's `ease-in-out` entry: starts and ends slowly, symmetric about the midpoint.
    pub const EASE_IN_OUT: Easing = Easing(Shape::Bezier(EASE_IN_OUT_CURVE));
    /// The cosine S-curve `0.5 − 0.5·cos(π·x)`.
    pub const COSINE: Easing = Easing(Shape::Cosine);

    /// Eased progress for linear progress from 0 to 1.
    pub fn eval(&self, x: f64) -> f64 {
        match self.0 {
            Shape::Bezier(b) => b.eval(x),
            Shape::Cosine => cosine_ease(x),
        }
    }

    /// This easing over `seconds`.
    pub fn over(self, seconds: f64) -> Timing {
        Timing(Motion::Eased {
            easing: self,
            seconds,
        })
    }

    /// The control points, for a crate that sends or evaluates them; `None` for the cosine ease.
    pub fn as_bezier(&self) -> Option<CubicBezier> {
        match self.0 {
            Shape::Bezier(b) => Some(b),
            Shape::Cosine => None,
        }
    }

    /// The Bézier given where one is required: the cosine ease as [`COSINE_EASE_FIT`].
    fn bezier(self) -> CubicBezier {
        self.as_bezier().unwrap_or(COSINE_EASE_FIT)
    }
}

/// A spring of the design catalogue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringPreset(SpringForm);

#[derive(Clone, Copy, Debug, PartialEq)]
enum SpringForm {
    /// Mass, stiffness and damping.
    Physical(SpringParams),
    /// The period of the undamped oscillation in seconds and the damping ratio (1 critically damped, below 1
    /// bouncing), as the compositor's springs take them.
    Ratio { response: f64, damping: f64 },
}

impl SpringPreset {
    /// The design's default spring (the timing table's `spring` entry): mass 1, stiffness 100, damping 10.
    pub const DEFAULT: SpringPreset = SpringPreset(SpringForm::Physical(DEFAULT_SPRING));
    /// The control panel's fade-in: a critically damped rise at 28 radians per second, so its undamped period is a
    /// turn at that rate.
    pub const CONTROL_PANEL_OPEN: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: std::f64::consts::TAU / 28.0,
        damping: 1.0,
    });
    /// A detail view of the control panel growing out of its tile, and shrinking back into it: critically damped, its
    /// undamped period 0.35 s, so it never overshoots and rests after 0.6 s.
    pub const CONTROL_PANEL_DETAIL: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.35,
        damping: 1.0,
    });
    /// The launcher panel's scale as it opens, with a little bounce.
    pub const LAUNCHER_OPEN: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.30,
        damping: 0.74,
    });
    /// A popover growing out of its anchor.
    pub const POPOVER_OPEN: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.29,
        damping: 0.82,
    });

    /// The spring's numbers, for a crate that sends or evaluates them.
    pub fn params(&self) -> SpringParams {
        match self.0 {
            SpringForm::Physical(params) => params,
            SpringForm::Ratio { response, damping } => SpringParams::from_ratio(response, damping),
        }
    }

    /// The spring as the compositor takes it: its undamped period and its damping ratio.
    fn layer_curve(self) -> LayerCurve {
        match self.0 {
            SpringForm::Ratio { response, damping } => LayerCurve::Spring { response, damping },
            SpringForm::Physical(p) => LayerCurve::Spring {
                response: p.perceptual_duration(),
                damping: p.damping / (2.0 * (p.stiffness * p.mass).sqrt()),
            },
        }
    }
}

/// How a value moves: an easing over a duration, or a spring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing(Motion);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Motion {
    Eased { easing: Easing, seconds: f64 },
    Spring(SpringPreset),
}

impl From<SpringPreset> for Timing {
    fn from(spring: SpringPreset) -> Timing {
        Timing(Motion::Spring(spring))
    }
}

impl Timing {
    /// The control panel's fade-out over 0.18 s: it falls away quickly at first and slows towards the end, a cubic
    /// Bézier through the ends of an exponential fall, fitted to it within a two-thousandth everywhere.
    pub const CONTROL_PANEL_CLOSE: Timing = Timing(Motion::Eased {
        // control points in 256ths, as the wire carries them
        easing: Easing(Shape::Bezier(CubicBezier::new(
            0.234375, 0.7421875, 0.54296875, 0.92578125,
        ))),
        seconds: 0.18,
    });
    /// A detail view of the control panel and its tile cross-fading in place with Reduce Motion, in place of
    /// [`SpringPreset::CONTROL_PANEL_DETAIL`]: constant speed over 0.2 s.
    pub const CONTROL_PANEL_DETAIL_FADE: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.2,
    });
    /// The on-screen displays' fade after their hold: constant speed over 0.7 s, so the alpha falls as `1 - t / 0.7`.
    pub const OSD_FADE: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.7,
    });
    /// The input-source switcher's panel fading out once the switch ends: constant speed over 0.3 s, so the alpha falls
    /// as `1 - t / 0.3` (`cupertino_keyboard::switcher::fade_alpha`).
    pub const INPUT_SOURCES_FADE: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.3,
    });
    /// A popover shrinking into its anchor over 0.22 s: `1 - (1 - t)²` as a cubic Bézier.
    pub const POPOVER_CLOSE: Timing = Timing(Motion::Eased {
        easing: Easing(Shape::Bezier(CubicBezier::new(
            1.0 / 3.0,
            2.0 / 3.0,
            2.0 / 3.0,
            1.0,
        ))),
        seconds: 0.22,
    });
    /// A closing menu's fade over 0.236 s: `1 - (1 - t)³` as a cubic Bézier.
    pub const MENU_CLOSE: Timing = Timing(Motion::Eased {
        easing: Easing(Shape::Bezier(CubicBezier::new(
            1.0 / 3.0,
            1.0,
            2.0 / 3.0,
            1.0,
        ))),
        seconds: 0.236,
    });
    /// A chosen menu row's highlight going off before the choice is made: a step after 0.034 s, two frames at 60 Hz.
    pub const MENU_CHOICE_OFF: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.034,
    });
    /// The chosen row's highlight back on after [`Timing::MENU_CHOICE_OFF`]: a step after 0.067 s, four frames at
    /// 60 Hz, when the choice is made.
    pub const MENU_CHOICE_ON: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.067,
    });
    /// The desktop's rubber band fading in as a drag on the desktop starts: constant speed over 0.15 s.
    pub const DESKTOP_BAND_FADE_IN: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.15,
    });
    /// The desktop's rubber band fading out once it is let go: constant speed over 0.2 s.
    pub const DESKTOP_BAND_FADE_OUT: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.2,
    });
    /// The scrim behind an authentication or keyring prompt fading in as the prompt opens: `1 - (1 - t)³` over 0.2 s,
    /// as a cubic Bézier.
    pub const AUTHENTICATION_SCRIM_FADE_IN: Timing = Timing(Motion::Eased {
        easing: Easing(Shape::Bezier(CubicBezier::new(
            1.0 / 3.0,
            1.0,
            2.0 / 3.0,
            1.0,
        ))),
        seconds: 0.2,
    });

    /// Progress `seconds` after the start: 0 at the start, 1 at rest; a spring may pass 1.
    pub fn value(self, seconds: f64) -> f64 {
        match self.0 {
            Motion::Eased {
                easing,
                seconds: duration,
            } => easing.eval(if duration <= 0.0 {
                1.0
            } else {
                seconds / duration
            }),
            Motion::Spring(spring) => Spring::new(spring.params()).value(seconds),
        }
    }

    /// Seconds until rest: an easing's duration, a spring's settling time.
    pub fn duration(self) -> f64 {
        match self.0 {
            Motion::Eased { seconds, .. } => seconds,
            Motion::Spring(spring) => Spring::new(spring.params()).settling_duration(),
        }
    }

    /// The numbers, for a crate that evaluates them: the cosine ease as the catalogue's Bézier fit to it.
    pub fn curve(self) -> Curve {
        match self.0 {
            Motion::Eased { easing, seconds } => Curve::Timed {
                duration: seconds,
                bezier: easing.bezier(),
            },
            Motion::Spring(spring) => Curve::Spring(spring.params()),
        }
    }

    /// The numbers the compositor runs: the cosine ease as the catalogue's Bézier fit to it.
    pub fn layer_curve(self) -> LayerCurve {
        match self.0 {
            Motion::Eased { easing, seconds } => LayerCurve::Timed {
                duration: seconds,
                bezier: easing.bezier(),
            },
            Motion::Spring(spring) => spring.layer_curve(),
        }
    }
}

/// The motions of the launcher's panel.
impl SpringPreset {
    /// The panel's enclosure growing to a larger shape (a scope unfolding, the results lengthening): an undamped
    /// period of 0.27 s at a damping ratio of 0.83, so it overshoots by a few points.
    pub const LAUNCHER_GROW: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.27,
        damping: 0.83,
    });
    /// The panel's enclosure shrinking to a smaller shape (a scope folding into the search field): 0.295 s at 0.86.
    pub const LAUNCHER_SHRINK: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.295,
        damping: 0.86,
    });
    /// The search field's end drawing back as the mode buttons bud out of it: 0.305 s at 0.68.
    pub const LAUNCHER_FIELD: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.305,
        damping: 0.68,
    });
    /// The mode buttons sliding out of the search field to their places, carried a little past them: 0.575 s at 0.69.
    pub const LAUNCHER_SPLIT: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.575,
        damping: 0.69,
    });
    /// The enclosure's width coming back from its dip as the files scope opens: critically damped, 0.25 s.
    pub const LAUNCHER_DIP_BACK: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.25,
        damping: 1.0,
    });
}

/// The motions of the launcher's panel.
impl Timing {
    /// The panel fading in as it opens: constant speed over 0.1 s.
    pub const LAUNCHER_OPEN_FADE: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.10,
    });
    /// A scope's content fading out as it folds into the search field: constant speed over 0.05 s.
    pub const LAUNCHER_FOLD_FADE: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.05,
    });
    /// The applications browser's content fading in as it unfolds from the search field: ease-out over 0.12 s.
    pub const LAUNCHER_UNFOLD_CONTENT: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_OUT,
        seconds: 0.12,
    });
    /// The files scope's content fading in once its enclosure has grown: ease-out over 0.1 s.
    pub const LAUNCHER_FILES_CONTENT: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_OUT,
        seconds: 0.10,
    });
    /// The panel closing: it grows a little, its content blurs and it fades out, ease-out over 0.1 s.
    pub const LAUNCHER_CLOSE: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_OUT,
        seconds: 0.10,
    });
    /// The mode buttons' symbols sharpening after they bud: ease-out over 0.2 s.
    pub const LAUNCHER_GLYPH_SHARPEN: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_OUT,
        seconds: 0.2,
    });
    /// The mode buttons merging back into the search field as a scope opens: ease-out over 0.05 s.
    pub const LAUNCHER_MERGE: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_OUT,
        seconds: 0.05,
    });
    /// The enclosure's width dipping as the files scope opens, before [`SpringPreset::LAUNCHER_DIP_BACK`] brings it
    /// back: ease-in-out over 0.14 s.
    pub const LAUNCHER_DIP: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_IN_OUT,
        seconds: 0.14,
    });
    /// The applications browser's and the files scope's scroll indicator fading out after it lingers: constant speed
    /// over 0.25 s.
    pub const LAUNCHER_SCROLLER_FADE: Timing = Timing(Motion::Eased {
        easing: Easing::LINEAR,
        seconds: 0.25,
    });
}

/// The motions of the launcher's panel.
impl Easing {
    /// A mode button swelling from a bud to its size as it leaves the search field, fast at first and settling:
    /// `1 − (1 − t)²`, which this cubic Bézier is exactly.
    pub const LAUNCHER_BUD: Easing = Easing(Shape::Bezier(CubicBezier::new(
        1.0 / 3.0,
        2.0 / 3.0,
        2.0 / 3.0,
        1.0,
    )));
}

/// The motions of the overview.
impl SpringPreset {
    /// The windows moving into the overview and back to their places, and the chrome fading with them, whether a
    /// shortcut or a gesture's release drives them: critically damped, its undamped period 0.25 s (the overview's
    /// open and close spring in `cupertino_wm::overview`).
    pub const OVERVIEW_OPEN: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.25,
        damping: 1.0,
    });
    /// A window of the open overview moving to its new place when the set of windows or the desktops band changes:
    /// critically damped, 0.2 s (the reflow of `cupertino_wm::overview`).
    pub const OVERVIEW_REFLOW: SpringPreset = SpringPreset(SpringForm::Ratio {
        response: 0.2,
        damping: 1.0,
    });
}

/// The motions of the overview.
impl Timing {
    /// The overview fading in and out in place of its motion with Reduce Motion: ease-in-out over 0.2 s.
    pub const OVERVIEW_FADE: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_IN_OUT,
        seconds: 0.2,
    });
}

/// The motions of the notification banners and the notification list.
impl SpringPreset {
    /// A banner springing back to rest after a leftward swipe: mass 1, stiffness 100, damping 100, a damping ratio of
    /// 5 that the spring takes as critically damped (it allows no overdamping).
    pub const NOTIFICATION_SWIPE_BACK: SpringPreset =
        SpringPreset(SpringForm::Physical(SpringParams {
            mass: 1.0,
            stiffness: 100.0,
            damping: 100.0,
            ..DEFAULT_SPRING
        }));
    /// The notification list sliding in from the output's right edge and out again as one piece: critically damped
    /// at 21 radians per second (stiffness 21², damping 2·21), about 0.4 s to settle.
    pub const NOTIFICATION_LIST_SLIDE: SpringPreset =
        SpringPreset(SpringForm::Physical(SpringParams {
            mass: 1.0,
            stiffness: 441.0,
            damping: 42.0,
            ..DEFAULT_SPRING
        }));
    /// The cards below a cleared notification rising into its gap: critically damped at 21.75 radians per second
    /// (stiffness 21.75², damping 2·21.75).
    pub const NOTIFICATION_RISE: SpringPreset = SpringPreset(SpringForm::Physical(SpringParams {
        mass: 1.0,
        stiffness: 473.0625,
        damping: 43.5,
        ..DEFAULT_SPRING
    }));
    /// A notification group expanding into its cards and collapsing again: the perceptual spring of 0.35 s without
    /// bounce (stiffness (2π/0.35)², damping 4π/0.35), its numbers computed as [`SpringParams::from_perceptual`]
    /// computes them.
    pub const NOTIFICATION_GROUP_EXPAND: SpringPreset =
        SpringPreset(SpringForm::Physical(SpringParams {
            mass: 1.0,
            stiffness: (std::f64::consts::TAU * NOTIFICATION_GROUP_EXPAND_RATE)
                * (std::f64::consts::TAU * NOTIFICATION_GROUP_EXPAND_RATE),
            damping: 1.0 * 4.0 * std::f64::consts::PI * NOTIFICATION_GROUP_EXPAND_RATE,
            ..DEFAULT_SPRING
        }));
}

/// The reciprocal of the perceptual duration of [`SpringPreset::NOTIFICATION_GROUP_EXPAND`], 0.35 s.
const NOTIFICATION_GROUP_EXPAND_RATE: f64 = 1.0 / 0.35;

/// The motions of the notification banners and the notification list.
impl Timing {
    /// A banner sliding in from the output's right edge and out again, or fading in and out with Reduce Motion: the
    /// timing table's ease-in-out over 0.25 s, the default transaction's duration. A swiped banner leaves on the same
    /// curve, faster as the swipe was.
    pub const NOTIFICATION_BANNER: Timing = Timing(Motion::Eased {
        easing: Easing::EASE_IN_OUT,
        seconds: 0.25,
    });
    /// A cleared notification blurring, shrinking and fading away over 0.13 s, slow at first and quick at the end:
    /// the smoothstep `3t² − 2t³`, which this cubic Bézier is exactly.
    pub const NOTIFICATION_CARD_LEAVE: Timing = Timing(Motion::Eased {
        easing: Easing(Shape::Bezier(CubicBezier::new(
            1.0 / 3.0,
            0.0,
            2.0 / 3.0,
            1.0,
        ))),
        seconds: 0.13,
    });
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
                    } => SpringParams {
                        mass,
                        stiffness,
                        damping,
                        initial_velocity,
                        allows_overdamping: false,
                    },
                    RawSpring::Perceptual {
                        perceptual_duration,
                        bounce,
                    } => SpringParams::from_perceptual(perceptual_duration, bounce),
                };
                let preset = SpringPreset(SpringForm::Physical(preset));
                (name, preset)
            })
            .collect();
        let curve_name = raw.dock.autohide.curve.as_str();
        let autohide_curve = if curve_name == "cosine_ease" {
            if !raw.cosine_ease {
                return Err(Error::NotFound("cosine_ease".into()));
            }
            Easing::COSINE
        } else {
            Easing(Shape::Bezier(*timing.get(curve_name).ok_or_else(|| {
                Error::Invalid(format!(
                    "unknown easing {curve_name} in dock.autohide.curve"
                ))
            })?))
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
            return self.cosine_ease.then_some(Easing::COSINE);
        }
        self.timing(name).map(|b| Easing(Shape::Bezier(b)))
    }

    /// Spring by name: `reference_default`, `window_move_resize_snap`, `overview_toggle`.
    pub fn spring(&self, name: &str) -> Option<SpringParams> {
        self.springs.get(name).map(SpringPreset::params)
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
