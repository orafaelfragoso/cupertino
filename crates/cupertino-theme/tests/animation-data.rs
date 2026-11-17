//! Parsing the animation presets: lookups, the values they feed, and the documents refused.

use cupertino_animation::SpringParams;
use cupertino_theme::animation::{
    Easing, MinimizeEffect, EASE_CURVE, EASE_IN_CURVE, EASE_IN_OUT_CURVE, EASE_OUT_CURVE,
    LINEAR_CURVE,
};
use cupertino_theme::{AnimationPresets, Error};

fn doc(curve: &str, cosine: bool, effects: &str) -> String {
    format!(
        r#"{{
        "timing_functions": {{
            "default": [0.25, 0.1, 0.25, 1.0],
            "linear": [0.0, 0.0, 1.0, 1.0],
            "easeIn": [0.42, 0.0, 1.0, 1.0],
            "easeOut": [0.0, 0.0, 0.58, 1.0],
            "easeInEaseOut": [0.42, 0.0, 0.58, 1.0]
        }},
        "cosine_ease": {cosine},
        "springs": {{
            "stiff": {{ "mass": 1.0, "stiffness": 100.0, "damping": 10.0, "initial_velocity": 0.0 }},
            "soft": {{ "perceptual_duration": 0.35, "bounce": 0.0 }}
        }},
        "settling_epsilon": 0.001,
        "scrolling": {{ "momentum_deceleration_per_ms": 0.998, "rubber_band_coefficient": 0.55 }},
        "dock": {{
            "fish_speed": 42.0, "ramp_while_dragging_ms": 270.0, "drop_speed": 350.0,
            "autohide": {{ "delay_s": 0.22, "duration_s": 0.3, "curve": "{curve}" }},
            "magnification_span": 3.0
        }},
        "minimize_effects": {effects}
    }}"#
    )
}

fn good() -> AnimationPresets {
    AnimationPresets::from_str(&doc("cosine_ease", true, r#"["warp", "scale", "suck"]"#))
        .expect("a valid document")
}

#[test]
fn timing_functions_and_easings_are_looked_up_by_name() {
    let p = good();
    for (name, curve) in [
        ("default", Some(EASE_CURVE)),
        ("linear", Some(LINEAR_CURVE)),
        ("easeIn", Some(EASE_IN_CURVE)),
        ("easeOut", Some(EASE_OUT_CURVE)),
        ("easeInEaseOut", Some(EASE_IN_OUT_CURVE)),
        ("missing", None),
    ] {
        assert_eq!(p.timing(name), curve, "{name}");
        assert_eq!(p.easing(name), curve.map(Easing::Bezier), "{name}");
    }
    let mut names: Vec<&str> = p.timing_names().collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["default", "easeIn", "easeInEaseOut", "easeOut", "linear"]
    );
    assert_eq!(p.easing("cosine_ease"), Some(Easing::CosineEase));
    let without = AnimationPresets::from_str(&doc("easeOut", false, "[]")).unwrap();
    assert_eq!(without.easing("cosine_ease"), None, "only with the flag");
}

#[test]
fn easings_evaluate_their_curve() {
    let cosine = |t: f64| 0.5 - 0.5 * (std::f64::consts::PI * t).cos();
    for (easing, t, expected) in [
        (Easing::CosineEase, 0.0, 0.0),
        (Easing::CosineEase, 0.25, cosine(0.25)),
        (Easing::CosineEase, 0.5, 0.5),
        (Easing::CosineEase, 1.0, 1.0),
        (Easing::Bezier(LINEAR_CURVE), 0.3, 0.3),
    ] {
        assert!(
            (easing.eval(t) - expected).abs() < 1e-6,
            "{easing:?} at {t}"
        );
    }
    assert_eq!(Easing::CosineEase.as_bezier(), None);
    assert_eq!(Easing::Bezier(LINEAR_CURVE).as_bezier(), Some(LINEAR_CURVE));
}

#[test]
fn springs_are_physical_or_perceptual_and_looked_up_by_name() {
    let p = good();
    let stiff = p.spring_preset("stiff").unwrap();
    assert_eq!(stiff.perceptual, None);
    assert_eq!(
        (
            stiff.params.mass,
            stiff.params.stiffness,
            stiff.params.damping,
            stiff.params.initial_velocity
        ),
        (1.0, 100.0, 10.0, 0.0)
    );
    assert!(!stiff.params.allows_overdamping);
    let soft = p.spring_preset("soft").unwrap();
    assert_eq!(soft.perceptual, Some((0.35, 0.0)));
    assert_eq!(soft.params, SpringParams::from_perceptual(0.35, 0.0));
    for (name, spring) in [
        ("stiff", Some(stiff.params)),
        ("soft", Some(soft.params)),
        ("missing", None),
    ] {
        assert_eq!(p.spring(name), spring, "{name}");
    }
    let mut names: Vec<&str> = p.spring_names().collect();
    names.sort_unstable();
    assert_eq!(names, ["soft", "stiff"]);
}

#[test]
fn the_dock_section_is_read_and_its_curve_names_an_easing() {
    let d = good().dock;
    assert_eq!(
        (
            d.fish_speed,
            d.ramp_while_dragging_ms,
            d.drop_speed,
            d.magnification_span,
            d.autohide_delay_s,
            d.autohide_duration_s
        ),
        (42.0, 270.0, 350.0, 3.0, 0.22, 0.3)
    );
    for (curve, cosine, easing) in [
        ("cosine_ease", true, Easing::CosineEase),
        ("easeOut", false, Easing::Bezier(EASE_OUT_CURVE)),
    ] {
        let p = AnimationPresets::from_str(&doc(curve, cosine, "[]")).unwrap();
        assert_eq!(p.dock.autohide_curve, easing, "{curve}");
    }
}

#[test]
fn minimize_effects_keep_their_order() {
    use MinimizeEffect::*;
    for (json, effects) in [
        (
            r#"["warp", "scale", "suck"]"#,
            vec![WarpMinimize, Scale, Suck],
        ),
        (r#"["suck"]"#, vec![Suck]),
        ("[]", vec![]),
    ] {
        let p = AnimationPresets::from_str(&doc("linear", false, json)).unwrap();
        assert_eq!(p.minimize_effects(), effects.as_slice(), "{json}");
    }
}

#[test]
fn the_scrolling_constants_feed_momentum_and_the_rubber_band() {
    let p = good();
    assert_eq!(p.settling_epsilon, 0.001);
    let m = p.momentum(10.0, 500.0, 0.0, 100.0);
    assert_eq!((m.factor_per_ms, m.origin), (0.998, 10.0));
    let r = p.rubber_band(200.0);
    assert_eq!((r.coefficient, r.dimension), (0.55, 200.0));
}

#[test]
fn a_document_that_cannot_be_used_is_refused_with_its_kind_of_error() {
    for (label, json, check) in [
        (
            "unknown autohide curve",
            doc("bounce", true, "[]"),
            (|e: &Error| matches!(e, Error::Invalid(m) if m.contains("bounce")))
                as fn(&Error) -> bool,
        ),
        (
            "cosine curve without the cosine flag",
            doc("cosine_ease", false, "[]"),
            |e| matches!(e, Error::NotFound(m) if m == "cosine_ease"),
        ),
        (
            "unknown minimize effect",
            doc("linear", false, r#"["spin"]"#),
            |e| matches!(e, Error::Invalid(m) if m.contains("spin")),
        ),
        ("malformed JSON", "{ not json".to_string(), |e| {
            matches!(e, Error::Json(_))
        }),
        ("a missing section", "{}".to_string(), |e| {
            matches!(e, Error::Json(_))
        }),
    ] {
        let e = AnimationPresets::from_str(&json).unwrap_err();
        assert!(check(&e), "{label}: {e}");
    }
}
