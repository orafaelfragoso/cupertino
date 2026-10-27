//! Connection, globals, surfaces and the event loop primitive.

use crate::backdrop::Backdrop;
use crate::clipboard::Clipboard;
use crate::dock::{Dock, GroupBuilder as DockGroup};
use crate::event::{ActivationRequest, Event, PointerTracker};
use crate::keyboard::{Modifiers, Repeat, Translator};
use crate::menu_bar::{GroupBuilder, MenuBar};
use crate::output::{GeometryBuilder, OutputId, OutputInfo};
use crate::popup::PopupGeometry;
use crate::scale::Scale;
use crate::shm::{BufferLayout, BufferPool, BufferTag, Damage, PixelRect};
use crate::{Anchor, ClientError, KeyboardInteractivity, Layer};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_geometry::{Point, Rect};
use cupertino_protocols::client::cupertino_backdrop_v1::CupertinoBackdropV1;
use cupertino_protocols::client::cupertino_dock_v1::{self, CupertinoDockV1};
use cupertino_protocols::client::cupertino_menu_bar_v1::{self, CupertinoMenuBarV1};
use cupertino_protocols::client::cupertino_session_v1::{self, CupertinoSessionV1, DisplayOption};
use cupertino_protocols::client::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_protocols::client::scroll::cupertino_pointer_scroll_v1::{
    self, CupertinoPointerScrollV1,
};
use cupertino_protocols::client::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use rustix::event::{poll, PollFd, PollFlags};
use std::io::{ErrorKind, Read, Write};
use std::ops::RangeInclusive;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tiny_skia::{Color, Pixmap};
use wayland_client::backend::{ObjectId, WaylandError};
use wayland_client::globals::{registry_queue_init, BindError, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_keyboard::{self, KeyState, WlKeyboard};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_pointer::{self, Axis, ButtonState, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, Capability, WlSeat};
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
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
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    self, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::{
    self, XdgActivationTokenV1,
};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::{
    self, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::shell::client::xdg_positioner::{
    Anchor as PositionerAnchor, Gravity, XdgPositioner,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, ZwlrLayerSurfaceV1,
};

/// Handle of a surface of a [`Client`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub(crate) u32);

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

/// Everything an application window is created with ([`Client::toplevel`]).
#[derive(Clone, Debug)]
pub struct ToplevelConfig {
    /// Identifier the server groups the windows of an application by (`xdg_toplevel.set_app_id`), normally the
    /// name of its desktop entry without `.desktop`.
    pub app_id: String,
    /// Shown in the title bar; change it with [`Client::set_title`].
    pub title: String,
    /// Logical size of the content until the server asks for another.
    pub size: (u32, u32),
    /// Smallest logical size the content can be laid out in.
    pub min_size: (u32, u32),
    /// How the server decorates the window.
    pub titlebar: TitlebarStyle,
}

/// The title bar the server draws on an application window (`cupertino_shell_v1.set_titlebar_style`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitlebarStyle {
    /// A title bar above the surface, with the window's title and the window control buttons.
    Standard,
    /// The surface fills the window and the server draws only the window control buttons over its top-left corner.
    /// The client draws the toolbar strip and the title, keeps the control buttons' area free, and moves the window
    /// itself ([`Client::toplevel_move`], [`Client::toplevel_toggle_maximized`]).
    Unified,
}

/// Where a session lock requested with [`Client::lock_session`] stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockState {
    /// Asked for; the server has not confirmed it.
    Requested,
    /// The server confirmed: only the lock surfaces are shown and only they take input.
    Locked,
    /// The server refused the lock, or ended it. Request again with [`Client::lock_session`].
    Finished,
}

/// What the pointer looks like over the application's surfaces ([`Client::set_cursor`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorLook {
    /// No cursor: the application draws its own.
    Hidden,
    /// The server's default arrow (`wp_cursor_shape_v1`).
    Arrow,
}

/// Wakes the thread blocked in [`Client::wait`]; safe to use from any thread.
#[derive(Clone)]
pub struct Waker(Arc<UnixStream>);

impl Waker {
    /// Make the next or current [`Client::wait`] return.
    pub fn wake(&self) {
        match (&*self.0).write(&[1]) {
            // a full channel already holds a pending wake-up
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => panic!("wake channel broken: {e}"),
        }
    }
}

struct Output {
    name: u32,
    wl: WlOutput,
    xdg: ZxdgOutputV1,
    scale: Scale,
    geometry: GeometryBuilder,
}

/// What gives a surface its place on screen.
enum Role {
    Layer(ZwlrLayerSurfaceV1),
    Popup(Popup),
    /// Covers an output while the session is locked.
    Lock(ExtSessionLockSurfaceV1),
    /// An application window.
    Toplevel(Toplevel),
}

/// What the `states` of an `xdg_toplevel.configure` say about the window.
#[derive(Clone, Copy, Default)]
struct ToplevelStates {
    activated: bool,
    maximized: bool,
}

/// An `xdg_toplevel` whose window control buttons the server draws.
struct Toplevel {
    xdg: XdgSurface,
    toplevel: XdgToplevel,
    decoration: ZxdgToplevelDecorationV1,
    /// Suggested size (zero extents leave it to the client) and states from the `xdg_toplevel.configure` that
    /// the next `xdg_surface.configure` completes.
    pending: Option<((u32, u32), ToplevelStates)>,
    /// The states as of the last completed configure.
    states: ToplevelStates,
}

struct SessionLock {
    proxy: ExtSessionLockV1,
    state: LockState,
}

struct Popup {
    xdg: XdgSurface,
    popup: XdgPopup,
    parent: SurfaceId,
    /// Size from the `xdg_popup.configure` that the next `xdg_surface.configure` completes.
    configured: Option<(u32, u32)>,
}

struct Surface {
    id: SurfaceId,
    wl: WlSurface,
    role: Role,
    /// Present when the server offers fractional scale and viewporter.
    viewport: Option<WpViewport>,
    fractional: Option<WpFractionalScaleV1>,
    fractional_scale: Option<Scale>,
    menu_group: GroupBuilder,
    dock_group: DockGroup,
    entered: Vec<ObjectId>,
    requested_size: (u32, u32),
    /// Logical size from the last configure.
    size: Option<(u32, u32)>,
    /// Scale last reported to the application.
    reported_scale: Scale,
    pool: Option<BufferPool>,
    pixmap: Option<Pixmap>,
    /// The scale the last frame was drawn at: a frame at another one is drawn whole.
    drawn_scale: Option<Scale>,
    frame_pending: bool,
    wants_redraw: bool,
}

/// What the server last said about the session (`cupertino_session_v1`).
struct Session {
    /// Kept alive: the server stops sending when the object is destroyed.
    _observer: CupertinoSessionV1,
    dark: bool,
    display: DisplayOptions,
    locked: bool,
    /// The state at connect is read; later changes also queue an [`Event`].
    settled: bool,
}

/// Dispatch state of the connection.
pub(crate) struct State {
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    shell: CupertinoShellV1,
    session: Session,
    activation: XdgActivationV1,
    wm_base: XdgWmBase,
    decoration_manager: Option<ZxdgDecorationManagerV1>,
    xdg_output: ZxdgOutputManagerV1,
    scaling: Option<(WpFractionalScaleManagerV1, WpViewporter)>,
    seat: WlSeat,
    session_lock_manager: Option<ExtSessionLockManagerV1>,
    idle_notifier: Option<ExtIdleNotifierV1>,
    session_lock: Option<SessionLock>,
    idle_notification: Option<ExtIdleNotificationV1>,
    /// The seat went idle since [`Client::take_idle`] last ran.
    idled: bool,
    pointer: Option<WlPointer>,
    /// Offered by the server: the scroll count of the pointer's scrolls (`cupertino_scroll_v1`).
    scroll_counts: Option<CupertinoScrollV1>,
    /// Kept alive: the server stops sending when the object is destroyed.
    pointer_scroll: Option<CupertinoPointerScrollV1>,
    /// Serial of the latest `wl_pointer.enter`, which a cursor change must carry.
    pointer_enter_serial: Option<u32>,
    cursor_shape: Option<WpCursorShapeManagerV1>,
    /// Created by the first [`Client::set_cursor`] that needs a shape.
    cursor_device: Option<WpCursorShapeDeviceV1>,
    keyboard: Option<WlKeyboard>,
    data_device_manager: Option<WlDataDeviceManager>,
    pub(crate) clipboard: Clipboard,
    outputs: Vec<Output>,
    surfaces: Vec<Surface>,
    next_surface: u32,
    next_generation: u64,
    next_request: u32,
    next_reposition: u32,
    pointer_tracker: PointerTracker,
    key_focus: Option<SurfaceId>,
    translator: Option<Translator>,
    repeat: Repeat,
    last_serial: Option<u32>,
    events: Vec<Event>,
    fault: Option<ClientError>,
}

impl State {
    fn surface_mut(&mut self, id: SurfaceId) -> Result<&mut Surface, ClientError> {
        self.surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(ClientError::UnknownSurface)
    }

    fn surface_of(&mut self, wl: &WlSurface) -> Option<&mut Surface> {
        let id = wl.data::<SurfaceId>()?;
        self.surfaces.iter_mut().find(|s| s.id == *id)
    }

    /// Scale a surface is drawn at: the preferred fractional scale with the viewporter path, else
    /// the largest integer scale of the outputs it is on.
    fn effective_scale(&self, s: &Surface) -> Scale {
        if s.viewport.is_some() {
            return s.fractional_scale.unwrap_or(Scale::ONE);
        }
        self.outputs
            .iter()
            .filter(|o| s.entered.contains(&o.wl.id()))
            .map(|o| o.scale)
            .max()
            .unwrap_or(Scale::ONE)
    }

    /// Report scale changes and surfaces that can take their requested frame.
    fn update(&mut self) {
        let scales: Vec<(usize, Scale)> = self
            .surfaces
            .iter()
            .enumerate()
            .filter(|(_, s)| s.size.is_some())
            .map(|(i, s)| (i, self.effective_scale(s)))
            .collect();
        for (i, scale) in scales {
            let s = &mut self.surfaces[i];
            if s.reported_scale != scale {
                s.reported_scale = scale;
                s.wants_redraw = true;
                self.events.push(Event::ScaleChanged {
                    surface: s.id,
                    scale,
                });
            }
        }
        for s in &mut self.surfaces {
            let buffer_free = s.pool.as_ref().is_none_or(BufferPool::has_free_slot);
            if s.wants_redraw && s.size.is_some() && !s.frame_pending && buffer_free {
                s.wants_redraw = false;
                self.events.push(Event::Draw(s.id));
            }
        }
    }

    /// Queue the event of a key on the focused surface, translated by the server's keymap.
    fn push_key(&mut self, surface: SurfaceId, key: u32, pressed: bool) {
        let Some(translator) = &self.translator else {
            return self.fail(ClientError::Protocol("key event before the keymap"));
        };
        let t = translator.translate(key);
        self.events.push(Event::Key {
            surface,
            key,
            pressed,
            mods: t.mods,
            character: t.character,
        });
    }

    /// Queue the repeats that came due.
    fn repeat_keys(&mut self, now: Instant) {
        let Some(surface) = self.key_focus else {
            return;
        };
        for key in self.repeat.due(now) {
            self.push_key(surface, key, true);
        }
    }

    fn fail(&mut self, e: ClientError) {
        self.fault.get_or_insert(e);
    }
}

/// Connection to the server and the surfaces of one shell process.
pub struct Client {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    wake: UnixStream,
    waker: Waker,
}

fn bind_required<I>(
    globals: &GlobalList,
    qh: &QueueHandle<State>,
    versions: RangeInclusive<u32>,
) -> Result<I, ClientError>
where
    I: Proxy + 'static,
    State: Dispatch<I, ()>,
{
    bind_optional(globals, qh, versions)?.ok_or(ClientError::MissingGlobal(I::interface().name))
}

fn bind_optional<I>(
    globals: &GlobalList,
    qh: &QueueHandle<State>,
    versions: RangeInclusive<u32>,
) -> Result<Option<I>, ClientError>
where
    I: Proxy + 'static,
    State: Dispatch<I, ()>,
{
    match globals.bind(qh, versions.clone(), ()) {
        Ok(p) => Ok(Some(p)),
        Err(BindError::NotPresent) => Ok(None),
        Err(BindError::UnsupportedVersion) => Err(ClientError::GlobalVersion {
            interface: I::interface().name,
            minimum: *versions.start(),
        }),
    }
}

impl Client {
    /// Connect to the display named by `WAYLAND_DISPLAY` and bind the globals. Requires the
    /// compositor, shm, seat, layer shell, `cupertino_shell_v1`, `xdg_activation_v1`, `xdg_wm_base` (version 3 or newer) and
    /// `zxdg_output_manager_v1` globals; fractional scale and viewporter are used when both are offered. The session
    /// lock, idle notification and decoration globals are optional here: [`Client::lock_session`],
    /// [`Client::watch_idle`] and [`Client::toplevel`] fail with [`ClientError::MissingGlobal`] when the server lacks them.
    pub fn connect() -> Result<Client, ClientError> {
        Self::from_connection(Connection::connect_to_env().map_err(ClientError::Connect)?)
    }

    /// [`Client::connect`] over an established connection.
    pub fn from_connection(conn: Connection) -> Result<Client, ClientError> {
        let (globals, mut queue) =
            registry_queue_init::<State>(&conn).map_err(ClientError::Registry)?;
        let qh = queue.handle();
        let scaling = match (
            bind_optional(&globals, &qh, 1..=1)?,
            bind_optional(&globals, &qh, 1..=1)?,
        ) {
            (Some(manager), Some(viewporter)) => Some((manager, viewporter)),
            _ => None,
        };
        // version 2 adds the display options of the session object, version 3 the screen capture state of the menu bar,
        // version 4 the title bar style of application windows
        let shell: CupertinoShellV1 = bind_required(&globals, &qh, 3..=4)?;
        let observer = shell.get_session(&qh, ());
        let seat: WlSeat = bind_required(&globals, &qh, 1..=7)?;
        let data_device_manager: Option<WlDataDeviceManager> = bind_optional(&globals, &qh, 1..=3)?;
        let clipboard = Clipboard::new(data_device_manager.as_ref(), &seat, &qh);
        let mut state = State {
            compositor: bind_required(&globals, &qh, 4..=6)?,
            shm: bind_required(&globals, &qh, 1..=1)?,
            layer_shell: bind_required(&globals, &qh, 1..=4)?,
            shell,
            session: Session {
                _observer: observer,
                dark: false,
                display: DisplayOptions::default(),
                locked: false,
                settled: false,
            },
            activation: bind_required(&globals, &qh, 1..=1)?,
            // version 3 added `xdg_popup.reposition`
            wm_base: bind_required(&globals, &qh, 3..=6)?,
            decoration_manager: bind_optional(&globals, &qh, 1..=1)?,
            // version 2 announces every change with a `done` event; version 3 moved that to wl_output
            xdg_output: bind_required(&globals, &qh, 1..=2)?,
            scaling,
            seat,
            session_lock_manager: bind_optional(&globals, &qh, 1..=1)?,
            idle_notifier: bind_optional(&globals, &qh, 1..=2)?,
            session_lock: None,
            idle_notification: None,
            idled: false,
            pointer: None,
            scroll_counts: bind_optional(&globals, &qh, 1..=1)?,
            pointer_scroll: None,
            pointer_enter_serial: None,
            cursor_shape: bind_optional(&globals, &qh, 1..=1)?,
            cursor_device: None,
            keyboard: None,
            data_device_manager,
            clipboard,
            outputs: Vec::new(),
            surfaces: Vec::new(),
            next_surface: 0,
            next_generation: 0,
            next_request: 0,
            next_reposition: 0,
            pointer_tracker: PointerTracker::default(),
            key_focus: None,
            translator: None,
            repeat: Repeat::default(),
            last_serial: None,
            events: Vec::new(),
            fault: None,
        };
        let registry = globals.registry();
        for g in globals
            .contents()
            .clone_list()
            .iter()
            .filter(|g| g.interface == "wl_output")
        {
            state.bind_output(registry, &qh, g.name, g.version);
        }
        // outputs and seat capabilities report before the first surface exists
        queue.roundtrip(&mut state)?;
        state.session.settled = true;
        let (wake, writer) = UnixStream::pair().map_err(ClientError::Io)?;
        wake.set_nonblocking(true).map_err(ClientError::Io)?;
        writer.set_nonblocking(true).map_err(ClientError::Io)?;
        Ok(Client {
            conn,
            queue,
            state,
            wake,
            waker: Waker(Arc::new(writer)),
        })
    }

    /// A handle that interrupts [`Client::wait`] from other threads.
    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// The outputs the server currently offers; [`Event::OutputsChanged`] reports changes.
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.state
            .outputs
            .iter()
            .map(|o| OutputInfo {
                id: OutputId(o.name),
                geometry: o.geometry.geometry(),
            })
            .collect()
    }

    /// The outputs a surface is on, in the order the server put it there (`wl_surface.enter` and
    /// `leave`). A layer surface created without an output finds the one the server chose here by
    /// its first [`Event::Configured`].
    pub fn surface_outputs(&mut self, id: SurfaceId) -> Result<Vec<OutputId>, ClientError> {
        let entered = self.state.surface_mut(id)?.entered.clone();
        Ok(entered
            .iter()
            .filter_map(|wl| {
                self.state
                    .outputs
                    .iter()
                    .find(|o| o.wl.id() == *wl)
                    .map(|o| OutputId(o.name))
            })
            .collect())
    }

    /// Create a layer surface. It is mapped and sized by the first [`Event::Configured`], which
    /// is followed by [`Event::Draw`].
    pub fn layer_surface(&mut self, config: &LayerSurfaceConfig) -> Result<SurfaceId, ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let output = match config.output {
            Some(id) => Some(
                st.outputs
                    .iter()
                    .find(|o| o.name == id.0)
                    .ok_or(ClientError::UnknownOutput)?
                    .wl
                    .clone(),
            ),
            None => None,
        };
        let (id, wl, viewport, fractional) = st.new_surface(&qh);
        let layer = st.layer_shell.get_layer_surface(
            &wl,
            output.as_ref(),
            config.layer,
            config.namespace.clone(),
            &qh,
            id,
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
        // the initial commit carries no buffer; the server answers with a configure
        wl.commit();
        st.surfaces.push(Surface::new(
            id,
            wl,
            Role::Layer(layer),
            viewport,
            fractional,
            config.size,
        ));
        Ok(id)
    }

    /// Create an application window. The server draws its title bar and window control buttons
    /// (`zxdg_decoration_manager_v1`, server-side mode); the surface is the content below them. It is mapped and
    /// sized by the first [`Event::Configured`]; resizes and focus changes ([`Client::toplevel_activated`]) arrive
    /// the same way, scrolling over it arrives as [`PointerEvent::Scroll`] (with the scroll count when the server offers it), and the request to close it is
    /// [`Event::Closed`]. Fails when the server lacks the decoration global or a size does not fit the protocol.
    pub fn toplevel(&mut self, config: &ToplevelConfig) -> Result<SurfaceId, ClientError> {
        let min_width = i32::try_from(config.min_size.0).map_err(|_| ClientError::InvalidShape)?;
        let min_height = i32::try_from(config.min_size.1).map_err(|_| ClientError::InvalidShape)?;
        if config.size.0 == 0 || config.size.1 == 0 {
            return Err(ClientError::InvalidShape);
        }
        let qh = self.queue.handle();
        let st = &mut self.state;
        let manager = st
            .decoration_manager
            .clone()
            .ok_or(ClientError::MissingGlobal(
                ZxdgDecorationManagerV1::interface().name,
            ))?;
        let (id, wl, viewport, fractional) = st.new_surface(&qh);
        let xdg = st.wm_base.get_xdg_surface(&wl, &qh, id);
        let toplevel = xdg.get_toplevel(&qh, id);
        toplevel.set_app_id(config.app_id.clone());
        toplevel.set_title(config.title.clone());
        toplevel.set_min_size(min_width, min_height);
        let decoration = manager.get_toplevel_decoration(&toplevel, &qh, id);
        decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
        if config.titlebar == TitlebarStyle::Unified {
            if st.shell.version() < 4 {
                return Err(ClientError::GlobalVersion {
                    interface: CupertinoShellV1::interface().name,
                    minimum: 4,
                });
            }
            st.shell
                .set_titlebar_style(&wl, cupertino_shell_v1::TitlebarStyle::Unified);
        }
        // the initial commit carries no buffer; the server answers with a configure
        wl.commit();
        let role = Role::Toplevel(Toplevel {
            xdg,
            toplevel,
            decoration,
            pending: None,
            states: ToplevelStates::default(),
        });
        st.surfaces.push(Surface::new(
            id,
            wl,
            role,
            viewport,
            fractional,
            config.size,
        ));
        Ok(id)
    }

    /// Change the title of an application window.
    pub fn set_title(&mut self, id: SurfaceId, title: &str) -> Result<(), ClientError> {
        match &self.state.surface_mut(id)?.role {
            Role::Toplevel(t) => t.toplevel.set_title(title.to_string()),
            Role::Layer(_) | Role::Popup(_) | Role::Lock(_) => {
                return Err(ClientError::NotToplevel)
            }
        }
        Ok(())
    }

    /// Whether an application window has the focus, as of its last [`Event::Configured`].
    pub fn toplevel_activated(&mut self, id: SurfaceId) -> Result<bool, ClientError> {
        match &self.state.surface_mut(id)?.role {
            Role::Toplevel(t) => Ok(t.states.activated),
            Role::Layer(_) | Role::Popup(_) | Role::Lock(_) => Err(ClientError::NotToplevel),
        }
    }

    /// Start moving the window with the pointer (`xdg_toplevel.move`), as the press that just arrived: call it on a
    /// left-button press over a part of a [`TitlebarStyle::Unified`] window that has no control. The server moves the
    /// window until the button is released.
    pub fn toplevel_move(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        let serial = self.state.last_serial.ok_or(ClientError::NoInputSerial)?;
        let st = &mut self.state;
        let seat = st.seat.clone();
        match &st.surface_mut(id)?.role {
            Role::Toplevel(t) => t.toplevel._move(&seat, serial),
            Role::Layer(_) | Role::Popup(_) | Role::Lock(_) => {
                return Err(ClientError::NotToplevel)
            }
        }
        Ok(())
    }

    /// Zoom the window, or restore it when it is zoomed already (`xdg_toplevel.set_maximized` / `unset_maximized`),
    /// as of its last [`Event::Configured`]: the action of a double click on the toolbar of a
    /// [`TitlebarStyle::Unified`] window.
    pub fn toplevel_toggle_maximized(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        match &self.state.surface_mut(id)?.role {
            Role::Toplevel(t) if t.states.maximized => t.toplevel.unset_maximized(),
            Role::Toplevel(t) => t.toplevel.set_maximized(),
            Role::Layer(_) | Role::Popup(_) | Role::Lock(_) => {
                return Err(ClientError::NotToplevel)
            }
        }
        Ok(())
    }

    /// Set the pointer's look over the surface it entered last; call it on [`PointerEvent::Enter`](crate::PointerEvent),
    /// because the server keeps the look of the latest request until another surface sets its own. Fails without a
    /// pointer enter yet, and for [`CursorLook::Arrow`] when the server lacks `wp_cursor_shape_manager_v1`.
    pub fn set_cursor(&mut self, look: CursorLook) -> Result<(), ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let (Some(serial), Some(pointer)) = (st.pointer_enter_serial, st.pointer.as_ref()) else {
            return Err(ClientError::NoInputSerial);
        };
        match look {
            CursorLook::Hidden => pointer.set_cursor(serial, None, 0, 0),
            CursorLook::Arrow => {
                let manager = st.cursor_shape.as_ref().ok_or(ClientError::MissingGlobal(
                    WpCursorShapeManagerV1::interface().name,
                ))?;
                let device = st
                    .cursor_device
                    .get_or_insert_with(|| manager.get_pointer(pointer, &qh, ()));
                device.set_shape(serial, Shape::Default);
            }
        }
        Ok(())
    }

    /// Serial of the latest input event (pointer enter or button, keyboard enter or key), for requests that need one,
    /// such as setting the clipboard selection on a connection of the application's own.
    pub fn input_serial(&self) -> Option<u32> {
        self.state.last_serial
    }

    /// Put `text` on the clipboard, as the answer to the latest input event (a copy or cut key). The client serves it
    /// to pasting clients from [`Client::wait`] until another selection replaces it.
    pub fn set_clipboard_text(&mut self, text: String) -> Result<(), ClientError> {
        let serial = self.state.last_serial.ok_or(ClientError::NoInputSerial)?;
        let qh = self.queue.handle();
        self.state
            .clipboard
            .set_text(self.state.data_device_manager.as_ref(), text, serial, &qh)
    }

    /// The clipboard's text for a paste, `None` when it holds no text. Reading another client's selection waits up to
    /// a second for it.
    pub fn clipboard_text(&mut self) -> Result<Option<String>, ClientError> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.state.clipboard.text(&self.conn)
    }

    /// The modifiers held now, as of the last `wl_keyboard.modifiers` event; none before the keymap arrives. Pointer
    /// events carry no modifiers, so a click reads them here.
    pub fn modifiers(&self) -> Modifiers {
        self.state
            .translator
            .as_ref()
            .map_or(Modifiers::default(), Translator::modifiers)
    }

    /// Create a popup of `parent` (a layer surface, an application window or another popup) covering `rect`, given in
    /// parent-local logical points and rounded outwards to whole points. The popup takes a grab on
    /// the seat with the serial of the latest input event, so the server dismisses it
    /// ([`Event::PopupDone`]) when the user clicks elsewhere. It is mapped and sized by the first
    /// [`Event::Configured`]. Destroy a popup before its parent.
    pub fn popup(&mut self, parent: SurfaceId, rect: &Rect) -> Result<SurfaceId, ClientError> {
        let geometry = PopupGeometry::from_rect(rect)?;
        let serial = self.state.last_serial.ok_or(ClientError::NoInputSerial)?;
        let qh = self.queue.handle();
        let st = &mut self.state;
        let (parent_layer, parent_xdg) = match &st.surface_mut(parent)?.role {
            Role::Layer(layer) => (Some(layer.clone()), None),
            Role::Popup(p) => (None, Some(p.xdg.clone())),
            Role::Toplevel(t) => (None, Some(t.xdg.clone())),
            Role::Lock(_) => return Err(ClientError::NotLayerSurface),
        };
        let (id, wl, viewport, fractional) = st.new_surface(&qh);
        let positioner = st.wm_base.create_positioner(&qh, ());
        place(&positioner, &geometry);
        let xdg = st.wm_base.get_xdg_surface(&wl, &qh, id);
        let popup = xdg.get_popup(parent_xdg.as_ref(), &positioner, &qh, id);
        if let Some(layer) = &parent_layer {
            layer.get_popup(&popup);
        }
        popup.grab(&st.seat, serial);
        positioner.destroy();
        // the initial commit carries no buffer; the server answers with a configure
        wl.commit();
        let size = (geometry.width as u32, geometry.height as u32);
        st.surfaces.push(Surface::new(
            id,
            wl,
            Role::Popup(Popup {
                xdg,
                popup,
                parent,
                configured: None,
            }),
            viewport,
            fractional,
            size,
        ));
        Ok(id)
    }

    /// Move or resize a popup to `rect` (parent-local, as for [`Client::popup`]). The new size
    /// arrives as [`Event::Configured`].
    pub fn reposition_popup(&mut self, id: SurfaceId, rect: &Rect) -> Result<(), ClientError> {
        let geometry = PopupGeometry::from_rect(rect)?;
        let qh = self.queue.handle();
        let st = &mut self.state;
        let token = st.next_reposition;
        st.next_reposition += 1;
        let positioner = st.wm_base.create_positioner(&qh, ());
        place(&positioner, &geometry);
        match &st.surface_mut(id)?.role {
            Role::Popup(p) => p.popup.reposition(&positioner, token),
            Role::Layer(_) | Role::Lock(_) | Role::Toplevel(_) => {
                return Err(ClientError::NotPopup)
            }
        }
        positioner.destroy();
        Ok(())
    }

    /// Destroy a surface with its role objects. Its popups must be destroyed first. Destroy a
    /// surface's backdrops before the surface.
    pub fn destroy_surface(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        let st = &mut self.state;
        if st
            .surfaces
            .iter()
            .any(|s| matches!(&s.role, Role::Popup(p) if p.parent == id))
        {
            return Err(ClientError::HasPopups);
        }
        let index = st
            .surfaces
            .iter()
            .position(|s| s.id == id)
            .ok_or(ClientError::UnknownSurface)?;
        let s = st.surfaces.remove(index);
        match s.role {
            Role::Layer(layer) => layer.destroy(),
            Role::Popup(p) => {
                p.popup.destroy();
                p.xdg.destroy();
            }
            Role::Lock(lock_surface) => lock_surface.destroy(),
            Role::Toplevel(t) => {
                t.decoration.destroy();
                t.toplevel.destroy();
                t.xdg.destroy();
            }
        }
        if let Some(viewport) = s.viewport {
            viewport.destroy();
        }
        if let Some(fractional) = s.fractional {
            fractional.destroy();
        }
        s.wl.destroy();
        st.pointer_tracker.forget(id);
        if st.key_focus == Some(id) {
            st.key_focus = None;
        }
        Ok(())
    }

    /// Logical size of a configured surface.
    pub fn size(&mut self, id: SurfaceId) -> Result<(u32, u32), ClientError> {
        self.state
            .surface_mut(id)?
            .size
            .ok_or(ClientError::NotConfigured)
    }

    /// Ask for an [`Event::Draw`] as soon as the surface can take a frame.
    pub fn request_redraw(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        self.state.surface_mut(id)?.wants_redraw = true;
        Ok(())
    }

    /// Change the keyboard interactivity; applies with the next draw.
    pub fn set_keyboard_interactivity(
        &mut self,
        id: SurfaceId,
        mode: KeyboardInteractivity,
    ) -> Result<(), ClientError> {
        match &self.state.surface_mut(id)?.role {
            Role::Layer(layer) => layer.set_keyboard_interactivity(mode),
            Role::Popup(_) | Role::Lock(_) | Role::Toplevel(_) => {
                return Err(ClientError::NotLayerSurface)
            }
        }
        Ok(())
    }

    /// Change the distance from the anchored edge that other surfaces keep clear; applies with the
    /// next draw.
    pub fn set_exclusive_zone(&mut self, id: SurfaceId, zone: i32) -> Result<(), ClientError> {
        match &self.state.surface_mut(id)?.role {
            Role::Layer(layer) => layer.set_exclusive_zone(zone),
            Role::Popup(_) | Role::Lock(_) | Role::Toplevel(_) => {
                return Err(ClientError::NotLayerSurface)
            }
        }
        Ok(())
    }

    /// Move or resize a layer surface: the edges it is anchored to and its size (0 across an axis anchored to both
    /// edges); applies with the next draw, after which the server configures it.
    pub fn set_layer_placement(
        &mut self,
        id: SurfaceId,
        anchor: Anchor,
        size: (u32, u32),
    ) -> Result<(), ClientError> {
        match &self.state.surface_mut(id)?.role {
            Role::Layer(layer) => {
                layer.set_anchor(anchor);
                layer.set_size(size.0, size.1);
            }
            Role::Popup(_) | Role::Lock(_) | Role::Toplevel(_) => {
                return Err(ClientError::NotLayerSurface)
            }
        }
        Ok(())
    }

    /// Ask the server to lock the session. Create a lock surface for every output right away
    /// ([`Client::lock_surface`]); the server confirms with [`LockState::Locked`] in
    /// [`Client::lock_state`], which changes while [`Client::wait`] runs. Fails when the server lacks
    /// `ext_session_lock_manager_v1` or a lock that is not [`LockState::Finished`] exists.
    pub fn lock_session(&mut self) -> Result<(), ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let manager = st
            .session_lock_manager
            .as_ref()
            .ok_or(ClientError::MissingGlobal(
                ExtSessionLockManagerV1::interface().name,
            ))?;
        match &st.session_lock {
            Some(lock) if lock.state != LockState::Finished => {
                return Err(ClientError::SessionLockActive)
            }
            Some(lock) => lock.proxy.destroy(),
            None => {}
        }
        st.session_lock = Some(SessionLock {
            proxy: manager.lock(&qh, ()),
            state: LockState::Requested,
        });
        Ok(())
    }

    /// Whether Caps Lock is on, as of the last `wl_keyboard.modifiers` event; `false` before the keymap arrives.
    pub fn caps_lock(&self) -> bool {
        self.state
            .translator
            .as_ref()
            .is_some_and(Translator::caps_lock)
    }

    /// The state of the lock requested with [`Client::lock_session`]; `None` before the first request and after
    /// [`Client::unlock_session`].
    pub fn lock_state(&self) -> Option<LockState> {
        self.state.session_lock.as_ref().map(|lock| lock.state)
    }

    /// Whether the session is dark, as of the last `cupertino_session_v1.appearance`. The value at connect is read
    /// before [`Client::connect`] returns, so no process starts with a guess; later changes arrive as [`Event::Appearance`].
    pub fn is_dark(&self) -> bool {
        self.state.session.dark
    }

    /// The accessibility display options, as of the last `cupertino_session_v1.display_options`. The value at connect is
    /// read before [`Client::connect`] returns; later changes arrive as [`Event::DisplayOptions`].
    pub fn display_options(&self) -> DisplayOptions {
        self.state.session.display
    }

    /// Whether the server holds the session locked, as of the last `locked` or `unlocked` event. The value at connect is
    /// read before [`Client::connect`] returns; later changes arrive as [`Event::SessionLock`].
    pub fn session_locked(&self) -> bool {
        self.state.session.locked
    }

    /// Create the lock surface of `output`. It is sized by the first [`Event::Configured`], which is followed by
    /// [`Event::Draw`]; it takes keyboard and pointer input once the session is locked. One per output.
    pub fn lock_surface(&mut self, output: OutputId) -> Result<SurfaceId, ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let output = st
            .outputs
            .iter()
            .find(|o| o.name == output.0)
            .ok_or(ClientError::UnknownOutput)?
            .wl
            .clone();
        let lock = match &st.session_lock {
            Some(lock) if lock.state != LockState::Finished => lock.proxy.clone(),
            _ => return Err(ClientError::NoSessionLock),
        };
        let (id, wl, viewport, fractional) = st.new_surface(&qh);
        // no initial commit: the server configures a lock surface when it is created
        let lock_surface = lock.get_lock_surface(&wl, &output, &qh, id);
        st.surfaces.push(Surface::new(
            id,
            wl,
            Role::Lock(lock_surface),
            viewport,
            fractional,
            (0, 0),
        ));
        Ok(id)
    }

    /// End the lock: destroy every lock surface, tell the server to unlock and wait until it has the request. Only
    /// after the server confirmed the lock; call it only once the user is authenticated.
    pub fn unlock_session(&mut self) -> Result<(), ClientError> {
        let st = &mut self.state;
        let lock = match st.session_lock.take() {
            Some(lock) if lock.state == LockState::Locked => lock,
            other => {
                st.session_lock = other;
                return Err(ClientError::NotLocked);
            }
        };
        let surfaces: Vec<SurfaceId> = st
            .surfaces
            .iter()
            .filter(|s| matches!(s.role, Role::Lock(_)))
            .map(|s| s.id)
            .collect();
        for id in surfaces {
            self.destroy_surface(id)?;
        }
        lock.proxy.unlock_and_destroy();
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    /// Be told when the seat has been idle for `timeout`, honouring idle inhibitors: [`Client::take_idle`] then returns
    /// `true`. Replaces an earlier watch. Fails when the server lacks `ext_idle_notifier_v1` or `timeout` does not
    /// fit the protocol's milliseconds.
    pub fn watch_idle(&mut self, timeout: Duration) -> Result<(), ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let notifier = st.idle_notifier.as_ref().ok_or(ClientError::MissingGlobal(
            ExtIdleNotifierV1::interface().name,
        ))?;
        let millis = u32::try_from(timeout.as_millis()).map_err(|_| ClientError::InvalidTimeout)?;
        if let Some(previous) = st.idle_notification.take() {
            previous.destroy();
        }
        st.idled = false;
        st.idle_notification = Some(notifier.get_idle_notification(millis, &st.seat, &qh, ()));
        Ok(())
    }

    /// `true` once for every time the seat went idle since the last call, unless input resumed it in between.
    pub fn take_idle(&mut self) -> bool {
        std::mem::take(&mut self.state.idled)
    }

    /// Restrict pointer input to `rects` (surface-local logical points, rounded outwards); input
    /// elsewhere reaches the surfaces below. An empty slice makes the surface input-transparent.
    /// Applies with the next draw.
    pub fn set_input_region(&mut self, id: SurfaceId, rects: &[Rect]) -> Result<(), ClientError> {
        let qh = self.queue.handle();
        let region = self.state.compositor.create_region(&qh, ());
        for r in rects {
            let (x, y, w, h) = outward_rect(r);
            region.add(x, y, w, h);
        }
        self.state
            .surface_mut(id)?
            .wl
            .set_input_region(Some(&region));
        region.destroy();
        Ok(())
    }

    /// Create a backdrop below the surface's content.
    pub fn backdrop(&mut self, id: SurfaceId) -> Result<Backdrop, ClientError> {
        let qh = self.queue.handle();
        let wl = self.state.surface_mut(id)?.wl.clone();
        let proxy: CupertinoBackdropV1 = self.state.shell.get_backdrop(&wl, &qh, ());
        Ok(Backdrop::new(proxy))
    }

    /// Give a layer surface the menu-bar role of `output`. The server then sends the active
    /// application and its menus as [`Event::MenuBar`], the first group right away.
    pub fn menu_bar(
        &mut self,
        surface: SurfaceId,
        output: OutputId,
    ) -> Result<MenuBar, ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let wl = st.surface_mut(surface)?.wl.clone();
        let output = st
            .outputs
            .iter()
            .find(|o| o.name == output.0)
            .ok_or(ClientError::UnknownOutput)?
            .wl
            .clone();
        Ok(MenuBar::new(
            st.shell.get_menu_bar(&wl, &output, &qh, surface),
        ))
    }

    /// Give a layer surface the dock role of `output`. The server then lists the windows of the
    /// session and reports changes as [`Event::Dock`], the first group right away.
    pub fn dock(&mut self, surface: SurfaceId, output: OutputId) -> Result<Dock, ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let wl = st.surface_mut(surface)?.wl.clone();
        let output = st
            .outputs
            .iter()
            .find(|o| o.name == output.0)
            .ok_or(ClientError::UnknownOutput)?
            .wl
            .clone();
        Ok(Dock::new(st.shell.get_dock(&wl, &output, &qh, surface)))
    }

    /// Request an `xdg_activation_v1` token for the surface from the latest input event; it
    /// arrives as [`Event::ActivationToken`].
    pub fn request_activation_token(
        &mut self,
        id: SurfaceId,
    ) -> Result<ActivationRequest, ClientError> {
        self.request_token(id, None)
    }

    /// Like [`Client::request_activation_token`], for the launch of the application whose desktop
    /// entry id is `app_id`: the token names it (`xdg_activation_token_v1.set_app_id`), so the
    /// server can tell the docks the application is launching before its first window maps.
    pub fn request_launch_token(
        &mut self,
        id: SurfaceId,
        app_id: &str,
    ) -> Result<ActivationRequest, ClientError> {
        self.request_token(id, Some(app_id))
    }

    fn request_token(
        &mut self,
        id: SurfaceId,
        app_id: Option<&str>,
    ) -> Result<ActivationRequest, ClientError> {
        let serial = self.state.last_serial.ok_or(ClientError::NoInputSerial)?;
        let qh = self.queue.handle();
        let request = ActivationRequest(self.state.next_request);
        self.state.next_request += 1;
        let wl = self.state.surface_mut(id)?.wl.clone();
        let token = self.state.activation.get_activation_token(&qh, request);
        token.set_serial(serial, &self.state.seat);
        if let Some(app_id) = app_id {
            token.set_app_id(app_id.to_string());
        }
        token.set_surface(&wl);
        token.commit();
        Ok(request)
    }

    /// Draw one frame: `paint` receives a cleared pixmap of the surface's device size and the
    /// scale to draw at (device pixels per point). Attaches the buffer, requests the frame
    /// callback and commits together with every pending backdrop, input-region and
    /// interactivity change. Call it on [`Event::Draw`].
    pub fn draw(
        &mut self,
        id: SurfaceId,
        paint: impl FnOnce(&mut Pixmap, Scale),
    ) -> Result<(), ClientError> {
        self.draw_frame(id, None, paint)
    }

    /// Draw one frame that changes only `damage` (rectangles in points) of the last: `paint` receives the last frame's
    /// pixmap with those rectangles (rounded out to device pixels) cleared, and must repaint them and leave every
    /// other pixel as it is. Only they are converted into the shm buffer and reported to the server, which uploads
    /// only them. Without a last frame of the same size and scale it is a whole frame, as [`Client::draw`].
    pub fn draw_damage(
        &mut self,
        id: SurfaceId,
        damage: &[Rect],
        paint: impl FnOnce(&mut Pixmap, Scale),
    ) -> Result<(), ClientError> {
        self.draw_frame(id, Some(damage), paint)
    }

    fn draw_frame(
        &mut self,
        id: SurfaceId,
        damage: Option<&[Rect]>,
        paint: impl FnOnce(&mut Pixmap, Scale),
    ) -> Result<(), ClientError> {
        let qh = self.queue.handle();
        let st = &mut self.state;
        let scale = {
            let s = st.surface_mut(id)?;
            s.size.ok_or(ClientError::NotConfigured)?;
            s.reported_scale
        };
        let shm = st.shm.clone();
        // a fresh generation per draw is only consumed when the pool is replaced
        let generation = st.next_generation;
        st.next_generation += 1;
        let s = st.surface_mut(id)?;
        let (width, height) = s.size.ok_or(ClientError::NotConfigured)?;
        let (pw, ph) = (scale.to_physical(width), scale.to_physical(height));
        let layout = BufferLayout::new(pw, ph).ok_or(ClientError::BufferSize {
            width: pw,
            height: ph,
        })?;
        // the last frame stands only when its pool (size) and its scale are those of this one
        let kept =
            s.pool.as_ref().is_some_and(|p| p.layout() == layout) && s.drawn_scale == Some(scale);
        if s.pool.as_ref().is_none_or(|p| p.layout() != layout) {
            s.pool = Some(BufferPool::new(&shm, &qh, id, generation, layout)?);
        }
        let mut pixmap = match s.pixmap.take() {
            Some(p) if p.width() == pw && p.height() == ph => p,
            _ => Pixmap::new(pw, ph).ok_or(ClientError::BufferSize {
                width: pw,
                height: ph,
            })?,
        };
        let damage = match damage {
            Some(rects) if kept => Damage::Rects(
                rects
                    .iter()
                    .filter_map(|r| pixel_rect(r, scale.factor()).clamp(layout))
                    .collect(),
            ),
            _ => Damage::Full,
        };
        match &damage {
            Damage::Full => pixmap.fill(Color::TRANSPARENT),
            Damage::Rects(rects) => {
                for r in rects {
                    clear(&mut pixmap, r);
                }
            }
        }
        paint(&mut pixmap, scale);
        let pool = s.pool.as_mut().expect("pool created above");
        let buffer = pool
            .fill(pixmap.data(), &damage)
            .ok_or(ClientError::NoFreeBuffer)?;
        match &s.viewport {
            Some(viewport) => viewport.set_destination(width as i32, height as i32),
            None => {
                s.wl.set_buffer_scale(scale.integer().expect("integer scale without viewporter"))
            }
        }
        s.wl.attach(Some(buffer), 0, 0);
        match &damage {
            Damage::Full => s.wl.damage_buffer(0, 0, i32::MAX, i32::MAX),
            Damage::Rects(rects) => {
                for r in rects {
                    s.wl.damage_buffer(r.x as i32, r.y as i32, r.width as i32, r.height as i32);
                }
            }
        }
        s.wl.frame(&qh, id);
        s.wl.commit();
        s.frame_pending = true;
        s.pixmap = Some(pixmap);
        s.drawn_scale = Some(scale);
        Ok(())
    }

    /// Flush requests and block until something needs the application's attention, the
    /// timeout (`None` = no limit) passes, a held key repeats or a [`Waker`] fires. The result may be empty after a
    /// timeout or a wake-up.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<Event>, ClientError> {
        self.queue.dispatch_pending(&mut self.state)?;
        self.state.repeat_keys(Instant::now());
        self.state.update();
        if self.state.events.is_empty() && self.state.fault.is_none() {
            // a key repeat due earlier than the caller's timeout ends the wait first
            let until_repeat = self
                .state
                .repeat
                .deadline()
                .map(|at| at.saturating_duration_since(Instant::now()));
            self.block(timeout.into_iter().chain(until_repeat).min())?;
            self.queue.dispatch_pending(&mut self.state)?;
            self.state.repeat_keys(Instant::now());
            self.state.update();
        }
        match self.state.fault.take() {
            Some(e) => Err(e),
            None => Ok(std::mem::take(&mut self.state.events)),
        }
    }

    /// Poll the connection and the wake channel once and read what is ready.
    fn block(&mut self, timeout: Option<Duration>) -> Result<(), ClientError> {
        let guard = loop {
            match self.queue.prepare_read() {
                Some(guard) => break guard,
                None => {
                    self.queue.dispatch_pending(&mut self.state)?;
                }
            }
        };
        match self.conn.flush() {
            Ok(()) => {}
            // the socket is full; the next call flushes again
            Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        let timespec = timeout.map(|d| rustix::event::Timespec {
            tv_sec: d.as_secs() as i64,
            tv_nsec: i64::from(d.subsec_nanos()),
        });
        let (connection_ready, wake_ready) = {
            let connection_fd = guard.connection_fd();
            let mut fds = [
                PollFd::new(&connection_fd, PollFlags::IN),
                PollFd::new(&self.wake, PollFlags::IN),
            ];
            match poll(&mut fds, timespec.as_ref()) {
                Ok(_) => {}
                // a signal interrupted the wait: report an empty wake-up
                Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(ClientError::Io(e.into())),
            }
            (
                fds[0].revents().contains(PollFlags::IN),
                fds[1].revents().contains(PollFlags::IN),
            )
        };
        if connection_ready {
            match guard.read() {
                Ok(_) => {}
                // another reader consumed the data
                Err(WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            drop(guard);
        }
        if wake_ready {
            let mut buf = [0u8; 64];
            loop {
                match (&self.wake).read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => return Err(ClientError::Io(e)),
                }
            }
        }
        Ok(())
    }
}

/// Smallest whole-point rectangle `(x, y, width, height)` that contains `r`.
/// The device pixels `r` (points) touches at `factor` pixels a point.
fn pixel_rect(r: &Rect, factor: f32) -> PixelRect {
    let f = f64::from(factor);
    let (x0, y0) = ((r.x() * f).floor().max(0.0), (r.y() * f).floor().max(0.0));
    let (x1, y1) = (
        ((r.x() + r.width()) * f).ceil().max(0.0),
        ((r.y() + r.height()) * f).ceil().max(0.0),
    );
    PixelRect {
        x: x0 as u32,
        y: y0 as u32,
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
    }
}

/// Make the pixels of `r` (inside `pixmap`) transparent.
fn clear(pixmap: &mut Pixmap, r: &PixelRect) {
    let stride = pixmap.width() as usize * 4;
    let (start, len) = (r.x as usize * 4, r.width as usize * 4);
    let data = pixmap.data_mut();
    for row in r.y..r.y + r.height {
        let at = row as usize * stride + start;
        data[at..at + len].fill(0);
    }
}

pub(crate) fn outward_rect(r: &Rect) -> (i32, i32, i32, i32) {
    let (left, top) = (r.min_x().floor() as i32, r.min_y().floor() as i32);
    let (right, bottom) = (r.max_x().ceil() as i32, r.max_y().ceil() as i32);
    (left, top, right - left, bottom - top)
}

/// Put the popup's top-left corner at the geometry's, whatever the parent's size.
fn place(positioner: &XdgPositioner, g: &PopupGeometry) {
    positioner.set_size(g.width, g.height);
    positioner.set_anchor_rect(g.x, g.y, g.width, g.height);
    positioner.set_anchor(PositionerAnchor::TopLeft);
    positioner.set_gravity(Gravity::BottomRight);
}

impl Surface {
    fn new(
        id: SurfaceId,
        wl: WlSurface,
        role: Role,
        viewport: Option<WpViewport>,
        fractional: Option<WpFractionalScaleV1>,
        requested_size: (u32, u32),
    ) -> Self {
        Surface {
            id,
            wl,
            role,
            viewport,
            fractional,
            fractional_scale: None,
            menu_group: GroupBuilder::default(),
            dock_group: DockGroup::default(),
            entered: Vec::new(),
            requested_size,
            size: None,
            reported_scale: Scale::ONE,
            pool: None,
            pixmap: None,
            drawn_scale: None,
            frame_pending: false,
            wants_redraw: false,
        }
    }
}

impl State {
    /// A `wl_surface` with its scaling objects; the caller gives it a role and pushes it.
    fn new_surface(
        &mut self,
        qh: &QueueHandle<State>,
    ) -> (
        SurfaceId,
        WlSurface,
        Option<WpViewport>,
        Option<WpFractionalScaleV1>,
    ) {
        let id = SurfaceId(self.next_surface);
        self.next_surface += 1;
        let wl = self.compositor.create_surface(qh, id);
        let (viewport, fractional) = match &self.scaling {
            Some((manager, viewporter)) => (
                Some(viewporter.get_viewport(&wl, qh, ())),
                Some(manager.get_fractional_scale(&wl, qh, id)),
            ),
            None => (None, None),
        };
        (id, wl, viewport, fractional)
    }

    fn bind_output(
        &mut self,
        registry: &WlRegistry,
        qh: &QueueHandle<State>,
        name: u32,
        version: u32,
    ) {
        if version < 2 {
            self.fail(ClientError::GlobalVersion {
                interface: "wl_output",
                minimum: 2,
            });
            return;
        }
        let wl = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, name);
        let xdg = self.xdg_output.get_xdg_output(&wl, qh, name);
        self.outputs.push(Output {
            name,
            wl,
            xdg,
            scale: Scale::ONE,
            geometry: GeometryBuilder::default(),
        });
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore wayland_client::protocol::wl_shm_pool::WlShmPool);
delegate_noop!(State: wayland_client::protocol::wl_region::WlRegion);
delegate_noop!(State: ZwlrLayerShellV1);
delegate_noop!(State: CupertinoShellV1);
delegate_noop!(State: CupertinoScrollV1);
delegate_noop!(State: CupertinoBackdropV1);
delegate_noop!(State: XdgActivationV1);
delegate_noop!(State: ExtSessionLockManagerV1);
delegate_noop!(State: ExtIdleNotifierV1);
delegate_noop!(State: ZxdgOutputManagerV1);
delegate_noop!(State: XdgPositioner);
delegate_noop!(State: WpFractionalScaleManagerV1);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: ZxdgDecorationManagerV1);
delegate_noop!(State: WpCursorShapeManagerV1);
delegate_noop!(State: WpCursorShapeDeviceV1);

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => state.bind_output(registry, qh, name, version),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(i) = state.outputs.iter().position(|o| o.name == name) {
                    let output = state.outputs.remove(i);
                    output.xdg.destroy();
                    if output.wl.version() >= 3 {
                        output.wl.release();
                    }
                    state.events.push(Event::OutputsChanged);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Scale { factor } = event {
            match u32::try_from(factor).ok().and_then(Scale::from_integer) {
                Some(scale) => {
                    if let Some(o) = state.outputs.iter_mut().find(|o| o.name == *name) {
                        o.scale = scale;
                    }
                }
                None => state.fail(ClientError::Protocol("wl_output scale below 1")),
            }
        }
    }
}

impl Dispatch<CupertinoMenuBarV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoMenuBarV1,
        event: cupertino_menu_bar_v1::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Ok(s) = state.surface_mut(*id) else {
            return;
        };
        if let Some(update) = s.menu_group.push(event) {
            state.events.push(Event::MenuBar {
                surface: *id,
                update,
            });
        }
    }
}

impl Dispatch<CupertinoSessionV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoSessionV1,
        event: cupertino_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let queued = match event {
            cupertino_session_v1::Event::Appearance { dark } => {
                if dark > 1 {
                    return state.fail(ClientError::Protocol("appearance other than 0 or 1"));
                }
                state.session.dark = dark == 1;
                Event::Appearance {
                    dark: state.session.dark,
                }
            }
            cupertino_session_v1::Event::DisplayOptions { options } => {
                let Ok(bits) = options.into_result() else {
                    return state.fail(ClientError::Protocol("display options with reserved bits"));
                };
                state.session.display = DisplayOptions {
                    reduce_motion: bits.contains(DisplayOption::ReduceMotion),
                    reduce_transparency: bits.contains(DisplayOption::ReduceTransparency),
                    increase_contrast: bits.contains(DisplayOption::IncreaseContrast),
                    differentiate_without_color: bits
                        .contains(DisplayOption::DifferentiateWithoutColor),
                };
                Event::DisplayOptions(state.session.display)
            }
            cupertino_session_v1::Event::Locked => {
                state.session.locked = true;
                Event::SessionLock { locked: true }
            }
            cupertino_session_v1::Event::Unlocked => {
                state.session.locked = false;
                Event::SessionLock { locked: false }
            }
            _ => return,
        };
        if state.session.settled {
            state.events.push(queued);
        }
    }
}

impl Dispatch<CupertinoDockV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoDockV1,
        event: cupertino_dock_v1::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // the panel's output arrives as this client's object for it: map it to the output's id
        let event = match event {
            cupertino_dock_v1::Event::SwitcherOutput { output } => {
                let Some(name) = state
                    .outputs
                    .iter()
                    .find(|o| o.wl == output)
                    .map(|o| o.name)
                else {
                    return state.fail(ClientError::Protocol(
                        "switcher_output names an output this client has not bound",
                    ));
                };
                let Ok(s) = state.surface_mut(*id) else {
                    return;
                };
                return s.dock_group.switcher_output(OutputId(name));
            }
            event => event,
        };
        let Ok(s) = state.surface_mut(*id) else {
            return;
        };
        match s.dock_group.push(event) {
            Ok(Some(update)) => state.events.push(Event::Dock {
                surface: *id,
                update,
            }),
            Ok(None) => {}
            Err(e) => state.fail(e),
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, SurfaceId> for State {
    fn event(
        state: &mut Self,
        xdg: &XdgSurface,
        event: xdg_surface::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        xdg.ack_configure(serial);
        let Ok(s) = state.surface_mut(*id) else {
            return;
        };
        let suggested = match &mut s.role {
            Role::Popup(p) => p.configured.take(),
            Role::Toplevel(t) => t.pending.take().map(|(size, states)| {
                t.states = states;
                size
            }),
            Role::Layer(_) | Role::Lock(_) => return,
        };
        let Some(suggested) = suggested else {
            return state.fail(ClientError::Protocol(
                "xdg_surface configure without the configure of its role",
            ));
        };
        let size = configured_size(suggested, s.size.unwrap_or(s.requested_size));
        s.size = Some(size);
        s.wants_redraw = true;
        state.events.push(Event::Configured { surface: *id, size });
    }
}

impl Dispatch<XdgPopup, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgPopup,
        event: xdg_popup::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure { width, height, .. } => {
                let Ok(s) = state.surface_mut(*id) else {
                    return;
                };
                let Role::Popup(p) = &mut s.role else { return };
                match (u32::try_from(width), u32::try_from(height)) {
                    (Ok(w), Ok(h)) if w > 0 && h > 0 => p.configured = Some((w, h)),
                    _ => state.fail(ClientError::Protocol("popup configure without a size")),
                }
            }
            xdg_popup::Event::PopupDone => state.events.push(Event::PopupDone(*id)),
            _ => {}
        }
    }
}

/// The size an xdg surface takes from its configure: a zero extent (toplevels only) leaves it to the client, which keeps
/// the `current` one (the requested size before the first configure).
fn configured_size(suggested: (u32, u32), current: (u32, u32)) -> (u32, u32) {
    (
        if suggested.0 == 0 {
            current.0
        } else {
            suggested.0
        },
        if suggested.1 == 0 {
            current.1
        } else {
            suggested.1
        },
    )
}

impl Dispatch<XdgToplevel, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
                    return state.fail(ClientError::Protocol(
                        "toplevel configure with a negative size",
                    ));
                };
                let has = |wanted: xdg_toplevel::State| {
                    toplevel_states(&states).any(|s| s == wanted as u32)
                };
                let states = ToplevelStates {
                    activated: has(xdg_toplevel::State::Activated),
                    maximized: has(xdg_toplevel::State::Maximized),
                };
                let Ok(s) = state.surface_mut(*id) else {
                    return;
                };
                if let Role::Toplevel(t) = &mut s.role {
                    t.pending = Some(((width, height), states));
                }
            }
            xdg_toplevel::Event::Close => state.events.push(Event::Closed(*id)),
            _ => {}
        }
    }
}

/// The values of the `states` array of `xdg_toplevel.configure` (native-endian `u32`s, protocol text).
fn toplevel_states(states: &[u8]) -> impl Iterator<Item = u32> + '_ {
    states
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| u32::from_ne_bytes(*c))
}

impl Dispatch<ZxdgToplevelDecorationV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // the toolkit draws no title bar of its own: an undecorated window could not be moved or closed with the pointer
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            if mode != WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ServerSide) {
                state.fail(ClientError::Protocol(
                    "the server refused server-side decorations",
                ));
            }
        }
    }
}

impl Dispatch<ZxdgOutputV1, u32> for State {
    fn event(
        state: &mut Self,
        _: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(output) = state.outputs.iter_mut().find(|o| o.name == *name) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => output.geometry.position(x, y),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                if !output.geometry.size(width, height) {
                    state.fail(ClientError::Protocol("negative logical output size"));
                }
            }
            zxdg_output_v1::Event::Done => {
                let changed = output.geometry.done().is_some();
                if changed {
                    state.events.push(Event::OutputsChanged);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(Capability::Pointer) && state.pointer.is_none() {
                let pointer = seat.get_pointer(qh, ());
                state.pointer_scroll = state
                    .scroll_counts
                    .as_ref()
                    .map(|counts| counts.get_scroll(&pointer, qh, ()));
                state.pointer = Some(pointer);
            }
            if caps.contains(Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let translated = match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                state.last_serial = Some(serial);
                state.pointer_enter_serial = Some(serial);
                surface.data::<SurfaceId>().map(|id| {
                    state
                        .pointer_tracker
                        .enter(*id, Point::new(surface_x, surface_y))
                })
            }
            wl_pointer::Event::Leave { .. } => state.pointer_tracker.leave(),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state
                .pointer_tracker
                .motion(Point::new(surface_x, surface_y)),
            wl_pointer::Event::Button {
                serial,
                button,
                state: button_state,
                ..
            } => {
                state.last_serial = Some(serial);
                state.pointer_tracker.button(
                    button,
                    matches!(button_state, WEnum::Value(ButtonState::Pressed)),
                )
            }
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                let counted = state.pointer_scroll.is_some();
                state
                    .pointer_tracker
                    .scroll(axis == Axis::VerticalScroll, value, counted)
            }
            // the frame brings the scrolls held back for their counts
            wl_pointer::Event::Frame => {
                state.events.extend(
                    state
                        .pointer_tracker
                        .frame()
                        .into_iter()
                        .map(|(surface, event)| Event::Pointer { surface, event }),
                );
                None
            }
            _ => None,
        };
        if let Some((surface, event)) = translated {
            state.events.push(Event::Pointer { surface, event });
        }
    }
}

impl Dispatch<CupertinoPointerScrollV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &CupertinoPointerScrollV1,
        event: cupertino_pointer_scroll_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_pointer_scroll_v1::Event::ScrollCount {
                axis: WEnum::Value(axis),
                count,
            } => state
                .pointer_tracker
                .scroll_count(axis == Axis::VerticalScroll, count),
            cupertino_pointer_scroll_v1::Event::ScrollCount { .. } => {
                state.fail(ClientError::Protocol("scroll count of an unknown axis"))
            }
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Enter {
                serial, surface, ..
            } => {
                state.last_serial = Some(serial);
                state.key_focus = surface.data::<SurfaceId>().copied();
            }
            wl_keyboard::Event::Keymap { format, fd, size } => match format {
                WEnum::Value(wl_keyboard::KeymapFormat::XkbV1) => {
                    match Translator::from_fd(fd, size as usize) {
                        Ok(translator) => state.translator = Some(translator),
                        Err(e) => state.fail(e),
                    }
                }
                _ => state.fail(ClientError::Protocol("keymap format is not xkb_v1")),
            },
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => match &mut state.translator {
                Some(translator) => {
                    translator.update_modifiers(mods_depressed, mods_latched, mods_locked, group)
                }
                None => state.fail(ClientError::Protocol("modifiers before the keymap")),
            },
            wl_keyboard::Event::RepeatInfo { rate, delay } => state.repeat.set_info(rate, delay),
            wl_keyboard::Event::Leave { .. } => {
                state.repeat.clear();
                if let Some(surface) = state.key_focus.take() {
                    state.events.push(Event::KeyboardLeave(surface));
                }
            }
            wl_keyboard::Event::Key {
                serial,
                key,
                state: key_state,
                ..
            } => {
                state.last_serial = Some(serial);
                let pressed = matches!(key_state, WEnum::Value(KeyState::Pressed));
                if pressed && state.translator.as_ref().is_some_and(|t| t.repeats(key)) {
                    state.repeat.press(key, Instant::now());
                } else if !pressed {
                    state.repeat.release(key);
                }
                if let Some(surface) = state.key_focus {
                    state.push_key(surface, key, pressed);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, SurfaceId> for State {
    fn event(
        state: &mut Self,
        surface: &WlSurface,
        event: wl_surface::Event,
        _: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(s) = state.surface_of(surface) else {
            return;
        };
        match event {
            wl_surface::Event::Enter { output } => s.entered.push(output.id()),
            wl_surface::Event::Leave { output } => s.entered.retain(|o| *o != output.id()),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                let Ok(s) = state.surface_mut(*id) else {
                    return;
                };
                let size = (
                    if width == 0 {
                        s.requested_size.0
                    } else {
                        width
                    },
                    if height == 0 {
                        s.requested_size.1
                    } else {
                        height
                    },
                );
                if size.0 == 0 || size.1 == 0 {
                    state.fail(ClientError::Protocol(
                        "configure without a size for a surface that requested none",
                    ));
                    return;
                }
                s.size = Some(size);
                s.wants_redraw = true;
                state.events.push(Event::Configured { surface: *id, size });
            }
            zwlr_layer_surface_v1::Event::Closed => state.events.push(Event::Closed(*id)),
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for State {
    fn event(
        state: &mut Self,
        lock: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let next = match event {
            ext_session_lock_v1::Event::Locked => LockState::Locked,
            ext_session_lock_v1::Event::Finished => LockState::Finished,
            _ => return,
        };
        // an event of a lock replaced since belongs to no one
        if let Some(current) = state
            .session_lock
            .as_mut()
            .filter(|current| current.proxy == *lock)
        {
            current.state = next;
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        lock_surface: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        else {
            return;
        };
        lock_surface.ack_configure(serial);
        let Ok(s) = state.surface_mut(*id) else {
            return;
        };
        if width == 0 || height == 0 {
            state.fail(ClientError::Protocol(
                "lock surface configured without a size",
            ));
            return;
        }
        s.size = Some((width, height));
        s.wants_redraw = true;
        state.events.push(Event::Configured {
            surface: *id,
            size: (width, height),
        });
    }
}

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => state.idled = true,
            ext_idle_notification_v1::Event::Resumed => state.idled = false,
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let wp_fractional_scale_v1::Event::PreferredScale { scale } = event else {
            return;
        };
        match Scale::from_120ths(scale) {
            Some(scale) => {
                if let Ok(s) = state.surface_mut(*id) {
                    s.fractional_scale = Some(scale);
                }
            }
            None => state.fail(ClientError::Protocol("preferred fractional scale of zero")),
        }
    }
}

impl Dispatch<WlCallback, SurfaceId> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        id: &SurfaceId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            if let Ok(s) = state.surface_mut(*id) {
                s.frame_pending = false;
            }
        }
    }
}

impl Dispatch<WlBuffer, BufferTag> for State {
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wl_buffer::Event,
        tag: &BufferTag,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            // a release from a replaced pool names a generation that no longer exists
            if let Some(pool) = state
                .surface_mut(tag.surface)
                .ok()
                .and_then(|s| s.pool.as_mut())
                .filter(|p| p.generation() == tag.generation)
            {
                pool.release(tag.slot);
            }
        }
    }
}

impl Dispatch<XdgActivationTokenV1, ActivationRequest> for State {
    fn event(
        state: &mut Self,
        token: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        request: &ActivationRequest,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token: value } = event {
            state.events.push(Event::ActivationToken {
                request: *request,
                token: value,
            });
            token.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_extent_in_a_configure_keeps_the_current_size() {
        assert_eq!(configured_size((0, 0), (920, 436)), (920, 436));
        assert_eq!(configured_size((1200, 0), (920, 436)), (1200, 436));
        assert_eq!(configured_size((640, 480), (920, 436)), (640, 480));
    }

    #[test]
    fn toplevel_states_are_native_endian_words() {
        let states: Vec<u8> = [1u32, 4, 2].iter().flat_map(|s| s.to_ne_bytes()).collect();
        assert_eq!(toplevel_states(&states).collect::<Vec<_>>(), [1, 4, 2]);
        assert!(toplevel_states(&states).any(|s| s == xdg_toplevel::State::Activated as u32));
    }

    #[test]
    fn input_regions_round_outwards() {
        assert_eq!(
            outward_rect(&Rect::new(10.2, 20.7, 100.0, 50.0)),
            (10, 20, 101, 51)
        );
        assert_eq!(
            outward_rect(&Rect::new(0.0, 0.0, 344.0, 72.0)),
            (0, 0, 344, 72)
        );
    }
}
