//! A minimize animation runs one of three effects towards a dock slot and back, and can be reversed mid-flight.

use cupertino_effects::{
    DockEdge, Ease, MinimizeAnimation, MinimizeEffect, MinimizeKind, MinimizeParams, MinimizePhase,
    WarpDirection,
};
use cupertino_geometry::Rect;

const WINDOW: Rect = Rect::new(300.0, 100.0, 500.0, 400.0);
const SLOT: Rect = Rect::new(520.0, 900.0, 60.0, 60.0);

fn effect(kind: MinimizeKind) -> MinimizeEffect {
    MinimizeEffect::new(kind, WINDOW, SLOT, DockEdge::Bottom)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn slow_motion_doubles_every_kinds_durations() {
    for kind in MinimizeKind::ALL {
        assert!(
            close(kind.duration_s(false), kind.duration_ms() / 1000.0),
            "{kind:?}"
        );
        assert!(
            close(kind.duration_s(true), 2.0 * kind.duration_s(false)),
            "{kind:?}"
        );
        assert!(
            close(
                kind.restore_duration_s(true),
                2.0 * kind.restore_duration_s(false)
            ),
            "{kind:?}"
        );
    }
}

#[test]
fn every_effect_runs_from_the_window_to_the_slot_within_its_vertex_budget() {
    for kind in MinimizeKind::ALL {
        let e = effect(kind);
        for step in 0..=20 {
            let mesh = e.mesh(f64::from(step) / 20.0);
            assert!(mesh.vertex_count() <= kind.max_vertices(), "{kind:?}");
            assert!(mesh.is_finite(), "{kind:?}");
        }
        let (start, end) = (e.mesh(0.0).bounds(), e.mesh(1.0).bounds());
        assert!(
            close(start.min_x(), WINDOW.min_x()) && close(start.max_y(), WINDOW.max_y()),
            "{kind:?}"
        );
        assert!(
            close(end.min_x(), SLOT.min_x()) && close(end.max_y(), SLOT.max_y()),
            "{kind:?}"
        );
        assert_eq!(e.mesh(-1.0), e.mesh(0.0), "{kind:?}");
        assert_eq!(e.mesh(3.0), e.mesh(1.0), "{kind:?}");
    }
}

#[test]
fn the_direction_follows_the_dock_edge_and_survives_a_moved_slot() {
    for (edge, direction) in [
        (DockEdge::Bottom, WarpDirection::Down),
        (DockEdge::Left, WarpDirection::Left),
    ] {
        let e = MinimizeEffect::new(MinimizeKind::Warp, WINDOW, SLOT, edge);
        assert_eq!(e.direction(), direction, "{edge:?}");
    }
    let mut e = effect(MinimizeKind::Warp);
    let moved = Rect::new(800.0, 900.0, 60.0, 60.0);
    e.set_slot(moved);
    assert_eq!(e.direction(), WarpDirection::Down, "the direction is kept");
    let end = e.mesh(1.0).bounds();
    assert!(close(end.min_x(), moved.min_x()) && close(end.max_x(), moved.max_x()));
}

#[test]
fn parameters_are_kept_with_the_effect() {
    let params = MinimizeParams {
        scale_ease: Ease::Linear,
        ..MinimizeParams::default()
    };
    let e =
        MinimizeEffect::with_params(MinimizeKind::Scale, WINDOW, SLOT, DockEdge::Bottom, params);
    let default = effect(MinimizeKind::Scale);
    let linear = e.mesh(0.5).bounds();
    assert!(close(linear.min_x(), (WINDOW.min_x() + SLOT.min_x()) / 2.0));
    assert!(
        close(default.mesh(0.5).bounds().min_x(), linear.min_x()),
        "the midpoint of both eases agrees"
    );
    assert!(default.mesh(0.25).bounds().min_x() < e.mesh(0.25).bounds().min_x());
}

#[test]
fn minimizing_and_restoring_run_for_their_durations() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Scale), false);
    assert!(!a.slow_motion());
    assert_eq!(a.phase(0.0), MinimizePhase::Idle);
    assert_eq!(a.progress(0.0), 0.0);
    assert!(!a.is_animating(0.0) && !a.is_minimized(0.0));
    a.minimize(10.0);
    assert_eq!(a.phase(10.1), MinimizePhase::Minimizing);
    assert!(close(a.progress(10.1), 0.5));
    assert!(a.is_animating(10.1) && !a.is_minimized(10.1));
    assert_eq!(a.phase(10.3), MinimizePhase::Idle);
    assert!(a.is_minimized(10.3));
    assert_eq!(a.progress(10.3), 1.0);

    let kind = MinimizeKind::Warp;
    let restore = kind.restore_duration_s(false);
    let mut a = MinimizeAnimation::new(effect(kind), false);
    a.minimize(0.0);
    a.restore(1.0);
    assert_eq!(a.phase(1.1), MinimizePhase::Restoring);
    assert!(close(a.progress(1.0 + restore / 2.0), 0.5));
    assert!(!a.is_animating(1.0 + restore + 0.01));
    assert_eq!(a.progress(5.0), 0.0);
    assert!(!a.is_minimized(5.0));

    let mut slow = MinimizeAnimation::new(effect(MinimizeKind::Scale), true);
    assert!(slow.slow_motion());
    slow.minimize(0.0);
    assert!(slow.is_animating(0.3));
    assert!(!slow.is_animating(0.41));
}

#[test]
fn reversing_in_flight_continues_from_the_current_progress_in_proportional_time() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Funnel), false);
    a.minimize(0.0);
    let at = a.progress(0.1);
    a.reverse(0.1);
    assert!(close(a.progress(0.1), at), "no jump");
    assert_eq!(a.phase(0.12), MinimizePhase::Restoring);
    assert!(a.progress(0.2) < at);
    a.reverse(0.2);
    assert_eq!(a.phase(0.21), MinimizePhase::Minimizing);

    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Scale), false);
    a.minimize(0.0);
    a.restore(0.05);
    // A quarter of the way out, a quarter of the restore duration back.
    assert!(a.is_animating(0.09));
    assert!(!a.is_animating(0.11));
}

#[test]
fn the_mesh_follows_the_animation_and_its_slot() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Scale), false);
    assert!(close(a.mesh(0.0).bounds().min_x(), WINDOW.min_x()));
    a.minimize(0.0);
    assert!(close(a.mesh(1.0).bounds().min_x(), SLOT.min_x()));
    a.set_slot(Rect::new(0.0, 900.0, 60.0, 60.0));
    assert!(close(a.mesh(1.0).bounds().min_x(), 0.0));
}
