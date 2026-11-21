//! Pointer constraints (`zwp_pointer_constraints_v1`): an application such as a game, a virtual machine or a remote
//! desktop client locks the pointer in place, or confines it to a region of its surface.
//!
//! The compositor decides, not the client. A constraint is active only while its surface is under the pointer and
//! belongs to the window with the keyboard focus, that window may hold the shortcuts (an application's toplevel, the
//! session unlocked, its client answering its pings), the person has not released it, no move, resize, popup grab or
//! switcher is running, and the pointer is inside the constraint's region. A lock keeps the pointer where
//! it is and the client gets only relative motion; a confine keeps the pointer inside the region. The release chord
//! gives the pointer back together with the shortcuts, and the same rule arms it again: focus from the person's own
//! input (`crate::shortcut_inhibit`). A released window never hides the cursor. A subsurface under the pointer counts as
//! its surface: games and video draw in one.

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::input::pointer::{MotionEvent, PointerHandle};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER};
use smithay::wayland::compositor::{with_states, SurfaceAttributes};
use smithay::wayland::pointer_constraints::{
    with_pointer_constraint, PointerConstraint, PointerConstraintsHandler,
};

use crate::state::{Cupertino, PointerMode};

pub use cupertino_server_input::pointer_constraints::{
    added, confine, in_region, origin_of, Constrained, PointerConstraints,
};

impl Cupertino {
    /// Whether a constraint of `surface` may be active now, the pointer aside, with `focus` the surface that has the
    /// keyboard.
    fn may_constrain(&self, surface: &WlSurface, focus: Option<&WlSurface>) -> bool {
        let Some(root) = self.root_window_surface(surface) else {
            return false;
        };
        focus == Some(&root)
            && self.may_hold_shortcuts(&root)
            && !self.shortcut_inhibit.is_released(&root)
            && self.pointer_mode == PointerMode::Normal
            && self.popup_grab_root().is_none()
            && self.wm.switcher().is_none()
    }

    /// Whether a constraint is active now.
    pub fn pointer_held(&self) -> bool {
        self.pointer_constraints.surfaces.iter().any(|surface| {
            with_pointer_constraint(surface, &self.pointer, |c| c.is_some_and(|c| c.is_active()))
        })
    }

    /// The surface with an active constraint under the pointer, itself or through a subsurface, with its origin.
    fn active_constraint(&self) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.pointer_constraints.surfaces.is_empty() {
            return None;
        }
        let under = self.surface_under(self.pointer.current_location())?;
        self.pointer_constraints
            .surfaces
            .iter()
            .find_map(|surface| {
                let origin = origin_of(surface, &under)?;
                with_pointer_constraint(surface, &self.pointer, |c| {
                    c.is_some_and(|c| c.is_active())
                })
                .then(|| (surface.clone(), origin))
            })
    }

    /// What the active constraint makes of a motion of the pointer to `to`, if one is active.
    pub fn constrain_motion(&self, to: Point<f64, Logical>) -> Option<Constrained> {
        let (surface, origin) = self.active_constraint()?;
        let from = self.pointer.current_location();
        let size = with_renderer_surface_state(&surface, |s| s.surface_size()).flatten()?;
        let whole = Rectangle::from_size(size);
        let input = with_states(&surface, |s| {
            s.cached_state
                .get::<SurfaceAttributes>()
                .current()
                .input_region
                .clone()
        });
        with_pointer_constraint(&surface, &self.pointer, |c| {
            let c = c?;
            Some(match &*c {
                PointerConstraint::Locked(_) => Constrained::Locked,
                PointerConstraint::Confined(_) => {
                    let region = c.region();
                    let allowed = |p: Point<f64, Logical>| {
                        whole.to_f64().contains(p)
                            && in_region(region, p)
                            && in_region(input.as_ref(), p)
                    };
                    let mut rects = Vec::new();
                    for a in added(region, whole) {
                        for b in added(input.as_ref(), whole) {
                            if let Some(r) = a.intersection(b).and_then(|r| r.intersection(whole)) {
                                rects.push(r);
                            }
                        }
                    }
                    Constrained::Confined(
                        confine(from - origin, to - origin, &rects, allowed) + origin,
                    )
                }
            })
        })
    }

    /// Make every constraint active or not by the rules above, with `focus` the surface that has the keyboard (the
    /// keyboard is not asked: this runs inside its focus change too).
    pub fn refresh_pointer_constraints(&mut self, focus: Option<&WlSurface>) {
        let pointer = self.pointer.clone();
        self.pointer_constraints.surfaces.retain(|surface| {
            surface.is_alive() && with_pointer_constraint(surface, &pointer, |c| c.is_some())
        });
        if focus.map(|f| f.id()) != self.pointer_constraints.noticed {
            self.pointer_constraints.noticed = None;
        }
        if self.pointer_constraints.surfaces.is_empty() {
            self.settle_hold_notice();
            return;
        }
        let pos = pointer.current_location();
        let under = self.surface_under(pos);
        for surface in self.pointer_constraints.surfaces.clone() {
            let origin = under.as_ref().and_then(|u| origin_of(&surface, u));
            let local = origin.map(|origin| pos - origin);
            let may = local.is_some() && self.may_constrain(&surface, focus);
            let (want, active, hint) = with_pointer_constraint(&surface, &pointer, |c| {
                let Some(c) = c else {
                    return (false, false, None);
                };
                let hint = match &*c {
                    PointerConstraint::Locked(locked) => locked.cursor_position_hint(),
                    PointerConstraint::Confined(_) => None,
                };
                let want = may && local.is_some_and(|p| in_region(c.region(), p));
                (want, c.is_active(), hint)
            });
            if want && !active {
                with_pointer_constraint(&surface, &pointer, |c| {
                    if let Some(c) = c {
                        c.activate();
                    }
                });
                if let Some(root) = self
                    .root_window_surface(&surface)
                    .filter(|root| self.pointer_constraints.noticed.as_ref() != Some(&root.id()))
                {
                    self.pointer_constraints.noticed = Some(root.id());
                    self.show_hold_notice(&root);
                }
                self.request_redraw();
            } else if !want && active {
                with_pointer_constraint(&surface, &pointer, |c| {
                    if let Some(c) = c {
                        c.deactivate();
                    }
                });
                // the pointer comes back where the client drew it, if that is on the surface
                if let (Some(hint), Some(origin)) = (hint, origin) {
                    let inside = with_renderer_surface_state(&surface, |s| s.surface_size())
                        .flatten()
                        .is_some_and(|size| Rectangle::from_size(size).to_f64().contains(hint));
                    if inside {
                        self.warp_pointer(&pointer, origin + hint);
                    }
                }
                self.request_redraw();
            }
        }
        // Smithay also ends a constraint by itself when its surface loses the pointer, without this pass
        self.settle_hold_notice();
    }

    /// The release chord gave the pointer back: the next hold shows its notice again.
    pub(crate) fn forget_pointer_notice(&mut self) {
        self.pointer_constraints.noticed = None;
    }

    /// Put the pointer at `pos` without a motion of the person's: no relative motion and no constraint check.
    fn warp_pointer(&mut self, pointer: &PointerHandle<Self>, pos: Point<f64, Logical>) {
        let under = self.surface_under(pos);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time: std::time::Duration::from(self.clock.now()).as_millis() as u32,
            },
        );
        pointer.frame(self);
    }

    /// Whether the cursor stays visible over `surface` whatever its client asks: the person released its window.
    pub fn cursor_forced_visible(&self) -> bool {
        self.pointer
            .current_focus()
            .and_then(|surface| self.root_window_surface(&surface))
            .is_some_and(|root| self.shortcut_inhibit.is_released(&root))
    }
}

impl PointerConstraintsHandler for Cupertino {
    fn new_constraint(&mut self, surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        if !self.pointer_constraints.surfaces.contains(surface) {
            self.pointer_constraints.surfaces.push(surface.clone());
        }
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        self.refresh_pointer_constraints(focus.as_ref());
    }

    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        _location: Point<f64, Logical>,
    ) {
        // read when the lock ends
    }
}

smithay::delegate_pointer_constraints!(Cupertino);
