//! Toplevel lifecycle state machine.
//!
//! ```text
//! Created ─map─► Opening ─► Normal ─minimize─► Minimizing ─► Minimized ─restore─► Restoring ─► (prior)
//! Normal ─zoom─► Zooming ─► Zoomed ─zoom─► Unzooming ─► Normal
//! (any layout state) ─fullscreen─► EnterFullScreen ─► FullScreen ─exit─► ExitFullScreen ─► (prior)
//! (any layout state) ─tile(kind)─► Tiling(kind) ─► Tiled(kind) ─drag out─► Normal
//! (any state) ─close─► Closing ─► Destroyed          (destroy short-circuits to Destroyed)
//! ```
//!
//! Every transient state is interruptible: a new command retargets the running
//! [`AnimatedValue`]s from their current position and velocity (C¹ continuity); a `sin²` zoom run
//! restarts from the current frame without velocity. The order-in / order-out transforms are
//! fixed functions of time: a close during an opening transform starts the close transform from
//! its beginning. Time is an absolute timestamp in seconds.
//!
//! open / close transforms per [`WindowKind`] (plain windows have
//! none; sheets slide), zoom duration and `sin²` ease, the full-screen and tiling curves
//! (see [`AnimationConfig`]).
//!
//! Invariants (spec §1): restoring returns the exact pre-state geometry. `normal_frame` is only
//! written while the window is in `Normal`/`Opening`; a window that goes Zoomed → Tiled → Normal
//! therefore comes back at the geometry it had before the zoom. A window that was zoomed/tiled
//! before it was minimized or sent to full screen returns to that zoomed/tiled state (design
//! decision: the spec shows "→ Normal" for the common case where the prior state is Normal).

use cupertino_animation::spring::Spring;
use cupertino_animation::{AnimatedValue, CubicBezier, Curve};
use cupertino_effects::{
    cosine, frame_change_duration, ProgressAnimator, TransformKind, TransformSample, WindowKind,
};

use crate::config::AnimationConfig;
use crate::geometry::{Edges, Rect};
use crate::tiling::TileKind;

/// Layout state a window rests in (and returns to after minimize / full screen).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resting {
    Normal,
    Zoomed,
    Tiled(TileKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    Created,
    Opening,
    Normal,
    Minimizing,
    Minimized,
    Restoring,
    Zooming,
    Zoomed,
    Unzooming,
    EnterFullScreen,
    FullScreen,
    ExitFullScreen,
    Tiling(TileKind),
    Tiled(TileKind),
    Closing,
    Destroyed,
}

impl LifecycleState {
    pub fn is_transient(self) -> bool {
        use LifecycleState::*;
        matches!(
            self,
            Opening
                | Minimizing
                | Restoring
                | Zooming
                | Unzooming
                | EnterFullScreen
                | ExitFullScreen
                | Tiling(_)
                | Closing
        )
    }
    /// Whether the window is drawn at all.
    pub fn is_visible(self) -> bool {
        !matches!(
            self,
            LifecycleState::Created | LifecycleState::Minimized | LifecycleState::Destroyed
        )
    }
    pub fn is_minimized(self) -> bool {
        matches!(self, LifecycleState::Minimizing | LifecycleState::Minimized)
    }
    pub fn is_fullscreen(self) -> bool {
        matches!(
            self,
            LifecycleState::EnterFullScreen | LifecycleState::FullScreen
        )
    }
    pub fn is_dead(self) -> bool {
        matches!(self, LifecycleState::Closing | LifecycleState::Destroyed)
    }
}

impl From<Resting> for LifecycleState {
    fn from(r: Resting) -> Self {
        match r {
            Resting::Normal => LifecycleState::Normal,
            Resting::Zoomed => LifecycleState::Zoomed,
            Resting::Tiled(k) => LifecycleState::Tiled(k),
        }
    }
}

/// `xdg_toplevel` states to send in a configure (the activated flag is added by the caller).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToplevelStates {
    pub activated: bool,
    pub maximized: bool,
    pub fullscreen: bool,
    pub resizing: bool,
    pub tiled: Edges,
}

/// How the compositor frames a window in its current states: the corner mask, the decorations and the shadow it draws
/// around the window's content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameShape {
    /// Rounded corners, the title bar and the shadow: every window that is not full screen, a zoomed window and one that
    /// fills the usable area included.
    Rounded,
    /// Square corners, no decorations and no shadow: a full-screen window meets the edges of its output.
    Square,
}

impl ToplevelStates {
    /// The frame of a window in these states: only full screen squares it.
    pub fn frame_shape(&self) -> FrameShape {
        if self.fullscreen {
            FrameShape::Square
        } else {
            FrameShape::Rounded
        }
    }
}

/// Everything the renderer needs to draw a window at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presentation {
    pub frame: Rect,
    pub opacity: f64,
    pub scale: f64,
    pub shadow: f64,
    /// 0 = at `frame`, 1 = collapsed into `minimize_target` (drives the warp/scale/funnel mesh).
    pub minimize_progress: f64,
    pub minimize_target: Rect,
    pub visible: bool,
}

/// A frame change eased with `sin²` (for zoom), which the bézier and spring
/// curves of [`AnimatedValue`] cannot express.
#[derive(Clone, Copy, Debug)]
struct Sin2Run {
    from: Rect,
    to: Rect,
    start: f64,
    duration: f64,
}

/// The animated window frame: a [`Curve`] animation or a `sin²` run.
#[derive(Clone, Debug)]
struct FrameMotion {
    curve: AnimatedValue<[f64; 4]>,
    sin2: Option<Sin2Run>,
}

impl FrameMotion {
    fn new(frame: Rect) -> Self {
        Self {
            curve: AnimatedValue::new(frame.to_array()),
            sin2: None,
        }
    }

    fn value(&self, now: f64) -> Rect {
        match self.sin2 {
            Some(r) if r.duration > 0.0 => Rect::lerp(
                r.from,
                r.to,
                cosine(((now - r.start) / r.duration).clamp(0.0, 1.0)),
            ),
            Some(r) => r.to,
            None => Rect::from_array(self.curve.value(now)),
        }
    }

    fn animate_to(&mut self, to: Rect, curve: Curve, now: f64) {
        if self.sin2.is_some() {
            let v = self.value(now);
            self.set_immediately(v);
        }
        self.curve.animate_to(to.to_array(), curve, now);
    }

    fn ease_to(&mut self, to: Rect, duration: f64, now: f64) {
        let from = self.value(now);
        self.curve.set_immediately(to.to_array());
        self.sin2 = Some(Sin2Run {
            from,
            to,
            start: now,
            duration,
        });
    }

    fn set_immediately(&mut self, frame: Rect) {
        self.sin2 = None;
        self.curve.set_immediately(frame.to_array());
    }
}

/// A running order-in / order-out transform.
#[derive(Clone, Copy, Debug)]
struct Transform {
    kind: TransformKind,
    progress: ProgressAnimator,
}

/// The window lagging behind its frame after a snap or unsnap jump: the offset decays to zero.
#[derive(Clone, Copy, Debug)]
struct CatchUp {
    offset: [f64; 4],
    start: f64,
    duration: f64,
    curve: CubicBezier,
}

#[derive(Clone, Debug)]
pub struct Lifecycle {
    state: LifecycleState,
    cfg: AnimationConfig,
    kind: WindowKind,
    rest: Resting,
    normal_frame: Rect,
    zoom_frame: Rect,
    tile_frame: Rect,
    fs_frame: Rect,
    minimize_target: Rect,
    frame: FrameMotion,
    transform: Option<Transform>,
    catch_up: Option<CatchUp>,
    shadow: AnimatedValue<f64>,
    warp: AnimatedValue<f64>,
    ends_at: f64,
    snap_pending: bool,
}

fn instant() -> Curve {
    Curve::Timed {
        duration: 0.0,
        bezier: CubicBezier::LINEAR,
    }
}

/// Largest absolute change of x, y, width or height (the `maxΔ` of ).
fn max_delta(a: Rect, b: Rect) -> f64 {
    [b.x - a.x, b.y - a.y, b.w - a.w, b.h - a.h]
        .into_iter()
        .fold(0.0, |m, d| m.max(d.abs()))
}

pub fn curve_duration(c: Curve) -> f64 {
    match c {
        Curve::Timed { duration, .. } => duration,
        Curve::Spring(p) => Spring::new(p).settling_duration(),
    }
}

impl Lifecycle {
    pub fn new(cfg: AnimationConfig, kind: WindowKind, frame: Rect) -> Self {
        Self {
            state: LifecycleState::Created,
            cfg,
            kind,
            rest: Resting::Normal,
            normal_frame: frame,
            zoom_frame: frame,
            tile_frame: frame,
            fs_frame: frame,
            minimize_target: frame,
            frame: FrameMotion::new(frame),
            transform: None,
            catch_up: None,
            shadow: AnimatedValue::new(1.0),
            warp: AnimatedValue::new(0.0),
            ends_at: 0.0,
            snap_pending: false,
        }
    }

    pub fn state(&self) -> LifecycleState {
        self.state
    }
    pub fn kind(&self) -> WindowKind {
        self.kind
    }
    pub fn resting(&self) -> Resting {
        self.rest
    }
    pub fn normal_frame(&self) -> Rect {
        self.normal_frame
    }
    pub fn minimize_target(&self) -> Rect {
        self.minimize_target
    }

    fn frame_of(&self, r: Resting) -> Rect {
        match r {
            Resting::Normal => self.normal_frame,
            Resting::Zoomed => self.zoom_frame,
            Resting::Tiled(_) => self.tile_frame,
        }
    }

    /// Logical frame the client is (or will be) configured to.
    pub fn target_frame(&self) -> Rect {
        match self.state {
            LifecycleState::EnterFullScreen | LifecycleState::FullScreen => self.fs_frame,
            _ => self.frame_of(self.rest),
        }
    }

    pub fn is_animating(&self, now: f64) -> bool {
        self.state.is_transient() || now < self.ends_at || self.is_catching_up(now)
    }

    /// Whether a snap catch-up started by [`Lifecycle::catch_up_to`] is still running.
    pub fn is_catching_up(&self, now: f64) -> bool {
        self.catch_up.is_some_and(|c| now < c.start + c.duration)
    }

    pub fn toplevel_states(&self) -> ToplevelStates {
        let mut s = ToplevelStates::default();
        let rest_states = |s: &mut ToplevelStates, r: Resting| match r {
            Resting::Normal => {}
            Resting::Zoomed => s.maximized = true,
            Resting::Tiled(k) => s.tiled = k.boundary_edges(),
        };
        match self.state {
            LifecycleState::Unzooming => {}
            LifecycleState::Zooming | LifecycleState::Zoomed => s.maximized = true,
            LifecycleState::EnterFullScreen | LifecycleState::FullScreen => s.fullscreen = true,
            LifecycleState::Tiling(k) | LifecycleState::Tiled(k) => s.tiled = k.boundary_edges(),
            LifecycleState::Minimizing
            | LifecycleState::Minimized
            | LifecycleState::Restoring
            | LifecycleState::ExitFullScreen => rest_states(&mut s, self.rest),
            _ => {}
        }
        s
    }

    pub fn presentation(&self, now: f64) -> Presentation {
        let warp = self.warp.value(now);
        let t = self.transform.map_or(TransformSample::IDENTITY, |t| {
            t.kind.sample(t.progress.value(now))
        });
        let shadow_factor = self
            .transform
            .map_or(1.0, |t| t.kind.shadow_factor(t.progress.value(now)));
        let mut frame = self.frame.value(now);
        if let Some(c) = self.catch_up {
            let left = 1.0 - c.curve.eval(((now - c.start) / c.duration).clamp(0.0, 1.0));
            let f = frame.to_array();
            frame = Rect::from_array(std::array::from_fn(|i| f[i] + c.offset[i] * left));
        }
        // a sheet slides along y while it opens and closes
        let frame = frame.translate(0.0, t.dy);
        Presentation {
            frame,
            opacity: t.alpha,
            scale: t.scale,
            shadow: self.shadow.value(now) * shadow_factor,
            minimize_progress: warp,
            minimize_target: self.minimize_target,
            visible: self.state.is_visible(),
        }
    }

    fn start(&mut self, state: LifecycleState, duration: f64, now: f64) {
        self.state = state;
        self.ends_at = now + duration;
    }

    /// Start `kind` (or no transform) and return its duration.
    fn play(&mut self, kind: Option<TransformKind>, now: f64) -> f64 {
        self.transform = kind.map(|kind| {
            let mut progress = kind.animator(false);
            progress.animate_to(1.0, now);
            Transform { kind, progress }
        });
        self.transform
            .map_or(0.0, |t| t.progress.driver().settle_time())
    }

    fn layout_active(&self) -> bool {
        use LifecycleState::*;
        matches!(
            self.state,
            Opening | Normal | Zooming | Zoomed | Unzooming | Tiling(_) | Tiled(_)
        )
    }

    fn enter_rest(
        &mut self,
        rest: Resting,
        target: Rect,
        state: LifecycleState,
        curve: Curve,
        now: f64,
    ) {
        self.rest = rest;
        match rest {
            Resting::Zoomed => self.zoom_frame = target,
            Resting::Tiled(_) => self.tile_frame = target,
            Resting::Normal => {}
        }
        self.frame.animate_to(target, curve, now);
        self.start(state, curve_duration(curve), now);
    }

    /// Created → Opening with the order-in transform of the window kind (none for plain windows,
    /// which become Normal on the next tick).
    pub fn map(&mut self, now: f64) -> bool {
        if self.state != LifecycleState::Created {
            return false;
        }
        let d = self.play(self.kind.order_in(), now);
        self.start(LifecycleState::Opening, d, now);
        true
    }

    /// Replace the Normal geometry (client-driven or interactive). Only valid while Created,
    /// Opening or Normal.
    pub fn set_normal_frame(&mut self, f: Rect) -> bool {
        use LifecycleState::*;
        if !matches!(self.state, Created | Opening | Normal) {
            return false;
        }
        self.normal_frame = f;
        self.frame.set_immediately(f);
        self.snap_pending = false;
        true
    }

    /// [`Lifecycle::set_normal_frame`] as a programmatic frame change: the drawn frame moves to `f`
    /// with the zoom duration and `sin²` ease (the size snaps of call the same
    /// animated frame setter, to take that path). False outside the normal layout.
    pub fn animate_normal_frame(&mut self, f: Rect, now: f64) -> bool {
        use LifecycleState::*;
        if !matches!(self.state, Opening | Normal) {
            return false;
        }
        let d = frame_change_duration(max_delta(self.frame.value(now), f));
        self.normal_frame = f;
        self.frame.ease_to(f, d, now);
        self.ends_at = now + d;
        self.snap_pending = true;
        true
    }

    /// How far a sheet is shown, for its parent's dimming (the progress `p`
    /// while it opens, `1 − p` while it closes, 1 in between); 0 for a window that is not a sheet
    /// or is not on screen.
    pub fn sheet_shown(&self, now: f64) -> f64 {
        if self.kind != WindowKind::Sheet || !self.state.is_visible() {
            return 0.0;
        }
        match self.transform {
            Some(Transform {
                kind: TransformKind::SheetIn,
                progress,
            }) => progress.value(now).clamp(0.0, 1.0),
            Some(Transform {
                kind: TransformKind::SheetOut,
                progress,
            }) => (1.0 - progress.value(now)).clamp(0.0, 1.0),
            _ if self.state == LifecycleState::Closing => 0.0,
            _ => 1.0,
        }
    }

    /// [`Lifecycle::set_normal_frame`] for a window that jumped away from the pointer: the drawn
    /// frame starts where it is now and catches up with `f` over `duration` along `curve`. Later
    /// frame changes keep the remaining lag until it has run out.
    pub fn catch_up_to(&mut self, f: Rect, duration: f64, curve: CubicBezier, now: f64) -> bool {
        let shown = self.presentation(now).frame.to_array();
        if !self.set_normal_frame(f) {
            return false;
        }
        let target = f.to_array();
        self.catch_up = Some(CatchUp {
            offset: std::array::from_fn(|i| shown[i] - target[i]),
            start: now,
            duration,
            curve,
        });
        true
    }

    /// The output was resized: replace the frame of the current resting layout (`frame` for the
    /// zoomed / tiled layouts, `fullscreen` for full screen). Returns true when the geometry
    /// changed. Applies immediately.
    pub fn refit(&mut self, frame: Rect, fullscreen: Rect) -> bool {
        use LifecycleState::*;
        match self.state {
            EnterFullScreen | FullScreen => {
                if self.fs_frame == fullscreen {
                    return false;
                }
                self.fs_frame = fullscreen;
                self.frame.set_immediately(fullscreen);
            }
            Zoomed => {
                if self.zoom_frame == frame {
                    return false;
                }
                self.zoom_frame = frame;
                self.frame.set_immediately(frame);
            }
            Tiled(_) | Tiling(_) => {
                if self.tile_frame == frame {
                    return false;
                }
                self.tile_frame = frame;
                self.frame.set_immediately(frame);
            }
            _ => return false,
        }
        true
    }

    /// The window moved to another output: replace every stored frame at once, `normal` for the normal layout, `zoom` and
    /// `tile` for the zoomed and tiled layouts and `fullscreen` for full screen, so leaving any of them later lands on
    /// the new output. The drawn frame jumps to the current one. Returns true when the configured frame changed.
    pub fn move_to_output(
        &mut self,
        normal: Rect,
        zoom: Rect,
        tile: Rect,
        fullscreen: Rect,
    ) -> bool {
        let before = self.target_frame();
        self.normal_frame = normal;
        self.zoom_frame = zoom;
        self.tile_frame = tile;
        self.fs_frame = fullscreen;
        let after = self.target_frame();
        self.frame.set_immediately(after);
        before != after
    }

    /// Zoom over `min(0.35, 181·ln(maxΔ/100 + 1)/750)` s with the `sin²` ease.
    pub fn zoom(&mut self, target: Rect, now: f64) -> bool {
        use LifecycleState::*;
        if !self.layout_active() || matches!(self.state, Zooming | Zoomed) {
            return false;
        }
        self.zoom_frame = target;
        self.rest = Resting::Zoomed;
        self.frame_change(target, Zooming, now);
        true
    }

    pub fn unzoom(&mut self, now: f64) -> bool {
        use LifecycleState::*;
        if !matches!(self.state, Zooming | Zoomed) {
            return false;
        }
        self.rest = Resting::Normal;
        self.frame_change(self.normal_frame, Unzooming, now);
        true
    }

    fn frame_change(&mut self, target: Rect, state: LifecycleState, now: f64) {
        let d = frame_change_duration(max_delta(self.frame.value(now), target));
        self.frame.ease_to(target, d, now);
        self.start(state, d, now);
    }

    pub fn tile(&mut self, kind: TileKind, target: Rect, now: f64) -> bool {
        use LifecycleState::*;
        if !self.layout_active() {
            return false;
        }
        if matches!(self.state, Tiling(_) | Tiled(_))
            && self.rest == Resting::Tiled(kind)
            && self.tile_frame == target
        {
            return false;
        }
        let c = if self.cfg.disable_tiling {
            instant()
        } else {
            self.cfg.tiling
        };
        self.enter_rest(Resting::Tiled(kind), target, Tiling(kind), c, now);
        true
    }

    /// Zoomed/Tiled → Normal without a transient state (the user dragged the window out). The
    /// frame animates to `frame` (default: the stored normal frame, i.e. the pre-state size).
    pub fn drag_out(&mut self, frame: Option<Rect>, now: f64) -> bool {
        use LifecycleState::*;
        if !matches!(self.state, Zooming | Zoomed | Tiling(_) | Tiled(_)) {
            return false;
        }
        if let Some(f) = frame {
            self.normal_frame = f;
        }
        let c = if self.cfg.disable_tiling {
            instant()
        } else {
            self.cfg.untile
        };
        let t = self.normal_frame;
        self.rest = Resting::Normal;
        self.frame.animate_to(t, c, now);
        self.state = Normal;
        self.ends_at = now + curve_duration(c);
        self.snap_pending = true;
        true
    }

    /// Start (or retarget) the minimize animation towards `target` (e.g. a dock tile rect).
    pub fn minimize(&mut self, target: Rect, now: f64) -> bool {
        use LifecycleState::*;
        if !(self.layout_active() || self.state == Restoring) {
            return false;
        }
        self.minimize_target = target;
        let c = self.cfg.minimizing;
        self.warp.animate_to(1.0, c, now);
        self.start(Minimizing, curve_duration(c), now);
        true
    }

    /// Retarget the rectangle of a minimizing or minimized window; false in any other state.
    pub fn set_minimize_target(&mut self, target: Rect) -> bool {
        let applies = matches!(
            self.state,
            LifecycleState::Minimizing | LifecycleState::Minimized
        );
        if applies {
            self.minimize_target = target;
        }
        applies
    }

    pub fn restore(&mut self, now: f64) -> bool {
        use LifecycleState::*;
        if !matches!(self.state, Minimizing | Minimized) {
            return false;
        }
        let c = self.cfg.restoring;
        self.warp.animate_to(0.0, c, now);
        self.start(Restoring, curve_duration(c), now);
        true
    }

    pub fn fullscreen(&mut self, output: Rect, now: f64) -> bool {
        use LifecycleState::*;
        if !(self.layout_active() || self.state == ExitFullScreen) {
            return false;
        }
        self.fs_frame = output;
        let c = self.cfg.enter_fullscreen;
        self.frame.animate_to(output, c, now);
        self.shadow.animate_to(0.0, c, now);
        self.start(EnterFullScreen, curve_duration(c), now);
        true
    }

    pub fn exit_fullscreen(&mut self, now: f64) -> bool {
        use LifecycleState::*;
        if !matches!(self.state, EnterFullScreen | FullScreen) {
            return false;
        }
        let c = self.cfg.exit_fullscreen;
        let t = self.frame_of(self.rest);
        self.frame.animate_to(t, c, now);
        self.shadow.animate_to(1.0, c, now);
        self.start(ExitFullScreen, curve_duration(c), now);
        true
    }

    /// Closing with the order-out transform of the window kind (none for plain and document
    /// windows, which are destroyed on the next tick). Windows that are not on screen are
    /// destroyed at once.
    pub fn close(&mut self, now: f64) -> bool {
        use LifecycleState::*;
        match self.state {
            Closing | Destroyed => false,
            Created | Minimized => {
                self.state = Destroyed;
                true
            }
            _ => {
                let d = self.play(self.kind.order_out(), now);
                self.start(Closing, d, now);
                true
            }
        }
    }

    /// Unmap/destroy at any time short-circuits to Destroyed.
    pub fn destroy(&mut self) -> bool {
        if self.state == LifecycleState::Destroyed {
            return false;
        }
        self.state = LifecycleState::Destroyed;
        true
    }

    /// Advance the machine; returns `(from, to)` when a transient state completed.
    pub fn tick(&mut self, now: f64) -> Option<(LifecycleState, LifecycleState)> {
        use LifecycleState::*;
        if self.snap_pending && now >= self.ends_at {
            self.snap_pending = false;
            if self.state == Normal {
                self.settle_frame();
            }
        }
        if !self.state.is_transient() || now < self.ends_at {
            return None;
        }
        let from = self.state;
        let to = match from {
            Opening => {
                self.transform = None;
                self.frame.set_immediately(self.normal_frame);
                Normal
            }
            Minimizing => {
                self.warp.set_immediately(1.0);
                Minimized
            }
            Restoring | ExitFullScreen => {
                self.warp.set_immediately(0.0);
                self.settle_frame();
                self.shadow.set_immediately(1.0);
                self.rest.into()
            }
            Zooming => {
                self.settle_frame();
                Zoomed
            }
            Unzooming => {
                self.settle_frame();
                Normal
            }
            EnterFullScreen => {
                self.frame.set_immediately(self.fs_frame);
                self.shadow.set_immediately(0.0);
                FullScreen
            }
            Tiling(k) => {
                self.settle_frame();
                Tiled(k)
            }
            Closing => Destroyed,
            _ => return None,
        };
        self.state = to;
        Some((from, to))
    }

    fn settle_frame(&mut self) {
        let t = self.frame_of(self.rest);
        self.frame.set_immediately(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use LifecycleState::*;

    const N: Rect = Rect::new(100.0, 100.0, 600.0, 400.0);
    const ZOOM: Rect = Rect::new(0.0, 24.0, 1920.0, 1056.0);
    const DOCK: Rect = Rect::new(900.0, 1000.0, 48.0, 48.0);
    const OUT: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

    fn new_normal() -> (Lifecycle, f64) {
        let mut l = Lifecycle::new(AnimationConfig::default(), WindowKind::Plain, N);
        assert!(l.map(0.0));
        let t = settle(&mut l, 0.0);
        assert_eq!(l.state(), Normal);
        (l, t)
    }

    fn settle(l: &mut Lifecycle, mut t: f64) -> f64 {
        let end = t + 10.0;
        while l.state().is_transient() && t < end {
            t += 1.0 / 120.0;
            l.tick(t);
        }
        assert!(!l.state().is_transient(), "did not settle: {:?}", l.state());
        t
    }

    fn rect_eq(a: Rect, b: Rect) -> bool {
        a.to_array()
            .iter()
            .zip(b.to_array())
            .all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn plain_windows_open_and_close_without_a_transform() {
        let mut l = Lifecycle::new(AnimationConfig::default(), WindowKind::Plain, N);
        assert!(!l.presentation(0.0).visible);
        l.map(1.0);
        let p = l.presentation(1.0);
        assert_eq!(
            (p.visible, p.opacity, p.scale, p.shadow),
            (true, 1.0, 1.0, 1.0)
        );
        assert_eq!(l.tick(1.0), Some((Opening, Normal)));
        assert!(!l.map(5.0), "map only from Created");
        assert!(l.close(2.0));
        assert_eq!(l.presentation(2.0).opacity, 1.0);
        assert_eq!(l.tick(2.0), Some((Closing, Destroyed)));
    }

    #[test]
    fn document_windows_grow_in_over_200ms() {
        let mut l = Lifecycle::new(AnimationConfig::default(), WindowKind::Document, N);
        l.map(1.0);
        let p0 = l.presentation(1.0);
        assert_eq!((p0.opacity, p0.scale, p0.shadow), (0.0, 0.0, 0.0));
        let mid = l.presentation(1.1);
        assert!(
            (mid.scale - 0.5).abs() < 1e-12 && (mid.opacity - 0.5).abs() < 1e-12,
            "{mid:?}"
        );
        assert!(l.tick(1.19).is_none());
        assert_eq!(l.tick(1.2), Some((Opening, Normal)));
        let p = l.presentation(1.2);
        assert_eq!((p.opacity, p.scale, p.shadow), (1.0, 1.0, 1.0));
        assert!(rect_eq(p.frame, N));
        assert!(l.close(2.0));
        assert_eq!(
            l.tick(2.0),
            Some((Closing, Destroyed)),
            "documents close without a transform"
        );
    }

    #[test]
    fn alerts_pop_in_and_scale_down_out() {
        let mut l = Lifecycle::new(AnimationConfig::default(), WindowKind::Alert, N);
        l.map(0.0);
        // eased progress 0.6 at p = acos(1 − 1.2)/π of the 0.4 s
        let peak = (1.0 - 2.0 * 0.6f64).acos() / std::f64::consts::PI * 0.4;
        assert!((l.presentation(peak).scale - 1.2).abs() < 1e-9);
        let t = settle(&mut l, 0.0);
        assert!(l.close(t));
        let mid = l.presentation(t + 0.075);
        assert!(
            (mid.scale - 0.95).abs() < 1e-12 && (mid.opacity - 0.5).abs() < 1e-12,
            "{mid:?}"
        );
        assert!(l.tick(t + 0.149).is_none());
        assert_eq!(l.tick(t + 0.15), Some((Closing, Destroyed)));
    }

    #[test]
    fn sheets_slide_down_into_place_and_back_up_over_260ms() {
        let mut l = Lifecycle::new(AnimationConfig::default(), WindowKind::Sheet, N);
        l.map(1.0);
        let p0 = l.presentation(1.0);
        assert!(
            rect_eq(p0.frame, N.translate(0.0, -32.0)),
            "starts 32 pt above its place: {p0:?}"
        );
        assert_eq!((p0.opacity, p0.scale), (0.0, 1.0));
        let mid = l.presentation(1.13);
        assert!(
            rect_eq(mid.frame, N.translate(0.0, -16.0)) && (mid.opacity - 1.0).abs() < 1e-12,
            "{mid:?}"
        );
        assert!(l.tick(1.25).is_none());
        assert_eq!(l.tick(1.26), Some((Opening, Normal)));
        assert!(rect_eq(l.presentation(1.26).frame, N));
        assert!(l.close(2.0));
        let out = l.presentation(2.13);
        assert!(
            rect_eq(out.frame, N.translate(0.0, -16.0)) && (out.opacity - 0.5).abs() < 1e-12,
            "{out:?}"
        );
        assert_eq!(l.tick(2.26), Some((Closing, Destroyed)));
    }

    #[test]
    fn zoom_runs_the_frame_change_duration_with_the_sin2_ease() {
        let (mut l, t) = new_normal();
        let near = Rect::new(150.0, 100.0, 700.0, 400.0);
        assert!(l.zoom(near, t));
        let d = 181.0 * 2f64.ln() / 750.0;
        assert!(rect_eq(
            l.presentation(t + d / 2.0).frame,
            Rect::lerp(N, near, 0.5)
        ));
        assert!(rect_eq(
            l.presentation(t + d / 4.0).frame,
            Rect::lerp(N, near, 0.5 - 0.5 * (std::f64::consts::PI / 4.0).cos())
        ));
        assert!(l.tick(t + d - 1e-6).is_none());
        assert_eq!(l.tick(t + d), Some((Zooming, Zoomed)));
        assert!(l.unzoom(t + 1.0));
        assert_eq!(
            l.tick(t + 1.0 + d),
            Some((Unzooming, Normal)),
            "the same distance back takes the same time"
        );
        assert!(l.zoom(ZOOM, t + 2.0));
        assert!(l.tick(t + 2.0 + 0.349).is_none());
        assert_eq!(
            l.tick(t + 2.0 + 0.35),
            Some((Zooming, Zoomed)),
            "large changes cap at 0.35 s"
        );
    }

    #[test]
    fn catch_up_lags_behind_the_frame_and_runs_out() {
        let (mut l, t) = new_normal();
        let moved = N.translate(30.0, 0.0);
        assert!(l.catch_up_to(moved, 0.15, CubicBezier::EASE_OUT, t));
        assert_eq!(l.normal_frame(), moved);
        assert!(rect_eq(l.presentation(t).frame, N));
        let half = l.presentation(t + 0.075).frame.x;
        let expected = moved.x - 30.0 * (1.0 - CubicBezier::EASE_OUT.eval(0.5));
        assert!((half - expected).abs() < 1e-9, "{half} vs {expected}");
        l.set_normal_frame(moved.translate(10.0, 0.0));
        assert!(
            (l.presentation(t + 0.075).frame.x - (expected + 10.0)).abs() < 1e-9,
            "the lag survives later moves"
        );
        assert!(l.is_catching_up(t + 0.149) && !l.is_animating(t + 0.15));
        assert!(rect_eq(
            l.presentation(t + 0.15).frame,
            moved.translate(10.0, 0.0)
        ));
    }

    #[test]
    fn minimize_restore_returns_exact_geometry() {
        let (mut l, t) = new_normal();
        assert!(l.minimize(DOCK, t));
        assert_eq!(l.state(), Minimizing);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Minimized);
        let p = l.presentation(t);
        assert!(!p.visible && p.minimize_progress == 1.0 && p.minimize_target == DOCK);
        assert!(l.restore(t));
        assert!(l.presentation(t).visible);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Normal);
        let p = l.presentation(t);
        assert_eq!(p.minimize_progress, 0.0);
        assert!(rect_eq(p.frame, N));
        assert!(rect_eq(l.target_frame(), N));
    }

    #[test]
    fn minimize_is_interruptible_without_jump() {
        let (mut l, t) = new_normal();
        l.minimize(DOCK, t);
        let t1 = t + 0.2;
        let before = l.presentation(t1).minimize_progress;
        assert!(before > 0.0 && before < 1.0);
        assert!(l.restore(t1));
        assert_eq!(l.state(), Restoring);
        assert!((l.presentation(t1).minimize_progress - before).abs() < 1e-9);
        assert!(l.presentation(t1 + 0.05).minimize_progress < before);
        // and back again while restoring
        let t2 = t1 + 0.1;
        let mid = l.presentation(t2).minimize_progress;
        assert!(l.minimize(DOCK, t2));
        assert!((l.presentation(t2).minimize_progress - mid).abs() < 1e-9);
        settle(&mut l, t2);
        assert_eq!(l.state(), Minimized);
    }

    #[test]
    fn zoom_unzoom_roundtrip_and_interrupt() {
        let (mut l, t) = new_normal();
        assert!(l.zoom(ZOOM, t));
        assert_eq!(l.state(), Zooming);
        assert!(l.toplevel_states().maximized);
        let t1 = t + 0.1;
        let f1 = l.presentation(t1).frame;
        assert!(f1.w > N.w && f1.w < ZOOM.w);
        assert!(l.unzoom(t1));
        assert_eq!(l.state(), Unzooming);
        assert!(
            rect_eq(l.presentation(t1).frame, f1),
            "retarget keeps position"
        );
        let t = settle(&mut l, t1);
        assert_eq!(l.state(), Normal);
        assert!(rect_eq(l.presentation(t).frame, N));
        assert!(!l.unzoom(t));

        l.zoom(ZOOM, t);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Zoomed);
        assert!(!l.zoom(ZOOM, t), "already zoomed");
        assert!(rect_eq(l.target_frame(), ZOOM));
    }

    #[test]
    fn zoomed_to_tiled_to_normal_restores_pre_zoom_geometry() {
        let (mut l, t) = new_normal();
        l.zoom(ZOOM, t);
        let t = settle(&mut l, t);
        let left = Rect::new(0.0, 24.0, 960.0, 1056.0);
        assert!(l.tile(TileKind::Left, left, t));
        assert_eq!(l.state(), Tiling(TileKind::Left));
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Tiled(TileKind::Left));
        assert!(rect_eq(l.target_frame(), left));
        let e = l.toplevel_states().tiled;
        assert!(e.left && e.top && e.bottom && !e.right);
        assert!(l.drag_out(None, t));
        assert_eq!(l.state(), Normal);
        let t = t + 2.0;
        assert!(rect_eq(l.presentation(t).frame, N));
        assert!(!l.is_animating(t));
        assert_eq!(l.toplevel_states(), ToplevelStates::default());
    }

    #[test]
    fn drag_out_with_new_origin_updates_normal_frame() {
        let (mut l, t) = new_normal();
        l.tile(TileKind::Right, Rect::new(960.0, 24.0, 960.0, 1056.0), t);
        let t = settle(&mut l, t);
        let placed = Rect::new(500.0, 40.0, 600.0, 400.0);
        l.drag_out(Some(placed), t);
        assert!(rect_eq(l.normal_frame(), placed));
        assert!(rect_eq(l.presentation(t + 3.0).frame, placed));
    }

    #[test]
    fn tile_retarget_between_kinds() {
        let (mut l, t) = new_normal();
        l.tile(TileKind::Left, Rect::new(0.0, 24.0, 960.0, 1056.0), t);
        let t1 = t + 0.1;
        let f = l.presentation(t1).frame;
        assert!(l.tile(TileKind::TopRight, Rect::new(960.0, 24.0, 960.0, 528.0), t1));
        assert!(rect_eq(l.presentation(t1).frame, f));
        settle(&mut l, t1);
        assert_eq!(l.state(), Tiled(TileKind::TopRight));
        assert!(!l.tile(
            TileKind::TopRight,
            Rect::new(960.0, 24.0, 960.0, 528.0),
            50.0
        ));
    }

    #[test]
    fn minimize_from_tiled_restores_tiled() {
        let (mut l, t) = new_normal();
        let r = Rect::new(960.0, 24.0, 960.0, 1056.0);
        l.tile(TileKind::Right, r, t);
        let t = settle(&mut l, t);
        l.minimize(DOCK, t);
        let t = settle(&mut l, t);
        assert!(l.toplevel_states().tiled.right);
        l.restore(t);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Tiled(TileKind::Right));
        assert!(rect_eq(l.presentation(t).frame, r));
        l.drag_out(None, t);
        assert!(rect_eq(l.presentation(t + 3.0).frame, N));
    }

    #[test]
    fn fullscreen_roundtrip_from_normal_and_zoomed() {
        let (mut l, t) = new_normal();
        assert!(l.fullscreen(OUT, t));
        assert!(l.toplevel_states().fullscreen);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), FullScreen);
        assert!(rect_eq(l.presentation(t).frame, OUT));
        assert_eq!(l.presentation(t).shadow, 0.0);
        assert!(!l.minimize(DOCK, t), "full screen windows cannot minimize");
        assert!(!l.zoom(ZOOM, t));
        assert!(l.exit_fullscreen(t));
        assert_eq!(l.state(), ExitFullScreen);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Normal);
        assert!(rect_eq(l.presentation(t).frame, N));
        assert_eq!(l.presentation(t).shadow, 1.0);

        l.zoom(ZOOM, t);
        let t = settle(&mut l, t);
        l.fullscreen(OUT, t);
        let t = settle(&mut l, t);
        l.exit_fullscreen(t);
        settle(&mut l, t);
        assert_eq!(l.state(), Zoomed);
    }

    #[test]
    fn zoomed_and_filled_windows_stay_rounded_and_only_full_screen_is_square() {
        let (mut l, t) = new_normal();
        assert_eq!(l.toplevel_states().frame_shape(), FrameShape::Rounded);
        assert!(l.zoom(ZOOM, t));
        let t = settle(&mut l, t);
        assert!(l.toplevel_states().maximized);
        assert_eq!(l.toplevel_states().frame_shape(), FrameShape::Rounded);
        assert_eq!(
            l.presentation(t).shadow,
            1.0,
            "a zoomed window keeps its shadow"
        );
        assert!(l.unzoom(t));
        let t = settle(&mut l, t);
        assert!(l.tile(TileKind::Fill, ZOOM, t));
        assert_eq!(l.toplevel_states().frame_shape(), FrameShape::Rounded);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Tiled(TileKind::Fill));
        assert_eq!(l.toplevel_states().frame_shape(), FrameShape::Rounded);
        assert_eq!(
            l.presentation(t).shadow,
            1.0,
            "a filled window keeps its shadow"
        );
        assert!(l.fullscreen(OUT, t));
        assert_eq!(
            l.toplevel_states().frame_shape(),
            FrameShape::Square,
            "square from the start of the transition"
        );
        let t = settle(&mut l, t);
        assert_eq!(l.toplevel_states().frame_shape(), FrameShape::Square);
        assert_eq!(l.presentation(t).shadow, 0.0);
        assert!(l.exit_fullscreen(t));
        assert_eq!(l.toplevel_states().frame_shape(), FrameShape::Rounded);
        let t = settle(&mut l, t);
        assert_eq!(l.state(), Tiled(TileKind::Fill));
        assert_eq!(l.toplevel_states().frame_shape(), FrameShape::Rounded);
        assert_eq!(l.presentation(t).shadow, 1.0);
    }

    #[test]
    fn fullscreen_interrupt() {
        let (mut l, t) = new_normal();
        l.fullscreen(OUT, t);
        let t1 = t + 0.15;
        let f = l.presentation(t1).frame;
        assert!(f.w > N.w && f.w < OUT.w);
        assert!(l.exit_fullscreen(t1));
        assert!(rect_eq(l.presentation(t1).frame, f));
        settle(&mut l, t1);
        assert_eq!(l.state(), Normal);
        assert!(l.fullscreen(OUT, 100.0));
        assert!(l.exit_fullscreen(100.0));
        assert!(l.fullscreen(OUT, 100.01), "ExitFullScreen can be reversed");
    }

    #[test]
    fn utility_windows_fade_out_then_destroy() {
        let mut l = Lifecycle::new(AnimationConfig::default(), WindowKind::Utility, N);
        l.map(0.0);
        let t = settle(&mut l, 0.0);
        assert!(l.close(t));
        assert_eq!(l.state(), Closing);
        let p = l.presentation(t + 0.075);
        assert!((p.opacity - 0.5).abs() < 1e-12 && p.scale == 1.0, "{p:?}");
        assert!(l.tick(t + 0.1).is_none());
        assert_eq!(l.tick(t + 0.15), Some((Closing, Destroyed)));
        assert!(!l.close(t + 1.0));
        assert!(!l.state().is_visible());
    }

    #[test]
    fn close_while_minimized_or_created_is_immediate() {
        let (mut l, t) = new_normal();
        l.minimize(DOCK, t);
        let t = settle(&mut l, t);
        assert!(l.close(t));
        assert_eq!(l.state(), Destroyed);
        let mut c = Lifecycle::new(AnimationConfig::default(), WindowKind::Plain, N);
        assert!(c.close(0.0));
        assert_eq!(c.state(), Destroyed);
    }

    #[test]
    fn destroy_short_circuits_every_state() {
        let mk = |step: usize| {
            let (mut l, t) = new_normal();
            match step {
                0 => {}
                1 => {
                    l.minimize(DOCK, t);
                }
                2 => {
                    l.zoom(ZOOM, t);
                }
                3 => {
                    l.fullscreen(OUT, t);
                }
                4 => {
                    l.tile(TileKind::Left, ZOOM, t);
                }
                _ => {
                    l.close(t);
                }
            }
            l
        };
        for s in 0..6 {
            let mut l = mk(s);
            assert!(l.destroy(), "step {s}");
            assert_eq!(l.state(), Destroyed);
            assert!(!l.destroy());
            assert!(!l.zoom(ZOOM, 9.0) && !l.minimize(DOCK, 9.0) && !l.restore(9.0));
        }
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let (mut l, t) = new_normal();
        assert!(!l.restore(t));
        assert!(!l.exit_fullscreen(t));
        assert!(!l.drag_out(None, t));
        assert!(!l.unzoom(t));
        l.minimize(DOCK, t);
        assert!(!l.zoom(ZOOM, t));
        assert!(!l.fullscreen(OUT, t));
        assert!(!l.tile(TileKind::Left, ZOOM, t));
        let mut c = Lifecycle::new(AnimationConfig::default(), WindowKind::Plain, N);
        assert!(!c.minimize(DOCK, 0.0) && !c.zoom(ZOOM, 0.0) && !c.restore(0.0));
    }

    #[test]
    fn set_normal_frame_only_in_normal() {
        let (mut l, t) = new_normal();
        let moved = Rect::new(300.0, 300.0, 600.0, 400.0);
        assert!(l.set_normal_frame(moved));
        assert!(rect_eq(l.presentation(t).frame, moved));
        l.zoom(ZOOM, t);
        assert!(!l.set_normal_frame(N));
        let t = settle(&mut l, t);
        l.unzoom(t);
        settle(&mut l, t);
        assert!(rect_eq(l.normal_frame(), moved));
    }

    #[test]
    fn sampling_is_refresh_rate_independent() {
        let run = |hz: f64| {
            let (mut l, t) = new_normal();
            l.zoom(ZOOM, t);
            let mut s = 0.0;
            for k in 0..=(0.5 * hz) as usize {
                s = l.presentation(t + k as f64 / hz).frame.w;
            }
            s
        };
        assert!((run(60.0) - run(120.0)).abs() < 1e-9);
    }
}
