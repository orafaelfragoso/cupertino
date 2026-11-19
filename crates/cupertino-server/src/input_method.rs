//! Compositor wiring for [`cupertino_server_input::input_method`].

use std::os::unix::net::UnixStream;

use smithay::reexports::calloop::channel::Sender as LoopSender;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::input_method::{InputMethodHandler, PopupSurface};
use smithay::{delegate_input_method_manager, delegate_text_input_manager};

use crate::kbd_ui::TextOp;
use crate::state::{ClientState, Cupertino};

pub use cupertino_server_input::input_method::{ImBridge, ImEvent};

impl Cupertino {
    /// Connect the in-process input method. Needs the display to already export the input-method
    /// global (created in `Cupertino::new` when the picker is configured).
    pub fn start_input_method(&mut self, events: LoopSender<ImEvent>) -> Option<ImBridge> {
        let (server_end, client_end) = UnixStream::pair().ok()?;
        let data = ClientState {
            internal: true,
            ..Default::default()
        };
        if let Err(e) = crate::state::insert_client(&self.display_handle, server_end, data) {
            tracing::warn!(error = %e, "input method bridge: cannot add the client");
            return None;
        }
        ImBridge::start(client_end, events)
    }

    /// Connect the bridge when the display exports the input-method global (an accent table was loaded).
    pub fn connect_input_method(&mut self) -> bool {
        use smithay::reexports::calloop::channel::{channel, Event};
        if self.input_method_state.is_none() || self.input_method.is_some() {
            return false;
        }
        let (tx, rx) = channel::<ImEvent>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let Event::Msg(ev) = event {
                state.on_input_method_event(ev);
            }
        });
        if inserted.is_err() {
            return false;
        }
        self.input_method = self.start_input_method(tx);
        self.input_method.is_some()
    }

    /// A report of the bridge thread.
    pub fn on_input_method_event(&mut self, ev: ImEvent) {
        match ev {
            ImEvent::Active(active) => {
                self.overlays.accent.bridge = active;
                if !active {
                    self.accent_dismiss();
                }
            }
            ImEvent::Surrounding { .. } => {}
        }
    }

    /// The text cursor of the focused application in global logical coordinates, from the rectangle the
    /// application sent (surface-local) and the window's content origin.
    pub fn text_cursor_global(&self) -> Option<cupertino_geometry::Rect> {
        let r = self.text_cursor?;
        let id = self.wm.key_window()?;
        let g = self.wm.window(id)?.geometry();
        let ins = self.entry(id).map(|e| e.insets()).unwrap_or_default();
        Some(cupertino_geometry::Rect::new(
            g.x + ins.left + f64::from(r.loc.x),
            g.y + ins.top + f64::from(r.loc.y),
            f64::from(r.size.w.max(1)),
            f64::from(r.size.h.max(1)),
        ))
    }

    /// Deliver one edit to the focused client's text input through the bridge.
    pub fn deliver_text_op(&self, op: &TextOp) -> bool {
        self.input_method
            .as_ref()
            .is_some_and(|b| b.send(op.clone()))
    }
}

impl InputMethodHandler for Cupertino {
    fn new_popup(&mut self, surface: PopupSurface) {
        self.text_cursor = Some(surface.text_input_rectangle());
        self.input_popup = Some(surface);
    }

    fn dismiss_popup(&mut self, surface: PopupSurface) {
        self.text_cursor = None;
        if self
            .input_popup
            .as_ref()
            .is_some_and(|p| p.wl_surface() == surface.wl_surface())
        {
            self.input_popup = None;
        }
    }

    fn popup_repositioned(&mut self, surface: PopupSurface) {
        self.text_cursor = Some(surface.text_input_rectangle());
        self.input_popup = Some(surface);
    }

    fn parent_geometry(&self, _parent: &WlSurface) -> Rectangle<i32, Logical> {
        Rectangle::default()
    }
}
delegate_input_method_manager!(Cupertino);
delegate_text_input_manager!(Cupertino);
