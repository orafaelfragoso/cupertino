//! A minimize animation runs one of three effects towards a dock slot and back, and can be reversed mid-flight.

use cupertino_effects::{
    DockEdge, MinimizeAnimation, MinimizeEffect, MinimizeKind, MinimizeParams, MinimizePhase,
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
fn each_kind_has_its_own_durations_and_slow_motion_doubles_them() {
    assert_eq!(MinimizeKind::ALL.len(), 3);
    for kind in MinimizeKind::ALL {
        assert!(close(kind.duration_s(false), kind.duration_ms() / 1000.0));
        assert!(close(kind.duration_s(true), 2.0 * kind.duration_s(false)));
        assert!(close(
            kind.restore_duration_s(true),
            2.0 * kind.restore_duration_s(false)
        ));
    }
    assert!(MinimizeKind::Scale.duration_ms() < MinimizeKind::Warp.duration_ms());
}

#[test]
fn the_vertex_budget_grows_with_the_effect() {
    assert!(MinimizeKind::Scale.max_vertices() < MinimizeKind::Funnel.max_vertices());
    assert!(MinimizeKind::Funnel.max_vertices() < MinimizeKind::Warp.max_vertices());
}

#[test]
fn every_effect_stays_within_its_vertex_budget_at_every_progress() {
    for kind in MinimizeKind::ALL {
        let e = effect(kind);
        for step in 0..=20 {
            let mesh = e.mesh(f64::from(step) / 20.0);
            assert!(mesh.vertex_count() <= kind.max_vertices(), "{kind:?}");
            assert!(mesh.is_finite());
        }
    }
}

#[test]
fn every_effect_begins_at_the_window_and_ends_at_the_slot() {
    for kind in MinimizeKind::ALL {
        let e = effect(kind);
        let (start, end) = (e.mesh(0.0).bounds(), e.mesh(1.0).bounds());
        assert!(close(start.min_x(), WINDOW.min_x()) && close(start.max_y(), WINDOW.max_y()));
        assert!(close(end.min_x(), SLOT.min_x()) && close(end.max_y(), SLOT.max_y()));
    }
}

#[test]
fn progress_outside_the_range_is_clamped() {
    let e = effect(MinimizeKind::Funnel);
    assert_eq!(e.mesh(-1.0), e.mesh(0.0));
    assert_eq!(e.mesh(3.0), e.mesh(1.0));
}

#[test]
fn the_direction_follows_the_dock_edge() {
    let down = MinimizeEffect::new(MinimizeKind::Warp, WINDOW, SLOT, DockEdge::Bottom);
    assert_eq!(down.direction(), WarpDirection::Down);
    let left = MinimizeEffect::new(MinimizeKind::Warp, WINDOW, SLOT, DockEdge::Left);
    assert_eq!(left.direction(), WarpDirection::Left);
}

#[test]
fn moving_the_slot_retargets_the_effect() {
    let mut e = effect(MinimizeKind::Warp);
    let moved = Rect::new(800.0, 900.0, 60.0, 60.0);
    e.set_slot(moved);
    assert_eq!(e.slot, moved);
    assert_eq!(e.direction(), WarpDirection::Down, "the direction is kept");
    let end = e.mesh(1.0).bounds();
    assert!(close(end.min_x(), moved.min_x()) && close(end.max_x(), moved.max_x()));
}

#[test]
fn parameters_are_kept_with_the_effect() {
    let params = MinimizeParams {
        scale_ease: cupertino_effects::Ease::Linear,
        ..MinimizeParams::default()
    };
    let e =
        MinimizeEffect::with_params(MinimizeKind::Scale, WINDOW, SLOT, DockEdge::Bottom, params);
    let linear = e.mesh(0.5).bounds();
    let eased = effect(MinimizeKind::Scale).mesh(0.5).bounds();
    assert!(close(linear.min_x(), (WINDOW.min_x() + SLOT.min_x()) / 2.0));
    assert!(
        close(eased.min_x(), linear.min_x()),
        "the midpoint of both eases agrees"
    );
    let early = e.mesh(0.25).bounds();
    let early_eased = effect(MinimizeKind::Scale).mesh(0.25).bounds();
    assert!(early_eased.min_x() < early.min_x());
}

#[test]
fn a_new_animation_is_idle_at_the_start() {
    let a = MinimizeAnimation::new(effect(MinimizeKind::Scale), false);
    assert_eq!(a.phase(0.0), MinimizePhase::Idle);
    assert_eq!(a.progress(0.0), 0.0);
    assert!(!a.is_animating(0.0));
    assert!(!a.is_minimized(0.0));
    assert!(!a.slow_motion());
}

#[test]
fn minimizing_runs_for_the_kinds_duration_and_ends_minimized() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Scale), false);
    a.minimize(10.0);
    assert_eq!(a.phase(10.1), MinimizePhase::Minimizing);
    assert!(close(a.progress(10.1), 0.5));
    assert!(a.is_animating(10.1));
    assert!(!a.is_minimized(10.1));
    assert_eq!(a.phase(10.3), MinimizePhase::Idle);
    assert!(a.is_minimized(10.3));
    assert_eq!(a.progress(10.3), 1.0);
}

#[test]
fn restoring_takes_the_restore_duration() {
    let kind = MinimizeKind::Warp;
    let mut a = MinimizeAnimation::new(effect(kind), false);
    a.minimize(0.0);
    a.restore(1.0);
    assert_eq!(a.phase(1.1), MinimizePhase::Restoring);
    let half = kind.restore_duration_s(false) / 2.0;
    assert!(close(a.progress(1.0 + half), 0.5));
    assert!(!a.is_animating(1.0 + kind.restore_duration_s(false) + 0.01));
    assert_eq!(a.progress(5.0), 0.0);
    assert!(!a.is_minimized(5.0));
}

#[test]
fn slow_motion_stretches_the_animation() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Scale), true);
    assert!(a.slow_motion());
    a.minimize(0.0);
    assert!(a.is_animating(0.3));
    assert!(!a.is_animating(0.41));
}

#[test]
fn reversing_in_flight_continues_from_the_current_progress() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Funnel), false);
    a.minimize(0.0);
    let at = a.progress(0.1);
    a.reverse(0.1);
    assert!(close(a.progress(0.1), at), "no jump");
    assert_eq!(a.phase(0.12), MinimizePhase::Restoring);
    assert!(a.progress(0.2) < at);
    a.reverse(0.2);
    assert_eq!(a.phase(0.21), MinimizePhase::Minimizing);
}

#[test]
fn a_part_way_reversal_takes_proportionally_less_time() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Scale), false);
    a.minimize(0.0);
    a.restore(0.05);
    // A quarter of the way out, a quarter of the restore duration back.
    assert!(a.is_animating(0.09));
    assert!(!a.is_animating(0.11));
}

#[test]
fn the_mesh_follows_the_animation() {
    let mut a = MinimizeAnimation::new(effect(MinimizeKind::Scale), false);
    assert!(close(a.mesh(0.0).bounds().min_x(), WINDOW.min_x()));
    a.minimize(0.0);
    assert!(close(a.mesh(1.0).bounds().min_x(), SLOT.min_x()));
    a.set_slot(Rect::new(0.0, 900.0, 60.0, 60.0));
    assert!(close(a.mesh(1.0).bounds().min_x(), 0.0));
}
