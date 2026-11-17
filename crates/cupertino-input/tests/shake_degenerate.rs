//! Degenerate reversal points (a vertical line, one repeated position) never read as a shake; this records the current behaviour.

use cupertino_input::{Point, ShakeDetector};

/// Feed `samples` to the detector one every 10 ms; true when any motion reported a shake.
fn feed(d: &mut ShakeDetector, samples: &[(f64, f64)]) -> bool {
    let mut triggered = false;
    for (i, &(x, y)) in samples.iter().enumerate() {
        triggered |= d.motion(Point::new(x, y), i as f64 * 0.01);
    }
    triggered
}

#[test]
fn an_exactly_vertical_shake_is_recognised_by_the_circle_test() {
    // Every slope between reversal points is infinite, so the line test is NaN and fails; the
    // points alternate between two quadrants around the centroid, all outside the radius,
    // which the circle test counts as a rotation.
    let mut d = ShakeDetector::new();
    let samples: Vec<(f64, f64)> = (0..60)
        .map(|i| (100.0, if i % 2 == 0 { 0.0 } else { 300.0 }))
        .collect();
    assert!(feed(&mut d, &samples));
    assert!(d.multiplier() > 1.0);
}

#[test]
fn coincident_reversal_points_are_not_a_shake() {
    // Every reversal happens at one position: the slopes are 0 / 0 and no point is outside
    // the radius.
    let mut d = ShakeDetector::new();
    let mut samples = vec![(0.0, 300.0), (10.0, 300.0)];
    for i in 0..40 {
        samples.push((0.0, 300.0));
        samples.push((if i % 2 == 0 { -5.0 } else { 5.0 }, 300.0));
    }
    assert!(!feed(&mut d, &samples));
    assert_eq!(d.evaluate(1.0), 1.0);
}
