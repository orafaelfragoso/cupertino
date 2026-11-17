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
fn every_ease_runs_from_zero_to_one_and_clamps_its_input() {
    for ease in [
        Ease::Linear,
        Ease::Cosine,
        Ease::Bezier(CubicBezier::new(0.25, 0.1, 0.25, 1.0)),
    ] {
        assert!(near(ease.apply(0.0), 0.0), "{ease:?}");
        assert!(near(ease.apply(1.0), 1.0), "{ease:?}");
        assert_eq!(ease.apply(-3.0), ease.apply(0.0), "{ease:?}");
        assert_eq!(ease.apply(7.0), ease.apply(1.0), "{ease:?}");
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
fn a_timed_animation_rests_until_sent_and_runs_its_ease_over_its_duration() {
    let mut a = ProgressAnimator::new(linear(2.0), 0.7);
    assert_eq!((a.value(123.0), a.target()), (0.7, 0.7));
    assert!(!a.is_animating(0.0));
    a.set(0.0);
    a.animate_to(1.0, 10.0);
    assert_eq!(a.target(), 1.0);
    for (time, value, animating) in [
        (10.0, 0.0, true),
        (11.0, 0.5, true),
        (12.0, 1.0, false),
        (50.0, 1.0, false),
    ] {
        assert!(near(a.value(time), value), "{time}");
        assert_eq!(a.is_animating(time), animating, "{time}");
    }
    assert_eq!(linear(0.4).settle_time(), 0.4);
    assert_eq!(linear(-1.0).settle_time(), 0.0, "never below zero");
}

#[test]
fn a_timed_trip_takes_time_in_proportion_to_its_length_up_to_the_duration() {
    let mut a = ProgressAnimator::new(linear(2.0), 0.0);
    a.animate_to(0.25, 0.0);
    assert!(a.is_animating(0.49));
    assert!(
        !a.is_animating(0.5),
        "a quarter of the trip, a quarter of the time"
    );
    assert!(near(a.value(0.25), 0.125));
    let mut long = ProgressAnimator::new(linear(1.0), 0.0);
    long.animate_to(5.0, 0.0);
    assert!(near(long.value(1.0), 5.0));
}

#[test]
fn retargeting_in_flight_starts_from_the_current_value() {
    for (name, driver) in [
        ("timed", linear(1.0)),
        ("spring", Driver::spring_response(0.4, 1.0)),
    ] {
        let mut a = ProgressAnimator::new(driver, 0.0);
        a.animate_to(1.0, 0.0);
        let mid = a.value(0.2);
        a.animate_to(0.0, 0.2);
        assert!((a.value(0.2) - mid).abs() < 1e-9, "{name}: no jump");
        assert!(a.value(5.0).abs() < 1e-6, "{name}");
    }
}

#[test]
fn setting_a_value_cancels_the_animation() {
    let mut a = ProgressAnimator::new(linear(1.0), 0.0);
    a.animate_to(1.0, 0.0);
    a.set(0.3);
    assert!(!a.is_animating(0.1));
    assert_eq!((a.value(0.1), a.target(), a.velocity(0.1)), (0.3, 0.3, 0.0));
}

#[test]
fn the_velocity_of_a_linear_animation_is_its_slope() {
    let mut a = ProgressAnimator::new(linear(2.0), 0.0);
    a.animate_to(1.0, 0.0);
    assert!((a.velocity(1.0) - 0.5).abs() < 1e-3);
    assert_eq!(a.velocity(-1.0), 0.0, "nothing before it starts");
}

#[test]
fn a_spring_settles_on_its_target_by_its_settling_time_and_a_stiffer_one_sooner() {
    let driver = Driver::spring_response(0.3, 1.0);
    let mut a = ProgressAnimator::new(driver, 0.0);
    a.animate_to(1.0, 0.0);
    let settle = driver.settle_time();
    assert!(settle > 0.1 && settle < 2.0, "{settle}");
    assert!(a.is_animating(settle * 0.5));
    assert!(a.value(settle * 0.5) < 1.0);
    assert!((a.value(settle + 0.5) - 1.0).abs() < 1e-6);
    assert!(
        Driver::spring_response(0.15, 1.0).settle_time()
            < Driver::spring_response(0.6, 1.0).settle_time()
    );

    let mut still = ProgressAnimator::new(driver, 0.5);
    still.animate_to(0.5, 1.0);
    assert!(!still.is_animating(1.0), "sent where it already is");
    assert_eq!(still.value(1.0), 0.5);
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
