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
    assert!(close(p.value(2.0), 0.0, 1e-6));
    assert!(close(p.value(2.5), 5.0, 1e-6));
    assert_eq!(p.value(3.0), 10.0);
    assert_eq!(p.value(30.0), 10.0);
}

#[test]
fn an_animation_that_has_not_started_counts_as_running() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(1.0), 2.0, 0.0);
    assert!(p.is_animating(1.0));
    assert!(p.is_animating(2.5));
    assert!(!p.is_animating(3.0));
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
fn a_zero_duration_timed_animation_jumps_to_its_target() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(0.0), 1.0, 0.0);
    assert_eq!(p.value(1.0), 10.0);
    assert!(!p.is_animating(1.0));
}

#[test]
fn a_timed_retarget_keeps_value_and_velocity_continuous() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(1.0), 0.0, 0.0);
    let (value, velocity) = (p.value(0.4), p.velocity(0.4));
    p.animate(-5.0, linear(1.0), 0.4, 0.0);
    assert!(close(p.value(0.4), value, 1e-9));
    assert!(close(p.velocity(0.4), velocity, 1e-2));
}

#[test]
fn a_timed_animation_from_rest_can_start_with_a_given_velocity() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(1.0), 0.0, 50.0);
    assert!(close(p.velocity(1e-4), 50.0, 0.5));
    assert!(close(p.value(1.0), 10.0, 1e-9));
}

#[test]
fn a_spring_ends_on_its_target_and_stays_there() {
    let mut p = LayerProperty::new(0.0);
    p.animate(1.0, spring(0.5, 1.0), 0.0, 0.0);
    let end = p.end().expect("a spring has an end");
    assert!(end > 0.0 && end < 5.0);
    assert!(close(p.value(0.0), 0.0, 1e-12));
    assert_eq!(p.value(end), 1.0);
    assert_eq!(p.value(end + 10.0), 1.0);
    assert!(!p.is_animating(end));
}

#[test]
fn a_critically_damped_spring_never_overshoots() {
    let mut p = LayerProperty::new(0.0);
    p.animate(1.0, spring(0.5, 1.0), 0.0, 0.0);
    for i in 0..400 {
        assert!(p.value(f64::from(i) * 0.005) <= 1.0 + 1e-9);
    }
}

#[test]
fn a_bouncing_spring_overshoots() {
    let mut p = LayerProperty::new(0.0);
    p.animate(1.0, spring(0.5, 0.4), 0.0, 0.0);
    let peak = (0..400)
        .map(|i| p.value(f64::from(i) * 0.005))
        .fold(f64::MIN, f64::max);
    assert!(peak > 1.05);
}

#[test]
fn an_overdamped_spring_creeps_up_without_overshoot() {
    let mut p = LayerProperty::new(0.0);
    p.animate(1.0, spring(0.5, 2.0), 0.0, 0.0);
    let mut last = 0.0;
    for i in 0..600 {
        let v = p.value(f64::from(i) * 0.005);
        assert!(v <= 1.0 + 1e-9 && v >= last - 1e-12);
        last = v;
    }
}

#[test]
fn a_spring_retarget_seeds_its_velocity_from_the_running_animation() {
    let mut p = LayerProperty::new(0.0);
    p.animate(100.0, linear(1.0), 0.0, 0.0);
    let (value, velocity) = (p.value(0.5), p.velocity(0.5));
    p.animate(0.0, spring(0.5, 1.0), 0.5, 0.0);
    assert!(close(p.value(0.5), value, 1e-9));
    assert!(close(p.velocity(0.5), velocity, 1e-3));
}

#[test]
fn a_spring_from_rest_uses_the_given_velocity() {
    let mut p = LayerProperty::new(0.0);
    p.animate(0.0, spring(0.5, 0.5), 0.0, 40.0);
    assert!(close(p.velocity(0.0), 40.0, 1e-6));
    let moved = (1..100)
        .map(|i| p.value(f64::from(i) * 0.005).abs())
        .fold(0.0, f64::max);
    assert!(moved > 0.1);
    assert_eq!(p.value(p.end().unwrap()), 0.0);
}

#[test]
fn a_spring_that_is_already_at_rest_ends_at_once() {
    let mut p = LayerProperty::new(2.0);
    p.animate(2.0, spring(0.5, 1.0), 1.0, 0.0);
    assert_eq!(p.end(), Some(1.0));
    assert!(!p.is_animating(1.0));
    assert_eq!(p.value(1.5), 2.0);
}

#[test]
fn samples_are_played_in_straight_lines_between_even_steps() {
    let mut p = LayerProperty::new(7.0);
    p.animate_samples(Arc::from([0.0f32, 10.0, 0.0]), 2.0, 1.0);
    assert_eq!(p.target(), 0.0);
    assert_eq!(p.end(), Some(3.0));
    assert_eq!(p.value(1.0), 0.0);
    assert!(close(p.value(1.5), 5.0, 1e-9));
    assert_eq!(p.value(2.0), 10.0);
    assert!(close(p.value(2.5), 5.0, 1e-9));
    assert_eq!(p.value(3.0), 0.0);
}

#[test]
fn samples_start_at_their_first_value_whatever_was_shown() {
    let mut p = LayerProperty::new(7.0);
    p.animate_samples(Arc::from([1.0f32, 2.0]), 1.0, 5.0);
    assert_eq!(p.value(0.0), 1.0);
    assert_eq!(p.value(5.0), 1.0);
}

#[test]
fn sample_velocity_is_the_slope_of_the_current_segment() {
    let mut p = LayerProperty::new(0.0);
    p.animate_samples(Arc::from([0.0f32, 10.0, 0.0]), 2.0, 1.0);
    assert!(close(p.velocity(1.5), 10.0, 1e-9));
    assert!(close(p.velocity(2.5), -10.0, 1e-9));
    assert_eq!(p.velocity(0.5), 0.0);
    assert_eq!(p.velocity(3.0), 0.0);
}

#[test]
fn degenerate_samples_rest_at_their_last_value() {
    let mut one = LayerProperty::new(0.0);
    one.animate_samples(Arc::from([4.0f32]), 1.0, 0.0);
    assert_eq!(one.target(), 4.0);
    assert_eq!(one.value(0.5), 4.0);
    assert!(!one.is_animating(0.0));
    let mut instant = LayerProperty::new(0.0);
    instant.animate_samples(Arc::from([1.0f32, 9.0]), 0.0, 0.0);
    assert_eq!(instant.value(0.0), 9.0);
}

#[test]
fn an_empty_sample_list_changes_nothing() {
    let mut p = LayerProperty::new(2.0);
    p.animate_samples(Arc::from([] as [f32; 0]), 1.0, 0.0);
    assert_eq!(p.target(), 2.0);
    assert_eq!(p.value(0.5), 2.0);
}

#[test]
fn set_jumps_and_rests() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(1.0), 0.0, 0.0);
    p.set(4.0);
    assert_eq!(p.value(0.5), 4.0);
    assert_eq!(p.target(), 4.0);
    assert_eq!(p.end(), None);
}

#[test]
fn cancel_stops_at_the_value_shown() {
    let mut p = LayerProperty::new(0.0);
    p.animate(10.0, linear(1.0), 0.0, 0.0);
    p.cancel(0.5);
    assert!(close(p.value(0.9), 5.0, 1e-6));
    assert_eq!(p.velocity(0.9), 0.0);
    assert!(!p.is_animating(0.9));
}
