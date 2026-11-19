//! Compositor wiring for [`cupertino_server_render::drag_return`].

use cupertino_wm::Rect;
use smithay::utils::{Logical, Point};

pub use cupertino_server_render::drag_return::*;

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
