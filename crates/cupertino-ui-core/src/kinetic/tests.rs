//! Traces of the scroll motion: inputs at given times, positions read at given times.

use super::*;

/// A frame at 60 Hz, in seconds.
const FRAME: f64 = 1.0 / 60.0;

/// A vertical document 4000 points long in a 470 × 698 viewport, at `y`.
fn document(y: f64) -> ScrollMotion {
    ScrollMotion::new(
        Point::new(0.0, y),
        Point::new(0.0, 4000.0 - 698.0),
        Size::new(470.0, 698.0),
        (false, true),
    )
}

/// A canvas that scrolls both ways, at `(x, y)`.
fn canvas(x: f64, y: f64) -> ScrollMotion {
    ScrollMotion::new(
        Point::new(x, y),
        Point::new(900.0 - 470.0, 4000.0 - 698.0),
        Size::new(470.0, 698.0),
        (true, true),
    )
}

/// Drag by `dy` once a frame for `n` frames from `t`; returns the time of the last movement.
fn pull(m: &mut ScrollMotion, dy: f64, n: usize, t: f64) -> f64 {
    let mut at = t;
    for i in 0..n {
        at = t + i as f64 * FRAME;
        m.drag(Point::new(0.0, dy), at);
    }
    at
}

fn close(a: f64, b: f64, within: f64) -> bool {
    (a - b).abs() <= within
}

#[test]
fn inside_the_range_the_content_follows_the_finger_one_for_one() {
    let mut m = document(600.0);
    m.drag(Point::new(0.0, 30.0), 0.0);
    m.drag(Point::new(0.0, 30.0), FRAME);
    m.drag(Point::new(0.0, 30.0), 2.0 * FRAME);
    assert_eq!(m.offset(2.0 * FRAME), Point::new(0.0, 690.0));
    assert_eq!(m.stretch(2.0 * FRAME), Point::ZERO);
    assert_eq!(m.phases(0.1), [Phase::Rest, Phase::Dragging]);
}

/// Eight pulls of 30 points past the top stretch the content about 2 points each, 17 in all: a restrained stretch,
/// not the sheet moving one for one.
#[test]
fn a_pull_past_the_edge_stretches_against_resistance() {
    let mut m = document(0.0);
    let mut stretches = Vec::new();
    for i in 0..8 {
        let t = i as f64 * FRAME;
        m.drag(Point::new(0.0, -30.0), t);
        stretches.push(-m.stretch(t).y);
    }
    assert!(close(stretches[0], 2.1, 0.2), "{stretches:?}");
    assert!(close(stretches[7], 17.0, 0.7), "{stretches:?}");
    assert!(stretches.windows(2).all(|w| w[1] > w[0]));
    let steps: Vec<f64> = stretches.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        steps.windows(2).all(|w| w[1] <= w[0] + 1e-9),
        "each pull shows less: {steps:?}"
    );
    assert_eq!(m.offset(1.0).y, 0.0, "the logical offset stays at the edge");
    // never more than the viewport, however far the finger goes
    m.drag(Point::new(0.0, -1e7), 1.0);
    assert!(-m.stretch(1.0).y < 698.0);
}

/// A finger that holds a 17 point stretch and lifts lets it go back without overshooting into the content: within
/// half a point of the edge about 0.28 s after the release, at rest soon after.
#[test]
fn a_released_stretch_returns_to_its_edge_in_about_a_quarter_second() {
    let mut m = document(0.0);
    let last = pull(&mut m, -30.0, 8, 0.0);
    let release = last + 0.3;
    m.release(release, true);
    assert_eq!(m.phases(release), [Phase::Rest, Phase::Returning]);
    let shown = |t: f64| m.shown(release + t).y;
    assert!(shown(0.0) < -16.0);
    assert!(shown(0.01) > shown(0.0), "it starts back at once");
    let first_within_half = (0..1000)
        .map(|i| i as f64 * 0.001)
        .find(|t| shown(*t) > -0.5)
        .unwrap();
    assert!(
        (0.25..0.31).contains(&first_within_half),
        "{first_within_half}"
    );
    assert!(
        (0..1000).all(|i| shown(i as f64 * 0.001) <= 0.0),
        "no crossing into the content"
    );
    assert!(!m.is_animating(release + 0.4));
    assert_eq!(m.shown(release + 0.4), Point::ZERO);
}

/// Moving back from a stretch unwinds it along the same curve; the movement that reaches the edge stops there and
/// the next one moves the content.
#[test]
fn a_reversal_unwinds_the_stretch_and_stops_at_the_edge() {
    let mut m = document(0.0);
    let t = pull(&mut m, -30.0, 8, 0.0);
    let before = m.stretch(t).y;
    m.drag(Point::new(0.0, 30.0), t + FRAME);
    let after = m.stretch(t + FRAME).y;
    assert!(after > before && after < 0.0, "{before} {after}");
    assert!(close(after - before, 2.0, 0.3), "back the way it came");
    // far more than the pull left: the content stops at the edge
    m.drag(Point::new(0.0, 500.0), t + 2.0 * FRAME);
    assert_eq!(m.shown(t + 2.0 * FRAME), Point::ZERO);
    m.drag(Point::new(0.0, 30.0), t + 3.0 * FRAME);
    assert_eq!(m.shown(t + 3.0 * FRAME), Point::new(0.0, 30.0));
}

/// A new drag catches a returning stretch where it is drawn, and its first inward movement takes it to the edge
/// and no further.
#[test]
fn a_new_drag_during_the_return_catches_the_stretch() {
    let mut m = document(0.0);
    let t = pull(&mut m, -40.0, 3, 0.0);
    m.release(t + 0.17, true);
    let catch = t + 0.17 + 0.15;
    let held = m.stretch(catch).y;
    assert!(held < -0.5, "{held}");
    m.drag(Point::ZERO, catch);
    assert!(close(m.stretch(catch).y, held, 1e-9), "caught where drawn");
    assert_eq!(m.phases(catch + 0.5)[1], Phase::Dragging, "held");
    m.drag(Point::new(0.0, 30.0), catch + FRAME);
    assert_eq!(m.shown(catch + FRAME), Point::ZERO);
}

/// A flick leaves kinetic motion that slows exponentially and comes to rest the launch speed times the time
/// constant away; it is drawn at rest from then on.
#[test]
fn a_flick_coasts_and_slows_exponentially() {
    let mut m = document(1000.0);
    let last = pull(&mut m, 30.0, 6, 0.0);
    let release = last + 0.008;
    let v0 = m.velocity(last).y;
    assert!(close(v0, 1800.0, 1.0), "{v0}");
    m.release(release, true);
    assert_eq!(m.phases(release)[1], Phase::Coasting);
    let from = m.shown(release).y;
    assert_eq!(from, 1180.0);
    let v = |t: f64| m.velocity(release + t).y;
    assert!(close(v(0.0), v0, 1.0));
    assert!(close(
        v(MOMENTUM_TIME_CONSTANT),
        v0 / std::f64::consts::E,
        1.0
    ));
    let rest = from + v0 * MOMENTUM_TIME_CONSTANT;
    assert!(m.shown(release + 1.0).y < rest);
    assert!((1..400).all(|i| {
        let t = release + i as f64 * 0.01;
        m.shown(t + 0.01).y >= m.shown(t).y
    }));
    assert!(m.is_animating(release + 2.0));
    assert!(!m.is_animating(release + 10.0));
    assert!(close(m.offset(release + 10.0).y, rest, 1e-9));
    assert_eq!(m.stretch(release + 10.0), Point::ZERO);
}

/// Kinetic motion that reaches an edge goes a little past it and comes back, never far.
#[test]
fn kinetic_motion_into_an_edge_bounces_back() {
    let mut m = document(400.0);
    let last = pull(&mut m, -40.0, 6, 0.0);
    m.release(last + 0.008, true);
    let start = last + 0.008;
    let samples: Vec<f64> = (0..300)
        .map(|i| m.shown(start + i as f64 * 0.005).y)
        .collect();
    let deepest = samples.iter().copied().fold(f64::MAX, f64::min);
    assert!(deepest < -2.0 && deepest > -40.0, "{deepest}");
    assert!(samples.iter().any(|y| *y < 0.0));
    assert!(!m.is_animating(start + 2.0));
    assert_eq!(m.shown(start + 2.0), Point::ZERO);
    assert!(
        (0..300).all(|i| m.offset(start + i as f64 * 0.005).y >= 0.0),
        "the logical offset stays in the range"
    );
}

/// A finger that rests before lifting, or moves too slowly, leaves no kinetic motion.
#[test]
fn a_still_or_slow_release_does_not_coast() {
    let mut m = document(1000.0);
    let last = pull(&mut m, 30.0, 6, 0.0);
    m.release(last + RELEASE_PAUSE + 0.01, true);
    assert_eq!(m.phases(last + 0.1), [Phase::Rest, Phase::Rest]);
    assert_eq!(m.offset(last + 1.0).y, 1180.0);
    let mut slow = document(1000.0);
    let last = pull(&mut slow, 0.5, 6, 0.0);
    slow.release(last + 0.008, true);
    assert!(!slow.is_animating(last + 0.01));
}

/// Without inertia (Reduce Motion) a flick stops where it is lifted; a stretch still returns.
#[test]
fn without_inertia_a_flick_stops_and_a_stretch_still_returns() {
    let mut m = document(1000.0);
    let last = pull(&mut m, 30.0, 6, 0.0);
    m.release(last + 0.008, false);
    assert!(!m.is_animating(last + 0.01));
    assert_eq!(m.offset(last + 1.0).y, 1180.0);
    let mut edge = document(0.0);
    let last = pull(&mut edge, -30.0, 8, 0.0);
    edge.release(last + 0.3, false);
    assert_eq!(edge.phases(last + 0.3)[1], Phase::Returning);
    assert_eq!(edge.shown(last + 1.0), Point::ZERO);
}

/// A cancelled drag (the window lost the pointer) leaves no kinetic motion, and its stretch returns.
#[test]
fn a_cancelled_drag_does_not_coast() {
    let mut m = document(1000.0);
    let last = pull(&mut m, 30.0, 6, 0.0);
    m.cancel(last + 0.001);
    assert!(!m.is_animating(last + 0.002));
    let mut edge = document(0.0);
    let last = pull(&mut edge, -30.0, 8, 0.0);
    edge.cancel(last);
    assert_eq!(edge.phases(last)[1], Phase::Returning);
    assert_eq!(edge.shown(last + 1.0), Point::ZERO);
}

/// A new drag during kinetic motion stops it where the content is drawn; the new movement starts from there.
#[test]
fn a_new_drag_stops_kinetic_motion() {
    let mut m = document(1000.0);
    let last = pull(&mut m, 30.0, 6, 0.0);
    m.release(last + 0.008, true);
    let catch = last + 0.2;
    let caught = m.shown(catch).y;
    m.drag(Point::new(0.0, 10.0), catch);
    assert_eq!(m.shown(catch).y, caught + 10.0);
    assert_eq!(m.shown(catch + 1.0).y, caught + 10.0, "nothing coasts on");
    m.release(catch + RELEASE_PAUSE * 2.0, true);
    assert!(!m.is_animating(catch + 0.2));
}

/// Wheel and key steps move at once inside the range, never coast or stretch, and stop kinetic motion.
#[test]
fn steps_move_at_once_without_inertia_or_stretch() {
    let mut m = document(600.0);
    m.step(Point::new(0.0, 10.0), 0.0);
    assert_eq!(m.offset(0.0).y, 610.0);
    assert!(!m.is_animating(0.0));
    m.step(Point::new(0.0, -5000.0), 0.1);
    assert_eq!(m.shown(0.1), Point::ZERO, "no stretch at the edge");
    let last = pull(&mut m, 30.0, 6, 1.0);
    m.release(last + 0.008, true);
    let at = m.shown(last + 0.1).y;
    m.step(Point::new(0.0, 10.0), last + 0.1);
    assert_eq!(
        m.shown(last + 1.0).y,
        at + 10.0,
        "the step stopped the coast"
    );
}

/// The first movement of a drag on a mostly vertical document decides its axis: a small sideways part is dropped
/// for the whole drag. A free canvas keeps both.
#[test]
fn a_drag_keeps_to_its_predominant_axis_unless_the_view_is_free() {
    let mut m = canvas(100.0, 600.0);
    for i in 0..3 {
        m.drag(Point::new(-4.0, 20.0), i as f64 * FRAME);
    }
    m.drag(Point::new(30.0, 0.0), 3.0 * FRAME);
    assert_eq!(m.offset(1.0), Point::new(100.0, 660.0));
    m.release(1.0, true);
    m.drag(Point::new(30.0, 0.0), 2.0);
    assert_eq!(
        m.offset(2.0),
        Point::new(130.0, 660.0),
        "a new drag decides again"
    );
    m.release(3.0, true);

    let mut free = canvas(100.0, 600.0).with_policy(AxisPolicy::Free);
    for i in 0..3 {
        free.drag(Point::new(4.0, 20.0), i as f64 * FRAME);
    }
    assert_eq!(free.offset(1.0), Point::new(112.0, 660.0));
}

/// Content that fits its viewport does not move or stretch, and says so.
#[test]
fn content_that_fits_does_not_move() {
    let mut m = ScrollMotion::new(
        Point::ZERO,
        Point::ZERO,
        Size::new(470.0, 698.0),
        (false, true),
    );
    assert!(!m.can_move(Point::new(0.0, -30.0), 0.0));
    assert!(!m.can_move(Point::new(0.0, 30.0), 0.0));
    m.drag(Point::new(0.0, -30.0), 0.0);
    assert_eq!(m.shown(0.0), Point::ZERO);
    let at_end = document(4000.0 - 698.0);
    assert!(!at_end.can_move(Point::new(0.0, 1.0), 0.0));
    assert!(at_end.can_move(Point::new(0.0, -1.0), 0.0));
}

/// A range that shrinks under content at rest puts it inside at once; one that changes during kinetic motion lets
/// it go on from where it is drawn.
#[test]
fn a_changed_range_keeps_the_content_inside_it() {
    let mut m = document(3000.0);
    m.set_bounds(Point::new(0.0, 1000.0), Size::new(470.0, 698.0), 0.0);
    assert_eq!(m.shown(0.0), Point::new(0.0, 1000.0));
    let last = pull(&mut m, -30.0, 6, 1.0);
    m.release(last + 0.008, true);
    let at = m.shown(last + 0.1).y;
    m.set_bounds(Point::new(0.0, 2000.0), Size::new(470.0, 698.0), last + 0.1);
    assert_eq!(m.shown(last + 0.1).y, at);
    assert!(m.is_animating(last + 0.2));
}

/// The same inputs at the same times give the same motion.
#[test]
fn a_trace_is_deterministic() {
    let run = || {
        let mut m = canvas(50.0, 0.0);
        let mut out = Vec::new();
        let last = pull(&mut m, -25.0, 5, 0.0);
        m.release(last + 0.01, true);
        for i in 0..40 {
            out.push(m.shown(last + i as f64 * 0.01));
        }
        let last = pull(&mut m, 35.0, 4, last + 0.4);
        m.release(last + 0.01, true);
        for i in 0..40 {
            out.push(m.shown(last + i as f64 * 0.02));
        }
        out
    };
    assert_eq!(run(), run());
}
