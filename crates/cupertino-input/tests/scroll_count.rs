//! The scroll counter turns the speed of a vertical scroll gesture into a repeat count for the shell.

use cupertino_input::{ScrollCounter, ScrollPhase};

#[test]
fn a_slow_scroll_never_starts_a_count() {
    let mut c = ScrollCounter::default();
    for i in 0..20 {
        assert_eq!(
            c.update(ScrollPhase::Changed, 0.0, 10.0, f64::from(i) * 10.0),
            0
        );
    }
}

#[test]
fn a_fast_scroll_starts_the_count_at_one() {
    let mut c = ScrollCounter::default();
    assert_eq!(c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0), 1);
}

#[test]
fn the_count_grows_with_speed_once_past_two() {
    let mut c = ScrollCounter::default();
    c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0);
    c.update(ScrollPhase::Ended, 0.0, 0.0, 10.0);
    // A new gesture within the window raises the count by one.
    assert_eq!(c.update(ScrollPhase::Began, 0.0, 40.0, 100.0), 2);
    c.update(ScrollPhase::Ended, 0.0, 0.0, 110.0);
    assert_eq!(c.update(ScrollPhase::Began, 0.0, 40.0, 200.0), 3);
    // Above two, motion adds speed / 2.5 per update.
    assert_eq!(c.update(ScrollPhase::Changed, 0.0, 50.0, 210.0), 3 + 20);
}

#[test]
fn a_gesture_after_the_window_starts_over() {
    let mut c = ScrollCounter::default();
    c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0);
    c.update(ScrollPhase::Ended, 0.0, 0.0, 10.0);
    assert_eq!(c.update(ScrollPhase::Began, 0.0, 40.0, 10.0 + 600.0), 0);
}

#[test]
fn a_began_phase_raises_the_count_only_once_per_gesture() {
    let mut c = ScrollCounter::default();
    c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0);
    c.update(ScrollPhase::Ended, 0.0, 0.0, 10.0);
    assert_eq!(c.update(ScrollPhase::Began, 0.0, 40.0, 100.0), 2);
    assert_eq!(c.update(ScrollPhase::Began, 0.0, 40.0, 110.0), 2);
}

#[test]
fn a_pause_in_the_motion_ends_the_count() {
    let mut c = ScrollCounter::default();
    assert_eq!(c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0), 1);
    assert_eq!(
        c.update(ScrollPhase::Changed, 0.0, 5.0, 200.0),
        1,
        "within the sustain time"
    );
    assert_eq!(
        c.update(ScrollPhase::Changed, 0.0, 5.0, 300.0),
        0,
        "past it"
    );
}

#[test]
fn a_fast_update_renews_the_sustain_time() {
    let mut c = ScrollCounter::default();
    c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0);
    c.update(ScrollPhase::Changed, 0.0, 30.0, 200.0);
    assert_eq!(c.update(ScrollPhase::Changed, 0.0, 5.0, 400.0), 1);
}

#[test]
fn reversing_direction_resets_the_count() {
    let mut c = ScrollCounter::default();
    assert_eq!(c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0), 1);
    // Reversal: the count restarts, and this update is too fast to be counted as a start only after the reset.
    assert_eq!(c.update(ScrollPhase::Changed, 0.0, -40.0, 10.0), 1);
    assert_eq!(c.update(ScrollPhase::Changed, 0.0, -5.0, 20.0), 1);
}

#[test]
fn cancelling_clears_everything() {
    let mut c = ScrollCounter::default();
    c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0);
    assert_eq!(c.update(ScrollPhase::Cancelled, 0.0, 0.0, 10.0), 0);
    assert_eq!(c.update(ScrollPhase::Began, 0.0, 40.0, 20.0), 0);
}

#[test]
fn a_horizontal_scroll_does_not_change_the_direction() {
    let mut c = ScrollCounter::default();
    c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0);
    assert_eq!(c.update(ScrollPhase::Changed, 100.0, 0.0, 10.0), 1);
}

#[test]
fn the_count_is_bounded() {
    let mut c = ScrollCounter::default();
    c.update(ScrollPhase::Changed, 0.0, 40.0, 0.0);
    c.update(ScrollPhase::Ended, 0.0, 0.0, 1.0);
    c.update(ScrollPhase::Began, 0.0, 40.0, 2.0);
    c.update(ScrollPhase::Ended, 0.0, 0.0, 3.0);
    c.update(ScrollPhase::Began, 0.0, 40.0, 4.0);
    let mut last = 0;
    for i in 0..200 {
        last = c.update(ScrollPhase::Changed, 0.0, 5000.0, 10.0 + f64::from(i));
    }
    assert_eq!(last, 2000);
}
