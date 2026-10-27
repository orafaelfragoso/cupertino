//! The protocols outside the transport's standard set, dispatched on its queue: layer shell, session lock, idle
//! notification, the app effects and the private shell roles. Each `Dispatch` impl reports through
//! [`State::emit`], so what it receives reaches [`crate::Client`] in order with the transport's own events.

use crate::dock::GroupBuilder as DockGroup;
use crate::menu_bar::GroupBuilder as MenuBarGroup;
use crate::{Event, OutputId, SurfaceId};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_appearance_v1::{
    self, CupertinoAppearanceV1,
};
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::CupertinoBackdropV1;
use cupertino_protocols::client::cupertino_dock_v1::{self, CupertinoDockV1};
use cupertino_protocols::client::cupertino_menu_bar_v1::{self, CupertinoMenuBarV1};
use cupertino_protocols::client::cupertino_session_v1::{self, CupertinoSessionV1, DisplayOption};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::scroll::cupertino_pointer_scroll_v1::{
    self, CupertinoPointerScrollV1,
};
use cupertino_protocols::client::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use cupertino_wayland_client::{Error, ExternalRole, State};
use std::sync::Mutex;
use wayland_client::protocol::wl_pointer::Axis;
use wayland_client::{Connection as WlConnection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::{
    self, ExtIdleNotificationV1,
};
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::{
    self, ExtSessionLockSurfaceV1,
};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::{
    self, ExtSessionLockV1,
};
use wayland_protocols::xdg::shell::client::xdg_popup::XdgPopup;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};

/// User data of the objects whose events carry nothing for the client.
pub(crate) struct Quiet;

macro_rules! quiet {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, Quiet> for State {
            fn event(
                _: &mut State,
                _: &$interface,
                _: <$interface as wayland_client::Proxy>::Event,
                _: &Quiet,
                _: &WlConnection,
                _: &QueueHandle<State>,
            ) {
            }
        }
    )*};
}

quiet!(
    ZwlrLayerShellV1,
    ExtSessionLockManagerV1,
    ExtIdleNotifierV1,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1,
    CupertinoShellV1,
    CupertinoScrollV1
);

/// What an extension object received, in the order it arrived.
pub(crate) enum Emitted {
    /// A layer or lock surface configure with the size as sent: a 0 dimension is the client's.
    Configure {
        surface: SurfaceId,
        size: (u32, u32),
    },
    /// The lock `generation` was confirmed (`true`) or finished (`false`).
    Lock {
        generation: u32,
        locked: bool,
    },
    Idle(bool),
    /// The scroll count of one axis of the pointer frame being received.
    ScrollCount {
        vertical: bool,
        count: u32,
    },
    /// The session's state changed; [`crate::Client`] keeps it and queues the event once connected.
    Session(Event),
    Event(Event),
}

/// A layer surface (`zwlr_layer_surface_v1`).
pub(crate) struct LayerRole(pub ZwlrLayerSurfaceV1);

impl ExternalRole for LayerRole {
    fn ack_configure(&self, serial: u32) {
        self.0.ack_configure(serial);
    }

    fn destroy(&self) {
        self.0.destroy();
    }

    fn adopt_popup(&self, popup: &XdgPopup) -> Result<(), Error> {
        self.0.get_popup(popup);
        Ok(())
    }
}

/// A lock surface, which the compositor configures unasked and which must not commit before its first ack.
pub(crate) struct LockRole(pub ExtSessionLockSurfaceV1);

impl ExternalRole for LockRole {
    fn ack_configure(&self, serial: u32) {
        self.0.ack_configure(serial);
    }

    fn destroy(&self) {
        self.0.destroy();
    }

    fn initial_commit(&self) -> bool {
        false
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, SurfaceId> for State {
    fn event(
        state: &mut State,
        _: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        surface: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                state.configure_external(surface.0, serial, (width, height));
                state.emit(Emitted::Configure {
                    surface: *surface,
                    size: (width, height),
                });
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.emit(Emitted::Event(Event::Closed(*surface)))
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, SurfaceId> for State {
    fn event(
        state: &mut State,
        _: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        surface: &SurfaceId,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            if width == 0 || height == 0 {
                return state.fail(Error::Protocol("lock surface configured without a size"));
            }
            state.configure_external(surface.0, serial, (width, height));
            state.emit(Emitted::Configure {
                surface: *surface,
                size: (width, height),
            });
        }
    }
}

/// User data of a session lock: the generation that tells it from the locks it replaced.
pub(crate) struct LockGeneration(pub u32);

impl Dispatch<ExtSessionLockV1, LockGeneration> for State {
    fn event(
        state: &mut State,
        _: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        lock: &LockGeneration,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        let locked = match event {
            ext_session_lock_v1::Event::Locked => true,
            ext_session_lock_v1::Event::Finished => false,
            _ => return,
        };
        state.emit(Emitted::Lock {
            generation: lock.0,
            locked,
        });
    }
}

impl Dispatch<ExtIdleNotificationV1, Quiet> for State {
    fn event(
        state: &mut State,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.emit(Emitted::Idle(true)),
            ext_idle_notification_v1::Event::Resumed => state.emit(Emitted::Idle(false)),
            _ => {}
        }
    }
}

impl Dispatch<CupertinoPointerScrollV1, Quiet> for State {
    fn event(
        state: &mut State,
        _: &CupertinoPointerScrollV1,
        event: cupertino_pointer_scroll_v1::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            cupertino_pointer_scroll_v1::Event::ScrollCount {
                axis: WEnum::Value(axis),
                count,
            } => state.emit(Emitted::ScrollCount {
                vertical: axis == Axis::VerticalScroll,
                count,
            }),
            cupertino_pointer_scroll_v1::Event::ScrollCount { .. } => {
                state.fail(Error::Protocol("scroll count of an unknown axis"))
            }
            _ => {}
        }
    }
}

fn dark(state: &mut State, dark: u32) {
    if dark > 1 {
        return state.fail(Error::Protocol("appearance other than 0 or 1"));
    }
    state.emit(Emitted::Session(Event::Appearance { dark: dark == 1 }));
}

fn display_options(state: &mut State, bits: DisplayOption) {
    state.emit(Emitted::Session(Event::DisplayOptions(DisplayOptions {
        reduce_motion: bits.contains(DisplayOption::ReduceMotion),
        reduce_transparency: bits.contains(DisplayOption::ReduceTransparency),
        increase_contrast: bits.contains(DisplayOption::IncreaseContrast),
        differentiate_without_color: bits.contains(DisplayOption::DifferentiateWithoutColor),
    })));
}

impl Dispatch<CupertinoSessionV1, Quiet> for State {
    fn event(
        state: &mut State,
        _: &CupertinoSessionV1,
        event: cupertino_session_v1::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        let session = match event {
            cupertino_session_v1::Event::Appearance { dark: value } => return dark(state, value),
            cupertino_session_v1::Event::DisplayOptions { options } => {
                return match options.into_result() {
                    Ok(bits) => display_options(state, bits),
                    Err(_) => state.fail(Error::Protocol("display options with reserved bits")),
                };
            }
            cupertino_session_v1::Event::Locked => Event::SessionLock { locked: true },
            cupertino_session_v1::Event::Unlocked => Event::SessionLock { locked: false },
            cupertino_session_v1::Event::LockRequested { explicit } => Event::LockRequested {
                explicit: explicit != 0,
            },
            cupertino_session_v1::Event::LogOutRequested => Event::LogOutRequested,
            _ => return,
        };
        state.emit(Emitted::Session(session));
    }
}

impl Dispatch<CupertinoAppearanceV1, Quiet> for State {
    fn event(
        state: &mut State,
        _: &CupertinoAppearanceV1,
        event: cupertino_appearance_v1::Event,
        _: &Quiet,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            cupertino_appearance_v1::Event::Appearance { dark: value } => dark(state, value),
            cupertino_appearance_v1::Event::DisplayOptions { options } => {
                match options.into_result() {
                    Ok(bits) => {
                        display_options(state, DisplayOption::from_bits_truncate(bits.bits()))
                    }
                    Err(_) => state.fail(Error::Protocol("display options with reserved bits")),
                }
            }
            _ => {}
        }
    }
}

/// User data of a menu-bar or dock role: its surface and the group of events being received.
pub(crate) struct RoleData<G> {
    pub surface: SurfaceId,
    pub group: Mutex<G>,
}

impl Dispatch<CupertinoMenuBarV1, RoleData<MenuBarGroup>> for State {
    fn event(
        state: &mut State,
        _: &CupertinoMenuBarV1,
        event: cupertino_menu_bar_v1::Event,
        data: &RoleData<MenuBarGroup>,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        let update = data.group.lock().expect("menu bar group").push(event);
        if let Some(update) = update {
            state.emit(Emitted::Event(Event::MenuBar {
                surface: data.surface,
                update,
            }));
        }
    }
}

impl Dispatch<CupertinoDockV1, RoleData<DockGroup>> for State {
    fn event(
        state: &mut State,
        _: &CupertinoDockV1,
        event: cupertino_dock_v1::Event,
        data: &RoleData<DockGroup>,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        let mut group = data.group.lock().expect("dock group");
        // the panel's output arrives as this client's object for it: map it to the output's id
        if let cupertino_dock_v1::Event::SwitcherOutput { output } = &event {
            return match state.output_id(output) {
                Some(id) => group.switcher_output(OutputId(id.registry_name())),
                None => state.fail(Error::Protocol(
                    "switcher_output names an output this client has not bound",
                )),
            };
        }
        match group.push(event) {
            Ok(Some(update)) => state.emit(Emitted::Event(Event::Dock {
                surface: data.surface,
                update,
            })),
            Ok(None) => {}
            Err(what) => state.fail(Error::Protocol(what)),
        }
    }
}
