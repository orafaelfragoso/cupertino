//! Scroll momentum models.

use cupertino_animation::{Momentum, MomentumModel, QuarticMomentum, QuarticMomentum2D};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

// quartic model

#[test]
fn the_quartic_constant_is_one_thousand() {
    assert_eq!(QuarticMomentum::CONSTANT, 1000.0);
    assert_eq!(QuarticMomentum::new(0.0, 1.0).constant, 1000.0);
}

#[test]
fn a_fling_of_4000_travels_1000_points_in_one_second() {
    let m = QuarticMomentum::new(50.0, 4000.0);
    assert!(close(m.displacement(), 1000.0, 1e-9));
    assert!(close(m.destination(), 1050.0, 1e-9));
    assert!(close(m.duration(), 1.0, 1e-12));
}

#[test]
fn a_negative_velocity_mirrors_the_motion() {
    let up = QuarticMomentum::new(0.0, 4000.0);
    let down = QuarticMomentum::new(0.0, -4000.0);
    assert!(close(down.displacement(), -up.displacement(), 1e-9));
    assert!(close(down.duration(), up.duration(), 1e-12));
    assert!(close(down.position(0.3), -up.position(0.3), 1e-9));
    assert!(close(down.velocity(0.3), -up.velocity(0.3), 1e-9));
}

#[test]
fn zero_velocity_goes_nowhere() {
    let m = QuarticMomentum::new(7.0, 0.0);
    assert_eq!(m.displacement(), 0.0);
    assert_eq!(m.duration(), 0.0);
    assert_eq!(m.position(1.0), 7.0);
    assert_eq!(m.velocity(0.0), 0.0);
}

#[test]
fn a_non_positive_constant_disables_the_fling() {
    let m = QuarticMomentum::with_constant(0.0, 4000.0, 0.0);
    assert_eq!(m.displacement(), 0.0);
    assert_eq!(m.duration(), 0.0);
}

#[test]
fn the_quartic_position_starts_at_the_origin_and_ends_at_the_destination() {
    let m = QuarticMomentum::new(10.0, 4000.0);
    assert!(close(m.position(0.0), 10.0, 1e-12));
    assert!(close(m.position(m.duration()), m.destination(), 1e-9));
    assert!(close(m.position(100.0), m.destination(), 1e-9));
    assert!(close(m.position(-1.0), 10.0, 1e-12));
}

#[test]
fn the_quartic_velocity_starts_at_four_c_t_cubed_and_falls_to_zero() {
    let m = QuarticMomentum::new(0.0, 4000.0);
    assert!(close(m.velocity(0.0), 4000.0, 1e-9));
    assert_eq!(m.velocity(m.duration()), 0.0);
    assert_eq!(m.velocity(5.0), 0.0);
    assert_eq!(m.velocity(-0.1), 0.0);
    let mut last = f64::MAX;
    for i in 0..100 {
        let v = m.velocity(f64::from(i) * 0.01);
        assert!(v <= last);
        last = v;
    }
}

#[test]
fn the_quartic_velocity_is_the_derivative_of_the_position() {
    let m = QuarticMomentum::new(0.0, 3000.0);
    for t in [0.1, 0.3, 0.6] {
        let h = 1e-6;
        let numeric = (m.position(t + h) - m.position(t - h)) / (2.0 * h);
        assert!(close(m.velocity(t), numeric, 1e-3), "t = {t}");
    }
}

#[test]
fn time_to_reach_inverts_the_position() {
    let m = QuarticMomentum::new(0.0, 4000.0);
    let t = m.time_to_reach(500.0).expect("reached");
    assert!(close(m.position(t), 500.0, 1e-6));
    assert!(close(t, 1.0 - 0.5f64.powf(0.25), 1e-12));
}

#[test]
fn time_to_reach_handles_the_origin_and_unreachable_bounds() {
    let m = QuarticMomentum::new(0.0, 4000.0);
    assert_eq!(m.time_to_reach(0.0), Some(0.0));
    assert_eq!(m.time_to_reach(1001.0), None);
    assert_eq!(m.time_to_reach(-10.0), None);
    let end = m.time_to_reach(1000.0).unwrap();
    assert!(close(end, m.duration(), 1e-6));
}

// per-millisecond model

#[test]
fn the_exponential_model_decelerates_by_0_998_per_millisecond() {
    assert_eq!(Momentum::REFERENCE_DECELERATION, 0.998);
    assert_eq!(Momentum::new(0.0, 1000.0, -1e9, 1e9).factor_per_ms, 0.998);
}

#[test]
fn the_exponential_displacement_is_the_geometric_sum() {
    let m = Momentum::new(100.0, 1000.0, -1e9, 1e9);
    assert!(close(m.displacement(), 1.0 * 0.998 / 0.002, 1e-9));
    assert!(close(m.destination(), 100.0 + 499.0, 1e-9));
}

#[test]
fn the_exponential_destination_is_clamped_to_the_range() {
    let m = Momentum::new(0.0, 1000.0, 0.0, 300.0);
    assert_eq!(m.destination(), 300.0);
    let back = Momentum::new(0.0, -1000.0, -200.0, 0.0);
    assert_eq!(back.destination(), -200.0);
}

#[test]
fn a_fling_slower_than_ten_points_per_second_does_not_move() {
    let m = Momentum::new(5.0, 9.0, -1e9, 1e9);
    assert_eq!(m.displacement(), 0.0);
    assert_eq!(m.destination(), 5.0);
}

#[test]
fn the_exponential_position_converges_on_the_destination() {
    let m = Momentum::new(0.0, 1000.0, -1e9, 1e9);
    assert!(close(m.position(0.0), 0.0, 1e-12));
    assert!(m.position(0.5) > 0.0 && m.position(0.5) < m.destination());
    assert!(close(m.position(60.0), m.destination(), 1e-3));
}

#[test]
fn the_exponential_position_advances_in_whole_milliseconds() {
    let m = Momentum::new(0.0, 1000.0, -1e9, 1e9);
    assert_eq!(m.position(0.0104), m.position(0.010));
}

#[test]
fn the_exponential_model_stops_when_the_velocity_falls_below_the_threshold() {
    let m = Momentum::new(0.0, 1000.0, -1e9, 1e9);
    let d = m.duration_until_stop(0.01);
    assert!(d > 2.29 && d < 2.31, "{d}");
    assert_eq!(
        Momentum::new(0.0, 5.0, -1.0, 1.0).duration_until_stop(0.01),
        0.0
    );
}

// selector

#[test]
fn the_default_model_is_the_quartic_one() {
    assert!(matches!(
        MomentumModel::default_for(0.0, 4000.0),
        MomentumModel::Quartic(_)
    ));
    assert!(matches!(
        MomentumModel::exponential(0.0, 4000.0),
        MomentumModel::Exponential(_)
    ));
}

#[test]
fn the_selector_forwards_to_the_quartic_model() {
    let q = QuarticMomentum::new(10.0, 4000.0);
    let m = MomentumModel::default_for(10.0, 4000.0);
    assert_eq!(m.position(0.4), q.position(0.4));
    assert_eq!(m.velocity(0.4), q.velocity(0.4));
    assert_eq!(m.duration(), q.duration());
    assert_eq!(m.destination(), q.destination());
}

#[test]
fn the_exponential_selector_reports_zero_velocity_after_the_stop() {
    let m = MomentumModel::exponential(0.0, 1000.0);
    assert!(close(m.velocity(0.0), 1000.0, 1e-9));
    assert!(m.velocity(0.5) < 1000.0 && m.velocity(0.5) > 0.0);
    assert_eq!(m.velocity(m.duration() + 1.0), 0.0);
    assert!(close(m.destination(), 499.0, 1e-9));
}

// two axes

#[test]
fn the_two_axis_model_matches_the_one_axis_model_on_a_single_axis() {
    let one = QuarticMomentum::new(5.0, 4000.0);
    let two = QuarticMomentum2D::new([5.0, 9.0], [4000.0, 0.0]);
    assert!(close(two.duration(), one.duration(), 1e-12));
    assert!(close(two.destination()[0], one.destination(), 1e-9));
    assert_eq!(two.destination()[1], 9.0);
    assert!(close(two.position(0.4)[0], one.position(0.4), 1e-9));
    assert!(close(two.velocity(0.4)[0], one.velocity(0.4), 1e-9));
}

#[test]
fn both_axes_share_one_duration_from_the_vector_length() {
    let two = QuarticMomentum2D::new([0.0, 0.0], [4000.0, 4000.0]);
    assert_eq!(two.displacement, [1000.0, 1000.0]);
    let expected = (1000f64.hypot(1000.0) / 1000.0).powf(0.25);
    assert!(close(two.duration(), expected, 1e-12));
    let end = two.position(two.duration());
    assert!(close(end[0], 1000.0, 1e-9) && close(end[1], 1000.0, 1e-9));
    assert_eq!(two.position(0.0), [0.0, 0.0]);
}

#[test]
fn the_two_axis_velocity_is_zero_outside_the_motion() {
    let two = QuarticMomentum2D::new([0.0, 0.0], [4000.0, -4000.0]);
    assert_eq!(two.velocity(-0.1), [0.0, 0.0]);
    assert_eq!(two.velocity(two.duration() + 0.1), [0.0, 0.0]);
    let v = two.velocity(0.0);
    assert!(v[0] > 0.0 && v[1] < 0.0);
    assert!(close(v[0], -v[1], 1e-9));
}

#[test]
fn a_two_axis_fling_without_velocity_stays_put() {
    let two = QuarticMomentum2D::new([3.0, 4.0], [0.0, 0.0]);
    assert_eq!(two.duration(), 0.0);
    assert_eq!(two.position(1.0), [3.0, 4.0]);
    assert_eq!(two.velocity(0.0), [0.0, 0.0]);
}
