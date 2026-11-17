//! A mouse's counts are turned into pointer motion by its resolution and recent speed.

use cupertino_input::{
    parse_mouse_dpi, resolve_dpi, AccelCurve, MouseAccel, ParametricCurve, DEFAULT_MOUSE_DPI,
    MOUSE_CURVES,
};

fn curve() -> AccelCurve {
    AccelCurve::Parametric(ParametricCurve::new(&MOUSE_CURVES, 0.6875).unwrap())
}

#[test]
fn the_marked_resolution_wins_and_otherwise_the_first() {
    assert_eq!(
        parse_mouse_dpi("400, 800"),
        Some(800.0),
        "an entry with a comma is not a number"
    );
    assert_eq!(parse_mouse_dpi("400 800 1600"), Some(400.0));
    assert_eq!(parse_mouse_dpi("400 *800 1600"), Some(800.0));
    assert_eq!(parse_mouse_dpi("400@125 *1600@1000"), Some(1600.0));
    assert_eq!(parse_mouse_dpi("*1200"), Some(1200.0));
}

#[test]
fn unusable_entries_are_skipped() {
    assert_eq!(parse_mouse_dpi(""), None);
    assert_eq!(parse_mouse_dpi("fast slow"), None);
    assert_eq!(parse_mouse_dpi("0 -400 nan inf 800"), Some(800.0));
    assert_eq!(parse_mouse_dpi("*0 400"), Some(400.0));
}

#[test]
fn a_stored_resolution_beats_the_hardware_database() {
    assert_eq!(resolve_dpi(Some(1600), Some("*800")), 1600.0);
    assert_eq!(resolve_dpi(None, Some("*800")), 800.0);
    assert_eq!(resolve_dpi(Some(0), Some("*800")), 800.0);
    assert_eq!(resolve_dpi(None, Some("garbage")), DEFAULT_MOUSE_DPI);
    assert_eq!(resolve_dpi(None, None), DEFAULT_MOUSE_DPI);
}

#[test]
fn without_a_curve_motion_passes_through() {
    let mut m = MouseAccel::new(None, 1000.0);
    assert_eq!(m.accelerate(3.0, -4.0, 1_000), (3.0, -4.0));
    let mut off = MouseAccel::linear(-1.0, 1000.0);
    assert_eq!(off.accelerate(3.0, -4.0, 1_000), (3.0, -4.0));
}

#[test]
fn linear_gain_is_normalised_to_the_default_resolution() {
    let mut same = MouseAccel::linear(1.0, DEFAULT_MOUSE_DPI);
    assert_eq!(same.accelerate(10.0, 5.0, 0), (10.0, 5.0));
    let mut high = MouseAccel::linear(1.0, 2.0 * DEFAULT_MOUSE_DPI);
    assert_eq!(
        high.accelerate(10.0, 5.0, 0),
        (5.0, 2.5),
        "a sharper mouse moves less per count"
    );
    let mut zero = MouseAccel::linear(0.0, DEFAULT_MOUSE_DPI);
    let (x, _) = zero.accelerate(10.0, 0.0, 0);
    assert!((x - 1.0).abs() < 1e-9);
}

#[test]
fn an_invalid_resolution_is_the_default() {
    let mut a = MouseAccel::linear(1.0, 0.0);
    let mut b = MouseAccel::linear(1.0, DEFAULT_MOUSE_DPI);
    assert_eq!(a.accelerate(7.0, 0.0, 0), b.accelerate(7.0, 0.0, 0));
}

#[test]
fn faster_hand_speed_moves_the_pointer_further_per_count() {
    let mut slow = MouseAccel::new(Some(curve()), 1000.0);
    let mut fast = MouseAccel::new(Some(curve()), 1000.0);
    let mut slow_gain = 0.0;
    let mut fast_gain = 0.0;
    for i in 0..40 {
        slow_gain = slow.accelerate(1.0, 0.0, i * 8_000).0;
        fast_gain = fast.accelerate(30.0, 0.0, i * 8_000).0 / 30.0;
    }
    assert!(fast_gain > slow_gain, "{fast_gain} vs {slow_gain}");
}

#[test]
fn a_pause_forgets_the_speed() {
    let mut m = MouseAccel::new(Some(curve()), 1000.0);
    for i in 0..30 {
        m.accelerate(40.0, 0.0, i * 8_000);
    }
    let after_pause = m.accelerate(2.0, 0.0, 30 * 8_000 + 8_000 + 400_000).0 / 2.0;
    let mut fresh = MouseAccel::new(Some(curve()), 1000.0);
    let first = fresh.accelerate(2.0, 0.0, 0).0 / 2.0;
    assert!((after_pause - first).abs() < 1e-9, "a pause starts over");
}

#[test]
fn the_direction_is_kept_and_zero_motion_stays_zero() {
    let mut m = MouseAccel::new(Some(curve()), 1000.0);
    let (x, y) = m.accelerate(-6.0, 8.0, 0);
    assert!(x < 0.0 && y > 0.0);
    assert!((x / y + 0.75).abs() < 1e-9);
    assert_eq!(m.accelerate(0.0, 0.0, 8_000), (0.0, 0.0));
}

#[test]
fn time_running_backwards_counts_as_no_time() {
    let mut m = MouseAccel::new(Some(curve()), 1000.0);
    m.accelerate(5.0, 0.0, 100_000);
    let (x, y) = m.accelerate(5.0, 0.0, 50_000);
    assert!(x.is_finite() && x > 0.0 && y == 0.0);
}

#[test]
fn reports_with_one_timestamp_keep_the_speed_of_the_last() {
    let mut m = MouseAccel::new(Some(curve()), 1000.0);
    let first = m.accelerate(5.0, 0.0, 10_000).0;
    let batch = m.accelerate(5.0, 0.0, 10_000).0;
    assert!(first.is_finite() && batch.is_finite() && batch > 0.0);
}
