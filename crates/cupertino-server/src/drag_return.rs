//! The icon of a client's drag that ended without a drop: a drag released where nothing takes it, refused by its
//! target or cancelled with Escape does not lose its icon at once. A snapshot of the icon flies back to where the drag
//! started and then goes; when the window the drag started in is no longer shown, the snapshot fades out where the
//! drag ended instead. A drag that drops removes its icon at once.

use cupertino_animation::CubicBezier;
use cupertino_effects::{Driver, Ease, ProgressAnimator};
use cupertino_wm::{Rect, WindowId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};

use crate::render::snapshots::SnapshotOwner;

/// How long the icon of a drag that ended without a drop takes to fly back to where the drag started, in seconds.
pub const DRAG_RETURN_DURATION: f64 = 0.4;
/// How long the icon of a drag that ended without a drop takes to fade out where it was let go, when there is nothing
/// to fly back to, in seconds.
pub const DRAG_FADE_DURATION: f64 = 0.25;
/// The easing of the flight back: fast at first, settling into place.
const RETURN_EASE: Ease = Ease::Bezier(CubicBezier::EASE_OUT);

/// Where a client's drag started, kept while it runs.
#[derive(Clone, Debug)]
pub struct DragStart {
    /// The surface the drag started from.
    pub origin: Option<WlSurface>,
    /// The window holding that surface, and the press from its frame's top-left corner, so the icon returns to where
    /// that point of the window is at the end.
    pub window: Option<(WindowId, Point<f64, Logical>)>,
    /// The press in the global space, for a drag from a surface that is not a window's.
    pub at: Point<f64, Logical>,
}

/// A drag that just ended without a drop, waiting for its icon to be captured: the icon surface, where it was drawn
/// from the pointer, and where the pointer was.
#[derive(Clone, Debug)]
pub struct DragEnd {
    pub icon: WlSurface,
    pub offset: Point<i32, Logical>,
    pub start: Option<DragStart>,
}

/// What the icon does after a drag without a drop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Back {
    /// Flies to this top-left corner of its area, in the global space.
    To(f64, f64),
    /// Fades out where it is.
    Fade,
}

/// The area and opacity of an icon let go in `from` that goes `back`, at `progress` (0 to 1) of its animation.
pub fn placement(from: Rect, back: Back, progress: f64) -> (Rect, f64) {
    let p = progress.clamp(0.0, 1.0);
    match back {
        Back::To(x, y) => (
            Rect::new(
                from.x + (x - from.x) * p,
                from.y + (y - from.y) * p,
                from.w,
                from.h,
            ),
            1.0,
        ),
        Back::Fade => (from, 1.0 - p),
    }
}

/// The captured icon of a drag that ended without a drop, on its way back or fading out.
pub struct DragReturn {
    /// The captured pixels, kept while the icon shows.
    pub snapshot: SnapshotOwner,
    /// The icon's area where it was let go, in global logical points.
    pub from: Rect,
    pub back: Back,
    anim: ProgressAnimator,
}

impl DragReturn {
    /// The icon let go in `from` at `now`, going `back`.
    pub fn new(snapshot: SnapshotOwner, from: Rect, back: Back, now: f64) -> DragReturn {
        let driver = match back {
            Back::To(..) => Driver::Timed {
                duration: DRAG_RETURN_DURATION,
                ease: RETURN_EASE,
            },
            Back::Fade => Driver::Timed {
                duration: DRAG_FADE_DURATION,
                ease: Ease::Linear,
            },
        };
        let mut anim = ProgressAnimator::new(driver, 0.0);
        anim.animate_to(1.0, now);
        DragReturn {
            snapshot,
            from,
            back,
            anim,
        }
    }

    pub fn progress(&self, now: f64) -> f64 {
        self.anim.value(now).clamp(0.0, 1.0)
    }

    /// The area it is drawn in and its opacity at `now`.
    pub fn placement(&self, now: f64) -> (Rect, f64) {
        placement(self.from, self.back, self.progress(now))
    }

    pub fn is_done(&self, now: f64) -> bool {
        !self.anim.is_animating(now)
    }
}

impl crate::state::Cupertino {
    /// Where a drag starting now starts: the press of the pointer's grab, on the surface it went to.
    pub(crate) fn drag_start(&self) -> Option<DragStart> {
        let grab = self.pointer.grab_start_data()?;
        let at = grab.location;
        let origin = grab.focus.map(|(surface, _)| surface);
        let window = origin
            .as_ref()
            .and_then(|s| self.window_id_for_surface(&self.root_window_surface(s)?))
            .and_then(|id| {
                let frame = self.wm.window(id)?.geometry();
                Some((id, Point::from((at.x - frame.x, at.y - frame.y))))
            });
        Some(DragStart { origin, window, at })
    }

    /// Where the press that started a drag is now, in the global space: `None` when the surface it started from is
    /// gone, or its window is no longer shown.
    fn drag_origin_now(&self, start: &DragStart) -> Option<Point<f64, Logical>> {
        use smithay::utils::IsAlive;
        if !start.origin.as_ref().is_some_and(|s| s.alive()) {
            return None;
        }
        let Some((id, from_frame)) = start.window else {
            return Some(start.at);
        };
        let window = self.wm.window(id)?;
        let shown = self.wm.window_on_screen(id)
            && !window.state().is_minimized()
            && self.entry(id).is_some_and(|e| e.mapped);
        let frame = window.geometry();
        shown.then(|| Point::from((frame.x + from_frame.x, frame.y + from_frame.y)))
    }

    /// The drag that just ended without a drop, if any: capture its icon where it was let go and send it back to where
    /// the drag started, or fade it out there when that is no longer shown. Called once the pointer is free again.
    pub fn drag_ended(&mut self) {
        let Some(end) = self.drag_ended.take() else {
            return;
        };
        // a drag the pointer did not start (touch) is not where the pointer is: its icon goes at once
        let Some(start) = end.start else {
            return;
        };
        let bbox = smithay::desktop::utils::bbox_from_surface_tree(&end.icon, (0, 0));
        if bbox.size.w <= 0 || bbox.size.h <= 0 {
            return;
        }
        let offset = end.offset.to_f64();
        let origin = self.pointer.current_location() + offset;
        let area = Rect::new(
            origin.x + f64::from(bbox.loc.x),
            origin.y + f64::from(bbox.loc.y),
            f64::from(bbox.size.w),
            f64::from(bbox.size.h),
        );
        let back = self.drag_origin_now(&start).map_or(Back::Fade, |at| {
            Back::To(
                at.x + offset.x + f64::from(bbox.loc.x),
                at.y + offset.y + f64::from(bbox.loc.y),
            )
        });
        let Some(snapshot) = self.capture_surface_snapshot(&end.icon, origin, area) else {
            return;
        };
        let now = self.now();
        self.fx.drag_return = Some(DragReturn::new(snapshot, area, back, now));
        self.request_redraw();
    }

    /// Escape during a client's drag: it ends with no drop, wherever it is. The target loses the drag first, so ending
    /// the grab drops nothing, and the source hears the drag cancelled; the surface under the pointer then gets the
    /// pointer back, as after a release.
    pub(crate) fn cancel_client_drag(&mut self, time: u32) {
        if self.client_drag.is_none() {
            return;
        }
        let pointer = self.pointer.clone();
        let location = pointer.current_location();
        pointer.motion(
            self,
            None,
            &smithay::input::pointer::MotionEvent {
                location,
                serial: smithay::utils::SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
        pointer.unset_grab(self, smithay::utils::SERIAL_COUNTER.next_serial(), time);
        self.client_drag = None;
        self.drag_ended();
        // leaving the target took the pointer focus from every surface, and ending the grab only gave that back
        self.pointer_moved_to(location, time, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_flies_back_to_the_start_whole_and_opaque() {
        let from = Rect::new(400.0, 300.0, 40.0, 20.0);
        let back = Back::To(100.0, 50.0);
        assert_eq!(placement(from, back, 0.0), (from, 1.0));
        let (half, alpha) = placement(from, back, 0.5);
        assert_eq!((half.x, half.y, half.w, half.h), (250.0, 175.0, 40.0, 20.0));
        assert_eq!(alpha, 1.0);
        let (end, _) = placement(from, back, 1.0);
        assert_eq!((end.x, end.y), (100.0, 50.0));
        assert_eq!(placement(from, back, 3.0), placement(from, back, 1.0));
    }

    #[test]
    fn an_icon_with_nowhere_to_go_fades_where_it_was_let_go() {
        let from = Rect::new(400.0, 300.0, 40.0, 20.0);
        assert_eq!(placement(from, Back::Fade, 0.0), (from, 1.0));
        assert_eq!(placement(from, Back::Fade, 0.25), (from, 0.75));
        assert_eq!(placement(from, Back::Fade, 1.0), (from, 0.0));
    }

    #[test]
    fn the_flight_eases_out_and_ends_after_its_duration() {
        let driver = Driver::Timed {
            duration: DRAG_RETURN_DURATION,
            ease: RETURN_EASE,
        };
        let mut anim = ProgressAnimator::new(driver, 0.0);
        anim.animate_to(1.0, 10.0);
        let half = anim.value(10.0 + DRAG_RETURN_DURATION / 2.0);
        assert!(
            half > 0.5,
            "more than half the way at half the time: {half}"
        );
        assert!(anim.is_animating(10.0 + DRAG_RETURN_DURATION - 0.01));
        assert!(!anim.is_animating(10.0 + DRAG_RETURN_DURATION));
        assert_eq!(anim.value(10.0 + DRAG_RETURN_DURATION), 1.0);
    }
}
