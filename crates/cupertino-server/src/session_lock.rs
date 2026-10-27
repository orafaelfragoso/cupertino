//! Session lock (`ext-session-lock-v1`): the lock client takes over every output; while the lock holds, the
//! compositor draws only its lock surfaces over solid black and delivers input only to them.
//!
//! [`LockState`] is the whole lifecycle. A lock request first becomes `Pending`: nothing but black and the lock
//! surfaces is drawn from then on, and `locked` is sent only once every output has presented such a frame and
//! has a committed lock surface (or the surfaces took too long). A lock whose client disappears becomes
//! `Orphaned`: the outputs stay black until another client takes the lock.

use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::delegate_session_lock;
use smithay::desktop::utils::under_from_surface_tree;
use smithay::desktop::WindowSurfaceType;
use smithay::input::pointer::CursorImageStatus;
use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker,
};

use crate::authorization::{authorized, ShellRole};
use crate::render::scene::LockView;
use crate::state::Cupertino;

/// Longest wait for lock surfaces before `locked` is sent with black outputs (approximate).
const LOCK_SURFACE_WAIT: f64 = 1.0;

/// Where the session lock stands.
pub enum LockState {
    Unlocked,
    /// A client asked for the lock; `locked` is still to be sent. `takeover`: the session was locked by a
    /// client that is gone, and stays locked if this one goes too.
    Pending {
        locker: SessionLocker,
        since: f64,
        takeover: bool,
    },
    /// The lock client holds the lock and has been told so.
    Locked {
        owner: ExtSessionLockV1,
        client: ClientId,
    },
    /// The lock client is gone without unlocking.
    Orphaned,
}

/// Session lock state of the compositor.
pub struct SessionLock {
    manager: SessionLockManagerState,
    state: LockState,
    /// An `unlock_and_destroy` arrived this dispatch round; acted on in [`Cupertino::tick_session_lock`].
    unlock_requested: bool,
}

impl SessionLock {
    pub fn new(dh: &DisplayHandle) -> Self {
        Self {
            manager: SessionLockManagerState::new::<Cupertino, _>(dh, |client| {
                authorized(client, ShellRole::SessionLock)
            }),
            state: LockState::Unlocked,
            unlock_requested: false,
        }
    }

    /// Whether the session is locked or about to be: normal drawing and input are off.
    pub fn is_active(&self) -> bool {
        !matches!(self.state, LockState::Unlocked)
    }

    /// The lock object of the client that holds the lock, while that client lives.
    fn owner(&self) -> Option<&ExtSessionLockV1> {
        match &self.state {
            LockState::Pending { locker, .. } => Some(locker.ext_session_lock()),
            LockState::Locked { owner, .. } => Some(owner),
            LockState::Unlocked | LockState::Orphaned => None,
        }
    }
}

/// Lock surface and frame bookkeeping of one output.
#[derive(Default)]
pub struct OutputLock {
    surface: Option<LockSurface>,
    /// The element list of the last frame built for the output was a locked scene.
    built_locked: bool,
    /// A locked frame including the current lock surface contents reached the screen.
    presented: bool,
}

impl OutputLock {
    fn has_buffer(&self) -> bool {
        self.surface.as_ref().is_some_and(|s| {
            with_renderer_surface_state(s.wl_surface(), |st| st.buffer().is_some()).unwrap_or(false)
        })
    }
}

impl SessionLockHandler for Cupertino {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock.manager
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.settle_session_lock();
        if !confirmation
            .ext_session_lock()
            .client()
            .is_some_and(|c| authorized(&c, ShellRole::SessionLock))
        {
            tracing::warn!("session lock refused: its client does not hold the session lock role");
            return;
        }
        // dropping the locker tells the requesting client `finished`
        if !matches!(
            self.session_lock.state,
            LockState::Unlocked | LockState::Orphaned
        ) {
            tracing::warn!("session lock refused: the session is already locked by another client");
            return;
        }
        // a popup grab left over would eat the first click on the lock screen as an outside press
        self.end_popup_grab(
            smithay::utils::SERIAL_COUNTER.next_serial(),
            std::time::Duration::from(self.clock.now()).as_millis() as u32,
        );
        let takeover = matches!(self.session_lock.state, LockState::Orphaned);
        self.begin_session_lock(confirmation, takeover);
    }

    // smithay reports the request without naming the lock object, and lets one from a lock that was refused
    // through; the request is honoured in `tick_session_lock` only if it came from the owner
    fn unlock(&mut self) {
        self.session_lock.unlock_requested = true;
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let from_owner = self.session_lock.owner().is_some_and(|o| {
            o.client().map(|c| c.id()) == surface.wl_surface().client().map(|c| c.id())
        });
        let Some(idx) = Output::from_resource(&output)
            .and_then(|out| self.outputs.iter().position(|o| o.output == out))
        else {
            return;
        };
        if !from_owner {
            tracing::warn!("lock surface ignored: its client does not hold the session lock");
            return;
        }
        self.outputs[idx].lock = OutputLock {
            surface: Some(surface),
            built_locked: false,
            presented: false,
        };
        self.configure_lock_surface(idx);
    }
}
delegate_session_lock!(Cupertino);

impl Cupertino {
    /// Hide everything and wait for the first locked frames.
    fn begin_session_lock(&mut self, locker: SessionLocker, takeover: bool) {
        let now = self.now();
        // grabs and drags of other clients end, the pointer and keyboard leave them
        let (serial, time) = (SERIAL_COUNTER.next_serial(), (now * 1000.0) as u32);
        let pointer = self.pointer.clone();
        pointer.unset_grab(self, serial, time);
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.unset_grab(self);
        keyboard.set_focus(self, None, serial);
        if self.pointer_mode == crate::state::PointerMode::Model {
            self.command(cupertino_wm::Command::PointerRelease);
            self.pointer_mode = crate::state::PointerMode::Normal;
        }
        self.cursor_status = CursorImageStatus::default_named();
        self.decor.cursor = None;
        for o in &mut self.outputs {
            o.lock = OutputLock::default();
        }
        let was_active = self.session_lock.is_active();
        self.session_lock.state = LockState::Pending {
            locker,
            since: now,
            takeover,
        };
        // a takeover of an orphaned lock finds the session locked already
        if !was_active {
            self.sessions.lock(true);
            self.stop_captures(true);
            crate::displays::management::session_locked(self);
        }
        self.refresh_pointer_focus();
        self.request_redraw();
    }

    /// Send `locked` once every output shows a locked frame with a lock surface (or the wait is over), and
    /// notice a lock client that went away.
    pub fn tick_session_lock(&mut self) {
        if std::mem::take(&mut self.session_lock.unlock_requested) {
            // the destructor request has destroyed the lock object of its sender by now; the owner's client must
            // still be connected, or the lock object died with a crash while another client sent the request
            let from_owner = matches!(&self.session_lock.state, LockState::Locked { owner, client }
                if !owner.is_alive() && self.display_handle.backend_handle().get_client_data(client.clone()).is_ok());
            if from_owner {
                self.end_session_lock();
                return;
            }
            tracing::warn!("unlock request ignored: it did not come from the client that holds the session lock");
        }
        self.settle_session_lock();
        let LockState::Pending { since, .. } = &self.session_lock.state else {
            return;
        };
        let waited = self.now() - since >= LOCK_SURFACE_WAIT;
        if !self
            .outputs
            .iter()
            .all(|o| o.lock.presented && (o.lock.has_buffer() || waited))
        {
            self.request_redraw();
            return;
        }
        let LockState::Pending { locker, .. } =
            std::mem::replace(&mut self.session_lock.state, LockState::Unlocked)
        else {
            return;
        };
        let owner = locker.ext_session_lock().clone();
        // `settle_session_lock` has just dropped a locker whose client is gone; should one slip through, the
        // session stays locked rather than open
        let Some(client) = owner.client().map(|c| c.id()) else {
            tracing::warn!(
                "lock client disconnected while taking the lock: the session stays locked"
            );
            self.session_lock.state = LockState::Orphaned;
            self.request_redraw();
            return;
        };
        locker.lock();
        self.session_lock.state = LockState::Locked { owner, client };
        tracing::info!("session locked");
    }

    /// React to the lock client disappearing: a lock that was never confirmed is dropped, a confirmed one
    /// stays in force.
    fn settle_session_lock(&mut self) {
        match &self.session_lock.state {
            LockState::Pending {
                locker, takeover, ..
            } if !locker.ext_session_lock().is_alive() => {
                if *takeover {
                    tracing::warn!(
                        "lock client disconnected before taking the lock: the session stays locked"
                    );
                    self.session_lock.state = LockState::Orphaned;
                    self.request_redraw();
                } else {
                    tracing::warn!("lock client disconnected before the session was locked");
                    self.restore_after_lock();
                }
            }
            LockState::Locked { owner, client }
                if !owner.is_alive()
                    && self
                        .display_handle
                        .backend_handle()
                        .get_client_data(client.clone())
                        .is_err() =>
            {
                tracing::warn!(
                    "lock client disconnected without unlocking: the session stays locked"
                );
                self.session_lock.state = LockState::Orphaned;
                for o in &mut self.outputs {
                    o.lock.surface = None;
                }
                let keyboard = self.seat.get_keyboard().unwrap();
                keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
                self.refresh_pointer_focus();
                self.request_redraw();
            }
            _ => {}
        }
    }

    fn end_session_lock(&mut self) {
        self.restore_after_lock();
        tracing::info!("session unlocked");
    }

    /// Back to normal drawing and focus.
    fn restore_after_lock(&mut self) {
        self.session_lock.state = LockState::Unlocked;
        self.sessions.lock(false);
        for o in &mut self.outputs {
            o.lock = OutputLock::default();
        }
        match self.exclusive_layer.clone() {
            Some(layer) => self.set_keyboard_focus(Some(layer)),
            None => self.restore_keyboard_focus(),
        }
        self.refresh_pointer_focus();
        self.request_redraw();
    }

    fn refresh_pointer_focus(&mut self) {
        let (pos, time) = (
            self.pointer.current_location(),
            (self.now() * 1000.0) as u32,
        );
        self.pointer_moved_to(pos, time, None);
    }

    /// Tell the lock surface of output `idx` its size: the whole output.
    pub fn configure_lock_surface(&mut self, idx: usize) {
        let out = &self.outputs[idx];
        let Some(surface) = &out.lock.surface else {
            return;
        };
        surface.with_pending_state(|s| {
            s.size = Some(((out.rect.w as u32), (out.rect.h as u32)).into())
        });
        surface.send_configure();
    }

    /// A surface committed: the first buffer of a lock surface takes the keyboard when nothing holds it and
    /// invalidates the locked frame shown so far.
    pub fn lock_surface_committed(&mut self, surface: &WlSurface) {
        let Some(idx) = self.outputs.iter().position(|o| {
            o.lock
                .surface
                .as_ref()
                .is_some_and(|s| s.wl_surface() == surface)
        }) else {
            return;
        };
        self.outputs[idx].lock.presented = false;
        let keyboard = self.seat.get_keyboard().unwrap();
        if keyboard.current_focus().is_none() {
            self.set_keyboard_focus(Some(surface.clone()));
        }
    }

    /// A press at `pos`: the lock surface there takes the keyboard.
    pub fn focus_lock_surface_at(&mut self, pos: Point<f64, Logical>) {
        let Some(surface) = self
            .output_at(pos)
            .and_then(|o| o.lock.surface.as_ref())
            .map(|s| s.wl_surface().clone())
        else {
            return;
        };
        self.set_keyboard_focus(Some(surface));
    }

    pub fn is_lock_surface(&self, surface: &WlSurface) -> bool {
        self.outputs.iter().any(|o| {
            o.lock
                .surface
                .as_ref()
                .is_some_and(|s| s.wl_surface() == surface)
        })
    }

    /// The lock surface (or one of its subsurfaces) under `pos` in the global logical space.
    pub fn lock_surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        let out = self.output_at(pos)?;
        let surface = out.lock.surface.as_ref()?;
        let origin = Point::<f64, Logical>::from((out.rect.x, out.rect.y));
        let (hit, loc) = under_from_surface_tree(
            surface.wl_surface(),
            pos - origin,
            (0, 0),
            WindowSurfaceType::ALL,
        )?;
        Some((hit, origin + loc.to_f64()))
    }

    /// The lock surface of output `idx` while it is alive.
    pub fn lock_surface_of(&self, idx: usize) -> Option<WlSurface> {
        self.outputs[idx]
            .lock
            .surface
            .as_ref()
            .filter(|s| s.alive())
            .map(|s| s.wl_surface().clone())
    }

    /// What output `idx` has to draw for the lock.
    pub fn lock_view(&self, idx: usize) -> LockView {
        if self.session_lock.is_active() {
            LockView::Locked {
                surface: self.lock_surface_of(idx),
            }
        } else {
            LockView::Unlocked
        }
    }

    /// The scene of output `idx` is being built.
    pub fn note_scene_built(&mut self, idx: usize) {
        let locked = self.session_lock.is_active();
        self.outputs[idx].lock.built_locked = locked;
    }

    /// The frame last built for output `idx` is on the screen.
    pub fn note_frame_shown(&mut self, idx: usize) {
        let lock = &mut self.outputs[idx].lock;
        lock.presented |= lock.built_locked;
    }
}
