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

fn end(r: &mut GestureRecognizer, time: f64, cancelled: bool) -> GestureOutput {
    let out = r.handle(GestureEvent::SwipeEnd { time, cancelled });
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

fn pinch(r: &mut GestureRecognizer, fingers: u32, scale: f64) -> Vec<GestureOutput> {
    r.handle(GestureEvent::PinchBegin { time: 0.0, fingers });
    r.handle(GestureEvent::PinchUpdate {
        time: 0.02,
        dx: 0.0,
        dy: 0.0,
        scale,
        rotation: 0.0,
    })
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

    let mut unstarted = GestureRecognizer::new(GestureConfig::default());
    unstarted.handle(GestureEvent::SwipeBegin {
        time: 0.0,
        fingers: 3,
    });
    assert!(
        unstarted
            .handle(GestureEvent::SwipeEnd {
                time: 0.1,
                cancelled: false
            })
            .is_empty(),
        "a swipe that never starts ends silently"
    );
    assert_eq!(unstarted.disposition(), Disposition::Idle);
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

    let mut diagonal = GestureRecognizer::new(GestureConfig::default());
    diagonal.handle(GestureEvent::SwipeBegin {
        time: 0.0,
        fingers: 3,
    });
    diagonal.handle(GestureEvent::SwipeUpdate {
        time: 0.01,
        dx: 20.0,
        dy: 20.0,
    });
    assert_eq!(
        diagonal.disposition(),
        Disposition::Pending,
        "a diagonal stays undecided"
    );
    diagonal.handle(GestureEvent::SwipeUpdate {
        time: 0.02,
        dx: 20.0,
        dy: 0.0,
    });
    assert_eq!(diagonal.active_kind(), Some(GestureKind::WorkspaceSwitch));
}

#[test]
fn a_gesture_with_no_binding_is_ignored_until_it_ends() {
    let only_four_for_overview = GestureConfig {
        overview_fingers: vec![4],
        ..GestureConfig::default()
    };
    for (name, config, fingers, dx, dy) in [
        ("five finger swipe", GestureConfig::default(), 5, 100.0, 0.0),
        ("unbound axis", only_four_for_overview, 3, 0.0, -100.0),
    ] {
        let mut r = GestureRecognizer::new(config);
        assert!(swipe_to(&mut r, fingers, dx, dy).is_empty(), "{name}");
        assert_eq!(r.disposition(), Disposition::Ignored, "{name}");
        assert!(r
            .handle(GestureEvent::SwipeEnd {
                time: 1.0,
                cancelled: false
            })
            .is_empty());
        assert_eq!(r.disposition(), Disposition::Idle, "{name}");
    }
    let mut r = GestureRecognizer::new(GestureConfig::default());
    assert!(pinch(&mut r, 3, 0.5).is_empty(), "three finger pinch");
    assert_eq!(r.disposition(), Disposition::Ignored);
}

#[test]
fn horizontal_progress_follows_the_natural_direction_setting() {
    for (natural, progress) in [(true, -0.5), (false, 0.5)] {
        let mut r = GestureRecognizer::new(GestureConfig {
            natural_direction: natural,
            ..GestureConfig::default()
        });
        let out = swipe_to(&mut r, 3, 150.0, 0.0);
        assert!(
            (final_progress(&out) - progress).abs() < 1e-9,
            "natural {natural}"
        );
    }
}

#[test]
fn a_swipe_ends_by_its_distance_and_velocity_in_a_commit_or_a_cancel() {
    let three_steps = GestureConfig {
        max_steps: 3,
        ..GestureConfig::default()
    };
    // (name, config, dx, dy, end time, cancelled, kind, outcome, direction, steps, target)
    for (name, config, dx, dy, time, cancelled, kind, outcome, direction, steps, target) in [
        (
            "a long swipe to the next workspace",
            GestureConfig::default(),
            -240.0,
            0.0,
            0.11,
            false,
            GestureKind::WorkspaceSwitch,
            Outcome::Commit,
            Some(Direction::Next),
            1,
            1.0,
        ),
        (
            "a long swipe to the previous workspace",
            GestureConfig::default(),
            240.0,
            0.0,
            0.11,
            false,
            GestureKind::WorkspaceSwitch,
            Outcome::Commit,
            Some(Direction::Previous),
            1,
            -1.0,
        ),
        (
            "a short swipe held still before lifting",
            GestureConfig::default(),
            -60.0,
            0.0,
            2.0,
            false,
            GestureKind::WorkspaceSwitch,
            Outcome::Cancel,
            None,
            0,
            0.0,
        ),
        (
            "a short fast swipe carried over by its velocity",
            GestureConfig::default(),
            -90.0,
            0.0,
            0.1,
            false,
            GestureKind::WorkspaceSwitch,
            Outcome::Commit,
            Some(Direction::Next),
            1,
            1.0,
        ),
        (
            "a cancelled gesture",
            GestureConfig::default(),
            -400.0,
            0.0,
            0.11,
            true,
            GestureKind::WorkspaceSwitch,
            Outcome::Cancel,
            None,
            0,
            0.0,
        ),
        (
            "several workspaces when the limit allows",
            three_steps,
            -750.0,
            0.0,
            2.0,
            false,
            GestureKind::WorkspaceSwitch,
            Outcome::Commit,
            Some(Direction::Next),
            3,
            3.0,
        ),
        (
            "an upward swipe to the overview",
            GestureConfig::default(),
            0.0,
            -300.0,
            0.11,
            false,
            GestureKind::Overview,
            Outcome::Commit,
            None,
            1,
            1.0,
        ),
    ] {
        let mut r = GestureRecognizer::new(config);
        swipe_to(&mut r, 3, dx, dy);
        let GestureOutput::End {
            kind: got_kind,
            outcome: got_outcome,
            direction: got_direction,
            steps: got_steps,
            target: got_target,
            velocity,
            ..
        } = end(&mut r, time, cancelled)
        else {
            panic!("{name}: an end");
        };
        assert_eq!(
            (got_kind, got_outcome, got_direction, got_target),
            (kind, outcome, direction, target),
            "{name}"
        );
        if kind == GestureKind::WorkspaceSwitch {
            assert_eq!(got_steps, steps, "{name}");
        }
        if name.contains("velocity") {
            assert!(velocity > 0.0, "{name}");
        }
        assert_eq!(r.disposition(), Disposition::Idle, "{name}");
    }
}

#[test]
fn pulling_a_swipe_back_below_the_commit_point_cancels_it() {
    let mut r = GestureRecognizer::new(GestureConfig::default());
    swipe_to(&mut r, 3, 0.0, -300.0);
    for i in 1..=10 {
        r.handle(GestureEvent::SwipeUpdate {
            time: 0.11 + f64::from(i) * 0.01,
            dx: 0.0,
            dy: 25.0,
        });
    }
    let GestureOutput::End { outcome, .. } = end(&mut r, 2.0, false) else {
        panic!("an end");
    };
    assert_eq!(outcome, Outcome::Cancel);
}

#[test]
fn pinching_in_opens_the_app_grid_and_spreading_shows_the_desktop() {
    for (scale, kind, progress) in [
        (0.8, GestureKind::AppGrid, 0.4),
        (1.3, GestureKind::ShowDesktop, 0.6),
    ] {
        let mut r = GestureRecognizer::new(GestureConfig::default());
        let out = pinch(&mut r, 4, scale);
        assert_eq!(out[0], GestureOutput::Begin { kind, fingers: 4 }, "{scale}");
        assert!((final_progress(&out) - progress).abs() < 1e-9, "{scale}");
        r.handle(GestureEvent::PinchEnd {
            time: 2.0,
            cancelled: false,
        });
        assert_eq!(r.disposition(), Disposition::Idle);
    }
    let mut small = GestureRecognizer::new(GestureConfig::default());
    assert!(
        pinch(&mut small, 4, 0.97).is_empty(),
        "below the lock distance"
    );
}

#[test]
fn a_swipe_update_does_not_feed_a_pinch() {
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
fn the_fluid_swipe_commits_on_projected_progress_in_the_same_direction() {
    for (name, progress, peak, velocity, commits) in [
        ("0.4 + 0.3 passes one half", 0.4, 0.5, 0.6, true),
        ("0.3 projected is too little", 0.2, 0.2, 0.2, false),
        ("thrown back the other way", 0.4, 0.5, -2.0, false),
        ("the other direction", -0.6, -0.6, -0.1, true),
        ("no progress, no commit", 0.0, 1.0, 5.0, false),
        ("well short of the peak", 0.6, 2.0, 0.0, false),
    ] {
        assert_eq!(
            fluid_swipe_commits(progress, peak, velocity, 0.5),
            commits,
            "{name}"
        );
    }
}
