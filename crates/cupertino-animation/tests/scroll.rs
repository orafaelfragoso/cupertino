//! Kinetic scrolling and velocity tracking.

use cupertino_animation::{
    KineticScroller, MomentumKind, QuarticMomentum, RubberBand, ScrollConfig, VelocityTracker,
};

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() < tol
}

fn scroller(min: f64, max: f64, position: f64) -> KineticScroller {
    KineticScroller::new(ScrollConfig::default(), min, max, position)
}

#[test]
fn a_resting_scroller_holds_its_position_and_jumps_within_the_bounds() {
    let mut s = scroller(0.0, 1000.0, 120.0);
    assert_eq!(s.position(0.0), 120.0);
    assert_eq!(s.velocity(0.0), 0.0);
    assert_eq!(s.bounds(), (0.0, 1000.0));
    assert!(!s.is_animating(0.0));
    assert!(!s.is_dragging());
    assert_eq!(s.end_time(), None);
    for (to, shown) in [(400.0, 400.0), (5000.0, 1000.0), (-5.0, 0.0)] {
        s.jump_to(to);
        assert_eq!(s.position(0.0), shown, "jump to {to}");
        assert!(!s.is_animating(0.0));
    }
    assert_eq!(
        scroller(10.0, 5.0, 10.0).bounds(),
        (10.0, 10.0),
        "inverted range"
    );
}

#[test]
fn dragging_moves_one_to_one_inside_the_range_and_elastically_past_it() {
    let mut s = scroller(0.0, 1000.0, 200.0);
    s.begin_drag(0.0);
    assert!(s.is_dragging() && s.is_animating(0.0));
    s.drag(100.0, 0.1);
    assert!(close(s.position(0.1), 300.0, 1e-9));
    s.drag(-50.0, 0.2);
    assert!(close(s.position(0.2), 250.0, 1e-9));

    let band = RubberBand::new(800.0);
    let mut edge = scroller(0.0, 1000.0, 0.0);
    edge.begin_drag(0.0);
    edge.drag(-100.0, 0.5);
    assert!(close(edge.position(0.5), band.map(-100.0), 1e-9));
    assert!(close(edge.position(0.5), -51.46, 0.01));
    edge.drag(2000.0, 0.6);
    assert!(close(edge.position(0.6), 1000.0 + band.map(900.0), 1e-9));
}

#[test]
fn touching_down_keeps_the_position_shown() {
    let mut over = scroller(0.0, 1000.0, 0.0);
    over.begin_drag(0.0);
    over.drag(-100.0, 0.5);
    over.end_drag(0.5);
    over.begin_drag(0.6);
    assert!(over.position(0.6) < 0.0, "during an overscroll return");
    over.drag(0.0, 0.7);
    assert!(close(over.position(0.7), over.position(0.6), 1e-9));

    let mut flung = scroller(0.0, 100_000.0, 0.0);
    flung.fling_with(4000.0, 0.0);
    let at = flung.position(0.4);
    flung.begin_drag(0.4);
    assert!(close(flung.position(0.4), at, 1e-9), "during a fling");
}

#[test]
fn releasing_an_overscroll_returns_to_the_edge() {
    let mut s = scroller(0.0, 1000.0, 0.0);
    s.begin_drag(0.0);
    s.drag(-100.0, 50.0);
    assert_eq!(s.end_drag(50.0), 0.0);
    assert!(s.is_animating(50.0));
    assert!(s.position(50.0) < 0.0);
    let end = s.end_time().expect("returning");
    assert!(end > 50.0);
    assert_eq!(s.position(end + 0.01), 0.0);
    assert!(!s.is_animating(end + 0.01));
}

#[test]
fn releasing_a_drag_flings_by_the_quartic_model_only_when_fast() {
    let mut slow = scroller(0.0, 1000.0, 0.0);
    slow.begin_drag(0.0);
    slow.drag(1.0, 1.0);
    assert_eq!(slow.end_drag(1.0), 0.0);
    assert!(!slow.is_animating(1.0));
    assert!(close(slow.position(5.0), 1.0, 1e-9));

    let mut fast = scroller(0.0, 100_000.0, 0.0);
    fast.begin_drag(0.0);
    for i in 1..=5 {
        fast.drag(40.0, f64::from(i) * 0.01);
    }
    let v = fast.end_drag(0.05);
    assert!(close(v, 4000.0, 1.0), "{v}");
    let model = QuarticMomentum::new(200.0, v);
    assert!(fast.is_animating(0.1));
    assert!(close(fast.position(0.05 + 0.3), model.position(0.3), 1e-6));
    let end = fast.end_time().unwrap();
    assert!(close(end, 0.05 + model.duration(), 1e-9));
    assert!(close(fast.position(end + 1.0), model.destination(), 1e-6));
}

#[test]
fn a_programmatic_fling_travels_forwards_the_quartic_distance_above_the_minimum_velocity() {
    let mut s = scroller(0.0, 100_000.0, 0.0);
    s.fling_with(4000.0, 0.0);
    assert!(close(s.velocity(0.0), 4000.0, 1e-9));
    assert!(s.is_animating(0.5));
    assert!(close(s.end_time().unwrap(), 1.0, 1e-9));
    let path: Vec<f64> = (0..=100).map(|i| s.position(f64::from(i) * 0.01)).collect();
    assert!(
        path.windows(2).all(|w| w[1] >= w[0]),
        "never goes backwards"
    );
    assert!(close(s.position(1.0), 1000.0, 1e-6));
    assert!(!s.is_animating(1.0));
    assert!(close(s.velocity(2.0), 0.0, 1e-9));

    let mut weak = scroller(0.0, 1000.0, 10.0);
    weak.fling_with(49.0, 0.0);
    assert!(!weak.is_animating(0.0));
    assert_eq!(weak.position(1.0), 10.0);
}

#[test]
fn a_fling_into_a_bound_overshoots_and_returns_to_the_edge() {
    // (start, velocity, edge)
    for (start, velocity, edge) in [(0.0, 4000.0, 500.0), (100.0, -4000.0, 0.0)] {
        let mut s = scroller(0.0, 500.0, start);
        s.fling_with(velocity, 0.0);
        let end = s.end_time().expect("running");
        let beyond = (0..=2000)
            .map(|i| (s.position(f64::from(i) * end / 2000.0) - edge) * velocity.signum())
            .fold(f64::MIN, f64::max);
        assert!(beyond > 0.0, "no overshoot towards {edge}");
        assert_eq!(s.position(end + 0.5), edge);
        assert!(!s.is_animating(end + 0.5));
    }
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
fn shrinking_the_range_under_a_resting_position_returns_it_elastically() {
    let mut s = scroller(0.0, 1000.0, 800.0);
    s.set_bounds(0.0, 500.0, 10.0);
    assert_eq!(s.bounds(), (0.0, 500.0));
    assert!(s.is_animating(10.0));
    assert!(close(s.position(10.0), 800.0, 1e-9));
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

#[test]
fn the_tracker_fits_a_least_squares_slope_over_its_window() {
    let mut t = VelocityTracker::default();
    t.push(0.0, 0.0);
    t.push(0.05, 5.0);
    t.push(0.1, 10.0);
    assert!(close(t.velocity(0.1), 100.0, 1e-9));

    let mut long = VelocityTracker::new(1.0);
    for (time, value) in [
        (0.0, 0.0),
        (0.1, 10.0),
        (0.2, 20.0),
        (0.3, 30.0),
        (0.4, 80.0),
    ] {
        long.push(time, value);
    }
    let v = long.velocity(0.4);
    assert!(v > 100.0 && v < 200.0, "not end to end: {v}");
}

#[test]
fn the_tracker_reports_zero_without_a_usable_window() {
    type Row = (&'static str, &'static [(f64, f64)], f64);
    // (row, samples, now)
    let rows: [Row; 5] = [
        ("no samples", &[], 0.0),
        ("one sample", &[(0.0, 3.0)], 0.0),
        ("newest sample stale", &[(0.0, 0.0), (0.05, 5.0)], 0.2),
        (
            "older samples dropped",
            &[(0.0, 0.0), (0.05, 100.0), (0.3, 100.0), (0.35, 100.0)],
            0.35,
        ),
        (
            "a sample from the past clears",
            &[(1.0, 0.0), (1.05, 50.0), (0.5, 0.0)],
            0.5,
        ),
    ];
    for (row, samples, now) in rows {
        let mut t = VelocityTracker::new(0.1);
        for &(time, value) in samples {
            t.push(time, value);
        }
        assert!(close(t.velocity(now), 0.0, 1e-9), "{row}");
    }
    let mut reset = VelocityTracker::default();
    reset.push(0.0, 0.0);
    reset.push(0.05, 5.0);
    reset.reset();
    assert_eq!(reset.velocity(0.05), 0.0, "reset forgets the samples");
}
