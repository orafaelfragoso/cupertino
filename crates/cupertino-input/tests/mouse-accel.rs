//! A mouse's counts are turned into pointer motion by its resolution and recent speed.

use cupertino_input::{
    parse_mouse_dpi, resolve_dpi, AccelCurve, MouseAccel, ParametricCurve, DEFAULT_MOUSE_DPI,
    MOUSE_CURVES,
};

fn curve() -> AccelCurve {
    AccelCurve::Parametric(ParametricCurve::new(&MOUSE_CURVES, 0.6875).unwrap())
}

#[test]
fn the_marked_resolution_wins_and_otherwise_the_first_usable_one() {
    for (entry, dpi) in [
        ("400, 800", Some(800.0)),
        ("400 800 1600", Some(400.0)),
        ("400 *800 1600", Some(800.0)),
        ("400@125 *1600@1000", Some(1600.0)),
        ("*1200", Some(1200.0)),
        ("", None),
        ("fast slow", None),
        ("0 -400 nan inf 800", Some(800.0)),
        ("*0 400", Some(400.0)),
    ] {
        assert_eq!(parse_mouse_dpi(entry), dpi, "{entry:?}");
    }
}

#[test]
fn a_stored_resolution_beats_the_hardware_database() {
    for (stored, database, dpi) in [
        (Some(1600), Some("*800"), 1600.0),
        (None, Some("*800"), 800.0),
        (Some(0), Some("*800"), 800.0),
        (None, Some("garbage"), DEFAULT_MOUSE_DPI),
        (None, None, DEFAULT_MOUSE_DPI),
    ] {
        assert_eq!(
            resolve_dpi(stored, database),
            dpi,
            "{stored:?} {database:?}"
        );
    }
}

#[test]
fn without_a_curve_the_gain_is_linear_and_normalised_to_the_default_resolution() {
    for (name, mut accel, counts, motion) in [
        (
            "no curve",
            MouseAccel::new(None, 1000.0),
            (3.0, -4.0),
            (3.0, -4.0),
        ),
        (
            "linear off",
            MouseAccel::linear(-1.0, 1000.0),
            (3.0, -4.0),
            (3.0, -4.0),
        ),
        (
            "the default resolution",
            MouseAccel::linear(1.0, DEFAULT_MOUSE_DPI),
            (10.0, 5.0),
            (10.0, 5.0),
        ),
        (
            "a sharper mouse moves less per count",
            MouseAccel::linear(1.0, 2.0 * DEFAULT_MOUSE_DPI),
            (10.0, 5.0),
            (5.0, 2.5),
        ),
        (
            "an invalid resolution is the default",
            MouseAccel::linear(1.0, 0.0),
            (7.0, 0.0),
            (7.0, 0.0),
        ),
    ] {
        assert_eq!(accel.accelerate(counts.0, counts.1, 0), motion, "{name}");
    }
    let (x, _) = MouseAccel::linear(0.0, DEFAULT_MOUSE_DPI).accelerate(10.0, 0.0, 0);
    assert!((x - 1.0).abs() < 1e-9, "a zero gain is the minimum");
}

#[test]
fn faster_hand_speed_moves_the_pointer_further_per_count_and_a_pause_forgets_it() {
    let mut slow = MouseAccel::new(Some(curve()), 1000.0);
    let mut fast = MouseAccel::new(Some(curve()), 1000.0);
    let mut slow_gain = 0.0;
    let mut fast_gain = 0.0;
    for i in 0..40 {
        slow_gain = slow.accelerate(1.0, 0.0, i * 8_000).0;
        fast_gain = fast.accelerate(30.0, 0.0, i * 8_000).0 / 30.0;
    }
    assert!(fast_gain > slow_gain, "{fast_gain} vs {slow_gain}");

    let after_pause = fast.accelerate(2.0, 0.0, 40 * 8_000 + 400_000).0 / 2.0;
    let first = MouseAccel::new(Some(curve()), 1000.0)
        .accelerate(2.0, 0.0, 0)
        .0
        / 2.0;
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
fn reports_at_one_time_or_back_in_time_still_move_the_pointer() {
    for (name, second) in [
        ("the same timestamp", 10_000),
        ("time running backwards", 5_000),
    ] {
        let mut m = MouseAccel::new(Some(curve()), 1000.0);
        m.accelerate(5.0, 0.0, 10_000);
        let (x, y) = m.accelerate(5.0, 0.0, second);
        assert!(x.is_finite() && x > 0.0 && y == 0.0, "{name}");
    }
}
