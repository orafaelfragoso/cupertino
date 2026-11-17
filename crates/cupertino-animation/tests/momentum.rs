//! Scroll momentum models.

use cupertino_animation::{Momentum, MomentumModel, QuarticMomentum, QuarticMomentum2D};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

#[test]
fn a_quartic_fling_travels_its_velocity_over_four_constants() {
    // (row, origin, velocity, constant, displacement, duration)
    for (row, origin, velocity, constant, displacement, duration) in [
        (
            "4000 travels 1000 in a second",
            50.0,
            4000.0,
            1000.0,
            1000.0,
            1.0,
        ),
        ("negative mirrors", 0.0, -4000.0, 1000.0, -1000.0, 1.0),
        ("zero velocity", 7.0, 0.0, 1000.0, 0.0, 0.0),
        ("non-positive constant disables", 0.0, 4000.0, 0.0, 0.0, 0.0),
    ] {
        let m = QuarticMomentum::with_constant(origin, velocity, constant);
        assert!(close(m.displacement(), displacement, 1e-9), "{row}");
        assert!(close(m.destination(), origin + displacement, 1e-9), "{row}");
        assert!(close(m.duration(), duration, 1e-12), "{row}");
    }
    let up = QuarticMomentum::new(0.0, 4000.0);
    let down = QuarticMomentum::new(0.0, -4000.0);
    for t in [0.1, 0.3, 0.7] {
        assert!(close(down.position(t), -up.position(t), 1e-9), "t = {t}");
        assert!(close(down.velocity(t), -up.velocity(t), 1e-9), "t = {t}");
    }
    let resting = QuarticMomentum::new(7.0, 0.0);
    assert_eq!(resting.position(1.0), 7.0);
    assert_eq!(resting.velocity(0.0), 0.0);
}

#[test]
fn the_quartic_trajectory_decelerates_from_the_fling_velocity_to_the_destination() {
    let m = QuarticMomentum::new(10.0, 4000.0);
    for (t, position) in [(-1.0, 10.0), (0.0, 10.0), (1.0, 1010.0), (100.0, 1010.0)] {
        assert!(close(m.position(t), position, 1e-9), "position at {t}");
    }
    for (t, velocity) in [(-0.1, 0.0), (0.0, 4000.0), (1.0, 0.0), (5.0, 0.0)] {
        assert!(close(m.velocity(t), velocity, 1e-9), "velocity at {t}");
    }
    let speeds: Vec<f64> = (0..100).map(|i| m.velocity(f64::from(i) * 0.01)).collect();
    assert!(speeds.windows(2).all(|w| w[1] <= w[0]));
    for t in [0.1, 0.3, 0.6] {
        let h = 1e-6;
        let numeric = (m.position(t + h) - m.position(t - h)) / (2.0 * h);
        assert!(close(m.velocity(t), numeric, 1e-3), "t = {t}");
    }
}

#[test]
fn time_to_reach_inverts_the_position_within_the_travel() {
    let m = QuarticMomentum::new(0.0, 4000.0);
    let t = m.time_to_reach(500.0).expect("reached");
    assert!(close(m.position(t), 500.0, 1e-6));
    assert!(close(t, 1.0 - 0.5f64.powf(0.25), 1e-12));
    assert_eq!(m.time_to_reach(0.0), Some(0.0));
    assert!(close(m.time_to_reach(1000.0).unwrap(), m.duration(), 1e-6));
    assert_eq!(m.time_to_reach(1001.0), None);
    assert_eq!(m.time_to_reach(-10.0), None);
}

#[test]
fn the_exponential_destination_is_the_geometric_sum_clamped_to_the_range() {
    // (row, origin, velocity, min, max, destination)
    for (row, origin, velocity, min, max, destination) in [
        (
            "0.998 per millisecond",
            100.0,
            1000.0,
            -1e9,
            1e9,
            100.0 + 0.998 / 0.002,
        ),
        ("clamped above", 0.0, 1000.0, 0.0, 300.0, 300.0),
        ("clamped below", 0.0, -1000.0, -200.0, 0.0, -200.0),
        ("under ten points per second", 5.0, 9.0, -1e9, 1e9, 5.0),
    ] {
        let m = Momentum::new(origin, velocity, min, max);
        assert!(close(m.destination(), destination, 1e-9), "{row}");
    }
}

#[test]
fn the_exponential_position_converges_in_whole_milliseconds_and_stops_under_the_threshold() {
    let m = Momentum::new(0.0, 1000.0, -1e9, 1e9);
    assert!(close(m.position(0.0), 0.0, 1e-12));
    assert!(m.position(0.5) > 0.0 && m.position(0.5) < m.destination());
    assert!(close(m.position(60.0), m.destination(), 1e-3));
    assert_eq!(m.position(0.0104), m.position(0.010));
    let d = m.duration_until_stop(0.01);
    assert!(d > 2.29 && d < 2.31, "{d}");
    assert_eq!(
        Momentum::new(0.0, 5.0, -1.0, 1.0).duration_until_stop(0.01),
        0.0
    );
}

#[test]
fn the_selector_defaults_to_the_quartic_model_and_forwards_to_either() {
    let q = QuarticMomentum::new(10.0, 4000.0);
    let m = MomentumModel::default_for(10.0, 4000.0);
    assert!(matches!(m, MomentumModel::Quartic(_)));
    assert_eq!(m.position(0.4), q.position(0.4));
    assert_eq!(m.velocity(0.4), q.velocity(0.4));
    assert_eq!(m.duration(), q.duration());
    assert_eq!(m.destination(), q.destination());

    let e = MomentumModel::exponential(0.0, 1000.0);
    assert!(matches!(e, MomentumModel::Exponential(_)));
    assert!(close(e.velocity(0.0), 1000.0, 1e-9));
    assert!(e.velocity(0.5) < 1000.0 && e.velocity(0.5) > 0.0);
    assert_eq!(e.velocity(e.duration() + 1.0), 0.0);
    assert!(close(e.destination(), 499.0, 1e-9));
}

#[test]
fn a_two_axis_fling_shares_one_duration_from_the_vector_length() {
    let one = QuarticMomentum::new(5.0, 4000.0);
    let single = QuarticMomentum2D::new([5.0, 9.0], [4000.0, 0.0]);
    assert!(close(single.duration(), one.duration(), 1e-12));
    assert!(close(single.destination()[0], one.destination(), 1e-9));
    assert_eq!(single.destination()[1], 9.0);
    assert!(close(single.position(0.4)[0], one.position(0.4), 1e-9));
    assert!(close(single.velocity(0.4)[0], one.velocity(0.4), 1e-9));

    let diagonal = QuarticMomentum2D::new([0.0, 0.0], [4000.0, -4000.0]);
    assert_eq!(diagonal.displacement, [1000.0, -1000.0]);
    let expected = (1000f64.hypot(1000.0) / 1000.0).powf(0.25);
    assert!(close(diagonal.duration(), expected, 1e-12));
    assert_eq!(diagonal.position(0.0), [0.0, 0.0]);
    let end = diagonal.position(diagonal.duration());
    assert!(close(end[0], 1000.0, 1e-9) && close(end[1], -1000.0, 1e-9));
    let v = diagonal.velocity(0.0);
    assert!(v[0] > 0.0 && close(v[0], -v[1], 1e-9));
    assert_eq!(diagonal.velocity(-0.1), [0.0, 0.0]);
    assert_eq!(diagonal.velocity(diagonal.duration() + 0.1), [0.0, 0.0]);

    let still = QuarticMomentum2D::new([3.0, 4.0], [0.0, 0.0]);
    assert_eq!(still.duration(), 0.0);
    assert_eq!(still.position(1.0), [3.0, 4.0]);
    assert_eq!(still.velocity(0.0), [0.0, 0.0]);
}
