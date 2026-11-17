//! Elastic overscroll mapping, the elastic return and the stretch filter.

use cupertino_animation::rubber_band::{apply, inverse, rubber_band, slope, DEFAULT_COEFFICIENT};
use cupertino_animation::{ElasticMotion, RubberBand, RubberBandFilter, RubberBandFilterConfig};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

#[test]
fn the_default_coefficient_is_0_55() {
    assert_eq!(DEFAULT_COEFFICIENT, 0.55);
    let band = RubberBand::new(800.0);
    assert_eq!(band.dimension, 800.0);
    assert_eq!(band.coefficient, 0.55);
    assert_eq!(RubberBand::with_coefficient(800.0, 0.3).coefficient, 0.3);
}

#[test]
fn the_mapping_has_a_known_value() {
    // 800 * (1 - 1 / (120 * 0.55 / 800 + 1))
    assert!(close(RubberBand::new(800.0).map(120.0), 60.970, 0.01));
}

#[test]
fn zero_offset_maps_to_zero() {
    assert_eq!(RubberBand::new(800.0).map(0.0), 0.0);
}

#[test]
fn the_mapping_is_odd_symmetric() {
    let b = RubberBand::new(800.0);
    for x in [1.0, 50.0, 400.0, 5000.0] {
        assert_eq!(b.map(-x), -b.map(x));
    }
}

#[test]
fn the_mapping_is_monotonic_and_bounded_by_the_dimension() {
    let b = RubberBand::new(800.0);
    let mut last = 0.0;
    for i in 1..=200 {
        let y = b.map(f64::from(i) * 100.0);
        assert!(y > last);
        assert!(y < 800.0);
        last = y;
    }
}

#[test]
fn a_non_positive_dimension_maps_to_zero() {
    assert_eq!(rubber_band(100.0, 0.0, 0.55), 0.0);
    assert_eq!(rubber_band(100.0, -5.0, 0.55), 0.0);
}

#[test]
fn the_slope_at_zero_is_the_coefficient_and_matches_the_numeric_derivative() {
    let b = RubberBand::new(800.0);
    assert!(close(b.slope(0.0), 0.55, 1e-12));
    for x in [10.0, 200.0, 1500.0] {
        let h = 1e-4;
        let numeric = (b.map(x + h) - b.map(x - h)) / (2.0 * h);
        assert!(close(b.slope(x), numeric, 1e-6), "x = {x}");
    }
    assert_eq!(slope(1.0, 0.0, 0.55), 0.0);
}

#[test]
fn the_inverse_undoes_the_mapping() {
    let b = RubberBand::new(800.0);
    for x in [-300.0, -1.0, 1.0, 70.0, 900.0] {
        assert!(close(b.inverse(b.map(x)), x, 1e-6), "x = {x}");
    }
}

#[test]
fn the_inverse_is_infinite_at_the_dimension_and_zero_at_zero() {
    let b = RubberBand::new(800.0);
    assert_eq!(b.inverse(800.0), f64::INFINITY);
    assert_eq!(b.inverse(-900.0), f64::NEG_INFINITY);
    assert_eq!(b.inverse(0.0), 0.0);
    assert_eq!(inverse(10.0, 0.0, 0.55), 0.0);
    assert_eq!(inverse(10.0, 800.0, 0.0), 0.0);
}

#[test]
fn apply_leaves_positions_inside_the_range_and_stretches_outside() {
    let b = RubberBand::new(800.0);
    assert_eq!(b.apply(50.0, 0.0, 100.0), 50.0);
    assert_eq!(b.apply(0.0, 0.0, 100.0), 0.0);
    assert_eq!(b.apply(100.0, 0.0, 100.0), 100.0);
    assert!(close(b.apply(-100.0, 0.0, 100.0), -b.map(100.0), 1e-9));
    assert!(close(
        b.apply(300.0, 0.0, 100.0),
        100.0 + b.map(200.0),
        1e-9
    ));
    assert_eq!(apply(50.0, 0.0, 100.0, 800.0, 0.55), 50.0);
}

#[test]
fn the_elastic_motion_has_its_documented_constants() {
    let e = ElasticMotion::default();
    assert_eq!(e.amplitude, 0.31);
    assert_eq!(e.period, 1.6);
    assert_eq!(e.stiffness, 20.0);
    assert_eq!(e.velocity_max, 20000.0);
    assert_eq!(e.settle_threshold, 0.5);
    assert!(close(e.decay_rate(), 12.5, 1e-12));
}

#[test]
fn the_elastic_displacement_follows_the_closed_form() {
    let e = ElasticMotion::default();
    assert_eq!(e.delta(100.0, 0.0, 0.0), 100.0);
    assert!(close(
        e.delta(100.0, 0.0, 0.2),
        100.0 * (-2.5f64).exp(),
        1e-9
    ));
    assert!(close(
        e.delta(10.0, 400.0, 0.1),
        (10.0 + 400.0 * 0.1 * 0.31) * (-1.25f64).exp(),
        1e-9
    ));
    assert_eq!(e.delta(100.0, 0.0, -1.0), 100.0);
}

#[test]
fn the_elastic_velocity_is_clamped_and_matches_the_derivative() {
    let e = ElasticMotion::default();
    assert_eq!(e.delta(0.0, 1e9, 0.1), e.delta(0.0, 20000.0, 0.1));
    for t in [0.05, 0.3] {
        let h = 1e-6;
        let numeric = (e.delta(20.0, 300.0, t + h) - e.delta(20.0, 300.0, t - h)) / (2.0 * h);
        assert!(close(e.velocity(20.0, 300.0, t), numeric, 1e-3), "t = {t}");
    }
    assert!(close(
        e.velocity(20.0, 300.0, 0.0),
        300.0 * 0.31 - 12.5 * 20.0,
        1e-9
    ));
}

#[test]
fn the_elastic_settle_time_is_when_the_displacement_stays_under_half_a_point() {
    let e = ElasticMotion::default();
    let t = e.settle_time(100.0, 0.0);
    assert!(t > 0.42 && t < 0.45, "{t}");
    assert!(e.delta(100.0, 0.0, t + 0.01).abs() < 0.5);
    assert!(e.delta(100.0, 0.0, t - 0.05).abs() > 0.5);
    assert!(close(e.settle_time(0.0, 0.0), 0.01, 1e-9));
}

#[test]
fn the_stretch_filter_defaults_are_documented() {
    let c = RubberBandFilterConfig::default();
    assert_eq!(c.stiffness, 20.0);
    assert_eq!(c.direction_lock_stretch_ratio, 1.0);
    assert_eq!(c.min_delta_before_stretch, 10.0);
}

#[test]
fn the_filter_holds_back_deltas_until_the_minimum_is_reached() {
    let mut f = RubberBandFilter::new(RubberBandFilterConfig::default());
    assert_eq!(f.add_delta([5.0, 0.0]), [0.0, 0.0]);
    assert!(!f.is_stretching());
    assert_eq!(f.add_delta([6.0, 0.0]), [11.0, 0.0]);
    assert!(f.is_stretching());
    assert_eq!(f.add_delta([3.0, 0.0]), [3.0, 0.0]);
}

#[test]
fn the_filter_locks_to_the_dominant_axis() {
    let mut f = RubberBandFilter::default();
    assert_eq!(f.add_delta([20.0, 5.0]), [20.0, 0.0]);
    let mut g = RubberBandFilter::default();
    assert_eq!(g.add_delta([5.0, 20.0]), [0.0, 20.0]);
}

#[test]
fn the_filter_direction_ratio_biases_the_lock() {
    let config = RubberBandFilterConfig {
        direction_lock_stretch_ratio: 0.5,
        ..RubberBandFilterConfig::default()
    };
    let mut f = RubberBandFilter::new(config);
    assert_eq!(f.add_delta([12.0, 20.0]), [0.0, 20.0]);
}

#[test]
fn resetting_the_filter_starts_the_gate_again() {
    let mut f = RubberBandFilter::default();
    f.add_delta([20.0, 0.0]);
    assert!(f.is_stretching());
    f.reset();
    assert!(!f.is_stretching());
    assert_eq!(f.add_delta([4.0, 0.0]), [0.0, 0.0]);
}
