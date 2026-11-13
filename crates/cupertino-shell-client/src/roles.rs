//! The shell protocols dispatched on the transport's queue. Each `Dispatch` impl reports through [`State::emit`].

use crate::dock::GroupBuilder as DockGroup;
use crate::menu_bar::GroupBuilder as MenuBarGroup;
use crate::{Dock, DockUpdate, Error, MenuBar, MenuBarUpdate};
use cupertino_protocol_app::{report_appearance, report_display_options};
use cupertino_protocols::client::cupertino_dock_v1::{self, CupertinoDockV1};
use cupertino_protocols::client::cupertino_menu_bar_v1::{self, CupertinoMenuBarV1};
use cupertino_protocols::client::cupertino_session_v1::{self, CupertinoSessionV1};
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_wayland_client::{Connection, ExternalRole, OutputId, State, SurfaceId};
use std::sync::Mutex;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection as WlConnection, Dispatch, Proxy, QueueHandle};
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
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

/// User data of the objects whose events carry nothing for the client.
struct Quiet;

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
    CupertinoShellV1
);

/// What a shell object received, in the order it arrived.
#[derive(Clone, Debug, PartialEq)]
pub enum ShellEvent {
    /// A layer or lock surface configure with the size as sent: a 0 dimension is the client's. The transport has
    /// acknowledged it.
    Configure {
        surface: SurfaceId,
        size: (u32, u32),
    },
    /// The server closed a layer surface.
    Closed(SurfaceId),
    /// The lock of `generation` ([`SessionLock::generation`]) was confirmed (`true`) or finished (`false`).
    Lock { generation: u32, locked: bool },
    /// The seat went idle (`true`) or input resumed (`false`).
    Idle(bool),
    /// The server holds the session locked or not; also sent once when the observer is created.
    SessionLock { locked: bool },
    /// The user asked to lock the session; `explicit` when by command rather than by a timeout.
    LockRequested { explicit: bool },
    /// The user asked to log out.
    LogOutRequested,
    /// The answer to [`Session::activate_app`] with this serial.
    AppActivation { serial: u32, result: AppActivation },
    MenuBar {
        surface: SurfaceId,
        update: Box<MenuBarUpdate>,
    },
    Dock {
        surface: SurfaceId,
        update: DockUpdate,
    },
}

/// Distance of a layer surface from its anchored edges, in logical points.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Margin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

/// Everything a layer surface is created with; the settings can be changed later only where
/// a setter exists.
#[derive(Clone, Debug)]
pub struct LayerSurfaceConfig {
    /// Role of the surface for the server (`wlr-layer-shell` namespace), for example `"notifications"`.
    pub namespace: String,
    /// The output the surface belongs to; `None` lets the server choose.
    pub output: Option<OutputId>,
    pub layer: Layer,
    pub anchor: Anchor,
    /// Requested logical size; a zero extent asks the server to fill the anchored axis.
    pub size: (u32, u32),
    pub margin: Margin,
    /// `-1` ignores other surfaces' exclusive zones, `0` respects them (protocol text).
    pub exclusive_zone: i32,
    pub keyboard: KeyboardInteractivity,
}

/// The layer shell global (`zwlr_layer_shell_v1`).
pub struct LayerShell(ZwlrLayerShellV1);

impl LayerShell {
    /// Bind the global; `None` when the server does not offer version 4, which on-demand keyboard interactivity needs.
    pub fn bind(conn: &mut Connection) -> Option<LayerShell> {
        conn.bind_extension(4..=4, Quiet).map(LayerShell)
    }

    /// Create a layer surface. Its configures arrive as [`ShellEvent::Configure`], acknowledged by the transport;
    /// [`LayerSurface::configured`] gives the size to draw at.
    pub fn layer_surface(
        &self,
        conn: &mut Connection,
        config: &LayerSurfaceConfig,
    ) -> Result<LayerSurface, Error> {
        let output = match config.output {
            Some(id) => Some(conn.wl_output(id).ok_or(Error::UnknownOutput)?),
            None => None,
        };
        let mut proxy = None;
        let surface = conn.external_surface(|wl, qh, id| {
            let layer = self.0.get_layer_surface(
                wl,
                output.as_ref(),
                config.layer,
                config.namespace.clone(),
                qh,
                OfSurface(id),
            );
            layer.set_size(config.size.0, config.size.1);
            layer.set_anchor(config.anchor);
            layer.set_margin(
                config.margin.top,
                config.margin.right,
                config.margin.bottom,
                config.margin.left,
            );
            layer.set_exclusive_zone(config.exclusive_zone);
            layer.set_keyboard_interactivity(config.keyboard);
            proxy = Some(layer.clone());
            LayerRole(layer)
        });
        Ok(LayerSurface {
            surface,
            proxy: proxy.expect("the role was assigned"),
            requested: config.size,
        })
    }
}

/// A layer surface's role object and the size it asked for. Destroying the surface with the transport destroys it.
pub struct LayerSurface {
    surface: SurfaceId,
    proxy: ZwlrLayerSurfaceV1,
    requested: (u32, u32),
}

impl LayerSurface {
    pub fn surface(&self) -> SurfaceId {
        self.surface
    }

    /// The size to draw at for a configure of `sent`: an extent the server left at 0 is the requested one. Fails when
    /// both left it at 0.
    pub fn configured(&self, sent: (u32, u32)) -> Result<(u32, u32), Error> {
        configured_size(sent, self.requested)
    }

    /// Change the keyboard interactivity; applies with the next commit.
    pub fn set_keyboard_interactivity(&self, mode: KeyboardInteractivity) {
        self.proxy.set_keyboard_interactivity(mode);
    }

    /// Change the distance from the anchored edge that other surfaces keep clear; applies with the next commit.
    pub fn set_exclusive_zone(&self, zone: i32) {
        self.proxy.set_exclusive_zone(zone);
    }

    /// Change the distance from the anchored edges; applies with the next commit.
    pub fn set_margin(&self, margin: Margin) {
        self.proxy
            .set_margin(margin.top, margin.right, margin.bottom, margin.left);
    }

    /// Move or resize: the edges it is anchored to and its size (0 across an axis anchored to both edges); applies with
    /// the next commit, after which the server configures it.
    pub fn set_placement(&mut self, anchor: Anchor, size: (u32, u32)) {
        self.proxy.set_anchor(anchor);
        self.proxy.set_size(size.0, size.1);
        self.requested = size;
    }
}

fn configured_size(sent: (u32, u32), requested: (u32, u32)) -> Result<(u32, u32), Error> {
    let pick = |sent: u32, requested: u32| if sent == 0 { requested } else { sent };
    match (pick(sent.0, requested.0), pick(sent.1, requested.1)) {
        (0, _) | (_, 0) => Err(Error::Unsized),
        size => Ok(size),
    }
}

/// User data of a layer or lock surface: the transport's id of it.
struct OfSurface(SurfaceId);

/// The role of a layer surface.
struct LayerRole(ZwlrLayerSurfaceV1);

impl ExternalRole for LayerRole {
    fn ack_configure(&self, serial: u32) {
        self.0.ack_configure(serial);
    }

    fn destroy(&self) {
        self.0.destroy();
    }

    fn adopt_popup(&self, popup: &XdgPopup) -> Result<(), cupertino_wayland_client::Error> {
        self.0.get_popup(popup);
        Ok(())
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, OfSurface> for State {
    fn event(
        state: &mut State,
        _: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        surface: &OfSurface,
        _: &WlConnection,
        _: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                let surface = surface.0;
                state.configure_external(surface, serial, (width, height));
                state.emit(ShellEvent::Configure {
                    surface,
                    size: (width, height),
                });
            }
            zwlr_layer_surface_v1::Event::Closed => state.emit(ShellEvent::Closed(surface.0)),
            _ => {}
        }
    }
}

/// The session lock global (`ext_session_lock_manager_v1`).
pub struct SessionLockManager {
    proxy: ExtSessionLockManagerV1,
    generations: u32,
}

impl SessionLockManager {
    /// Bind the global; `None` when the server does not offer it.
    pub fn bind(conn: &mut Connection) -> Option<SessionLockManager> {
        conn.bind_extension(1..=1, Quiet)
            .map(|proxy| SessionLockManager {
                proxy,
                generations: 0,
            })
    }

    /// Ask the server to lock the session. It answers with [`ShellEvent::Lock`] of the new lock's generation.
    pub fn lock(&mut self, conn: &Connection) -> SessionLock {
        self.generations += 1;
        let generation = self.generations;
        SessionLock {
            proxy: self
                .proxy
                .lock(&conn.queue_handle(), LockGeneration(generation)),
            generation,
        }
    }
}

/// A session lock (`ext_session_lock_v1`). End it with [`SessionLock::unlock`] once confirmed, else with
/// [`SessionLock::destroy`].
pub struct SessionLock {
    proxy: ExtSessionLockV1,
    generation: u32,
}

impl SessionLock {
    /// Tells this lock's events from those of the locks it replaced.
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Create the lock surface of `output`. The compositor configures it unasked; it must not commit before its first
    /// configure, which arrives as [`ShellEvent::Configure`].
    pub fn lock_surface(
        &self,
        conn: &mut Connection,
        output: OutputId,
    ) -> Result<SurfaceId, Error> {
        let output = conn.wl_output(output).ok_or(Error::UnknownOutput)?;
        Ok(conn.external_surface(|wl, qh, id| {
            LockRole(self.proxy.get_lock_surface(wl, &output, qh, OfSurface(id)))
        }))
    }

    /// Tell the server to unlock; only after it confirmed the lock.
    pub fn unlock(self) {
        self.proxy.unlock_and_destroy();
    }

    /// Drop a lock the server finished or has not confirmed.
    pub fn destroy(self) {
        self.proxy.destroy();
    }
}

/// User data of a session lock: its generation.
struct LockGeneration(u32);

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
        state.emit(ShellEvent::Lock {
            generation: lock.0,
            locked,
        });
    }
}

/// A lock surface, which the compositor configures unasked and which must not commit before its first ack.
struct LockRole(ExtSessionLockSurfaceV1);

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

impl Dispatch<ExtSessionLockSurfaceV1, OfSurface> for State {
    fn event(
        state: &mut State,
        _: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        surface: &OfSurface,
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
                return state.fail(cupertino_wayland_client::Error::Protocol(
                    "lock surface configured without a size",
                ));
            }
            let surface = surface.0;
            state.configure_external(surface, serial, (width, height));
            state.emit(ShellEvent::Configure {
                surface,
                size: (width, height),
            });
        }
    }
}

/// The idle notification global (`ext_idle_notifier_v1`).
pub struct IdleNotifier(ExtIdleNotifierV1);

impl IdleNotifier {
    /// Bind the global; `None` when the server does not offer it.
    pub fn bind(conn: &mut Connection) -> Option<IdleNotifier> {
        conn.bind_extension(1..=2, Quiet).map(IdleNotifier)
    }

    /// Report [`ShellEvent::Idle`] when `seat` has been idle for `millis`, honouring idle inhibitors, and when input
    /// resumes, while the result lives.
    pub fn watch(&self, conn: &Connection, seat: &WlSeat, millis: u32) -> IdleWatch {
        IdleWatch(
            self.0
                .get_idle_notification(millis, seat, &conn.queue_handle(), Quiet),
        )
    }
}

/// An idle notification; dropping it stops the events.
pub struct IdleWatch(ExtIdleNotificationV1);

impl Drop for IdleWatch {
    fn drop(&mut self) {
        self.0.destroy();
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
            ext_idle_notification_v1::Event::Idled => state.emit(ShellEvent::Idle(true)),
            ext_idle_notification_v1::Event::Resumed => state.emit(ShellEvent::Idle(false)),
            _ => {}
        }
    }
}

/// The private shell global (`cupertino_shell_v1`), offered only to the shell processes the server started.
pub struct Shell(CupertinoShellV1);

impl Shell {
    /// Bind the global; `None` when the server does not offer it to this client.
    pub fn bind(conn: &mut Connection) -> Option<Shell> {
        // version 2 adds the display options of the session object, version 3 the screen capture state of the menu bar,
        // version 5 moves the backdrops and the title bar style to the app effects, version 6 adds the menu bar's session actions,
        // version 10 the menu bar's window list, shortcuts and default-menu commands, version 11 its edit request,
        // version 12 its Help search shortcut, version 13 its force_quit_app request, version 14 its backdrop lightness,
        // version 15 the enabled controls of the active window, version 16 whether it is in full screen,
        // version 17 the dock's window icons, version 18 the dock's engage event, version 19 its resident applications,
        // version 20 the session object's activate_app, version 21 the menu bar's backdrop bands
        conn.bind_extension(9..=21, Quiet).map(Shell)
    }

    /// Observe the session: its appearance and display options arrive as
    /// [`cupertino_protocol_app::AppEvent`], its lock state and the user's requests as [`ShellEvent`]s, now and on
    /// every change while the observer lives.
    pub fn session(&self, conn: &Connection) -> Session {
        Session(self.0.get_session(&conn.queue_handle(), Quiet))
    }

    /// Give the layer surface `surface` the menu-bar role of `output`. The server then sends the active application
    /// and its menus as [`ShellEvent::MenuBar`], the first group right away.
    pub fn menu_bar(
        &self,
        conn: &Connection,
        surface: SurfaceId,
        output: OutputId,
    ) -> Result<MenuBar, Error> {
        let wl = conn.wl_surface(surface)?;
        let output = conn.wl_output(output).ok_or(Error::UnknownOutput)?;
        let data = RoleData {
            surface,
            group: Mutex::new(MenuBarGroup::new(self.0.version())),
        };
        Ok(MenuBar::new(self.0.get_menu_bar(
            &wl,
            &output,
            &conn.queue_handle(),
            data,
        )))
    }

    /// Give the layer surface `surface` the dock role of `output`. The server then lists the windows of the session
    /// and reports changes as [`ShellEvent::Dock`], the first group right away.
    pub fn dock(
        &self,
        conn: &Connection,
        surface: SurfaceId,
        output: OutputId,
    ) -> Result<Dock, Error> {
        let wl = conn.wl_surface(surface)?;
        let output = conn.wl_output(output).ok_or(Error::UnknownOutput)?;
        let data = RoleData {
            surface,
            group: Mutex::default(),
        };
        Ok(Dock::new(self.0.get_dock(
            &wl,
            &output,
            &conn.queue_handle(),
            data,
        )))
    }
}

/// A session observer; dropping it stops the events.
pub struct Session(CupertinoSessionV1);

/// Shell version of `cupertino_session_v1.activate_app`.
const ACTIVATE_APP_SINCE: u32 = 20;

/// What the server found for [`Session::activate_app`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppActivation {
    /// The application showed a window and was brought forward, as a click on its dock tile brings it.
    Activated,
    /// A process of the application runs but shows no window: it may be asked for one.
    Resident,
    /// No process of the application runs.
    NotRunning,
}

impl Session {
    /// Bring the running application of desktop file ID `desktop_id` forward the way the dock's `activate_app` does;
    /// the answer arrives as [`ShellEvent::AppActivation`] with `serial`. False when the server is older than shell
    /// version 20 and cannot be asked (nothing is sent).
    pub fn activate_app(&self, serial: u32, desktop_id: &str) -> bool {
        if self.0.version() < ACTIVATE_APP_SINCE {
            return false;
        }
        self.0.activate_app(serial, desktop_id.to_owned());
        true
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.0.destroy();
    }
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
        let event = match event {
            cupertino_session_v1::Event::Appearance { dark } => {
                return report_appearance(state, dark)
            }
            cupertino_session_v1::Event::DisplayOptions { options } => {
                return match options.into_result() {
                    Ok(bits) => report_display_options(state, bits.bits()),
                    Err(_) => state.fail(cupertino_wayland_client::Error::Protocol(
                        "display options with reserved bits",
                    )),
                };
            }
            cupertino_session_v1::Event::Locked => ShellEvent::SessionLock { locked: true },
            cupertino_session_v1::Event::Unlocked => ShellEvent::SessionLock { locked: false },
            cupertino_session_v1::Event::LockRequested { explicit } => ShellEvent::LockRequested {
                explicit: explicit != 0,
            },
            cupertino_session_v1::Event::LogOutRequested => ShellEvent::LogOutRequested,
            cupertino_session_v1::Event::AppActivation { serial, result } => {
                let result = match result.into_result() {
                    Ok(cupertino_session_v1::Activation::Activated) => AppActivation::Activated,
                    Ok(cupertino_session_v1::Activation::Resident) => AppActivation::Resident,
                    // an answer newer than this client knows: it launches as without a window
                    _ => AppActivation::NotRunning,
                };
                ShellEvent::AppActivation { serial, result }
            }
            _ => return,
        };
        state.emit(event);
    }
}

/// User data of a menu-bar or dock role: its surface and the group of events being received.
struct RoleData<G> {
    surface: SurfaceId,
    group: Mutex<G>,
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
            state.emit(ShellEvent::MenuBar {
                surface: data.surface,
                update: Box::new(update),
            });
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
                Some(id) => group.switcher_output(id),
                None => state.fail(cupertino_wayland_client::Error::Protocol(
                    "switcher_output names an output this client has not bound",
                )),
            };
        }
        match group.push(event) {
            Ok(Some(update)) => state.emit(ShellEvent::Dock {
                surface: data.surface,
                update,
            }),
            Ok(None) => {}
            Err(what) => state.fail(cupertino_wayland_client::Error::Protocol(what)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extent_left_at_0_is_the_requested_one() {
        assert_eq!(configured_size((0, 0), (200, 200)).unwrap(), (200, 200));
        assert_eq!(configured_size((1920, 0), (0, 30)).unwrap(), (1920, 30));
        assert_eq!(configured_size((640, 480), (200, 200)).unwrap(), (640, 480));
    }

    #[test]
    fn an_extent_neither_side_sized_fails() {
        assert!(matches!(
            configured_size((0, 30), (0, 30)),
            Err(Error::Unsized)
        ));
        assert!(matches!(
            configured_size((1920, 0), (0, 0)),
            Err(Error::Unsized)
        ));
    }
}
