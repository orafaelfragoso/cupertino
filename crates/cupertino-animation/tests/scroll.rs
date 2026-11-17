//! Kinetic scrolling and velocity tracking.

use cupertino_animation::{
    ElasticMotion, KineticScroller, MomentumKind, QuarticMomentum, RubberBand, ScrollConfig,
    VelocityTracker,
};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

fn scroller(min: f64, max: f64, position: f64) -> KineticScroller {
    KineticScroller::new(ScrollConfig::default(), min, max, position)
}

#[test]
fn the_default_configuration_is_documented() {
    let c = ScrollConfig::default();
    assert_eq!(c.momentum, MomentumKind::Quartic);
    assert_eq!(c.dimension, 800.0);
    assert_eq!(c.drag_coefficient, 0.55);
    assert_eq!(c.velocity_window, 0.1);
    assert_eq!(c.min_fling_velocity, 50.0);
    assert_eq!(c.elastic, ElasticMotion::default());
}

#[test]
fn a_new_scroller_rests_at_its_position() {
    let s = scroller(0.0, 1000.0, 120.0);
    assert_eq!(s.position(0.0), 120.0);
    assert_eq!(s.velocity(0.0), 0.0);
    assert_eq!(s.bounds(), (0.0, 1000.0));
    assert!(!s.is_animating(0.0));
    assert!(!s.is_dragging());
    assert_eq!(s.end_time(), None);
}

#[test]
fn an_inverted_range_collapses_to_its_minimum() {
    assert_eq!(scroller(10.0, 5.0, 10.0).bounds(), (10.0, 10.0));
}

#[test]
fn jumping_clamps_to_the_bounds_without_animation() {
    let mut s = scroller(0.0, 1000.0, 0.0);
    s.jump_to(400.0);
    assert_eq!(s.position(0.0), 400.0);
    s.jump_to(5000.0);
    assert_eq!(s.position(0.0), 1000.0);
    s.jump_to(-5.0);
    assert_eq!(s.position(0.0), 0.0);
    assert!(!s.is_animating(0.0));
}

#[test]
fn dragging_inside_the_range_moves_one_to_one() {
    let mut s = scroller(0.0, 1000.0, 200.0);
    s.begin_drag(0.0);
    assert!(s.is_dragging());
    assert!(s.is_animating(0.0));
    s.drag(100.0, 0.1);
    assert!(close(s.position(0.1), 300.0, 1e-9));
    s.drag(-50.0, 0.2);
    assert!(close(s.position(0.2), 250.0, 1e-9));
}

#[test]
fn dragging_past_a_bound_goes_through_the_elastic_curve() {
    let mut s = scroller(0.0, 1000.0, 0.0);
    s.begin_drag(0.0);
    s.drag(-100.0, 0.5);
    let band = RubberBand::new(800.0);
    assert!(close(s.position(0.5), band.map(-100.0), 1e-9));
    assert!(close(s.position(0.5), -51.46, 0.01));
    s.drag(2000.0, 0.6);
    assert!(close(s.position(0.6), 1000.0 + band.map(900.0), 1e-9));
}

#[test]
fn a_new_drag_during_an_overscroll_keeps_the_shown_position() {
    let mut s = scroller(0.0, 1000.0, 0.0);
    s.begin_drag(0.0);
    s.drag(-100.0, 0.5);
    s.end_drag(0.5);
    s.begin_drag(0.6);
    assert!(s.position(0.6) < 0.0);
    s.drag(0.0, 0.7);
    assert!(close(s.position(0.7), s.position(0.6), 1e-9));
}

#[test]
fn releasing_an_overscroll_returns_to_the_edge() {
    let mut s = scroller(0.0, 1000.0, 0.0);
    s.begin_drag(0.0);
    s.drag(-100.0, 50.0);
    assert!(s.position(50.0) < 0.0);
    let v = s.end_drag(50.0);
    assert_eq!(v, 0.0);
    assert!(s.is_animating(50.0));
    let end = s.end_time().expect("returning");
    assert!(end > 50.0);
    assert!(s.position(50.0) < 0.0);
    assert_eq!(s.position(end + 0.01), 0.0);
    assert!(!s.is_animating(end + 0.01));
}

#[test]
fn a_slow_release_inside_the_range_just_stops() {
    let mut s = scroller(0.0, 1000.0, 0.0);
    s.begin_drag(0.0);
    s.drag(1.0, 1.0);
    assert_eq!(s.end_drag(1.0), 0.0);
    assert!(!s.is_animating(1.0));
    assert!(close(s.position(5.0), 1.0, 1e-9));
}

#[test]
fn a_fast_release_flings_by_the_quartic_model() {
    let mut s = scroller(0.0, 100_000.0, 0.0);
    s.begin_drag(0.0);
    for i in 1..=5 {
        s.drag(40.0, f64::from(i) * 0.01);
    }
    let v = s.end_drag(0.05);
    assert!(close(v, 4000.0, 1.0), "{v}");
    let model = QuarticMomentum::new(200.0, v);
    assert!(s.is_animating(0.1));
    assert!(close(s.position(0.05 + 0.3), model.position(0.3), 1e-6));
    let end = s.end_time().unwrap();
    assert!(close(end, 0.05 + model.duration(), 1e-9));
    assert!(close(s.position(end + 1.0), model.destination(), 1e-6));
}

#[test]
fn a_programmatic_fling_travels_the_quartic_distance_and_stops() {
    let mut s = scroller(0.0, 100_000.0, 0.0);
    s.fling_with(4000.0, 0.0);
    assert!(close(s.velocity(0.0), 4000.0, 1e-9));
    assert!(s.is_animating(0.5));
    assert!(close(s.end_time().unwrap(), 1.0, 1e-9));
    assert!(close(s.position(1.0), 1000.0, 1e-6));
    assert!(!s.is_animating(1.0));
    assert!(close(s.velocity(2.0), 0.0, 1e-9));
}

#[test]
fn a_fling_position_never_goes_backwards() {
    let mut s = scroller(0.0, 100_000.0, 0.0);
    s.fling_with(4000.0, 0.0);
    let mut last = 0.0;
    for i in 0..=100 {
        let p = s.position(f64::from(i) * 0.01);
        assert!(p >= last);
        last = p;
    }
}

#[test]
fn a_fling_below_the_minimum_velocity_does_not_start() {
    let mut s = scroller(0.0, 1000.0, 10.0);
    s.fling_with(49.0, 0.0);
    assert!(!s.is_animating(0.0));
    assert_eq!(s.position(1.0), 10.0);
}

#[test]
fn a_fling_into_a_bound_overshoots_and_returns_to_the_edge() {
    let mut s = scroller(0.0, 500.0, 0.0);
    s.fling_with(4000.0, 0.0);
    let end = s.end_time().expect("running");
    let mut peak = 0.0f64;
    for i in 0..=2000 {
        peak = peak.max(s.position(f64::from(i) * end / 2000.0));
    }
    assert!(peak > 500.0, "no overshoot: {peak}");
    assert_eq!(s.position(end + 0.5), 500.0);
    assert!(!s.is_animating(end + 0.5));
}

#[test]
fn a_fling_towards_the_minimum_overshoots_below_it() {
    let mut s = scroller(0.0, 500.0, 100.0);
    s.fling_with(-4000.0, 0.0);
    let end = s.end_time().unwrap();
    let trough = (0..=2000)
        .map(|i| s.position(f64::from(i) * end / 2000.0))
        .fold(f64::MAX, f64::min);
    assert!(trough < 0.0);
    assert_eq!(s.position(end + 0.5), 0.0);
}

#[test]
fn stopping_a_fling_freezes_it_in_place() {
    let mut s = scroller(0.0, 100_000.0, 0.0);
    s.fling_with(4000.0, 0.0);
    let at = s.position(0.4);
    s.stop(0.4);
    assert!(close(s.position(5.0), at, 1e-9));
    assert!(!s.is_animating(5.0));
    assert_eq!(s.velocity(5.0), 0.0);
}

#[test]
fn touching_down_on_a_fling_continues_from_where_it_is() {
    let mut s = scroller(0.0, 100_000.0, 0.0);
    s.fling_with(4000.0, 0.0);
    let at = s.position(0.4);
    s.begin_drag(0.4);
    assert!(close(s.position(0.4), at, 1e-9));
}

#[test]
fn shrinking_the_range_under_a_resting_position_returns_it_elastically() {
    let mut s = scroller(0.0, 1000.0, 800.0);
    s.set_bounds(0.0, 500.0, 10.0);
    assert_eq!(s.bounds(), (0.0, 500.0));
    assert!(s.is_animating(10.0));
    assert!(close(s.position(10.0), 500.0 + 300.0, 1e-9));
    let end = s.end_time().unwrap();
    assert_eq!(s.position(end + 0.1), 500.0);
}

#[test]
fn the_exponential_model_can_be_selected() {
    let cfg = ScrollConfig {
        momentum: MomentumKind::Exponential,
        ..ScrollConfig::default()
    };
    let mut s = KineticScroller::new(cfg, 0.0, 1e9, 0.0);
    s.fling_with(1000.0, 0.0);
    assert!(close(s.velocity(0.0), 1000.0, 1e-9));
    let end = s.end_time().unwrap();
    assert!(end > 2.2 && end < 2.4, "{end}");
    assert!(close(s.position(end + 1.0), 499.0, 1.5));
}

// velocity tracker

#[test]
fn the_tracker_reports_the_slope_over_its_window() {
    let mut t = VelocityTracker::default();
    t.push(0.0, 0.0);
    t.push(0.05, 5.0);
    t.push(0.1, 10.0);
    assert!(close(t.velocity(0.1), 100.0, 1e-9));
}

#[test]
fn the_tracker_needs_two_samples() {
    let mut t = VelocityTracker::new(0.1);
    assert_eq!(t.velocity(0.0), 0.0);
    t.push(0.0, 3.0);
    assert_eq!(t.velocity(0.0), 0.0);
}

#[test]
fn the_tracker_reports_zero_once_the_newest_sample_is_stale() {
    let mut t = VelocityTracker::new(0.1);
    t.push(0.0, 0.0);
    t.push(0.05, 5.0);
    assert!(close(t.velocity(0.1), 100.0, 1e-9));
    assert_eq!(t.velocity(0.2), 0.0);
}

#[test]
fn the_tracker_drops_samples_older_than_the_window() {
    let mut t = VelocityTracker::new(0.1);
    t.push(0.0, 0.0);
    t.push(0.05, 100.0);
    t.push(0.3, 100.0);
    t.push(0.35, 100.0);
    assert!(close(t.velocity(0.35), 0.0, 1e-9));
}

#[test]
fn a_sample_from_the_past_clears_the_window() {
    let mut t = VelocityTracker::new(0.1);
    t.push(1.0, 0.0);
    t.push(1.05, 50.0);
    t.push(0.5, 0.0);
    assert_eq!(t.velocity(0.5), 0.0);
}

#[test]
fn the_tracker_is_least_squares_not_end_to_end() {
    let mut t = VelocityTracker::new(1.0);
    for (time, value) in [
        (0.0, 0.0),
        (0.1, 10.0),
        (0.2, 20.0),
        (0.3, 30.0),
        (0.4, 80.0),
    ] {
        t.push(time, value);
    }
    let v = t.velocity(0.4);
    assert!(v > 100.0 && v < 200.0, "{v}");
}

#[test]
fn reset_forgets_the_samples() {
    let mut t = VelocityTracker::default();
    t.push(0.0, 0.0);
    t.push(0.05, 5.0);
    t.reset();
    assert_eq!(t.velocity(0.05), 0.0);
}
