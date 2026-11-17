//! Cubic-Bézier timing functions and the cosine ease.

use cupertino_animation::{cosine_ease, CubicBezier};

const EASE: CubicBezier = CubicBezier::new(0.25, 0.1, 0.25, 1.0);
const EASE_IN_OUT: CubicBezier = CubicBezier::new(0.42, 0.0, 0.58, 1.0);
const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

#[test]
fn a_curve_keeps_its_control_points() {
    let c = CubicBezier::new(0.1, 0.2, 0.3, 0.4);
    assert_eq!((c.x1, c.y1, c.x2, c.y2), (0.1, 0.2, 0.3, 0.4));
}

#[test]
fn every_curve_starts_at_zero_and_ends_at_one() {
    for c in [
        EASE,
        EASE_IN_OUT,
        LINEAR,
        CubicBezier::new(0.3, -0.5, 0.7, 1.5),
    ] {
        assert_eq!(c.eval(0.0), 0.0);
        assert_eq!(c.eval(1.0), 1.0);
    }
}

#[test]
fn input_outside_the_unit_interval_is_clamped() {
    assert_eq!(EASE.eval(-3.0), 0.0);
    assert_eq!(EASE.eval(7.0), 1.0);
}

#[test]
fn the_linear_curve_is_the_identity() {
    for i in 0..=20 {
        let x = f64::from(i) / 20.0;
        assert!((LINEAR.eval(x) - x).abs() < 1e-6, "x = {x}");
    }
}

#[test]
fn the_symmetric_ease_in_out_passes_through_the_centre() {
    assert!((EASE_IN_OUT.eval(0.5) - 0.5).abs() < 1e-6);
    for i in 1..10 {
        let x = f64::from(i) / 20.0;
        assert!((EASE_IN_OUT.eval(x) + EASE_IN_OUT.eval(1.0 - x) - 1.0).abs() < 1e-6);
    }
}

#[test]
fn the_ease_curve_is_monotonic() {
    let mut last = 0.0;
    for i in 1..=200 {
        let v = EASE.eval(f64::from(i) / 200.0);
        assert!(v >= last, "decreased at step {i}");
        last = v;
    }
}

#[test]
fn the_ease_curve_leads_a_linear_one_early_on() {
    assert!(EASE.eval(0.25) > 0.25);
}

#[test]
fn the_ease_in_out_curve_lags_early_and_leads_late() {
    assert!(EASE_IN_OUT.eval(0.2) < 0.2);
    assert!(EASE_IN_OUT.eval(0.8) > 0.8);
}

#[test]
fn control_points_beyond_the_unit_square_overshoot() {
    let c = CubicBezier::new(0.3, 0.0, 0.7, 1.5);
    let peak = (1..100)
        .map(|i| c.eval(f64::from(i) / 100.0))
        .fold(0.0, f64::max);
    assert!(peak > 1.0);
}

#[test]
fn a_known_ease_value_is_reproduced() {
    // x = 0.5 on the ease curve: y is about 0.8024
    assert!((EASE.eval(0.5) - 0.8024).abs() < 1e-3);
}

#[test]
fn cosine_ease_runs_from_zero_to_one_through_the_middle() {
    assert_eq!(cosine_ease(0.0), 0.0);
    assert!((cosine_ease(0.5) - 0.5).abs() < 1e-12);
    assert!((cosine_ease(1.0) - 1.0).abs() < 1e-12);
}

#[test]
fn cosine_ease_clamps_its_input_and_is_monotonic() {
    assert_eq!(cosine_ease(-1.0), 0.0);
    assert!((cosine_ease(2.0) - 1.0).abs() < 1e-12);
    let mut last = 0.0;
    for i in 1..=100 {
        let v = cosine_ease(f64::from(i) / 100.0);
        assert!(v >= last);
        last = v;
    }
}
