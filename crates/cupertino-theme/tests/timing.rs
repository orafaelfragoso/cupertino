//! The timing-function table names the design's curves and its default spring; a named motion reads out as the
//! curves that run it.

use cupertino_animation::{CubicBezier, Curve, LayerCurve, SpringParams};
use cupertino_theme::animation::{
    Easing, SpringPreset, Timing, TimingPreset, DEFAULT_SPRING, EASE_CURVE, EASE_IN_CURVE,
    EASE_IN_OUT_CURVE, EASE_OUT_CURVE, LINEAR_CURVE,
};

#[test]
fn each_entry_is_found_by_name_and_index_and_is_its_curve_or_the_default_spring() {
    for (preset, name, bezier) in [
        (TimingPreset::Ease, "ease", Some(EASE_CURVE)),
        (TimingPreset::Linear, "linear", Some(LINEAR_CURVE)),
        (TimingPreset::EaseIn, "ease-in", Some(EASE_IN_CURVE)),
        (TimingPreset::EaseOut, "ease-out", Some(EASE_OUT_CURVE)),
        (
            TimingPreset::EaseInOut,
            "ease-in-out",
            Some(EASE_IN_OUT_CURVE),
        ),
        (TimingPreset::Spring, "spring", None),
    ] {
        assert_eq!(TimingPreset::from_name(name), Some(preset), "{name}");
        assert_eq!(
            TimingPreset::from_index(preset.index()),
            Some(preset),
            "{name}"
        );
        assert_eq!(preset.bezier(), bezier, "{name}");
        let spring = bezier.is_none().then_some(DEFAULT_SPRING);
        assert_eq!(preset.spring(), spring, "{name}");
    }
    assert_eq!(TimingPreset::from_name("bounce"), None);
}

#[test]
fn a_named_motion_reads_out_as_the_curves_that_run_it() {
    let tau = std::f64::consts::TAU;
    for (timing, layer) in [
        (
            Easing::EASE_OUT.over(0.1),
            LayerCurve::Timed {
                duration: 0.1,
                bezier: EASE_OUT_CURVE,
            },
        ),
        (
            // the cosine ease as the Bézier fitted to it
            Easing::COSINE.over(0.3),
            LayerCurve::Timed {
                duration: 0.3,
                bezier: CubicBezier::new(0.364, 0.0, 0.636, 1.0),
            },
        ),
        (
            Timing::from(SpringPreset::LAUNCHER_OPEN),
            LayerCurve::Spring {
                response: 0.30,
                damping: 0.74,
            },
        ),
        (
            // a physical spring as its undamped period and its damping ratio
            Timing::from(SpringPreset::DEFAULT),
            LayerCurve::Spring {
                response: tau / 10.0,
                damping: 0.5,
            },
        ),
    ] {
        match (timing.layer_curve(), layer) {
            (
                LayerCurve::Spring { response, damping },
                LayerCurve::Spring {
                    response: r,
                    damping: d,
                },
            ) => assert!(
                (response - r).abs() < 1e-12 && (damping - d).abs() < 1e-12,
                "{timing:?}"
            ),
            (sent, layer) => assert_eq!(sent, layer, "{timing:?}"),
        }
        match (timing.curve(), layer) {
            (
                Curve::Timed { duration, bezier },
                LayerCurve::Timed {
                    duration: d,
                    bezier: b,
                },
            ) => assert_eq!((duration, bezier), (d, b), "{timing:?}"),
            (Curve::Spring(params), LayerCurve::Spring { .. }) => assert!(
                (Timing::from(SpringPreset::DEFAULT) == timing && params == DEFAULT_SPRING)
                    || params == SpringParams::from_ratio(0.30, 0.74),
                "{timing:?}"
            ),
            other => panic!("{timing:?} reads out as {other:?}"),
        }
        assert_eq!(timing.value(0.0), 0.0, "{timing:?} starts at 0");
        assert!(
            (timing.value(timing.duration()) - 1.0).abs() < 2e-3,
            "{timing:?} rests at 1"
        );
    }
}
