//! Elastic overscroll mapping, the elastic return and the stretch filter.

use cupertino_animation::rubber_band::{apply, inverse, rubber_band, slope};
use cupertino_animation::{ElasticMotion, RubberBand, RubberBandFilter, RubberBandFilterConfig};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

#[test]
fn the_mapping_is_odd_monotonic_and_bounded_by_the_dimension() {
    let b = RubberBand::new(800.0);
    // 800 * (1 - 1 / (120 * 0.55 / 800 + 1))
    assert!(close(b.map(120.0), 60.970, 0.01));
    assert_eq!(b.map(0.0), 0.0);
    for x in [1.0, 50.0, 400.0, 5000.0] {
        assert_eq!(b.map(-x), -b.map(x), "x = {x}");
    }
    let shown: Vec<f64> = (0..=200).map(|i| b.map(f64::from(i) * 100.0)).collect();
    assert!(shown.windows(2).all(|w| w[1] > w[0]));
    assert!(shown.iter().all(|&y| y < 800.0));
    for dimension in [0.0, -5.0] {
        assert_eq!(
            rubber_band(100.0, dimension, 0.55),
            0.0,
            "dimension {dimension}"
        );
    }
}

#[test]
fn the_slope_starts_at_the_coefficient_and_is_the_derivative() {
    let b = RubberBand::new(800.0);
    assert!(close(b.slope(0.0), 0.55, 1e-12));
    assert!(close(
        RubberBand::with_coefficient(800.0, 0.3).slope(0.0),
        0.3,
        1e-12
    ));
    for x in [10.0, 200.0, 1500.0] {
        let h = 1e-4;
        let numeric = (b.map(x + h) - b.map(x - h)) / (2.0 * h);
        assert!(close(b.slope(x), numeric, 1e-6), "x = {x}");
    }
    assert_eq!(slope(1.0, 0.0, 0.55), 0.0);
}

#[test]
fn the_inverse_undoes_the_mapping_and_is_infinite_at_the_dimension() {
    let b = RubberBand::new(800.0);
    for x in [-300.0, -1.0, 1.0, 70.0, 900.0] {
        assert!(close(b.inverse(b.map(x)), x, 1e-6), "x = {x}");
    }
    assert_eq!(b.inverse(800.0), f64::INFINITY);
    assert_eq!(b.inverse(-900.0), f64::NEG_INFINITY);
    assert_eq!(b.inverse(0.0), 0.0);
    assert_eq!(inverse(10.0, 0.0, 0.55), 0.0, "no dimension");
    assert_eq!(inverse(10.0, 800.0, 0.0), 0.0, "no coefficient");
}

#[test]
fn apply_leaves_positions_inside_the_range_and_stretches_outside() {
    let b = RubberBand::new(800.0);
    for (position, expected) in [
        (50.0, 50.0),
        (0.0, 0.0),
        (100.0, 100.0),
        (-100.0, -b.map(100.0)),
        (300.0, 100.0 + b.map(200.0)),
    ] {
        assert!(
            close(b.apply(position, 0.0, 100.0), expected, 1e-9),
            "{position}"
        );
        assert!(
            close(apply(position, 0.0, 100.0, 800.0, 0.55), expected, 1e-9),
            "free function at {position}"
        );
    }
}

#[test]
fn the_elastic_return_follows_its_closed_form_with_a_clamped_velocity() {
    let e = ElasticMotion::default();
    // (origin, velocity, t, displacement)
    for (origin, velocity, t, expected) in [
        (100.0, 0.0, 0.0, 100.0),
        (100.0, 0.0, -1.0, 100.0),
        (100.0, 0.0, 0.2, 100.0 * (-2.5f64).exp()),
        (
            10.0,
            400.0,
            0.1,
            (10.0 + 400.0 * 0.1 * 0.31) * (-1.25f64).exp(),
        ),
    ] {
        assert!(
            close(e.delta(origin, velocity, t), expected, 1e-9),
            "({origin}, {velocity}, {t})"
        );
    }
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
fn the_filter_holds_back_deltas_until_the_minimum_and_reset_restarts_the_gate() {
    let mut f = RubberBandFilter::new(RubberBandFilterConfig::default());
    assert_eq!(f.add_delta([5.0, 0.0]), [0.0, 0.0]);
    assert!(!f.is_stretching());
    assert_eq!(f.add_delta([6.0, 0.0]), [11.0, 0.0]);
    assert!(f.is_stretching());
    assert_eq!(f.add_delta([3.0, 0.0]), [3.0, 0.0]);
    f.reset();
    assert!(!f.is_stretching());
    assert_eq!(f.add_delta([4.0, 0.0]), [0.0, 0.0]);
}

#[test]
fn the_filter_locks_to_the_dominant_axis_biased_by_the_ratio() {
    // (ratio, delta, passed)
    for (ratio, delta, passed) in [
        (1.0, [20.0, 5.0], [20.0, 0.0]),
        (1.0, [5.0, 20.0], [0.0, 20.0]),
        (0.5, [12.0, 20.0], [0.0, 20.0]),
    ] {
        let mut f = RubberBandFilter::new(RubberBandFilterConfig {
            direction_lock_stretch_ratio: ratio,
            ..RubberBandFilterConfig::default()
        });
        assert_eq!(f.add_delta(delta), passed, "ratio {ratio}, {delta:?}");
    }
}
