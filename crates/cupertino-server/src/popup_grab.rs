//! The popup policy of the pointer: while a popup grab is active the pointer follows the popup chain (a press on a popup
//! goes to that popup), and a press anywhere else dismisses the whole chain and is consumed, so it never reaches the
//! window or shell surface under it. The keyboard stays with the popup until the grab ends.

use smithay::desktop::{find_popup_root_surface, PopupGrab, PopupUngrabStrategy};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};
use smithay::wayland::compositor::get_parent;

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
    /// Whether `surface`, or the surface its subsurfaces hang from, is a popup of the grabbed toplevel.
    fn in_popup_chain(&self, surface: &WlSurface, root: &WlSurface) -> bool {
        let mut top = surface.clone();
        while let Some(parent) = get_parent(&top) {
            top = parent;
        }
        self.popups
            .find_popup(&top)
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

    /// The surface whose popups hold the grab, while the grab is active.
    pub fn popup_grab_root(&self) -> Option<&WlSurface> {
        self.popup_grab
            .as_ref()
            .filter(|active| !active.grab.has_ended())
            .map(|active| &active.root)
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
        self.end_popup_grab(serial, time);
        let under = self.surface_under(pos);
        self.send_pointer_motion(under, pos, time, None);
        true
    }

    /// The keyboard is about to go to `surface` ([`Cupertino::set_keyboard_focus`], the one path every move of the
    /// keyboard takes): when that is outside the chain of the popup grab (another window, a layer surface such as the
    /// launcher, Control Center or the authentication prompt, a lock surface, or none), the grab ends first and its
    /// popups are dismissed, as a press elsewhere would end it, so a menu never stays open without its keys. The
    /// keyboard goes straight to `surface`, never back to the grab's toplevel first. The toplevel itself and the popups
    /// of its chain keep the grab.
    pub(crate) fn end_popup_grab_for_focus(&mut self, surface: Option<&WlSurface>) {
        let Some(root) = self.live_popup_grab().map(|active| active.root.clone()) else {
            return;
        };
        if surface.is_some_and(|s| s == &root || self.in_popup_chain(s, &root)) {
            return;
        }
        self.release_popup_grab(
            SERIAL_COUNTER.next_serial(),
            std::time::Duration::from(self.clock.now()).as_millis() as u32,
            surface.cloned(),
        );
    }

    /// A client destroyed a popup. When that was the last one of the grab of a layer surface (a menu of the menu bar
    /// was closed on a choice), the grab ends now and the keyboard goes back to the exclusive layer, else the key
    /// window: a layer surface holds the keyboard only while it asks for it, and smithay would hand it to the grab's
    /// root at the next key. A window's grab is left to smithay, which gives the keyboard back to the window.
    pub fn popup_dismissed_by_client(&mut self) {
        self.popups.cleanup();
        let Some(root) = self
            .popup_grab
            .as_ref()
            .filter(|active| active.grab.has_ended())
            .map(|active| active.root.clone())
        else {
            return;
        };
        if self.entry_for_surface(&root).is_some() {
            return;
        }
        let back = self.keyboard_after_grab(root);
        self.release_popup_grab(
            SERIAL_COUNTER.next_serial(),
            std::time::Duration::from(self.clock.now()).as_millis() as u32,
            back,
        );
    }

    /// Where the keyboard goes when the grab of `root`'s popups ends: back to `root` when it is a window. A layer surface
    /// (the menu bar) holds the keyboard only while it asks for it, so the keyboard of its grab goes to the exclusive
    /// layer, else the key window, never to the layer itself.
    fn keyboard_after_grab(&self, root: WlSurface) -> Option<WlSurface> {
        if self.entry_for_surface(&root).is_some() {
            return Some(root);
        }
        self.exclusive_layer.clone().or_else(|| {
            self.wm
                .key_window()
                .and_then(|w| self.entry(w))
                .map(|e| e.surface().clone())
        })
    }

    /// End the popup grab, if any: the popups of its chain are dismissed, the keyboard goes back to the toplevel (for a
    /// layer surface's popups, where [`Cupertino::keyboard_after_grab`] says) and the pointer grab is released.
    pub fn end_popup_grab(&mut self, serial: Serial, time: u32) {
        let focus = self
            .popup_grab
            .as_ref()
            .map(|active| active.root.clone())
            .and_then(|root| self.keyboard_after_grab(root));
        self.release_popup_grab(serial, time, focus);
    }

    /// End the popup grab, if any, giving the keyboard to `focus`.
    fn release_popup_grab(&mut self, serial: Serial, time: u32, focus: Option<WlSurface>) {
        let Some(mut active) = self.popup_grab.take() else {
            return;
        };
        {
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
                    keyboard.set_focus(self, focus, SERIAL_COUNTER.next_serial());
                }
            }
        }
        let pointer = self.pointer.clone();
        pointer.unset_grab(self, serial, time);
    }
}
