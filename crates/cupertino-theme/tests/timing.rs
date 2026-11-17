//! The timing-function table names the design's curves and its default spring.

use cupertino_theme::animation::{
    TimingPreset, DEFAULT_SPRING, EASE_CURVE, EASE_IN_CURVE, EASE_IN_OUT_CURVE, EASE_OUT_CURVE,
    LINEAR_CURVE,
};

fn points(name: &str) -> (f64, f64, f64, f64) {
    let b = TimingPreset::from_name(name)
        .and_then(TimingPreset::bezier)
        .expect("a bézier entry");
    (b.x1, b.y1, b.x2, b.y2)
}

#[test]
fn each_bezier_entry_is_its_named_curve() {
    assert_eq!(TimingPreset::Ease.bezier(), Some(EASE_CURVE));
    assert_eq!(TimingPreset::Linear.bezier(), Some(LINEAR_CURVE));
    assert_eq!(TimingPreset::EaseIn.bezier(), Some(EASE_IN_CURVE));
    assert_eq!(TimingPreset::EaseOut.bezier(), Some(EASE_OUT_CURVE));
    assert_eq!(TimingPreset::EaseInOut.bezier(), Some(EASE_IN_OUT_CURVE));
    assert_eq!(TimingPreset::Spring.bezier(), None);
}

#[test]
fn the_curves_have_their_control_points() {
    assert_eq!(points("ease"), (0.25, 0.1, 0.25, 1.0));
    assert_eq!(points("linear"), (0.0, 0.0, 1.0, 1.0));
    assert_eq!(points("ease-in"), (0.42, 0.0, 1.0, 1.0));
    assert_eq!(points("ease-out"), (0.0, 0.0, 0.58, 1.0));
    assert_eq!(points("ease-in-out"), (0.42, 0.0, 0.58, 1.0));
}

#[test]
fn the_spring_entry_is_the_default_spring() {
    let spring = TimingPreset::Spring.spring().expect("a spring entry");
    assert_eq!(spring, DEFAULT_SPRING);
    assert_eq!(
        (spring.mass, spring.stiffness, spring.damping),
        (1.0, 100.0, 10.0)
    );
    assert_eq!(spring.initial_velocity, 0.0);
    assert!(!spring.allows_overdamping);
    assert_eq!(TimingPreset::EaseOut.spring(), None);
}
