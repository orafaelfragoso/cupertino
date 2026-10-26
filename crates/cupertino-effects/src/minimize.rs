//! Reversible, interruptible minimize animations on top of the three mesh generators.
//!
//! Durations are as designed: warp 500 ms, funnel 400 ms, scale 200 ms, doubled
//! while the slow-motion modifier is held. The progress runs linearly from 0 (window) to 1 (slot);
//! the easing is inside the generators. Restoring evaluates the same meshes with the progress
//! running backwards, and the animation may be redirected at any time: the new run starts from the
//! current progress and, for a partial run, takes proportionally less time (approximate).

use crate::ease::{Driver, Ease, ProgressAnimator};
use crate::funnel::{funnel_mesh, FunnelParams};
use crate::mesh::Mesh;
use crate::scale::scale_mesh;
use crate::warp::{DockEdge, WarpDirection, WarpGeometry, WarpParams};
use cupertino_geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MinimizeKind {
    /// Two-phase funnel, 500 ms.
    Warp,
    /// Grid of vertices flying to the slot, 400 ms.
    Funnel,
    /// Plain scale, 200 ms.
    Scale,
}

impl MinimizeKind {
    pub const ALL: [MinimizeKind; 3] = [MinimizeKind::Warp, MinimizeKind::Funnel, MinimizeKind::Scale];

    pub fn duration_ms(self) -> f64 {
        match self {
            MinimizeKind::Warp => 500.0,
            MinimizeKind::Funnel => 400.0,
            MinimizeKind::Scale => 200.0,
        }
    }

    pub fn duration_s(self, slow_motion: bool) -> f64 {
        self.duration_ms() / 1000.0 * if slow_motion { 2.0 } else { 1.0 }
    }

    /// Largest vertex count the generator can emit.
    pub fn max_vertices(self) -> usize {
        match self {
            MinimizeKind::Warp => 800,
            MinimizeKind::Funnel => 400,
            MinimizeKind::Scale => 4,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MinimizeParams {
    pub warp: WarpParams,
    pub funnel: FunnelParams,
    /// Easing of the scale effect (approximate).
    pub scale_ease: Ease,
}

impl Default for MinimizeParams {
    fn default() -> Self {
        Self { warp: WarpParams::default(), funnel: FunnelParams::default(), scale_ease: Ease::Cosine }
    }
}

/// Geometry of one minimize: window, slot and effect; evaluates the mesh at a progress.
#[derive(Clone, Copy, Debug)]
pub struct MinimizeEffect {
    pub kind: MinimizeKind,
    pub window: Rect,
    pub slot: Rect,
    pub params: MinimizeParams,
    warp: WarpGeometry,
}

impl MinimizeEffect {
    pub fn new(kind: MinimizeKind, window: Rect, slot: Rect, edge: DockEdge) -> Self {
        Self::with_params(kind, window, slot, edge, MinimizeParams::default())
    }

    pub fn with_params(kind: MinimizeKind, window: Rect, slot: Rect, edge: DockEdge, params: MinimizeParams) -> Self {
        let direction = WarpDirection::for_dock(edge, window, slot);
        let warp = WarpGeometry::new(window, slot, direction, &params.warp);
        Self { kind, window, slot, params, warp }
    }

    pub fn direction(&self) -> WarpDirection {
        self.warp.direction
    }

    pub fn mesh(&self, progress: f64) -> Mesh {
        let p = progress.clamp(0.0, 1.0);
        match self.kind {
            MinimizeKind::Warp => self.warp.mesh(p),
            MinimizeKind::Funnel => funnel_mesh(self.window, self.slot, p, &self.params.funnel),
            MinimizeKind::Scale => scale_mesh(self.window, self.slot, p, self.params.scale_ease),
        }
    }

    /// Change the slot (the dock re-laid out during the animation) keeping the window.
    pub fn set_slot(&mut self, slot: Rect) {
        let direction = self.warp.direction;
        self.slot = slot;
        self.warp = WarpGeometry::new(self.window, slot, direction, &self.params.warp);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinimizePhase {
    Idle,
    Minimizing,
    Restoring,
}

#[derive(Clone, Copy, Debug)]
pub struct MinimizeAnimation {
    pub effect: MinimizeEffect,
    anim: ProgressAnimator,
    slow_motion: bool,
}

impl MinimizeAnimation {
    /// Starts un-minimized (progress 0) and idle.
    pub fn new(effect: MinimizeEffect, slow_motion: bool) -> Self {
        let driver = Driver::Timed { duration: effect.kind.duration_s(slow_motion), ease: Ease::Linear };
        Self { effect, anim: ProgressAnimator::new(driver, 0.0), slow_motion }
    }

    pub fn slow_motion(&self) -> bool {
        self.slow_motion
    }

    /// Run towards the slot from the current progress.
    pub fn minimize(&mut self, now: f64) {
        self.anim.animate_to(1.0, now);
    }

    /// Run back to the window from the current progress.
    pub fn restore(&mut self, now: f64) {
        self.anim.animate_to(0.0, now);
    }

    /// Redirect: minimize if currently restoring or idle at the window, restore otherwise.
    pub fn reverse(&mut self, now: f64) {
        if self.anim.target() >= 0.5 {
            self.restore(now);
        } else {
            self.minimize(now);
        }
    }

    pub fn progress(&self, now: f64) -> f64 {
        self.anim.value(now).clamp(0.0, 1.0)
    }

    pub fn phase(&self, now: f64) -> MinimizePhase {
        if !self.anim.is_animating(now) {
            MinimizePhase::Idle
        } else if self.anim.target() >= 0.5 {
            MinimizePhase::Minimizing
        } else {
            MinimizePhase::Restoring
        }
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.anim.is_animating(now)
    }

    /// Finished minimizing: at the slot and no longer animating.
    pub fn is_minimized(&self, now: f64) -> bool {
        !self.is_animating(now) && self.anim.target() >= 0.5
    }

    pub fn mesh(&self, now: f64) -> Mesh {
        self.effect.mesh(self.progress(now))
    }

    pub fn set_slot(&mut self, slot: Rect) {
        self.effect.set_slot(slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(kind: MinimizeKind) -> MinimizeEffect {
        MinimizeEffect::new(kind, Rect::new(300.0, 100.0, 800.0, 500.0), Rect::new(1200.0, 1000.0, 64.0, 64.0), DockEdge::Bottom)
    }

    #[test]
    fn durations_are_verified() {
        assert_eq!(MinimizeKind::Warp.duration_ms(), 500.0);
        assert_eq!(MinimizeKind::Funnel.duration_ms(), 400.0);
        assert_eq!(MinimizeKind::Scale.duration_ms(), 200.0);
        assert_eq!(MinimizeKind::Scale.duration_s(true), 0.4);
    }

    #[test]
    fn every_kind_starts_at_the_window_and_ends_at_the_slot() {
        for kind in MinimizeKind::ALL {
            let e = effect(kind);
            let (b0, b1) = (e.mesh(0.0).bounds(), e.mesh(1.0).bounds());
            assert!((b0.min_x() - e.window.min_x()).abs() < 1e-3 && (b0.max_y() - e.window.max_y()).abs() < 1e-3, "{kind:?}");
            assert!((b1.min_x() - e.slot.min_x()).abs() < 1e-3 && (b1.max_x() - e.slot.max_x()).abs() < 1e-3, "{kind:?}");
            assert!((b1.min_y() - e.slot.min_y()).abs() < 1e-3 && (b1.max_y() - e.slot.max_y()).abs() < 1e-3, "{kind:?}");
        }
    }

    #[test]
    fn meshes_are_finite_and_within_budget_for_all_progress_values() {
        for kind in MinimizeKind::ALL {
            let e = effect(kind);
            for k in -5..=105 {
                let m = e.mesh(k as f64 / 100.0);
                assert!(m.is_finite());
                assert!(m.vertex_count() <= kind.max_vertices(), "{kind:?}: {}", m.vertex_count());
            }
        }
    }

    #[test]
    fn texture_corners_approach_their_slot_corners_monotonically() {
        for kind in MinimizeKind::ALL {
            for (win, slot, edge) in [
                (Rect::new(300.0, 100.0, 800.0, 500.0), Rect::new(1200.0, 1000.0, 64.0, 64.0), DockEdge::Bottom),
                (Rect::new(300.0, 900.0, 800.0, 400.0), Rect::new(200.0, 300.0, 64.0, 64.0), DockEdge::Bottom),
                (Rect::new(300.0, 100.0, 800.0, 500.0), Rect::new(1800.0, 400.0, 64.0, 64.0), DockEdge::Right),
                (Rect::new(300.0, 100.0, 800.0, 500.0), Rect::new(20.0, 300.0, 64.0, 64.0), DockEdge::Left),
            ] {
                let e = MinimizeEffect::new(kind, win, slot, edge);
                let corners = [([0.0f32, 0.0f32], (slot.min_x(), slot.min_y())), ([1.0, 1.0], (slot.max_x(), slot.max_y()))];
                for (uv, target) in corners {
                    let mut prev = f64::INFINITY;
                    for k in 0..=200 {
                        let m = e.mesh(k as f64 / 200.0);
                        let v = m.vertices.iter().find(|v| v.uv == uv).unwrap();
                        let d = (v.position[0] as f64 - target.0).hypot(v.position[1] as f64 - target.1);
                        assert!(d <= prev + 1e-2, "{kind:?} {edge:?} uv {uv:?} k={k}: {d} > {prev}");
                        prev = d;
                    }
                    assert!(prev < 1e-2);
                }
            }
        }
    }

    #[test]
    fn full_run_takes_the_verified_duration() {
        for kind in MinimizeKind::ALL {
            let mut a = MinimizeAnimation::new(effect(kind), false);
            a.minimize(10.0);
            let d = kind.duration_s(false);
            assert_eq!(a.phase(10.0 + d * 0.5), MinimizePhase::Minimizing);
            assert!(a.is_animating(10.0 + d - 1e-6));
            assert!(!a.is_animating(10.0 + d + 1e-6));
            assert!(a.is_minimized(10.0 + d + 1e-6));
            assert!((a.progress(10.0 + d + 1.0) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn restore_retraces_the_minimize_meshes() {
        let mut down = MinimizeAnimation::new(effect(MinimizeKind::Warp), false);
        down.minimize(0.0);
        let mut up = down;
        up.restore(1.0);
        let d = MinimizeKind::Warp.duration_s(false);
        for k in 0..=10 {
            let f = k as f64 / 10.0;
            let a = down.mesh(f * d);
            let b = up.mesh(1.0 + (1.0 - f) * d);
            assert!(a.max_distance(&b) < 1e-2, "f={f}");
        }
        assert!(up.mesh(1.0 + d + 0.01).bounds().height() > 400.0);
    }

    #[test]
    fn interrupting_keeps_the_mesh_continuous() {
        for kind in MinimizeKind::ALL {
            let mut a = MinimizeAnimation::new(effect(kind), false);
            a.minimize(0.0);
            let t = kind.duration_s(false) * 0.4;
            let before = a.mesh(t);
            a.reverse(t);
            let after = a.mesh(t);
            assert!(before.max_distance(&after) < 1e-3, "{kind:?}");
            assert_eq!(a.phase(t + 0.01), MinimizePhase::Restoring);
            assert!(a.progress(t + 0.05) < a.progress(t));
            assert!(a.progress(t + 10.0) < 1e-12);
            a.reverse(t + 10.0);
            assert_eq!(a.phase(t + 10.01), MinimizePhase::Minimizing);
        }
    }

    #[test]
    fn slow_motion_doubles_the_duration() {
        let mut a = MinimizeAnimation::new(effect(MinimizeKind::Funnel), true);
        a.minimize(0.0);
        assert!(a.is_animating(0.7) && !a.is_animating(0.81));
    }

    #[test]
    fn slot_can_move_during_the_animation() {
        let mut a = MinimizeAnimation::new(effect(MinimizeKind::Warp), false);
        a.minimize(0.0);
        a.set_slot(Rect::new(1300.0, 1000.0, 64.0, 64.0));
        let b = a.mesh(10.0).bounds();
        assert!((b.min_x() - 1300.0).abs() < 1e-3);
    }
}
