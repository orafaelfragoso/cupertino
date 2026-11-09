//! What happens to what a display showed when it goes away, and when it comes back.
//!
//! An output goes away when its display is unplugged or turned off. Its windows move to the first remaining output,
//! fitted into its usable area (`cupertino_wm`), and everything that needs the person stays reachable there: the pointer
//! comes onto the nearest remaining output (a drag goes on with it), a menu or popup of a window that was on the output
//! closes, an overview open on it ends, and while the session is locked the keyboard goes to a remaining lock surface.
//!
//! Restore policy: each window that moved is remembered with the identity of its display (`Head::key`: connector, make,
//! model and serial) and its frame there. When a display with that identity comes back, a window the person has left
//! where it was put (same output, same normal frame) goes back to it, at the same distance from its top-left corner
//! and fitted into its usable area. A window the person moved, resized or put elsewhere meanwhile stays. A window that
//! left full screen when its display went comes back in its normal frame, not in full screen.

use cupertino_displays::arrangement::{self, Area};
use cupertino_wm::{Command, OutputId, Rect, WindowId};
use smithay::utils::{IsAlive, Logical, Point, SERIAL_COUNTER};

use crate::state::Cupertino;

/// A window that moved off a display that went away, remembered until the display comes back.
#[derive(Clone, Debug, PartialEq)]
pub struct Departure {
    /// The identity of the display it was on (`Head::key`).
    pub identity: String,
    pub window: WindowId,
    /// The display's logical rectangle and the window's normal frame on it.
    pub from: Rect,
    pub frame: Rect,
    /// Where the window was put: the output and its normal frame there, once it moved.
    pub placed: Option<(OutputId, Rect)>,
}

fn area(r: Rect) -> Area {
    Area::new(r.x, r.y, r.w, r.h)
}

impl Cupertino {
    /// Output `id`, whose display has `identity`, is about to go: remember its windows and where they are on it.
    pub(crate) fn note_departures(&mut self, id: OutputId, identity: &str, from: Rect) {
        let windows: Vec<(WindowId, Rect)> = self
            .wm
            .windows()
            .filter(|w| w.output == id && !w.state().is_dead())
            .map(|w| (w.id, w.lifecycle.normal_frame()))
            .collect();
        let departed = &mut self.displays.departed;
        departed.retain(|d| !windows.iter().any(|(w, _)| *w == d.window));
        departed.extend(windows.into_iter().map(|(window, frame)| Departure {
            identity: identity.to_owned(),
            window,
            from,
            frame,
            placed: None,
        }));
    }

    /// The windows of a display that went are on a remaining output now: remember where they were put.
    pub(crate) fn note_placements(&mut self) {
        for i in 0..self.displays.departed.len() {
            if self.displays.departed[i].placed.is_some() {
                continue;
            }
            let window = self.displays.departed[i].window;
            let placed = self
                .wm
                .window(window)
                .map(|w| (w.output, w.lifecycle.normal_frame()));
            self.displays.departed[i].placed = placed;
        }
        self.displays.departed.retain(|d| d.placed.is_some());
    }

    /// A display with `identity` came back as `output`: the windows that left it and were not moved since go back.
    pub(crate) fn restore_departed(&mut self, output: OutputId, identity: &str) {
        let Some(to) = self.output_entry(output).map(|o| o.rect) else {
            return;
        };
        let (back, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.displays.departed)
            .into_iter()
            .partition(|d| d.identity == identity);
        self.displays.departed = kept;
        for d in back {
            let Some(w) = self.wm.window(d.window) else {
                continue;
            };
            let untouched = d
                .placed
                .is_some_and(|(o, frame)| w.output == o && w.lifecycle.normal_frame() == frame);
            if w.state().is_dead() || !untouched {
                continue;
            }
            let frame = arrangement::relocate(area(d.frame), area(d.from), area(to));
            tracing::info!(
                window = d.window.0,
                output = output.0,
                "a window goes back to the display it was on"
            );
            self.command(Command::MoveWindowToOutput {
                window: d.window,
                output,
                frame: Rect::new(frame.x, frame.y, frame.w, frame.h),
            });
        }
        self.refresh_window_scales();
    }

    /// The pointer is where no output is any more: it comes onto the nearest point of the nearest output, as a motion,
    /// so a drag in progress follows it there.
    pub(crate) fn bring_pointer_onto_outputs(&mut self) {
        let areas: Vec<Area> = self.outputs.iter().map(|o| area(o.rect)).collect();
        let at = self.pointer.current_location();
        let Some((x, y)) = arrangement::reachable_point(&areas, (at.x, at.y)) else {
            return;
        };
        if (x, y) == (at.x, at.y) {
            return;
        }
        let time = (self.now() * 1000.0) as u32;
        let to: Point<f64, Logical> = (x, y).into();
        self.pointer_moved_to(to, time, None);
    }

    /// While the session is locked and the keyboard is on no lock surface (the one it was on went with its output),
    /// the lock surface under the pointer takes it, else the first one there is.
    pub(crate) fn refocus_lock_keyboard(&mut self) {
        if !self.session_lock.is_active() {
            return;
        }
        let keyboard = self.seat.get_keyboard().expect("the seat has a keyboard");
        if keyboard
            .current_focus()
            .is_some_and(|s| s.alive() && self.is_lock_surface(&s))
        {
            return;
        }
        let at = self.pointer.current_location();
        let idx = self
            .output_at(at)
            .and_then(|o| self.outputs.iter().position(|e| e.id == o.id));
        let surface = idx
            .and_then(|i| self.lock_surface_of(i))
            .or_else(|| (0..self.outputs.len()).find_map(|i| self.lock_surface_of(i)));
        self.set_keyboard_focus(surface);
    }

    /// Output `id` is going: a popup grab of a window on it ends, so its menus close rather than hang over another
    /// output.
    pub(crate) fn end_popup_grab_on(&mut self, id: OutputId) {
        let on_output = self
            .popup_grab_root()
            .and_then(|root| self.window_id_for_surface(root))
            .and_then(|w| self.wm.window(w))
            .is_some_and(|w| w.output == id);
        if on_output {
            let time = std::time::Duration::from(self.clock.now()).as_millis() as u32;
            self.end_popup_grab(SERIAL_COUNTER.next_serial(), time);
        }
    }
}
