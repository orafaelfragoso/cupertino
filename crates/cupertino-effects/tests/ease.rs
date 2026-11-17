//! Eases map progress to progress, and a progress animator follows its driver from wherever it currently is.

use cupertino_animation::CubicBezier;
use cupertino_effects::{cosine, Driver, Ease, ProgressAnimator};

fn linear(duration: f64) -> Driver {
    Driver::Timed {
        duration,
        ease: Ease::Linear,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn every_ease_starts_at_zero_and_ends_at_one() {
    for ease in [
        Ease::Linear,
        Ease::Cosine,
        Ease::Bezier(CubicBezier::new(0.25, 0.1, 0.25, 1.0)),
    ] {
        assert!(near(ease.apply(0.0), 0.0), "{ease:?}");
        assert!(near(ease.apply(1.0), 1.0), "{ease:?}");
    }
}

#[test]
fn an_ease_clamps_its_input() {
    for ease in [Ease::Linear, Ease::Cosine] {
        assert_eq!(ease.apply(-3.0), ease.apply(0.0));
        assert_eq!(ease.apply(7.0), ease.apply(1.0));
    }
}

#[test]
fn the_cosine_ease_is_symmetric_and_flat_at_both_ends() {
    assert!(near(cosine(0.5), 0.5));
    assert!(near(cosine(0.25) + cosine(0.75), 1.0));
    assert!(cosine(0.01) < 0.001, "starts slowly");
    assert!(cosine(0.99) > 0.999, "ends slowly");
}

#[test]
fn a_timed_animation_runs_its_ease_over_its_duration() {
    let mut a = ProgressAnimator::new(linear(2.0), 0.0);
    assert!(!a.is_animating(0.0));
    a.animate_to(1.0, 10.0);
    assert_eq!(a.target(), 1.0);
    assert!(a.is_animating(10.5));
    assert!(near(a.value(10.0), 0.0));
    assert!(near(a.value(11.0), 0.5));
    assert!(near(a.value(12.0), 1.0));
    assert!(!a.is_animating(12.0));
    assert!(near(a.value(50.0), 1.0), "stays at the target");
}

#[test]
fn a_shorter_trip_takes_proportionally_less_time() {
    let mut a = ProgressAnimator::new(linear(2.0), 0.0);
    a.animate_to(0.25, 0.0);
    assert!(a.is_animating(0.49));
    assert!(
        !a.is_animating(0.5),
        "a quarter of the trip, a quarter of the time"
    );
    assert!(near(a.value(0.25), 0.125));
}

#[test]
fn a_longer_trip_does_not_take_longer_than_the_duration() {
    let mut a = ProgressAnimator::new(linear(1.0), 0.0);
    a.animate_to(5.0, 0.0);
    assert!(near(a.value(1.0), 5.0));
}

#[test]
fn retargeting_starts_from_the_current_value() {
    let mut a = ProgressAnimator::new(linear(1.0), 0.0);
    a.animate_to(1.0, 0.0);
    let mid = a.value(0.6);
    a.animate_to(0.0, 0.6);
    assert!(near(a.value(0.6), mid), "no jump when reversing");
    assert!(a.value(0.8) < mid);
    assert!(near(a.value(2.0), 0.0));
}

#[test]
fn setting_a_value_cancels_the_animation() {
    let mut a = ProgressAnimator::new(linear(1.0), 0.0);
    a.animate_to(1.0, 0.0);
    a.set(0.3);
    assert!(!a.is_animating(0.1));
    assert_eq!(a.value(0.1), 0.3);
    assert_eq!(a.target(), 0.3);
    assert_eq!(a.velocity(0.1), 0.0);
}

#[test]
fn the_velocity_of_a_linear_animation_is_its_slope() {
    let mut a = ProgressAnimator::new(linear(2.0), 0.0);
    a.animate_to(1.0, 0.0);
    assert!((a.velocity(1.0) - 0.5).abs() < 1e-3);
    assert_eq!(a.velocity(-1.0), 0.0, "nothing before it starts");
}

#[test]
fn an_initial_value_is_where_the_animator_rests() {
    let a = ProgressAnimator::new(linear(1.0), 0.7);
    assert_eq!(a.value(123.0), 0.7);
    assert_eq!(a.target(), 0.7);
    assert!(!a.is_animating(0.0));
}

#[test]
fn a_spring_settles_on_its_target_and_not_before_its_settling_time() {
    let driver = Driver::spring_response(0.3, 1.0);
    let mut a = ProgressAnimator::new(driver, 0.0);
    a.animate_to(1.0, 0.0);
    let settle = driver.settle_time();
    assert!(settle > 0.1 && settle < 2.0, "{settle}");
    assert!(a.is_animating(settle * 0.5));
    assert!(a.value(settle * 0.5) < 1.0);
    assert!((a.value(settle + 0.5) - 1.0).abs() < 1e-6);
}

#[test]
fn a_bouncy_spring_overshoots_its_target() {
    let mut a = ProgressAnimator::new(Driver::spring_perceptual(0.5, 0.5), 0.0);
    a.animate_to(1.0, 0.0);
    let peak = (0..200)
        .map(|i| a.value(f64::from(i) * 0.005))
        .fold(0.0, f64::max);
    assert!(peak > 1.0, "{peak}");
}

#[test]
fn a_spring_retargeted_in_flight_keeps_its_position() {
    let mut a = ProgressAnimator::new(Driver::spring_response(0.4, 1.0), 0.0);
    a.animate_to(1.0, 0.0);
    let mid = a.value(0.2);
    a.animate_to(0.0, 0.2);
    assert!((a.value(0.2) - mid).abs() < 1e-9);
    assert!((a.value(5.0)).abs() < 1e-6);
}

#[test]
fn a_spring_sent_where_it_already_is_does_not_animate() {
    let mut a = ProgressAnimator::new(Driver::spring_response(0.4, 1.0), 0.5);
    a.animate_to(0.5, 1.0);
    assert!(!a.is_animating(1.0));
    assert_eq!(a.value(1.0), 0.5);
}

#[test]
fn a_timed_driver_settles_at_its_duration_and_never_below_zero() {
    assert_eq!(linear(0.4).settle_time(), 0.4);
    assert_eq!(linear(-1.0).settle_time(), 0.0);
}

#[test]
fn a_stiffer_spring_settles_sooner() {
    let slow = Driver::spring_response(0.6, 1.0).settle_time();
    let quick = Driver::spring_response(0.15, 1.0).settle_time();
    assert!(quick < slow);
}
