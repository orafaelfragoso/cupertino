//! Pushing the pointer up against the top edge in a near-vertical stroke fires once after enough travel.

use cupertino_input::{TopZone, TopZoneConfig};

#[test]
fn a_straight_push_up_fires_once_after_the_distance() {
    let mut z = TopZone::default();
    let mut fired = 0;
    let mut at = None;
    for i in 0..20 {
        if z.motion(0.0, 0.0, -10.0) {
            fired += 1;
            at = Some(i);
        }
    }
    assert_eq!(fired, 1);
    assert_eq!(
        at,
        Some(6),
        "64 points of travel is reached by the seventh 10 point step"
    );
}

#[test]
fn the_pointer_must_be_at_the_top_edge() {
    let mut z = TopZone::default();
    for _ in 0..20 {
        assert!(!z.motion(5.0, 0.0, -10.0));
    }
}

#[test]
fn leaving_the_edge_starts_the_count_again_and_allows_another_fire() {
    let mut z = TopZone::default();
    for _ in 0..7 {
        z.motion(0.0, 0.0, -10.0);
    }
    assert!(!z.motion(0.0, 0.0, -10.0), "already fired");
    z.motion(30.0, 0.0, 0.0);
    let fired = (0..7).filter(|_| z.motion(0.0, 0.0, -10.0)).count();
    assert_eq!(fired, 1);
}

#[test]
fn motion_down_or_sideways_resets_the_push() {
    let mut z = TopZone::default();
    for _ in 0..5 {
        z.motion(0.0, 0.0, -10.0);
    }
    z.motion(0.0, 0.0, 3.0);
    assert!(!z.motion(0.0, 0.0, -10.0));
    for _ in 0..5 {
        z.motion(0.0, 0.0, -10.0);
    }
    z.motion(0.0, 8.0, 0.0);
    assert!(!z.motion(0.0, 0.0, -10.0));
}

#[test]
fn a_stroke_too_far_off_vertical_does_not_count() {
    let mut z = TopZone::default();
    // atan(10 / 20) is well above the limit of a sixteenth of a half turn.
    for _ in 0..50 {
        assert!(!z.motion(0.0, 10.0, -20.0));
    }
    // The same stroke slightly slanted still counts.
    let fired = (0..10).filter(|_| z.motion(0.0, 1.0, -20.0)).count();
    assert_eq!(fired, 1);
}

#[test]
fn a_reset_clears_the_push_and_the_fired_state() {
    let mut z = TopZone::default();
    for _ in 0..7 {
        z.motion(0.0, 0.0, -10.0);
    }
    z.reset();
    let fired = (0..7).filter(|_| z.motion(0.0, 0.0, -10.0)).count();
    assert_eq!(fired, 1);
}

#[test]
fn the_distance_comes_from_the_configuration() {
    let mut z = TopZone::new(TopZoneConfig {
        distance: 15.0,
        ..TopZoneConfig::default()
    });
    assert!(!z.motion(0.0, 0.0, -10.0));
    assert!(z.motion(0.0, 0.0, -10.0));
}
