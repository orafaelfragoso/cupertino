//! The workspace slide is a damped pull towards a target position with a rubber band beyond the first and last space.

use cupertino_effects::space_slide::{SPACE_GAP, STEP_RATE};
use cupertino_effects::{point_in_x, settle_time, SlideStart, SpaceSlideState};

fn run(mut state: SpaceSlideState, target: f64, steps: usize) -> SpaceSlideState {
    for _ in 0..steps {
        state = state.step(target);
    }
    state
}

#[test]
fn the_slide_settles_on_its_target_with_only_a_small_overshoot() {
    let rest = SpaceSlideState::at_rest(0.5);
    assert_eq!(rest.step(0.5), rest, "at rest on its target it stays");
    let mut s = SpaceSlideState::at_rest(0.0);
    let mut peak: f64 = 0.0;
    for _ in 0..600 {
        s = s.step(1.0);
        peak = peak.max(s.x);
    }
    assert!(peak < 1.1, "{peak}");
    assert!((s.x - 1.0).abs() < 1e-3);
    assert!(s.v.abs() < 0.01);
}

#[test]
fn velocity_carries_a_release_further() {
    let flick = SpaceSlideState { x: 0.5, v: 2.0 };
    let still = SpaceSlideState::at_rest(0.5);
    assert!(run(flick, 0.5, 10).x > run(still, 0.5, 10).x);
}

#[test]
fn beyond_the_first_and_last_space_a_band_pulls_the_slide_back() {
    // No pull from the target (it is where the slide is), only the rubber band.
    for (x, pull_right) in [(1.2, false), (-0.2, true)] {
        let after = SpaceSlideState { x, v: 0.0 }.step(x);
        assert_eq!(after.v > 0.0, pull_right, "{x}");
        assert_ne!(after.v, 0.0, "{x}");
    }
}

#[test]
fn rest_needs_both_a_small_speed_and_a_small_distance() {
    for (x, v, at_rest) in [(1.0005, 0.0, true), (1.0, 0.5, false), (1.1, 0.0, false)] {
        assert_eq!(
            SpaceSlideState { x, v }.is_at_rest(1.0, 0.001),
            at_rest,
            "{x} {v}"
        );
    }
}

#[test]
fn settling_takes_whole_steps_and_longer_from_farther_away() {
    assert_eq!(settle_time(SpaceSlideState::at_rest(1.0), 1.0, 0.001), 0.0);
    let near = settle_time(SpaceSlideState::at_rest(0.8), 1.0, 0.001);
    let far = settle_time(SpaceSlideState::at_rest(0.0), 1.0, 0.001);
    assert!(near > 0.0 && far > near && far < 10.0, "{near} {far}");
    let steps = far * STEP_RATE;
    assert!((steps - steps.round()).abs() < 1e-9);
}

#[test]
fn a_programmatic_slide_stops_within_one_point_of_travel_and_a_release_within_a_thousandth() {
    let width = 1920.0;
    assert_eq!(SlideStart::Release.tolerance(5, width), 0.001);
    let tolerance = SlideStart::Programmatic.tolerance(5, width);
    assert_eq!(tolerance, point_in_x(5, width));
    // Positions are in spaces; four steps between five spaces, each a pitch of one screen and a gap.
    assert!((tolerance - 1.0 / (4.0 * (width + SPACE_GAP))).abs() < 1e-15);
    let one = point_in_x(1, 1000.0);
    assert!(
        one.is_finite() && one > 0.0,
        "a single space still has a tolerance"
    );
    assert_eq!(one, point_in_x(0, 1000.0));
    assert_eq!(one, point_in_x(2, 1000.0));
}
