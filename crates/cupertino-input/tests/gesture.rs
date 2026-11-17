//! Swipes and pinches are claimed by an axis, drive a progress value and end with a commit or a cancel.

use cupertino_input::{
    fluid_swipe_commits, Direction, Disposition, GestureConfig, GestureEvent, GestureKind,
    GestureOutput, GestureRecognizer, Outcome,
};

fn swipe_to(r: &mut GestureRecognizer, fingers: u32, dx: f64, dy: f64) -> Vec<GestureOutput> {
    let mut out = r.handle(GestureEvent::SwipeBegin { time: 0.0, fingers });
    // A steady swipe, ten updates over 0.1 s.
    for i in 1..=10 {
        out.extend(r.handle(GestureEvent::SwipeUpdate {
            time: f64::from(i) * 0.01,
            dx: dx / 10.0,
            dy: dy / 10.0,
        }));
    }
    out
}

fn end(r: &mut GestureRecognizer, time: f64) -> GestureOutput {
    let out = r.handle(GestureEvent::SwipeEnd {
        time,
        cancelled: false,
    });
    assert_eq!(out.len(), 1);
    out[0]
}

fn final_progress(outputs: &[GestureOutput]) -> f64 {
    outputs
        .iter()
        .rev()
        .find_map(|o| match o {
            GestureOutput::Update { progress, .. } => Some(*progress),
            _ => None,
        })
        .expect("an update")
}

#[test]
fn a_swipe_waits_for_the_lock_distance_before_it_claims_anything() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    assert_eq!(r.disposition(), Disposition::Idle);
    r.handle(GestureEvent::SwipeBegin {
        time: 0.0,
        fingers: 3,
    });
    assert_eq!(r.disposition(), Disposition::Pending);
    assert!(r
        .handle(GestureEvent::SwipeUpdate {
            time: 0.01,
            dx: 3.0,
            dy: 0.0
        })
        .is_empty());
    assert_eq!(r.active_kind(), None);
    let out = r.handle(GestureEvent::SwipeUpdate {
        time: 0.02,
        dx: 6.0,
        dy: 0.0,
    });
    assert_eq!(r.disposition(), Disposition::Claimed);
    assert_eq!(
        out[0],
        GestureOutput::Begin {
            kind: GestureKind::WorkspaceSwitch,
            fingers: 3
        }
    );
    assert!(matches!(out[1], GestureOutput::Update { .. }));
}

#[test]
fn the_axis_and_direction_decide_the_kind() {
    for (dx, dy, kind) in [
        (60.0, 5.0, GestureKind::WorkspaceSwitch),
        (-60.0, 5.0, GestureKind::WorkspaceSwitch),
        (5.0, -60.0, GestureKind::Overview),
        (5.0, 60.0, GestureKind::AppOverview),
    ] {
        let mut r = GestureRecognizer::new(GestureConfig::default());
        swipe_to(&mut r, 3, dx, dy);
        assert_eq!(r.active_kind(), Some(kind), "({dx}, {dy})");
    }
}

#[test]
fn a_diagonal_swipe_stays_undecided_until_one_axis_wins() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    r.handle(GestureEvent::SwipeBegin {
        time: 0.0,
        fingers: 3,
    });
    r.handle(GestureEvent::SwipeUpdate {
        time: 0.01,
        dx: 20.0,
        dy: 20.0,
    });
    assert_eq!(r.disposition(), Disposition::Pending);
    r.handle(GestureEvent::SwipeUpdate {
        time: 0.02,
        dx: 20.0,
        dy: 0.0,
    });
    assert_eq!(r.active_kind(), Some(GestureKind::WorkspaceSwitch));
}

#[test]
fn a_finger_count_with_no_binding_is_ignored_until_the_gesture_ends() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    assert!(swipe_to(&mut r, 5, 100.0, 0.0).is_empty());
    assert_eq!(r.disposition(), Disposition::Ignored);
    assert!(r
        .handle(GestureEvent::SwipeEnd {
            time: 1.0,
            cancelled: false
        })
        .is_empty());
    assert_eq!(r.disposition(), Disposition::Idle);
}

#[test]
fn an_axis_with_no_binding_for_that_finger_count_is_ignored() {
    let cfg = GestureConfig {
        overview_fingers: vec![4],
        ..GestureConfig::default()
    };
    let mut r = GestureRecognizer::new(cfg);
    assert!(swipe_to(&mut r, 3, 0.0, -100.0).is_empty());
    assert_eq!(r.disposition(), Disposition::Ignored);
}

#[test]
fn horizontal_progress_follows_the_natural_direction_setting() {
    let mut natural = GestureRecognizer::new(GestureConfig::default());
    let out = swipe_to(&mut natural, 3, 150.0, 0.0);
    assert!(
        (final_progress(&out) + 0.5).abs() < 1e-9,
        "fingers right, content right: previous"
    );
    let mut reverse = GestureRecognizer::new(GestureConfig {
        natural_direction: false,
        ..GestureConfig::default()
    });
    let out = swipe_to(&mut reverse, 3, 150.0, 0.0);
    assert!((final_progress(&out) - 0.5).abs() < 1e-9);
}

#[test]
fn a_long_swipe_commits_to_the_next_workspace() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    let out = swipe_to(&mut r, 3, -240.0, 0.0);
    assert!((final_progress(&out) - 0.8).abs() < 1e-9);
    let GestureOutput::End {
        kind,
        outcome,
        direction,
        steps,
        target,
        ..
    } = end(&mut r, 0.11)
    else {
        panic!("an end");
    };
    assert_eq!(kind, GestureKind::WorkspaceSwitch);
    assert_eq!(outcome, Outcome::Commit);
    assert_eq!(direction, Some(Direction::Next));
    assert_eq!(steps, 1);
    assert_eq!(target, 1.0);
    assert_eq!(r.disposition(), Disposition::Idle);
}

#[test]
fn a_swipe_the_other_way_commits_to_the_previous_workspace() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    swipe_to(&mut r, 3, 240.0, 0.0);
    let GestureOutput::End {
        direction, target, ..
    } = end(&mut r, 0.11)
    else {
        panic!("an end");
    };
    assert_eq!(direction, Some(Direction::Previous));
    assert_eq!(target, -1.0);
}

#[test]
fn a_short_slow_swipe_cancels() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    swipe_to(&mut r, 3, -60.0, 0.0);
    // Held still for a while before lifting: no velocity to project.
    let GestureOutput::End {
        outcome,
        target,
        direction,
        steps,
        ..
    } = end(&mut r, 2.0)
    else {
        panic!("an end");
    };
    assert_eq!(outcome, Outcome::Cancel);
    assert_eq!((target, direction, steps), (0.0, None, 0));
}

#[test]
fn velocity_can_carry_a_short_swipe_over_the_commit_point() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    let out = swipe_to(&mut r, 3, -90.0, 0.0);
    assert!(final_progress(&out) < 0.5);
    let GestureOutput::End {
        outcome, velocity, ..
    } = end(&mut r, 0.1)
    else {
        panic!("an end");
    };
    assert!(velocity > 0.0);
    assert_eq!(outcome, Outcome::Commit);
}

#[test]
fn a_cancelled_gesture_never_commits() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    swipe_to(&mut r, 3, -400.0, 0.0);
    let out = r.handle(GestureEvent::SwipeEnd {
        time: 0.11,
        cancelled: true,
    });
    let [GestureOutput::End {
        outcome, target, ..
    }] = out[..]
    else {
        panic!("an end");
    };
    assert_eq!(outcome, Outcome::Cancel);
    assert_eq!(target, 0.0);
}

#[test]
fn an_upward_swipe_commits_the_overview_and_a_downward_one_does_not() {
    let mut up = GestureRecognizer::new(GestureConfig::default());
    swipe_to(&mut up, 3, 0.0, -300.0);
    let GestureOutput::End {
        kind,
        outcome,
        target,
        direction,
        ..
    } = end(&mut up, 0.11)
    else {
        panic!("an end");
    };
    assert_eq!(
        (kind, outcome, target, direction),
        (GestureKind::Overview, Outcome::Commit, 1.0, None)
    );
    // Pulling back down after going up: progress below the commit point cancels.
    let mut back = GestureRecognizer::new(GestureConfig::default());
    swipe_to(&mut back, 3, 0.0, -300.0);
    for i in 1..=10 {
        back.handle(GestureEvent::SwipeUpdate {
            time: 0.11 + f64::from(i) * 0.01,
            dx: 0.0,
            dy: 25.0,
        });
    }
    let GestureOutput::End { outcome, .. } = end(&mut back, 2.0) else {
        panic!("an end");
    };
    assert_eq!(outcome, Outcome::Cancel);
}

#[test]
fn pinching_in_opens_the_app_grid_and_spreading_shows_the_desktop() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    r.handle(GestureEvent::PinchBegin {
        time: 0.0,
        fingers: 4,
    });
    assert!(r
        .handle(GestureEvent::PinchUpdate {
            time: 0.01,
            dx: 0.0,
            dy: 0.0,
            scale: 0.97,
            rotation: 0.0
        })
        .is_empty());
    let out = r.handle(GestureEvent::PinchUpdate {
        time: 0.02,
        dx: 0.0,
        dy: 0.0,
        scale: 0.8,
        rotation: 0.0,
    });
    assert_eq!(
        out[0],
        GestureOutput::Begin {
            kind: GestureKind::AppGrid,
            fingers: 4
        }
    );
    assert!((final_progress(&out) - 0.4).abs() < 1e-9);
    r.handle(GestureEvent::PinchEnd {
        time: 2.0,
        cancelled: false,
    });
    assert_eq!(r.disposition(), Disposition::Idle);

    let mut s = GestureRecognizer::new(GestureConfig::default());
    s.handle(GestureEvent::PinchBegin {
        time: 0.0,
        fingers: 4,
    });
    let out = s.handle(GestureEvent::PinchUpdate {
        time: 0.02,
        dx: 0.0,
        dy: 0.0,
        scale: 1.3,
        rotation: 0.0,
    });
    assert_eq!(
        out[0],
        GestureOutput::Begin {
            kind: GestureKind::ShowDesktop,
            fingers: 4
        }
    );
    assert!((final_progress(&out) - 0.6).abs() < 1e-9);
}

#[test]
fn a_pinch_with_an_unbound_finger_count_is_ignored() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    r.handle(GestureEvent::PinchBegin {
        time: 0.0,
        fingers: 3,
    });
    assert_eq!(r.disposition(), Disposition::Ignored);
    assert!(r
        .handle(GestureEvent::PinchUpdate {
            time: 0.1,
            dx: 0.0,
            dy: 0.0,
            scale: 0.5,
            rotation: 0.0
        })
        .is_empty());
}

#[test]
fn a_swipe_update_does_not_feed_a_pinch_and_the_reverse() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    r.handle(GestureEvent::PinchBegin {
        time: 0.0,
        fingers: 4,
    });
    assert!(r
        .handle(GestureEvent::SwipeUpdate {
            time: 0.1,
            dx: 100.0,
            dy: 0.0
        })
        .is_empty());
    assert_eq!(r.disposition(), Disposition::Pending);
}

#[test]
fn holds_are_reported_with_their_finger_count() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    assert_eq!(
        r.handle(GestureEvent::HoldBegin {
            time: 0.0,
            fingers: 2
        }),
        [GestureOutput::HoldBegin { fingers: 2 }]
    );
    assert_eq!(r.disposition(), Disposition::Idle);
    assert_eq!(
        r.handle(GestureEvent::HoldEnd {
            time: 0.5,
            cancelled: true
        }),
        [GestureOutput::HoldEnd {
            fingers: 2,
            cancelled: true
        }]
    );
    assert!(r
        .handle(GestureEvent::HoldEnd {
            time: 0.6,
            cancelled: false
        })
        .is_empty());
}

#[test]
fn a_swipe_that_never_starts_ends_silently() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    r.handle(GestureEvent::SwipeBegin {
        time: 0.0,
        fingers: 3,
    });
    assert!(r
        .handle(GestureEvent::SwipeEnd {
            time: 0.1,
            cancelled: false
        })
        .is_empty());
    assert_eq!(r.disposition(), Disposition::Idle);
}

#[test]
fn several_workspaces_can_be_crossed_when_the_limit_allows() {
    let mut r = GestureRecognizer::new(GestureConfig {
        max_steps: 3,
        ..GestureConfig::default()
    });
    swipe_to(&mut r, 3, -750.0, 0.0);
    let GestureOutput::End { steps, target, .. } = end(&mut r, 2.0) else {
        panic!("an end");
    };
    assert_eq!(steps, 3);
    assert_eq!(target, 3.0);
}

#[test]
fn the_fluid_swipe_commits_on_projected_progress_in_the_same_direction() {
    assert!(
        fluid_swipe_commits(0.4, 0.5, 0.6, 0.5),
        "0.4 + 0.3 passes one half"
    );
    assert!(
        !fluid_swipe_commits(0.2, 0.2, 0.2, 0.5),
        "0.3 projected is too little"
    );
    assert!(
        !fluid_swipe_commits(0.4, 0.5, -2.0, 0.5),
        "thrown back the other way"
    );
    assert!(fluid_swipe_commits(-0.6, -0.6, -0.1, 0.5));
    assert!(
        !fluid_swipe_commits(0.0, 1.0, 5.0, 0.5),
        "no progress, no commit"
    );
    assert!(
        !fluid_swipe_commits(0.6, 2.0, 0.0, 0.5),
        "well short of the peak"
    );
}
