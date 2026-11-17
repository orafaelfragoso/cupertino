//! Reversible, interruptible minimize animations on top of the three mesh generators.
//!
//! Durations: the warp bends into the slot in 450 ms and unfolds back out of it in 420 ms; the funnel takes 400 ms and
//! the scale 200 ms either way. All are doubled while the slow-motion modifier is held. The progress runs linearly from
//! 0 (window) to 1 (slot); the easing is inside the generators, which already start and end slowly. Restoring
//! evaluates the same meshes with the progress running backwards, and the animation may be redirected at any time: the
//! new run starts from the current progress and, for a partial run, takes proportionally less time (approximate).

use crate::ease::{Driver, Ease, ProgressAnimator};
use crate::funnel::{funnel_mesh, FunnelParams};
use crate::mesh::Mesh;
use crate::scale::scale_mesh;
use crate::warp::{DockEdge, WarpDirection, WarpGeometry, WarpParams};
use cupertino_geometry::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MinimizeKind {
    /// Two-phase funnel, 450 ms to the slot and 420 ms back.
    Warp,
    /// Grid of vertices flying to the slot, 400 ms.
    Funnel,
    /// Plain scale, 200 ms.
    Scale,
}

impl MinimizeKind {
    pub const ALL: [MinimizeKind; 3] = [
        MinimizeKind::Warp,
        MinimizeKind::Funnel,
        MinimizeKind::Scale,
    ];

    /// How long a whole run from the window into the slot takes.
    pub fn duration_ms(self) -> f64 {
        match self {
            MinimizeKind::Warp => 450.0,
            MinimizeKind::Funnel => 400.0,
            MinimizeKind::Scale => 200.0,
        }
    }

    /// How long a whole run from the slot back to the window takes: the warp unfolds a little faster than it bends.
    pub fn restore_duration_ms(self) -> f64 {
        match self {
            MinimizeKind::Warp => 420.0,
            MinimizeKind::Funnel => 400.0,
            MinimizeKind::Scale => 200.0,
        }
    }

    pub fn duration_s(self, slow_motion: bool) -> f64 {
        self.duration_ms() / 1000.0 * slow_factor(slow_motion)
    }

    pub fn restore_duration_s(self, slow_motion: bool) -> f64 {
        self.restore_duration_ms() / 1000.0 * slow_factor(slow_motion)
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

/// How much the slow-motion modifier stretches every minimize run.
fn slow_factor(slow_motion: bool) -> f64 {
    if slow_motion {
        2.0
    } else {
        1.0
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
        Self {
            warp: WarpParams::default(),
            funnel: FunnelParams::default(),
            scale_ease: Ease::Cosine,
        }
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

    pub fn with_params(
        kind: MinimizeKind,
        window: Rect,
        slot: Rect,
        edge: DockEdge,
        params: MinimizeParams,
    ) -> Self {
        let direction = WarpDirection::for_dock(edge, window, slot);
        let warp = WarpGeometry::new(window, slot, direction, &params.warp);
        Self {
            kind,
            window,
            slot,
            params,
            warp,
        }
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
        Self {
            effect,
            anim: ProgressAnimator::new(Self::driver(effect.kind.duration_s(slow_motion)), 0.0),
            slow_motion,
        }
    }

    fn driver(duration: f64) -> Driver {
        Driver::Timed {
            duration,
            ease: Ease::Linear,
        }
    }

    /// Run to `to` from the current progress on a whole run of `duration`.
    fn run(&mut self, to: f64, duration: f64, now: f64) {
        let mut anim = ProgressAnimator::new(Self::driver(duration), 0.0);
        anim.set(self.anim.value(now));
        anim.animate_to(to, now);
        self.anim = anim;
    }

    pub fn slow_motion(&self) -> bool {
        self.slow_motion
    }

    /// Run towards the slot from the current progress.
    pub fn minimize(&mut self, now: f64) {
        let duration = self.effect.kind.duration_s(self.slow_motion);
        self.run(1.0, duration, now);
    }

    /// Run back to the window from the current progress.
    pub fn restore(&mut self, now: f64) {
        let duration = self.effect.kind.restore_duration_s(self.slow_motion);
        self.run(0.0, duration, now);
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
