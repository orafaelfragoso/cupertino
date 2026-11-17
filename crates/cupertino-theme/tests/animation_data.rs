//! Parsing the animation presets: values, lookups and each error.

use cupertino_theme::animation::{
    Easing, MinimizeEffect, EASE_CURVE, EASE_IN_CURVE, EASE_IN_OUT_CURVE, EASE_OUT_CURVE,
    LINEAR_CURVE,
};
use cupertino_theme::{AnimationPresets, Error};

fn doc(curve: &str, cosine: bool, effects: &str, spring: &str) -> String {
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
        "springs": {{ {spring} }},
        "settling_epsilon": 0.001,
        "scrolling": {{ "momentum_deceleration_per_ms": 0.998, "rubber_band_coefficient": 0.55 }},
        "dock": {{
            "fish_speed": 42.0, "ramp_while_dragging_ms": 270.0, "drop_speed": 350.0,
            "autohide": {{ "delay_s": 0.22, "duration_s": 0.22, "curve": "{curve}" }},
            "magnification_span": 3.0
        }},
        "minimize_effects": {effects}
    }}"#
    )
}

const SPRINGS: &str = r#"
    "stiff": { "mass": 1.0, "stiffness": 100.0, "damping": 10.0, "initial_velocity": 0.0 },
    "soft": { "perceptual_duration": 0.35, "bounce": 0.0 }"#;

fn good() -> AnimationPresets {
    AnimationPresets::from_str(&doc(
        "cosine_ease",
        true,
        r#"["warp", "scale", "suck"]"#,
        SPRINGS,
    ))
    .expect("a valid document")
}

#[test]
fn timing_functions_are_read_by_name() {
    let p = good();
    assert_eq!(p.timing("default"), Some(EASE_CURVE));
    assert_eq!(p.timing("linear"), Some(LINEAR_CURVE));
    assert_eq!(p.timing("easeIn"), Some(EASE_IN_CURVE));
    assert_eq!(p.timing("easeOut"), Some(EASE_OUT_CURVE));
    assert_eq!(p.timing("easeInEaseOut"), Some(EASE_IN_OUT_CURVE));
    assert_eq!(p.timing("missing"), None);
}

#[test]
fn timing_names_are_listed() {
    let p = good();
    let mut names: Vec<&str> = p.timing_names().collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["default", "easeIn", "easeInEaseOut", "easeOut", "linear"]
    );
}

#[test]
fn the_cosine_ease_is_a_named_easing_when_enabled() {
    let p = good();
    assert_eq!(p.easing("cosine_ease"), Some(Easing::CosineEase));
    assert_eq!(p.easing("linear"), Some(Easing::Bezier(LINEAR_CURVE)));
    assert_eq!(p.easing("nope"), None);
}

#[test]
fn the_cosine_ease_is_an_s_curve_and_not_a_bezier() {
    let e = Easing::CosineEase;
    assert!(e.eval(0.0).abs() < 1e-12);
    assert!((e.eval(0.5) - 0.5).abs() < 1e-12);
    assert!((e.eval(1.0) - 1.0).abs() < 1e-12);
    assert!((e.eval(0.25) - (0.5 - 0.5 * (std::f64::consts::PI * 0.25).cos())).abs() < 1e-12);
    assert_eq!(e.as_bezier(), None);
    assert_eq!(Easing::Bezier(LINEAR_CURVE).as_bezier(), Some(LINEAR_CURVE));
}

#[test]
fn a_bezier_easing_evaluates_its_curve() {
    let e = Easing::Bezier(LINEAR_CURVE);
    assert!((e.eval(0.3) - 0.3).abs() < 1e-6);
}

#[test]
fn physical_springs_keep_their_constants() {
    let s = good().spring("stiff").unwrap();
    assert_eq!(
        (s.mass, s.stiffness, s.damping, s.initial_velocity),
        (1.0, 100.0, 10.0, 0.0)
    );
    assert!(!s.allows_overdamping);
    assert_eq!(good().spring_preset("stiff").unwrap().perceptual, None);
}

#[test]
fn perceptual_springs_keep_their_duration_and_bounce() {
    let p = good();
    let preset = p.spring_preset("soft").unwrap();
    assert_eq!(preset.perceptual, Some((0.35, 0.0)));
    assert_eq!(
        preset.params,
        cupertino_animation::SpringParams::from_perceptual(0.35, 0.0)
    );
    assert_eq!(p.spring("soft"), Some(preset.params));
    assert_eq!(p.spring("missing"), None);
}

#[test]
fn spring_names_are_listed() {
    let p = good();
    let mut names: Vec<&str> = p.spring_names().collect();
    names.sort_unstable();
    assert_eq!(names, vec!["soft", "stiff"]);
}

#[test]
fn the_dock_numbers_are_read() {
    let d = good().dock;
    assert_eq!(
        (
            d.fish_speed,
            d.ramp_while_dragging_ms,
            d.drop_speed,
            d.magnification_span
        ),
        (42.0, 270.0, 350.0, 3.0)
    );
    assert_eq!((d.autohide_delay_s, d.autohide_duration_s), (0.22, 0.22));
    assert_eq!(d.autohide_curve, Easing::CosineEase);
}

#[test]
fn the_autohide_curve_may_name_a_timing_function() {
    let p = AnimationPresets::from_str(&doc("easeOut", false, "[]", SPRINGS)).unwrap();
    assert_eq!(p.dock.autohide_curve, Easing::Bezier(EASE_OUT_CURVE));
    assert_eq!(p.easing("cosine_ease"), None);
}

#[test]
fn minimize_effects_keep_their_order() {
    assert_eq!(
        good().minimize_effects(),
        &[
            MinimizeEffect::WarpMinimize,
            MinimizeEffect::Scale,
            MinimizeEffect::Suck
        ]
    );
    let p = AnimationPresets::from_str(&doc("linear", false, r#"["suck"]"#, SPRINGS)).unwrap();
    assert_eq!(p.minimize_effects(), &[MinimizeEffect::Suck]);
}

#[test]
fn the_scrolling_constants_feed_momentum_and_the_rubber_band() {
    let p = good();
    assert_eq!(p.settling_epsilon, 0.001);
    assert_eq!(p.momentum_deceleration_per_ms, 0.998);
    assert_eq!(p.rubber_band_coefficient, 0.55);
    let m = p.momentum(10.0, 500.0, 0.0, 100.0);
    assert_eq!(m.factor_per_ms, 0.998);
    assert_eq!(m.origin, 10.0);
    let r = p.rubber_band(200.0);
    assert_eq!(r.coefficient, 0.55);
    assert_eq!(r.dimension, 200.0);
}

#[test]
fn an_unknown_autohide_curve_is_invalid() {
    let e = AnimationPresets::from_str(&doc("bounce", true, "[]", SPRINGS)).unwrap_err();
    assert!(
        matches!(&e, Error::Invalid(m) if m.contains("bounce")),
        "{e}"
    );
}

#[test]
fn the_cosine_curve_needs_the_cosine_flag() {
    let e = AnimationPresets::from_str(&doc("cosine_ease", false, "[]", SPRINGS)).unwrap_err();
    assert!(
        matches!(&e, Error::NotFound(m) if m == "cosine_ease"),
        "{e}"
    );
}

#[test]
fn an_unknown_minimize_effect_is_invalid() {
    let e = AnimationPresets::from_str(&doc("linear", false, r#"["spin"]"#, SPRINGS)).unwrap_err();
    assert!(matches!(&e, Error::Invalid(m) if m.contains("spin")), "{e}");
}

#[test]
fn malformed_json_is_a_json_error() {
    let e = AnimationPresets::from_str("{ not json").unwrap_err();
    assert!(matches!(e, Error::Json(_)));
    assert!(e.to_string().starts_with("invalid specification JSON"));
}

#[test]
fn a_missing_section_is_a_json_error() {
    let e = AnimationPresets::from_str("{}").unwrap_err();
    assert!(matches!(e, Error::Json(_)));
}

#[test]
fn error_messages_say_what_went_wrong() {
    assert_eq!(Error::NotFound("x".into()).to_string(), "not found: x");
    assert_eq!(
        Error::Invalid("y".into()).to_string(),
        "invalid specification data: y"
    );
}
