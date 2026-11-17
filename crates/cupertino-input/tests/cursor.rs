//! A rapid back-and-forth or circular pointer motion enlarges the cursor so it can be found, and it shrinks back.

use cupertino_input::{
    clamp_cursor_scale, Point, ShakeDetector, CURSOR_SCALE_MAX, CURSOR_SCALE_MIN, SHAKE_SCALE_MAX,
};

/// Feed a horizontal shake: `sweeps` sweeps of `width` points, one sample every `dt` seconds.
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

/// Feed `samples` one every 10 ms; true when any motion reported a shake.
fn feed(d: &mut ShakeDetector, samples: &[(f64, f64)]) -> bool {
    let mut triggered = false;
    for (i, &(x, y)) in samples.iter().enumerate() {
        triggered |= d.motion(Point::new(x, y), i as f64 * 0.01);
    }
    triggered
}

#[test]
fn the_cursor_scale_is_clamped_to_its_range() {
    for (scale, clamped) in [
        (0.2, CURSOR_SCALE_MIN),
        (2.5, 2.5),
        (99.0, CURSOR_SCALE_MAX),
    ] {
        assert_eq!(clamp_cursor_scale(scale), clamped, "{scale}");
    }
}

#[test]
#[should_panic(expected = "NaN")]
fn a_nan_cursor_scale_is_a_bug_in_the_caller() {
    clamp_cursor_scale(f64::NAN);
}

#[test]
fn only_a_fast_wide_shake_grows_the_cursor() {
    let straight: Vec<(f64, f64)> = (0..200).map(|i| (f64::from(i) * 8.0, 100.0)).collect();
    // Every slope between reversal points is infinite, so the line test fails; the points alternate
    // between two quadrants around the centroid outside the radius, which the circle test counts.
    let vertical: Vec<(f64, f64)> = (0..60)
        .map(|i| (100.0, if i % 2 == 0 { 0.0 } else { 300.0 }))
        .collect();
    // Every reversal happens at one position: the slopes are 0 / 0 and no point is outside the radius.
    let mut coincident = vec![(0.0, 300.0), (10.0, 300.0)];
    for i in 0..40 {
        coincident.push((0.0, 300.0));
        coincident.push((if i % 2 == 0 { -5.0 } else { 5.0 }, 300.0));
    }
    for (name, samples, shakes) in [
        ("straight travel", straight, false),
        ("an exactly vertical shake", vertical, true),
        ("coincident reversal points", coincident, false),
    ] {
        let mut d = ShakeDetector::new();
        assert_eq!(feed(&mut d, &samples), shakes, "{name}");
        assert_eq!(d.multiplier() > 1.0, shakes, "{name}");
    }
    for (name, sweeps, width, dt, shakes) in [
        ("fast and wide", 14, 300.0, 0.01, true),
        ("a tiny jitter", 30, 6.0, 0.01, false),
        ("a reversal every half second", 14, 300.0, 0.1, false),
    ] {
        let mut d = ShakeDetector::new();
        assert_eq!(d.multiplier(), 1.0, "a new detector");
        let (triggered, t) = shake(&mut d, sweeps, width, dt, 0.0);
        assert_eq!(triggered, shakes, "{name}");
        let m = d.evaluate(t);
        assert!(m >= 1.0 && m <= SHAKE_SCALE_MAX, "{name}: {m}");
        if shakes {
            assert!(m > 1.0, "{name}: {m}");
        }
        assert_eq!(d.multiplier(), m, "{name}");
    }
}

#[test]
fn the_enlargement_decays_once_the_shaking_stops_and_release_resets_it() {
    let mut d = ShakeDetector::new();
    let (_, t) = shake(&mut d, 16, 300.0, 0.01, 0.0);
    assert!(d.evaluate(t) > 1.0);
    let mut later = 0.0;
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
        t = end;
    }
    assert_eq!(count, 1);
}
