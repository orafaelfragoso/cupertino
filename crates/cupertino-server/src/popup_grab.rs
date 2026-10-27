//! The popup policy of the pointer: while a popup grab is active the pointer follows the popup chain (a press on a popup
//! goes to that popup), and a press anywhere else dismisses the whole chain and is consumed, so it never reaches the
//! window or shell surface under it. The keyboard stays with the popup until the grab ends.

use smithay::desktop::{find_popup_root_surface, PopupGrab, PopupUngrabStrategy};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};

use crate::state::Cupertino;

/// An active popup grab and the toplevel its popups belong to.
pub struct ActivePopupGrab {
    grab: PopupGrab<Cupertino>,
    root: WlSurface,
}

impl ActivePopupGrab {
    pub fn new(grab: PopupGrab<Cupertino>, root: WlSurface) -> Self {
        Self { grab, root }
    }
}

impl Cupertino {
    /// Whether `surface` is a popup of the grabbed toplevel.
    fn in_popup_chain(&self, surface: &WlSurface, root: &WlSurface) -> bool {
        self.popups
            .find_popup(surface)
            .and_then(|popup| find_popup_root_surface(&popup).ok())
            .is_some_and(|popup_root| &popup_root == root)
    }

    /// The popup grab, or `None` once it ended (the popups were dismissed or their toplevel is gone).
    fn live_popup_grab(&mut self) -> Option<&mut ActivePopupGrab> {
        if self
            .popup_grab
            .as_ref()
            .is_some_and(|active| active.grab.has_ended())
        {
            self.popup_grab = None;
        }
        self.popup_grab.as_mut()
    }

    /// While a popup grab is active, give the pointer focus to what is under it now. Focus otherwise only changes on
    /// motion, so a popup that maps or commits under a pointer that does not move would stay unfocused and take its
    /// first press as one on the window below.
    pub fn refocus_pointer(&mut self) {
        if self.live_popup_grab().is_none() {
            return;
        }
        let pos = self.pointer.current_location();
        let under = self.surface_under(pos);
        let current = self.pointer.current_focus();
        if under.as_ref().map(|(surface, _)| surface) != current.as_ref() {
            self.send_pointer_motion(
                under,
                pos,
                std::time::Duration::from(self.clock.now()).as_millis() as u32,
                None,
            );
        }
    }

    /// A press at `pos` while a popup grab is active: outside the popup chain it dismisses the chain, hands the keyboard
    /// back to the toplevel, focuses what is under the pointer and is consumed (`true`).
    pub fn dismiss_popup_grab_for_press(
        &mut self,
        pos: Point<f64, Logical>,
        serial: Serial,
        time: u32,
    ) -> bool {
        let Some(root) = self.live_popup_grab().map(|active| active.root.clone()) else {
            return false;
        };
        if self
            .surface_under(pos)
            .is_some_and(|(surface, _)| self.in_popup_chain(&surface, &root))
        {
            return false;
        }
        if let Some(mut active) = self.popup_grab.take() {
            let _ = active.grab.ungrab(PopupUngrabStrategy::All);
            if let Some(keyboard) = self.seat.get_keyboard() {
                if keyboard.is_grabbed()
                    && (keyboard.has_grab(active.grab.serial())
                        || keyboard.has_grab(
                            active
                                .grab
                                .previous_serial()
                                .unwrap_or(active.grab.serial()),
                        ))
                {
                    keyboard.unset_grab(self);
                    keyboard.set_focus(
                        self,
                        Some(active.root.clone()),
                        SERIAL_COUNTER.next_serial(),
                    );
                }
            }
        }
        let pointer = self.pointer.clone();
        pointer.unset_grab(self, serial, time);
        let under = self.surface_under(pos);
        self.send_pointer_motion(under, pos, time, None);
        true
    }
}
