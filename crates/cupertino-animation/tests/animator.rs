//! Retargetable animated values.

use cupertino_animation::{AnimatedValue, CubicBezier, Curve, SpringParams};

const LINEAR: CubicBezier = CubicBezier::new(0.0, 0.0, 1.0, 1.0);

fn linear(duration: f64) -> Curve {
    Curve::Timed {
        duration,
        bezier: LINEAR,
    }
}

fn spring() -> Curve {
    Curve::Spring(SpringParams::from_perceptual(0.4, 0.0))
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

#[test]
fn a_new_value_rests() {
    let v = AnimatedValue::new(5.0f64);
    assert_eq!(v.value(0.0), 5.0);
    assert_eq!(v.value(100.0), 5.0);
    assert_eq!(v.target(), 5.0);
    assert!(!v.is_animating(0.0));
    assert_eq!(v.velocity(0.0), 0.0);
}

#[test]
fn a_timed_animation_follows_its_curve_between_its_end_points() {
    let mut v = AnimatedValue::new(5.0f64);
    v.animate_to(10.0, linear(1.0), 2.0);
    assert_eq!(v.target(), 10.0);
    assert!(close(v.value(2.0), 5.0, 1e-6));
    assert!(close(v.value(2.5), 7.5, 1e-6));
    assert!(close(v.value(3.0), 10.0, 1e-6));
    assert_eq!(v.value(9.0), 10.0);
}

#[test]
fn a_timed_animation_holds_its_start_value_before_its_start_time() {
    let mut v = AnimatedValue::new(5.0f64);
    v.animate_to(10.0, linear(1.0), 2.0);
    assert!(close(v.value(1.0), 5.0, 1e-6));
}

#[test]
fn a_timed_animation_is_running_until_its_end() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(1.0, linear(1.0), 0.0);
    assert!(v.is_animating(0.5));
    assert!(!v.is_animating(1.0));
}

#[test]
fn a_zero_duration_animation_jumps() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(8.0, linear(0.0), 1.0);
    assert_eq!(v.value(1.0), 8.0);
    assert!(!v.is_animating(1.0));
}

#[test]
fn the_timed_velocity_is_the_curve_slope() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(10.0, linear(1.0), 0.0);
    assert!(close(v.velocity(0.5), 10.0, 1e-3));
}

#[test]
fn a_spring_animation_ends_on_its_target() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(100.0, spring(), 0.0);
    assert!(v.is_animating(0.1));
    assert!(close(v.value(10.0), 100.0, 1e-6));
    assert!(!v.is_animating(10.0));
    assert!(v.value(0.1) > 0.0 && v.value(0.1) < 100.0);
}

#[test]
fn retargeting_a_timed_animation_keeps_the_shown_value() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(10.0, linear(1.0), 0.0);
    let before = v.value(0.4);
    v.animate_to(-20.0, linear(1.0), 0.4);
    assert!(close(v.value(0.4), before, 1e-9));
    assert_eq!(v.target(), -20.0);
}

#[test]
fn retargeting_a_spring_keeps_the_value_and_the_velocity() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(100.0, spring(), 0.0);
    let (value, velocity) = (v.value(0.1), v.velocity(0.1));
    assert!(velocity > 0.0);
    v.animate_to(0.0, spring(), 0.1);
    assert!(close(v.value(0.1), value, 1e-6));
    assert!(((v.velocity(0.1) - velocity) / velocity).abs() < 0.01);
}

#[test]
fn a_spring_started_from_a_fast_moving_value_carries_the_velocity_past_the_target() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(10.0, linear(0.1), 0.0);
    // moving at 100 per second with 5 to go: faster than the spring's own rate, so it overshoots
    v.animate_to(10.0, spring(), 0.05);
    let peak = (0..400)
        .map(|i| v.value(0.05 + f64::from(i) * 0.005))
        .fold(f64::MIN, f64::max);
    assert!(peak > 10.0);
    assert!(close(v.value(10.0), 10.0, 1e-6));
}

#[test]
fn set_immediately_stops_and_sets_the_value() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(10.0, linear(1.0), 0.0);
    v.set_immediately(3.0);
    assert_eq!(v.value(0.5), 3.0);
    assert_eq!(v.target(), 3.0);
    assert!(!v.is_animating(0.5));
}

#[test]
fn cancel_freezes_the_value_shown_at_that_time() {
    let mut v = AnimatedValue::new(0.0f64);
    v.animate_to(10.0, linear(1.0), 0.0);
    v.cancel(0.5);
    assert!(close(v.value(0.9), 5.0, 1e-6));
    assert!(!v.is_animating(0.9));
}

#[test]
fn vector_values_animate_componentwise() {
    let mut v = AnimatedValue::new([0.0f64, 10.0]);
    v.animate_to([10.0, 0.0], linear(1.0), 0.0);
    let mid = v.value(0.5);
    assert!(close(mid[0], 5.0, 1e-6) && close(mid[1], 5.0, 1e-6));
    let end = v.value(1.0);
    assert!(close(end[0], 10.0, 1e-6) && close(end[1], 0.0, 1e-6));
}

#[test]
fn a_vector_spring_reaches_its_target_on_every_axis() {
    let mut v = AnimatedValue::new([0.0f64, 0.0]);
    v.animate_to([30.0, -40.0], spring(), 0.0);
    let end = v.value(10.0);
    assert!(close(end[0], 30.0, 1e-6) && close(end[1], -40.0, 1e-6));
}

#[test]
fn tracking_follows_the_finger_one_to_one() {
    let mut v = AnimatedValue::new(0.0f64);
    v.track(10.0, 0.0);
    assert_eq!(v.value(0.0), 10.0);
    v.track(20.0, 0.1);
    assert_eq!(v.value(5.0), 20.0);
    assert_eq!(v.target(), 20.0);
    assert!(v.is_animating(5.0));
}

#[test]
fn the_tracked_velocity_is_smoothed() {
    let mut v = AnimatedValue::new(0.0f64);
    v.track(10.0, 0.0);
    v.track(20.0, 0.1);
    // instantaneous 100 per second, blended 0.6 with the previous 0
    assert!(close(v.velocity(0.1), 60.0, 1e-9));
}

#[test]
fn releasing_a_gesture_hands_its_velocity_to_a_spring() {
    let mut v = AnimatedValue::new(0.0f64);
    v.track(0.0, 0.0);
    v.track(10.0, 0.05);
    v.track(20.0, 0.1);
    v.release(20.0, SpringParams::from_perceptual(0.4, 0.0), 0.1);
    // already at the target: the spring carries the velocity as an excursion before settling
    assert!(close(v.value(0.1), 20.0, 1e-9));
    assert!(v.is_animating(0.1));
    assert!(close(v.value(10.0), 20.0, 1e-6));
}

#[test]
fn releasing_towards_a_target_settles_there() {
    let mut v = AnimatedValue::new(0.0f64);
    v.track(50.0, 0.0);
    v.release(0.0, SpringParams::from_perceptual(0.3, 0.0), 0.0);
    assert!(close(v.value(0.0), 50.0, 1e-9));
    assert!(close(v.value(10.0), 0.0, 1e-6));
    assert_eq!(v.target(), 0.0);
}
