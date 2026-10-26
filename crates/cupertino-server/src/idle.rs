//! Idle detection: `ext-idle-notify-v1` reports how long the seat has seen no input, and
//! `zwp-idle-inhibit-v1` lets a client (a video player) hold the notifications back while its surface lives.

use smithay::delegate_idle_inhibit;
use smithay::delegate_idle_notify;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::idle_inhibit::{IdleInhibitHandler, IdleInhibitManagerState};
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};

use crate::state::Cupertino;

/// Protocol state and the surfaces that hold an inhibitor.
pub struct Idle {
    pub notifier: IdleNotifierState<Cupertino>,
    _inhibit: IdleInhibitManagerState,
    inhibitors: Vec<WlSurface>,
}

impl Idle {
    pub fn new(notifier: IdleNotifierState<Cupertino>, inhibit: IdleInhibitManagerState) -> Self {
        Self {
            notifier,
            _inhibit: inhibit,
            inhibitors: Vec::new(),
        }
    }

    /// Hold the notifications back while a live surface has an inhibitor. A client that disconnects never
    /// destroys its inhibitors, so liveness is checked here.
    /// A live surface holds an inhibitor.
    pub fn inhibited(&self) -> bool {
        !self.inhibitors.is_empty()
    }

    pub fn refresh_inhibition(&mut self) {
        self.inhibitors.retain(Resource::is_alive);
        self.notifier.set_is_inhibited(!self.inhibitors.is_empty());
    }
}

impl IdleNotifierHandler for Cupertino {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle.notifier
    }
}
delegate_idle_notify!(Cupertino);

impl IdleInhibitHandler for Cupertino {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle.inhibitors.push(surface);
        self.idle.refresh_inhibition();
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        if let Some(i) = self.idle.inhibitors.iter().position(|s| *s == surface) {
            self.idle.inhibitors.remove(i);
        }
        self.idle.refresh_inhibition();
    }
}
delegate_idle_inhibit!(Cupertino);
