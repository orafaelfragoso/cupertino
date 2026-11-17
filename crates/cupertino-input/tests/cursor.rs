//! A rapid back-and-forth or circular pointer motion enlarges the cursor so it can be found, and it shrinks back.

use cupertino_input::{
    clamp_cursor_scale, Point, ShakeDetector, CURSOR_SCALE_MAX, CURSOR_SCALE_MIN, SHAKE_SCALE_MAX,
};

/// Feed a horizontal shake: `reversals` sweeps of `width` points, one sample every `dt` seconds.
fn shake(d: &mut ShakeDetector, sweeps: usize, width: f64, dt: f64, start: f64) -> (bool, f64) {
    let mut t = start;
    let mut x = 0.0;
    let mut triggered = false;
    d.motion(Point::new(x, 300.0), t);
    for i in 0..sweeps {
        let target = if i % 2 == 0 { width } else { 0.0 };
        for step in 1..=3 {
            t += dt;
            x = x + (target - x) / f64::from(4 - step);
            triggered |= d.motion(Point::new(x, 300.0), t);
        }
    }
    (triggered, t)
}

#[test]
fn the_cursor_scale_is_clamped_to_its_range() {
    assert_eq!(clamp_cursor_scale(0.2), CURSOR_SCALE_MIN);
    assert_eq!(clamp_cursor_scale(2.5), 2.5);
    assert_eq!(clamp_cursor_scale(99.0), CURSOR_SCALE_MAX);
}

#[test]
#[should_panic(expected = "NaN")]
fn a_nan_cursor_scale_is_a_bug_in_the_caller() {
    clamp_cursor_scale(f64::NAN);
}

#[test]
fn a_new_detector_does_not_enlarge_the_cursor() {
    let d = ShakeDetector::new();
    assert_eq!(d.multiplier(), 1.0);
}

#[test]
fn straight_travel_is_not_a_shake() {
    let mut d = ShakeDetector::new();
    for i in 0..200 {
        assert!(!d.motion(Point::new(f64::from(i) * 8.0, 100.0), f64::from(i) * 0.008));
    }
    assert_eq!(d.evaluate(2.0), 1.0);
}

#[test]
fn a_fast_wide_shake_triggers_and_grows_the_cursor() {
    let mut d = ShakeDetector::new();
    let (triggered, t) = shake(&mut d, 14, 300.0, 0.01, 0.0);
    assert!(triggered);
    let m = d.evaluate(t);
    assert!(m > 1.0 && m <= SHAKE_SCALE_MAX, "{m}");
    assert_eq!(d.multiplier(), m);
}

#[test]
fn a_tiny_jitter_is_not_a_shake() {
    let mut d = ShakeDetector::new();
    let (triggered, _) = shake(&mut d, 30, 6.0, 0.01, 0.0);
    assert!(!triggered);
}

#[test]
fn slow_reversals_are_not_a_shake() {
    let mut d = ShakeDetector::new();
    let (triggered, _) = shake(&mut d, 14, 300.0, 0.1, 0.0);
    assert!(
        !triggered,
        "a reversal every half second is just moving the mouse"
    );
}

#[test]
fn the_enlargement_decays_once_the_shaking_stops_and_release_resets_it() {
    let mut d = ShakeDetector::new();
    let (_, t) = shake(&mut d, 16, 300.0, 0.01, 0.0);
    let grown = d.evaluate(t);
    assert!(grown > 1.0);
    let mut later = grown;
    let mut now = t;
    for _ in 0..60 {
        now += 1.0 / 60.0 + 0.1;
        later = d.evaluate(now);
    }
    assert_eq!(later, 1.0, "decayed back");
    let (_, t2) = shake(&mut d, 16, 300.0, 0.01, now + 1.0);
    assert!(d.evaluate(t2) > 1.0);
    d.release();
    assert_eq!(d.multiplier(), 1.0);
}

#[test]
fn a_detector_triggers_once_per_shake() {
    let mut d = ShakeDetector::new();
    let mut count = 0;
    let mut t = 0.0;
    for _ in 0..3 {
        let (triggered, end) = shake(&mut d, 14, 300.0, 0.01, t);
        count += usize::from(triggered);
        t = end + 0.0;
    }
    assert_eq!(count, 1);
}
