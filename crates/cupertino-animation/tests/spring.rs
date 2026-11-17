//! The damped spring and its perceptual constructors.

use std::f64::consts::TAU;

use cupertino_animation::{Spring, SpringParams};

fn peak(s: &Spring) -> f64 {
    (1..4000)
        .map(|i| s.value(f64::from(i) * 0.001))
        .fold(f64::MIN, f64::max)
}

#[test]
fn perceptual_constructor_without_bounce_is_critically_damped() {
    let p = SpringParams::from_perceptual(0.5, 0.0);
    assert_eq!(p.mass, 1.0);
    assert!((p.stiffness - (TAU / 0.5).powi(2)).abs() < 1e-9);
    assert!((p.damping - 4.0 * std::f64::consts::PI / 0.5).abs() < 1e-9);
    assert_eq!(p.initial_velocity, 0.0);
    assert!(!p.allows_overdamping);
    assert!((Spring::new(p).damping_ratio() - 1.0).abs() < 1e-12);
}

#[test]
fn positive_bounce_lowers_the_damping_ratio_to_one_minus_bounce() {
    for bounce in [0.1, 0.3, 0.6] {
        let s = Spring::new(SpringParams::from_perceptual(0.4, bounce));
        assert!(
            (s.damping_ratio() - (1.0 - bounce)).abs() < 1e-12,
            "{bounce}"
        );
    }
}

#[test]
fn negative_bounce_raises_the_damping_ratio_above_one() {
    let s = Spring::new(SpringParams::from_perceptual(0.4, -0.5));
    assert!((s.damping_ratio() - 2.0).abs() < 1e-12);
}

#[test]
fn perceptual_duration_and_bounce_read_back() {
    for (d, b) in [(0.35, 0.0), (0.5, 0.3), (0.25, -0.5), (1.0, 0.7)] {
        let p = SpringParams::from_perceptual(d, b);
        assert!((p.perceptual_duration() - d).abs() < 1e-12, "({d}, {b})");
        assert!((p.bounce() - b).abs() < 1e-9, "({d}, {b})");
    }
}

#[test]
fn a_zero_duration_has_infinite_stiffness() {
    assert!(SpringParams::from_perceptual(0.0, 0.0)
        .stiffness
        .is_infinite());
}

#[test]
fn ratio_constructor_uses_response_as_the_period() {
    let p = SpringParams::from_ratio(0.5, 0.8);
    assert_eq!(p.mass, 1.0);
    assert!((p.stiffness - (TAU / 0.5).powi(2)).abs() < 1e-9);
    assert!((p.damping - 2.0 * 0.8 * TAU / 0.5).abs() < 1e-9);
    assert!((Spring::new(p).damping_ratio() - 0.8).abs() < 1e-12);
    assert!((p.perceptual_duration() - 0.5).abs() < 1e-12);
}

#[test]
fn a_spring_is_at_rest_at_time_zero_and_before() {
    let s = Spring::new(SpringParams::from_perceptual(0.4, 0.3));
    assert_eq!(s.value(0.0), 0.0);
    assert_eq!(s.value(-1.0), 0.0);
}

#[test]
fn every_spring_ends_at_one() {
    for (d, b) in [(0.35, 0.0), (0.5, 0.3), (0.4, 0.8)] {
        let s = Spring::new(SpringParams::from_perceptual(d, b));
        assert!((s.value(20.0) - 1.0).abs() < 1e-9, "({d}, {b})");
        assert!(s.velocity(20.0).abs() < 1e-9);
    }
}

#[test]
fn a_critically_damped_spring_never_overshoots_and_rises_monotonically() {
    let s = Spring::new(SpringParams::from_perceptual(0.35, 0.0));
    let mut last = 0.0;
    for i in 1..=2000 {
        let v = s.value(f64::from(i) * 0.002);
        assert!(v <= 1.0 + 1e-12);
        assert!(v >= last - 1e-12);
        last = v;
    }
}

#[test]
fn an_underdamped_spring_overshoots() {
    let s = Spring::new(SpringParams::from_perceptual(0.5, 0.4));
    assert!(peak(&s) > 1.05);
}

#[test]
fn more_bounce_overshoots_more() {
    let low = peak(&Spring::new(SpringParams::from_perceptual(0.5, 0.2)));
    let high = peak(&Spring::new(SpringParams::from_perceptual(0.5, 0.6)));
    assert!(high > low);
}

#[test]
fn initial_velocity_is_the_starting_slope_in_every_regime() {
    for ratio in [0.5, 1.0, 2.0] {
        let params = SpringParams {
            initial_velocity: 3.0,
            allows_overdamping: true,
            ..SpringParams::from_ratio(0.4, ratio)
        };
        let s = Spring::new(params);
        assert!((s.velocity(0.0) - 3.0).abs() < 1e-6, "zeta {ratio}");
        let h = 1e-6;
        assert!(
            ((s.value(h) - s.value(0.0)) / h - 3.0).abs() < 1e-3,
            "zeta {ratio}"
        );
    }
}

#[test]
fn velocity_is_the_derivative_of_value() {
    let s = Spring::new(SpringParams::from_perceptual(0.5, 0.3));
    for t in [0.05, 0.2, 0.4, 0.9] {
        let h = 1e-6;
        let numeric = (s.value(t + h) - s.value(t - h)) / (2.0 * h);
        assert!((s.velocity(t) - numeric).abs() < 1e-4, "t = {t}");
    }
}

#[test]
fn an_overdamped_spring_without_the_flag_acts_critically_damped() {
    let over = Spring::new(SpringParams::from_perceptual(0.4, -0.5));
    let w0 = TAU / 0.4;
    let critical = Spring::new(SpringParams {
        mass: 1.0,
        stiffness: w0 * w0,
        damping: 2.0 * w0,
        initial_velocity: 0.0,
        allows_overdamping: false,
    });
    for t in [0.1, 0.3, 0.8] {
        assert!((over.value(t) - critical.value(t)).abs() < 1e-12);
    }
}

#[test]
fn an_allowed_overdamped_spring_is_slower_but_never_overshoots() {
    let params = SpringParams {
        allows_overdamping: true,
        ..SpringParams::from_perceptual(0.4, -0.5)
    };
    let over = Spring::new(params);
    let critical = Spring::new(SpringParams::from_perceptual(0.4, 0.0));
    assert!(over.value(0.3) < critical.value(0.3));
    assert!(peak(&over) <= 1.0 + 1e-9);
    assert!((over.value(20.0) - 1.0).abs() < 1e-9);
}

#[test]
fn settling_duration_is_when_the_error_falls_below_a_thousandth() {
    for (d, b) in [(0.35, 0.0), (0.5, 0.3), (0.4, 0.7)] {
        let s = Spring::new(SpringParams::from_perceptual(d, b));
        let settle = s.settling_duration();
        assert!(settle > 0.0 && settle < 10.0, "({d}, {b}): {settle}");
        for k in 0..200 {
            let t = settle + f64::from(k) * 0.01;
            assert!((1.0 - s.value(t)).abs() <= 1.5e-3, "({d}, {b}) at {t}");
        }
    }
}

#[test]
fn a_smaller_epsilon_takes_longer_to_settle() {
    let s = Spring::new(SpringParams::from_perceptual(0.5, 0.3));
    assert!(s.duration_for_epsilon(1e-5) > s.duration_for_epsilon(1e-2));
    assert_eq!(s.settling_duration(), s.duration_for_epsilon(1e-3));
}

#[test]
fn a_stiffer_spring_settles_sooner() {
    let slow = Spring::new(SpringParams::from_perceptual(0.6, 0.0));
    let fast = Spring::new(SpringParams::from_perceptual(0.2, 0.0));
    assert!(fast.settling_duration() < slow.settling_duration());
}
