//! Compositor-side window effects: closing ghosts (a snapshot of the last frame animated out with
//! the window kind's order-out transform) and the show-desktop slide. The geometry comes from
//! `cupertino-effects` (`TransformKind`, `ShowDesktop`, `MinimizeEffect`); the window model keeps
//! owning minimize / open / zoom progress.

use crate::render::snapshots::SnapshotOwner;
use cupertino_effects::{MinimizeKind, ProgressAnimator, ShowDesktop, TransformKind, WindowKind};
use cupertino_wm::{OutputId, Rect, WindowId};

/// A window that is gone from the model but still fading out on screen.
pub struct ClosingWindow {
    pub id: WindowId,
    pub output: OutputId,
    /// Captured area in global logical points (frame plus shadow margin).
    pub rect: Rect,
    /// The captured pixels, kept while the ghost shows.
    pub snapshot: SnapshotOwner,
    kind: TransformKind,
    anim: ProgressAnimator,
}

impl ClosingWindow {
    /// Fade progress in `[0, 1]`.
    pub fn progress(&self, now: f64) -> f64 {
        self.anim.value(now).clamp(0.0, 1.0)
    }

    /// Drawn area and opacity at `now`: the captured area scaled about its centre and moved by the
    /// transform's slide (sheets).
    pub fn placement(&self, now: f64) -> (Rect, f64) {
        let s = self.kind.sample(self.progress(now));
        let (w, h) = (self.rect.w * s.scale, self.rect.h * s.scale);
        let r = Rect::new(
            self.rect.x + (self.rect.w - w) / 2.0,
            self.rect.y + (self.rect.h - h) / 2.0 + s.dy,
            w,
            h,
        );
        (r, s.alpha)
    }

    pub fn is_done(&self, now: f64) -> bool {
        !self.anim.is_animating(now) && self.anim.target() >= 1.0
    }
}

/// Minimum time between two stored snapshots of one window (seconds). approximate.
pub const SNAPSHOT_INTERVAL: f64 = 0.5;

struct Snapshot {
    owner: SnapshotOwner,
    area: Rect,
    frame_origin: (f64, f64),
    taken: f64,
}

/// Which minimize effect to draw. `None` falls back to the plain rescale of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectsConfig {
    pub minimize: Option<MinimizeKind>,
    /// Fade plain windows out when they close. Off by default: the design animates only the
    /// window kinds that have an order-out transform.
    pub closing: bool,
}

impl Default for EffectsConfig {
    fn default() -> Self {
        Self {
            minimize: Some(MinimizeKind::Warp),
            closing: false,
        }
    }
}

pub struct Effects {
    pub cfg: EffectsConfig,
    pub closing: Vec<ClosingWindow>,
    /// The icon of a client's drag that ended without a drop, on its way back (`crate::drag_return`).
    pub drag_return: Option<crate::drag_return::DragReturn>,
    snapshots: std::collections::HashMap<WindowId, Snapshot>,
    pub desktop: ShowDesktop,
    show_desktop: ProgressAnimator,
}

impl Effects {
    pub fn new(cfg: EffectsConfig) -> Self {
        let desktop = ShowDesktop::default();
        Self {
            cfg,
            closing: Vec::new(),
            drag_return: None,
            snapshots: Default::default(),
            show_desktop: desktop.animator(0.0),
            desktop,
        }
    }

    /// The close transform of a window of `kind`: its order-out transform, else a fade when
    /// [`EffectsConfig::closing`] is on; `None` = the window disappears at once.
    pub fn close_kind(&self, kind: WindowKind) -> Option<TransformKind> {
        kind.order_out()
            .or(self.cfg.closing.then_some(TransformKind::FadeOut))
    }

    /// Start animating a snapshot of `id` out with `kind`.
    pub fn start_close(
        &mut self,
        id: WindowId,
        output: OutputId,
        rect: Rect,
        snapshot: SnapshotOwner,
        kind: TransformKind,
        now: f64,
    ) {
        self.closing.retain(|c| c.id != id);
        let mut anim = kind.animator(false);
        anim.animate_to(1.0, now);
        self.closing.push(ClosingWindow {
            id,
            output,
            rect,
            snapshot,
            kind,
            anim,
        });
    }

    /// Store the latest capture of a window.
    pub fn store_snapshot(
        &mut self,
        id: WindowId,
        owner: SnapshotOwner,
        area: Rect,
        frame_origin: (f64, f64),
        now: f64,
    ) {
        self.snapshots.insert(
            id,
            Snapshot {
                owner,
                area,
                frame_origin,
                taken: now,
            },
        );
    }

    /// Seconds since the stored snapshot of `id` was taken (infinite when there is none).
    pub fn snapshot_age(&self, id: WindowId, now: f64) -> f64 {
        self.snapshots
            .get(&id)
            .map_or(f64::INFINITY, |s| now - s.taken)
    }

    /// The stored snapshot of `id`, its area moved to the window's current frame origin.
    pub fn stored_snapshot(
        &mut self,
        id: WindowId,
        frame_origin: (f64, f64),
    ) -> Option<(SnapshotOwner, Rect, (f64, f64))> {
        let s = self.snapshots.remove(&id)?;
        let (dx, dy) = (
            frame_origin.0 - s.frame_origin.0,
            frame_origin.1 - s.frame_origin.1,
        );
        Some((s.owner, s.area.translate(dx, dy), frame_origin))
    }

    pub fn forget_snapshot(&mut self, id: WindowId) {
        self.snapshots.remove(&id);
    }

    /// Drops every stored snapshot and closing ghost: their textures belong to a rendering context that is gone.
    pub fn drop_snapshots(&mut self) {
        self.snapshots.clear();
        self.closing.clear();
        self.drag_return = None;
    }

    pub fn has_ghost(&self, id: WindowId) -> bool {
        self.closing.iter().any(|c| c.id == id)
    }

    pub fn show_desktop_progress(&self, now: f64) -> f64 {
        self.show_desktop.value(now).clamp(0.0, 1.0)
    }

    pub fn show_desktop_active(&self) -> bool {
        self.show_desktop.target() >= 0.5
    }

    pub fn set_show_desktop(&mut self, on: bool, now: f64) {
        self.show_desktop
            .animate_to(if on { 1.0 } else { 0.0 }, now);
    }

    pub fn toggle_show_desktop(&mut self, now: f64) {
        let on = !self.show_desktop_active();
        self.set_show_desktop(on, now);
    }

    /// Where a window rests when the desktop is shown, at `progress` of the slide.
    pub fn show_desktop_rect(&self, window: Rect, screen: Rect, progress: f64) -> Rect {
        let p = self
            .desktop
            .placement(window.into_geom(), screen.into_geom(), progress);
        Rect::new(p.rect.x(), p.rect.y(), p.rect.width(), p.rect.height())
    }

    pub fn tick(&mut self, now: f64) -> bool {
        let before = self.closing.len();
        self.closing.retain(|c| !c.is_done(now));
        let returned = self.drag_return.take_if(|r| r.is_done(now)).is_some();
        before != self.closing.len() || returned
    }

    pub fn wants_tick(&self, now: f64) -> bool {
        self.show_desktop.is_animating(now)
            || self.closing.iter().any(|c| !c.is_done(now))
            || self.drag_return.as_ref().is_some_and(|r| !r.is_done(now))
    }
}

/// Conversion to the geometry crate's rectangle.
pub trait IntoGeom {
    fn into_geom(self) -> cupertino_geometry::Rect;
}

impl IntoGeom for Rect {
    fn into_geom(self) -> cupertino_geometry::Rect {
        cupertino_geometry::Rect::new(self.x, self.y, self.w, self.h)
    }
}

pub fn from_geom(r: cupertino_geometry::Rect) -> Rect {
    Rect::new(r.x(), r.y(), r.width(), r.height())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_desktop_slides_a_window_out_and_back() {
        let mut fx = Effects::new(EffectsConfig::default());
        let screen = Rect::new(0.0, 0.0, 1000.0, 800.0);
        let win = Rect::new(100.0, 100.0, 300.0, 200.0);
        assert_eq!(fx.show_desktop_progress(0.0), 0.0);
        fx.toggle_show_desktop(1.0);
        assert!(fx.show_desktop_active());
        assert!(fx.wants_tick(1.1));
        let end = fx.show_desktop_progress(5.0);
        assert!((end - 1.0).abs() < 1e-6);
        let hidden = fx.show_desktop_rect(win, screen, 1.0);
        assert!(
            hidden.right() <= screen.x + 1e-6 || hidden.x >= screen.right() - 1e-6,
            "{hidden:?}"
        );
        assert_eq!(fx.show_desktop_rect(win, screen, 0.0), win);
        fx.toggle_show_desktop(6.0);
        assert!(!fx.show_desktop_active());
    }

    #[test]
    fn close_transform_follows_the_window_kind() {
        let fx = Effects::new(EffectsConfig::default());
        assert_eq!(
            fx.close_kind(WindowKind::Plain),
            Some(TransformKind::WindowClose)
        );
        assert_eq!(
            fx.close_kind(WindowKind::Document),
            Some(TransformKind::WindowClose)
        );
        assert_eq!(
            fx.close_kind(WindowKind::Utility),
            Some(TransformKind::FadeOut)
        );
        assert_eq!(
            fx.close_kind(WindowKind::Sheet),
            Some(TransformKind::SheetOut)
        );
        let fade = Effects::new(EffectsConfig {
            closing: true,
            ..EffectsConfig::default()
        });
        assert_eq!(
            fade.close_kind(WindowKind::Plain),
            Some(TransformKind::WindowClose),
            "a kind's own close goes before the fade"
        );
    }
}
