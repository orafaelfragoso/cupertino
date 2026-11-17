//! The damped spring and its perceptual constructors.

use std::f64::consts::TAU;

use cupertino_animation::{Spring, SpringParams};

fn peak(s: &Spring) -> f64 {
    (1..4000)
        .map(|i| s.value(f64::from(i) * 0.001))
        .fold(f64::MIN, f64::max)
}

#[test]
fn constructors_set_the_period_and_damping_ratio_and_read_back() {
    // (row, params, damping ratio, perceptual duration, bounce)
    for (row, p, ratio, duration, bounce) in [
        (
            "no bounce",
            SpringParams::from_perceptual(0.5, 0.0),
            1.0,
            0.5,
            0.0,
        ),
        (
            "bounce 0.3",
            SpringParams::from_perceptual(0.5, 0.3),
            0.7,
            0.5,
            0.3,
        ),
        (
            "bounce 0.7",
            SpringParams::from_perceptual(1.0, 0.7),
            0.3,
            1.0,
            0.7,
        ),
        (
            "negative bounce",
            SpringParams::from_perceptual(0.25, -0.5),
            2.0,
            0.25,
            -0.5,
        ),
        (
            "ratio 0.8",
            SpringParams::from_ratio(0.5, 0.8),
            0.8,
            0.5,
            0.2,
        ),
    ] {
        assert_eq!(p.mass, 1.0, "{row}");
        assert!(
            (p.stiffness - (TAU / duration).powi(2)).abs() < 1e-9,
            "{row}"
        );
        assert!(
            (Spring::new(p).damping_ratio() - ratio).abs() < 1e-12,
            "{row}"
        );
        assert!((p.perceptual_duration() - duration).abs() < 1e-12, "{row}");
        assert!((p.bounce() - bounce).abs() < 1e-9, "{row}");
        assert_eq!(p.initial_velocity, 0.0, "{row}");
        assert!(!p.allows_overdamping, "{row}");
    }
    assert!(SpringParams::from_perceptual(0.0, 0.0)
        .stiffness
        .is_infinite());
}

#[test]
fn a_spring_rests_at_zero_until_it_starts_and_ends_at_one() {
    for (d, b) in [(0.35, 0.0), (0.5, 0.3), (0.4, 0.8)] {
        let s = Spring::new(SpringParams::from_perceptual(d, b));
        assert_eq!(s.value(0.0), 0.0, "({d}, {b})");
        assert_eq!(s.value(-1.0), 0.0, "({d}, {b})");
        assert!((s.value(20.0) - 1.0).abs() < 1e-9, "({d}, {b})");
        assert!(s.velocity(20.0).abs() < 1e-9, "({d}, {b})");
    }
}

#[test]
fn overshoot_grows_with_bounce_and_a_critical_spring_rises_monotonically() {
    let critical = Spring::new(SpringParams::from_perceptual(0.35, 0.0));
    let mut last = 0.0;
    for i in 1..=2000 {
        let v = critical.value(f64::from(i) * 0.002);
        assert!(v <= 1.0 + 1e-12 && v >= last - 1e-12, "step {i}");
        last = v;
    }
    let low = peak(&Spring::new(SpringParams::from_perceptual(0.5, 0.2)));
    let high = peak(&Spring::new(SpringParams::from_perceptual(0.5, 0.6)));
    assert!(peak(&Spring::new(SpringParams::from_perceptual(0.5, 0.4))) > 1.05);
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
fn a_spring_is_overdamped_only_when_allowed() {
    let w0 = TAU / 0.4;
    let critical = Spring::new(SpringParams {
        mass: 1.0,
        stiffness: w0 * w0,
        damping: 2.0 * w0,
        initial_velocity: 0.0,
        allows_overdamping: false,
    });
    let clamped = Spring::new(SpringParams::from_perceptual(0.4, -0.5));
    for t in [0.1, 0.3, 0.8] {
        assert!(
            (clamped.value(t) - critical.value(t)).abs() < 1e-12,
            "t = {t}"
        );
    }
    let over = Spring::new(SpringParams {
        allows_overdamping: true,
        ..SpringParams::from_perceptual(0.4, -0.5)
    });
    assert!(over.value(0.3) < critical.value(0.3));
    assert!(peak(&over) <= 1.0 + 1e-9);
    assert!((over.value(20.0) - 1.0).abs() < 1e-9);
}

#[test]
fn settling_duration_is_when_the_error_stays_below_the_epsilon() {
    for (d, b) in [(0.35, 0.0), (0.5, 0.3), (0.4, 0.7)] {
        let s = Spring::new(SpringParams::from_perceptual(d, b));
        let settle = s.settling_duration();
        assert_eq!(settle, s.duration_for_epsilon(1e-3), "({d}, {b})");
        assert!(settle > 0.0 && settle < 10.0, "({d}, {b}): {settle}");
        for k in 0..200 {
            let t = settle + f64::from(k) * 0.01;
            assert!((1.0 - s.value(t)).abs() <= 1.5e-3, "({d}, {b}) at {t}");
        }
    }
    let s = Spring::new(SpringParams::from_perceptual(0.5, 0.3));
    assert!(s.duration_for_epsilon(1e-5) > s.duration_for_epsilon(1e-2));
    let slow = Spring::new(SpringParams::from_perceptual(0.6, 0.0));
    let fast = Spring::new(SpringParams::from_perceptual(0.2, 0.0));
    assert!(fast.settling_duration() < slow.settling_duration());
}
