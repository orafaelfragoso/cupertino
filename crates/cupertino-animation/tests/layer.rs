//! Animated layer properties.

use std::sync::Arc;

use cupertino_animation::{CubicBezier, LayerCurve, LayerProperty};

const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

fn linear(duration: f64) -> LayerCurve {
    LayerCurve::Timed {
        duration,
        bezier: LINEAR,
    }
}

fn spring(response: f64, damping: f64) -> LayerCurve {
    LayerCurve::Spring { response, damping }
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

#[test]
fn a_new_property_rests_at_its_value() {
    let p = LayerProperty::new(3.0);
    assert_eq!(p.value(0.0), 3.0);
    assert_eq!(p.target(), 3.0);
    assert_eq!(p.velocity(0.0), 0.0);
    assert_eq!(p.end(), None);
    assert!(!p.is_animating(0.0));
}

#[test]
fn a_timed_animation_runs_from_its_start_to_its_end() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(1.0), 2.0, 0.0);
    assert_eq!(p.target(), 10.0);
    assert_eq!(p.end(), Some(3.0));
    for (t, expected) in [(2.0, 0.0), (2.5, 5.0), (3.0, 10.0), (30.0, 10.0)] {
        assert!(close(p.value(t), expected, 1e-6), "t = {t}");
    }
    // one that has not started yet counts as running
    for (t, running) in [(1.0, true), (2.5, true), (3.0, false)] {
        assert_eq!(p.is_animating(t), running, "t = {t}");
    }

    let mut jump = LayerProperty::new(0.0);
    jump.animate(10.0, linear(0.0), 1.0, 0.0);
    assert_eq!(jump.value(1.0), 10.0, "a zero duration jumps");
    assert!(!jump.is_animating(1.0));
}

#[test]
fn a_timed_animation_from_rest_follows_the_curves_own_slope() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(1.0), 0.0, 0.0);
    assert!(close(p.velocity(0.5), 10.0, 1e-3));
    assert_eq!(p.velocity(1.0), 0.0);
    assert_eq!(p.velocity(-1.0), 0.0);
}

#[test]
fn an_animation_from_rest_starts_with_the_given_velocity() {
    let mut timed = LayerProperty::new(0.0);
    timed.animate(10.0, linear(1.0), 0.0, 50.0);
    assert!(close(timed.velocity(1e-4), 50.0, 0.5));
    assert!(close(timed.value(1.0), 10.0, 1e-9));

    let mut sprung = LayerProperty::new(0.0);
    sprung.animate(0.0, spring(0.5, 0.5), 0.0, 40.0);
    assert!(close(sprung.velocity(0.0), 40.0, 1e-6));
    let moved = (1..100)
        .map(|i| sprung.value(f64::from(i) * 0.005).abs())
        .fold(0.0, f64::max);
    assert!(moved > 0.1);
    assert_eq!(sprung.value(sprung.end().unwrap()), 0.0);
}

#[test]
fn a_retarget_keeps_value_and_velocity_continuous() {
    for (row, next) in [("timed", linear(1.0)), ("spring", spring(0.5, 1.0))] {
        let mut p = LayerProperty::new(0.0);
        p.animate(100.0, linear(1.0), 0.0, 0.0);
        let (value, velocity) = (p.value(0.4), p.velocity(0.4));
        p.animate(-5.0, next, 0.4, 0.0);
        assert!(close(p.value(0.4), value, 1e-9), "{row}");
        assert!(close(p.velocity(0.4), velocity, 1e-1), "{row}");
    }
}

#[test]
fn a_spring_ends_exactly_on_its_target_and_stays_there() {
    let mut p = LayerProperty::new(0.0);
    p.animate(1.0, spring(0.5, 1.0), 0.0, 0.0);
    let end = p.end().expect("a spring has an end");
    assert!(end > 0.0 && end < 5.0);
    assert!(close(p.value(0.0), 0.0, 1e-12));
    assert_eq!(p.value(end), 1.0);
    assert_eq!(p.value(end + 10.0), 1.0);
    assert!(!p.is_animating(end));

    let mut resting = LayerProperty::new(2.0);
    resting.animate(2.0, spring(0.5, 1.0), 1.0, 0.0);
    assert_eq!(resting.end(), Some(1.0), "already at rest ends at once");
    assert!(!resting.is_animating(1.0));
    assert_eq!(resting.value(1.5), 2.0);
}

#[test]
fn spring_damping_decides_overshoot() {
    // (damping, overshoots, rises monotonically)
    for (damping, overshoots, monotonic) in
        [(0.4, true, false), (1.0, false, true), (2.0, false, true)]
    {
        let mut p = LayerProperty::new(0.0);
        p.animate(1.0, spring(0.5, damping), 0.0, 0.0);
        let values: Vec<f64> = (0..600).map(|i| p.value(f64::from(i) * 0.005)).collect();
        let peak = values.iter().copied().fold(f64::MIN, f64::max);
        assert_eq!(peak > 1.05, overshoots, "damping {damping}: peak {peak}");
        if !overshoots {
            assert!(peak <= 1.0 + 1e-9, "damping {damping}");
        }
        if monotonic {
            assert!(
                values.windows(2).all(|w| w[1] >= w[0] - 1e-12),
                "damping {damping}"
            );
        }
    }
}

#[test]
fn samples_are_played_in_straight_lines_between_even_steps() {
    let mut p = LayerProperty::new(7.0);
    p.animate_samples(Arc::from([0.0f32, 10.0, 0.0]), 2.0, 1.0);
    assert_eq!(p.target(), 0.0);
    assert_eq!(p.end(), Some(3.0));
    // before the start it shows the first sample, not the old value
    for (t, value) in [
        (0.5, 0.0),
        (1.0, 0.0),
        (1.5, 5.0),
        (2.0, 10.0),
        (2.5, 5.0),
        (3.0, 0.0),
    ] {
        assert!(close(p.value(t), value, 1e-9), "value at {t}");
    }
    for (t, velocity) in [(0.5, 0.0), (1.5, 10.0), (2.5, -10.0), (3.0, 0.0)] {
        assert!(close(p.velocity(t), velocity, 1e-9), "velocity at {t}");
    }
}

#[test]
fn degenerate_samples_rest_at_their_last_value_and_empty_ones_change_nothing() {
    let mut one = LayerProperty::new(0.0);
    one.animate_samples(Arc::from([4.0f32]), 1.0, 0.0);
    assert_eq!(one.target(), 4.0);
    assert_eq!(one.value(0.5), 4.0);
    assert!(!one.is_animating(0.0));

    let mut instant = LayerProperty::new(0.0);
    instant.animate_samples(Arc::from([1.0f32, 9.0]), 0.0, 0.0);
    assert_eq!(instant.value(0.0), 9.0);

    let mut empty = LayerProperty::new(2.0);
    empty.animate_samples(Arc::from([] as [f32; 0]), 1.0, 0.0);
    assert_eq!(empty.target(), 2.0);
    assert_eq!(empty.value(0.5), 2.0);
}

#[test]
fn set_jumps_and_cancel_stops_at_the_value_shown() {
    let mut set = LayerProperty::new(0.0);
    set.animate(10.0, linear(1.0), 0.0, 0.0);
    set.set(4.0);
    assert_eq!(set.value(0.5), 4.0);
    assert_eq!(set.target(), 4.0);
    assert_eq!(set.end(), None);

    let mut cancelled = LayerProperty::new(0.0);
    cancelled.animate(10.0, linear(1.0), 0.0, 0.0);
    cancelled.cancel(0.5);
    assert!(close(cancelled.value(0.9), 5.0, 1e-6));
    assert_eq!(cancelled.velocity(0.9), 0.0);
    assert!(!cancelled.is_animating(0.9));
}
