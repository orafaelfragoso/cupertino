//! The notification banners' and list's motions are the springs and the curve they were designed with.

use cupertino_animation::{Spring, SpringParams};
use cupertino_theme::animation::{Easing, SpringPreset, Timing};

/// How close the leaving curve is to the smoothstep it is, at the Bézier solver's precision.
const SOLVER_PRECISION: f64 = 1e-6;

#[test]
fn the_list_and_the_rise_are_critically_damped_at_their_rates() {
    for (preset, rate) in [
        (SpringPreset::NOTIFICATION_LIST_SLIDE, 21.0),
        (SpringPreset::NOTIFICATION_RISE, 21.75),
    ] {
        let p = preset.params();
        assert_eq!(
            (p.mass, p.stiffness, p.damping),
            (1.0, rate * rate, 2.0 * rate)
        );
        assert_eq!(p.initial_velocity, 0.0);
        assert_eq!(Spring::new(p).damping_ratio(), 1.0);
    }
}

#[test]
fn a_group_expands_on_the_perceptual_spring_of_0_35_seconds_without_bounce() {
    assert_eq!(
        SpringPreset::NOTIFICATION_GROUP_EXPAND.params(),
        SpringParams::from_perceptual(0.35, 0.0)
    );
}

#[test]
fn a_swiped_banner_springs_back_critically_damped() {
    let p = SpringPreset::NOTIFICATION_SWIPE_BACK.params();
    assert_eq!((p.mass, p.stiffness, p.damping), (1.0, 100.0, 100.0));
    assert!(!p.allows_overdamping);
    // critically damped at 10 radians per second: 1 − e^{−10t}(1 + 10t)
    let timing = Timing::from(SpringPreset::NOTIFICATION_SWIPE_BACK);
    for t in [0.05_f64, 0.1, 0.2, 0.4] {
        let expected = 1.0 - (-10.0 * t).exp() * (1.0 + 10.0 * t);
        assert!((timing.value(t) - expected).abs() < 1e-12, "{t}");
    }
}

#[test]
fn a_banner_slides_on_the_ease_in_out_over_a_quarter_second() {
    let timing = Timing::NOTIFICATION_BANNER;
    assert_eq!(timing.duration(), 0.25);
    for i in 0..=10 {
        let t = 0.025 * f64::from(i);
        assert_eq!(timing.value(t), Easing::EASE_IN_OUT.eval(t / 0.25), "{t}");
    }
}

#[test]
fn a_cleared_card_leaves_on_the_smoothstep_over_0_13_seconds() {
    let timing = Timing::NOTIFICATION_CARD_LEAVE;
    assert_eq!(timing.duration(), 0.13);
    for i in 0..=26 {
        let t = 0.005 * f64::from(i);
        let x = t / 0.13;
        let smoothstep = x * x * (3.0 - 2.0 * x);
        assert!(
            (timing.value(t) - smoothstep).abs() < SOLVER_PRECISION,
            "{t}"
        );
    }
}
