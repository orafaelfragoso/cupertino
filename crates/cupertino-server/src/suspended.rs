//! Which windows nobody can see, told so: a window that is not drawn (minimized, of a hidden application or of a
//! workspace not shown) and one whose every pixel on the outputs lies under the opaque content of windows at rest above
//! it gets the `suspended` state of `xdg_toplevel` (version 6 and later). A client may then let go of what it keeps to
//! draw quickly and stop drawing; it keeps its last frame, which still shows where a window is drawn from it. The state
//! goes as soon as any part of the window shows again.
//!
//! Being covered is decided conservatively, as a window told it is suspended while it shows would look stale: only a
//! window at rest (no animation, not moved or resized, at full opacity, no overview, stage or desktop reveal on its
//! output) covers, and only with the region its client said is opaque, its corners cut by the window's corner radius
//! and cut to its output. A window with a popup open is never covered. Nothing is decided while the session is locked.
//!
//! The windows are looked at again only when something may have changed: the window model's effects, a commit of a
//! window's surfaces, the outputs, and every tick while anything animates (and the tick after).

use std::collections::HashSet;

use cupertino_wm::{LifecycleState, Rect, WindowId};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::PopupManager;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Rectangle};

use crate::state::{Cupertino, Shrink};

/// The first `xdg_wm_base` version whose toplevels know the `suspended` state.
const SUSPENDED_SINCE: u32 = 6;

/// When the windows are looked at again.
#[derive(Default)]
pub struct Suspension {
    /// Something that may change which windows show happened since the last look.
    dirty: bool,
    /// The last look was taken while something animated.
    animated: bool,
}

impl Suspension {
    /// Look at the windows again on the next refresh.
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }
}

/// A rectangle grown to the whole points it touches.
fn outward(r: Rect) -> Rectangle<i32, Logical> {
    let (x0, y0) = (r.x.floor() as i32, r.y.floor() as i32);
    let (x1, y1) = (r.right().ceil() as i32, r.bottom().ceil() as i32);
    Rectangle::new((x0, y0).into(), ((x1 - x0).max(0), (y1 - y0).max(0)).into())
}

/// A rectangle shrunk to the whole points it holds.
fn inward(r: Rect) -> Rectangle<i32, Logical> {
    let (x0, y0) = (r.x.ceil() as i32, r.y.ceil() as i32);
    let (x1, y1) = (r.right().floor() as i32, r.bottom().floor() as i32);
    Rectangle::new((x0, y0).into(), ((x1 - x0).max(0), (y1 - y0).max(0)).into())
}

/// Whether any part of `shown` (the pieces of a window on the outputs) lies outside every rectangle of `covers`.
pub fn shows(shown: &[Rect], covers: &[Rect]) -> bool {
    let pieces: Vec<Rectangle<i32, Logical>> = shown
        .iter()
        .map(|r| outward(*r))
        .filter(|r| !r.is_empty())
        .collect();
    let covers: Vec<Rectangle<i32, Logical>> = covers
        .iter()
        .map(|r| inward(*r))
        .filter(|r| !r.is_empty())
        .collect();
    !Rectangle::subtract_rects_many(pieces, covers).is_empty()
}

/// The part of `rect` a rounded corner of `radius` leaves whole: the rectangle less its corner squares, as two
/// rectangles (a cross).
pub fn cross(rect: Rect, radius: f64) -> [Rect; 2] {
    let r = radius.max(0.0).min(rect.w / 2.0).min(rect.h / 2.0);
    [
        Rect::new(rect.x + r, rect.y, (rect.w - 2.0 * r).max(0.0), rect.h),
        Rect::new(rect.x, rect.y + r, rect.w, (rect.h - 2.0 * r).max(0.0)),
    ]
}

/// The intersection of two rectangles, empty (zero size) when they do not overlap.
fn clip(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.max(b.x), a.y.max(b.y));
    let (x1, y1) = (a.right().min(b.right()), a.bottom().min(b.bottom()));
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

fn at_rest(state: LifecycleState) -> bool {
    matches!(
        state,
        LifecycleState::Normal
            | LifecycleState::Zoomed
            | LifecycleState::FullScreen
            | LifecycleState::Tiled(_)
    )
}

impl Cupertino {
    /// Tell each window whether it is suspended (see the module documentation), when something may have changed.
    pub fn refresh_suspended(&mut self) {
        let animating = self.animating();
        let look =
            std::mem::take(&mut self.suspension.dirty) || animating || self.suspension.animated;
        self.suspension.animated = animating;
        if !look || self.session_lock.is_active() {
            return;
        }
        let hidden = self.hidden_windows();
        for e in &self.windows {
            if !e.mapped || e.toplevel.xdg_toplevel().version() < SUSPENDED_SINCE {
                continue;
            }
            let want = hidden.contains(&e.id);
            let t = &e.toplevel;
            let have =
                t.with_pending_state(|st| st.states.contains(xdg_toplevel::State::Suspended));
            if have == want || !t.is_initial_configure_sent() {
                continue;
            }
            t.with_pending_state(|st| {
                if want {
                    st.states.set(xdg_toplevel::State::Suspended);
                } else {
                    st.states.unset(xdg_toplevel::State::Suspended);
                }
            });
            t.send_pending_configure();
        }
    }

    /// Whether `window` was told it is suspended.
    pub fn window_suspended(&self, window: WindowId) -> bool {
        self.entry(window).is_some_and(|e| {
            e.toplevel
                .with_pending_state(|st| st.states.contains(xdg_toplevel::State::Suspended))
        })
    }

    /// The mapped windows no output shows.
    fn hidden_windows(&self) -> HashSet<WindowId> {
        let now = self.now();
        let options = self.display_options();
        let transition = crate::render::motion::WorkspaceTransition::for_options(options);
        let interacting = self.wm.interacting_window();
        let outputs: Vec<Rect> = self.outputs.iter().map(|o| o.rect).collect();
        let desk = self.fx.show_desktop_progress(now) > 0.0;
        let mut drawn = HashSet::new();
        let mut hidden = HashSet::new();
        for o in &self.outputs {
            let list =
                crate::render::scene::windows_on_output(&self.wm, o.id, o.rect.w, transition, now);
            drawn.extend(list.iter().map(|(id, _, _)| *id));
            // windows drawn elsewhere than at their frame: nothing on this output is covered
            if self.shell.overview_on(o.id).is_some()
                || self.window_managers.stage_active(o.id)
                || desk
            {
                continue;
            }
            let mut covers: Vec<Rect> = Vec::new();
            for (id, slide, workspace_opacity) in list.into_iter().rev() {
                let Some(entry) = self.entry(id) else {
                    continue;
                };
                let Some(w) = self.wm.window(id) else {
                    continue;
                };
                let Some((plan, _)) =
                    crate::render::scene::plan_for(&self.wm, entry, slide, now, &self.fx, options)
                else {
                    continue;
                };
                if !plan.exact || slide != 0.0 {
                    continue;
                }
                let geo = entry.window.geometry();
                let ins = entry.insets();
                // the root surface's origin
                let (sx, sy) = (
                    plan.visual.x + ins.left - f64::from(geo.loc.x),
                    plan.visual.y + ins.top - f64::from(geo.loc.y),
                );
                let surface = entry.surface();
                let popups = PopupManager::popups_for_surface(surface).next().is_some();
                if !popups {
                    let b = entry.window.bbox();
                    let bbox = Rect::new(
                        sx + f64::from(b.loc.x),
                        sy + f64::from(b.loc.y),
                        f64::from(b.size.w),
                        f64::from(b.size.h),
                    );
                    let whole = union(bbox, plan.visual);
                    let shown: Vec<Rect> = outputs.iter().map(|out| clip(whole, *out)).collect();
                    if !shows(&shown, &covers) {
                        hidden.insert(id);
                    }
                }
                let covering = plan.opacity >= 1.0
                    && workspace_opacity >= 1.0
                    && at_rest(w.state())
                    && matches!(entry.shrink, Shrink::None)
                    && interacting != Some(id);
                if !covering {
                    continue;
                }
                let opaque =
                    with_renderer_surface_state(surface, |s| s.opaque_regions().map(<[_]>::to_vec))
                        .flatten()
                        .unwrap_or_default();
                let content = Rect::new(
                    plan.visual.x + ins.left,
                    plan.visual.y + ins.top,
                    f64::from(geo.size.w),
                    f64::from(geo.size.h),
                );
                let radius = match plan.corner {
                    Some(r) => r,
                    None => self.config.window.corner_radius,
                };
                let cut = cross(content, radius);
                for r in opaque {
                    let r = Rect::new(
                        sx + f64::from(r.loc.x),
                        sy + f64::from(r.loc.y),
                        f64::from(r.size.w),
                        f64::from(r.size.h),
                    );
                    for c in cut {
                        let piece = clip(clip(r, c), o.rect);
                        if piece.w > 0.0 && piece.h > 0.0 {
                            covers.push(piece);
                        }
                    }
                }
            }
        }
        for e in self.windows.iter().filter(|e| e.mapped) {
            if !drawn.contains(&e.id) && self.wm.window(e.id).is_some() {
                hidden.insert(e.id);
            }
        }
        hidden
    }
}

/// The smallest rectangle holding both.
fn union(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = (a.right().max(b.right()), a.bottom().max(b.bottom()));
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_under_covers_that_leave_no_point_of_it_does_not_show() {
        let window = Rect::new(10.0, 10.0, 100.0, 50.0);
        assert!(!shows(&[window], &[Rect::new(0.0, 0.0, 200.0, 200.0)]));
        // two covers that meet over it
        assert!(!shows(
            &[window],
            &[
                Rect::new(0.0, 0.0, 60.0, 100.0),
                Rect::new(60.0, 0.0, 100.0, 100.0)
            ]
        ));
        // a gap of a point between them
        assert!(shows(
            &[window],
            &[
                Rect::new(0.0, 0.0, 60.0, 100.0),
                Rect::new(61.0, 0.0, 100.0, 100.0)
            ]
        ));
        // a cover short of the edge by a fraction of a point leaves that point
        assert!(shows(&[window], &[Rect::new(10.5, 0.0, 200.0, 200.0)]));
        assert!(shows(&[window], &[]));
        // nothing on the outputs: nothing shows
        assert!(!shows(&[Rect::new(0.0, 0.0, 0.0, 0.0)], &[]));
    }

    #[test]
    fn a_rounded_window_covers_all_but_its_corners() {
        let [a, b] = cross(Rect::new(0.0, 0.0, 100.0, 60.0), 10.0);
        assert_eq!(a, Rect::new(10.0, 0.0, 80.0, 60.0));
        assert_eq!(b, Rect::new(0.0, 10.0, 100.0, 40.0));
        // a window of the same frame behind it shows in the corners
        assert!(shows(&[Rect::new(0.0, 0.0, 100.0, 60.0)], &[a, b]));
        // one inside the corners does not
        assert!(!shows(&[Rect::new(10.0, 10.0, 80.0, 40.0)], &[a, b]));
        // square corners cover everything
        let square = cross(Rect::new(0.0, 0.0, 100.0, 60.0), 0.0);
        assert!(!shows(&[Rect::new(0.0, 0.0, 100.0, 60.0)], &square));
    }
}
