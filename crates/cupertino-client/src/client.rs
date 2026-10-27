//! The shell processes' client API on the transport (`cupertino-wayland-client`): the shell's protocols are extensions
//! on its queue ([`crate::extensions`]), and its events are translated into this crate's. The API stays until the shell
//! processes move to the transport, and goes away then.

use crate::backdrop::Backdrop;
use crate::dock::Dock;
use crate::event::{ActivationRequest, Event, PointerEvent, ScrollCounts};
use crate::extensions::{Emitted, LayerRole, LockGeneration, LockRole, Quiet, RoleData};
use crate::keyboard::Modifiers;
use crate::menu_bar::MenuBar;
use crate::output::{OutputId, OutputInfo};
use crate::pixels::{convert_rects, rgba_to_argb8888};
use crate::popup::PopupGeometry;
use crate::scale::Scale;
use crate::{Anchor, ClientError, KeyboardInteractivity, Layer};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_geometry::Rect;
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::{
    self, CupertinoAppEffectsV1,
};
use cupertino_protocols::client::app_effects::cupertino_appearance_v1::CupertinoAppearanceV1;
use cupertino_protocols::client::cupertino_session_v1::CupertinoSessionV1;
use cupertino_protocols::client::cupertino_shell_v1::CupertinoShellV1;
use cupertino_protocols::client::scroll::cupertino_pointer_scroll_v1::CupertinoPointerScrollV1;
use cupertino_protocols::client::scroll::cupertino_scroll_v1::CupertinoScrollV1;
use cupertino_wayland_client::{
    Anchor as PopupAnchor, BufferFormat, Capability, ClipboardOffer, Connection,
    ConstraintAdjustment, CursorShape, Decorations, Error, Gravity, KeyboardEvent, PixelRect,
    Popup, PopupPlacement, Toplevel, ToplevelConfig as WindowConfig,
};
use std::sync::Mutex;
use std::time::Duration;
use tiny_skia::{Color, Pixmap};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notification_v1::ExtIdleNotificationV1;
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_v1::ExtSessionLockV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

pub use cupertino_wayland_client::Waker;

/// The MIME types of text on the clipboard, in order of preference.
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];

/// Handle of a surface of a [`Client`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub(crate) cupertino_wayland_client::SurfaceId);

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

/// The title bar the server draws on an application window (`cupertino_app_effects_v1.set_titlebar_style`).
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

/// What gives a surface its place on screen.
enum Role {
    Layer(ZwlrLayerSurfaceV1),
    Popup {
        popup: Popup,
        parent: SurfaceId,
    },
    /// Covers an output while the session is locked.
    Lock,
    /// An application window, with the states of its last configure.
    Toplevel {
        window: Toplevel,
        activated: bool,
        maximized: bool,
    },
}

struct Surface {
    id: SurfaceId,
    role: Role,
    requested_size: (u32, u32),
    /// Logical size from the last configure.
    size: Option<(u32, u32)>,
    /// Scale last reported to the application.
    reported_scale: Scale,
    pixmap: Option<Pixmap>,
    /// The scale the last frame was drawn at: a frame at another one is drawn whole.
    drawn_scale: Option<Scale>,
    frame_pending: bool,
    wants_redraw: bool,
}

impl Surface {
    fn new(id: SurfaceId, role: Role, requested_size: (u32, u32)) -> Self {
        Surface {
            id,
            role,
            requested_size,
            size: None,
            reported_scale: Scale::ONE,
            pixmap: None,
            drawn_scale: None,
            frame_pending: false,
            wants_redraw: false,
        }
    }
}

struct SessionLock {
    proxy: ExtSessionLockV1,
    generation: u32,
    state: LockState,
}

/// What the server last said about the session: `cupertino_session_v1` on a shell connection, else the appearance and
/// display options of `cupertino_appearance_v1` (the lock state stays false).
struct Session {
    /// Kept alive: the server stops sending when the object is destroyed.
    _session: Option<CupertinoSessionV1>,
    _appearance: Option<CupertinoAppearanceV1>,
    dark: bool,
    display: DisplayOptions,
    locked: bool,
    /// The state at connect is read; later changes also queue an [`Event`].
    settled: bool,
}

/// The extension globals.
struct Globals {
    layer_shell: ZwlrLayerShellV1,
    effects: CupertinoAppEffectsV1,
    /// Bound on a shell connection only ([`Client::connect_shell`]).
    shell: Option<CupertinoShellV1>,
    session_lock_manager: Option<ExtSessionLockManagerV1>,
    idle_notifier: Option<ExtIdleNotifierV1>,
    /// Offered by the server: the scroll count of the pointer's scrolls (`cupertino_scroll_v1`).
    scroll_counts: Option<CupertinoScrollV1>,
}

/// Connection to the server and the surfaces of one shell process.
pub struct Client {
    conn: Connection,
    globals: Globals,
    session: Session,
    session_lock: Option<SessionLock>,
    next_lock: u32,
    idle_notification: Option<ExtIdleNotificationV1>,
    /// The seat went idle since [`Client::take_idle`] last ran.
    idled: bool,
    /// Kept alive: the server stops sending when the object is destroyed.
    pointer_scroll: Option<CupertinoPointerScrollV1>,
    scroll: ScrollCounts,
    surfaces: Vec<Surface>,
    key_focus: Option<SurfaceId>,
    selection: Option<ClipboardOffer>,
    /// This client's text, until the server reports a selection.
    own_text: Option<String>,
    events: Vec<Event>,
    fault: Option<ClientError>,
}

/// Bind an extension global the client needs.
fn require<I>(
    conn: &mut Connection,
    versions: std::ops::RangeInclusive<u32>,
) -> Result<I, ClientError>
where
    I: wayland_client::Proxy + 'static,
    cupertino_wayland_client::State: wayland_client::Dispatch<I, Quiet>,
{
    conn.bind_extension(versions, Quiet)
        .ok_or(ClientError::MissingGlobal(I::interface().name))
}

impl Client {
    /// Connect to the display named by `WAYLAND_DISPLAY` and bind the globals an application may use. Requires the
    /// compositor, shm, seat, layer shell, `cupertino_app_effects_v1`, `xdg_activation_v1`, `xdg_wm_base` and
    /// `zxdg_output_manager_v1` globals; fractional scale and viewporter are used when both are offered. The session
    /// lock, idle notification and decoration globals are optional here: [`Client::lock_session`],
    /// [`Client::watch_idle`] and [`Client::toplevel`] fail with [`ClientError::MissingGlobal`] when the server lacks them.
    pub fn connect() -> Result<Client, ClientError> {
        Self::with(Connection::connect()?, false)
    }

    /// [`Client::connect`] over an established connection to the server, which the process may share with event queues
    /// of its own. Binds no private object: the menu-bar and dock roles
    /// and [`Client::session_locked`] need [`Client::from_shell_connection`].
    pub fn from_connection(conn: wayland_client::Connection) -> Result<Client, ClientError> {
        Self::with(Connection::from_wayland(conn)?, false)
    }

    /// [`Client::connect`] for a shell process the server started: the connection is the one the server made for the
    /// process (`WAYLAND_SOCKET`, else `WAYLAND_DISPLAY`), and `cupertino_shell_v1` is bound as well, so the session
    /// observer reports the lock state and the roles the server gave the process can be asked for.
    pub fn connect_shell() -> Result<Client, ClientError> {
        Self::with(Connection::connect()?, true)
    }

    /// [`Client::connect_shell`] over an established connection to the server.
    pub fn from_shell_connection(conn: wayland_client::Connection) -> Result<Client, ClientError> {
        Self::with(Connection::from_wayland(conn)?, true)
    }

    fn with(mut conn: Connection, shell_roles: bool) -> Result<Client, ClientError> {
        for capability in [
            Capability::Seat,
            Capability::Activation,
            Capability::OutputLayout,
        ] {
            if !conn.capabilities().contains(capability) {
                return Err(Error::Unsupported(capability).into());
            }
        }
        let qh = conn.queue_handle();
        let effects: CupertinoAppEffectsV1 = require(&mut conn, 1..=1)?;
        // version 2 adds the display options of the session object, version 3 the screen capture state of the menu bar,
        // version 5 moves the backdrops and the title bar style to the app effects, version 6 adds the menu bar's session actions
        let (shell, session, appearance) = if shell_roles {
            let shell: CupertinoShellV1 = require(&mut conn, 9..=9)?;
            let session = shell.get_session(&qh, Quiet);
            (Some(shell), Some(session), None)
        } else {
            (None, None, Some(effects.get_appearance(&qh, Quiet)))
        };
        let globals = Globals {
            layer_shell: require(&mut conn, 1..=4)?,
            effects,
            shell,
            session_lock_manager: conn.bind_extension(1..=1, Quiet),
            idle_notifier: conn.bind_extension(1..=2, Quiet),
            scroll_counts: conn.bind_extension(1..=1, Quiet),
        };
        let mut client = Client {
            conn,
            globals,
            session: Session {
                _session: session,
                _appearance: appearance,
                dark: false,
                display: DisplayOptions::default(),
                locked: false,
                settled: false,
            },
            session_lock: None,
            next_lock: 0,
            idle_notification: None,
            idled: false,
            pointer_scroll: None,
            scroll: ScrollCounts::default(),
            surfaces: Vec::new(),
            key_focus: None,
            selection: None,
            own_text: None,
            events: Vec::new(),
            fault: None,
        };
        // the session's state at connect is read before the process draws anything
        client.conn.roundtrip()?;
        for event in client.conn.wait(Some(Duration::ZERO))? {
            client.translate(event);
        }
        if let Some(e) = client.fault.take() {
            return Err(e);
        }
        client.session.settled = true;
        Ok(client)
    }

    /// A handle that interrupts [`Client::wait`] from other threads.
    pub fn waker(&self) -> Waker {
        self.conn.waker()
    }

    /// The outputs the server currently offers; [`Event::OutputsChanged`] reports changes.
    pub fn outputs(&self) -> Vec<OutputInfo> {
        self.conn
            .outputs()
            .into_iter()
            .map(OutputInfo::from)
            .collect()
    }

    /// The outputs a surface is on, in the order the server put it there (`wl_surface.enter` and
    /// `leave`). A layer surface created without an output finds the one the server chose here by
    /// its first [`Event::Configured`].
    pub fn surface_outputs(&mut self, id: SurfaceId) -> Result<Vec<OutputId>, ClientError> {
        Ok(self
            .conn
            .surface_outputs(id.0)?
            .into_iter()
            .map(|o| OutputId(o.registry_name()))
            .collect())
    }

    /// Create a layer surface. It is mapped and sized by the first [`Event::Configured`], which
    /// is followed by [`Event::Draw`].
    pub fn layer_surface(&mut self, config: &LayerSurfaceConfig) -> Result<SurfaceId, ClientError> {
        let output = match config.output {
            Some(id) => Some(self.wl_output(id)?),
            None => None,
        };
        let shell = self.globals.layer_shell.clone();
        let mut proxy = None;
        let id = self.conn.external_surface(|wl, qh, id| {
            let layer = shell.get_layer_surface(
                wl,
                output.as_ref(),
                config.layer,
                config.namespace.clone(),
                qh,
                SurfaceId(id),
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
        let layer = proxy.expect("the role was assigned");
        self.surfaces
            .push(Surface::new(SurfaceId(id), Role::Layer(layer), config.size));
        Ok(SurfaceId(id))
    }

    /// Create an application window. The server draws its title bar and window control buttons
    /// (`zxdg_decoration_manager_v1`, server-side mode); the surface is the content below them. It is mapped and
    /// sized by the first [`Event::Configured`]; resizes and focus changes ([`Client::toplevel_activated`]) arrive
    /// the same way, scrolling over it arrives as [`PointerEvent::Scroll`] (with the scroll count when the server offers it), and the request to close it is
    /// [`Event::Closed`]. Fails when the server lacks the decoration global or a size does not fit the protocol.
    pub fn toplevel(&mut self, config: &ToplevelConfig) -> Result<SurfaceId, ClientError> {
        let fits = |v: u32| i32::try_from(v).is_ok();
        if !fits(config.min_size.0) || !fits(config.min_size.1) {
            return Err(ClientError::InvalidShape);
        }
        if config.size.0 == 0 || config.size.1 == 0 {
            return Err(ClientError::InvalidShape);
        }
        if !self
            .conn
            .capabilities()
            .contains(Capability::ServerDecoration)
        {
            return Err(Error::Unsupported(Capability::ServerDecoration).into());
        }
        let window = self.conn.toplevel(&WindowConfig {
            app_id: config.app_id.clone(),
            title: config.title.clone(),
            min_size: Some(config.min_size),
            max_size: None,
            decorations: Decorations::Server,
        })?;
        if config.titlebar == TitlebarStyle::Unified {
            // before the first buffer, which the server reads the style with
            self.globals.effects.set_titlebar_style(
                &self.conn.wl_surface(window.surface())?,
                cupertino_app_effects_v1::TitlebarStyle::Unified,
            );
        }
        let id = SurfaceId(window.surface());
        let role = Role::Toplevel {
            window,
            activated: false,
            maximized: false,
        };
        self.surfaces.push(Surface::new(id, role, config.size));
        Ok(id)
    }

    fn window(&self, id: SurfaceId) -> Result<(Toplevel, bool, bool), ClientError> {
        match self.surface(id)?.role {
            Role::Toplevel {
                window,
                activated,
                maximized,
            } => Ok((window, activated, maximized)),
            Role::Layer(_) | Role::Popup { .. } | Role::Lock => Err(ClientError::NotToplevel),
        }
    }

    /// Change the title of an application window.
    pub fn set_title(&mut self, id: SurfaceId, title: &str) -> Result<(), ClientError> {
        let (window, ..) = self.window(id)?;
        Ok(self.conn.set_title(window, title)?)
    }

    /// Whether an application window has the focus, as of its last [`Event::Configured`].
    pub fn toplevel_activated(&mut self, id: SurfaceId) -> Result<bool, ClientError> {
        Ok(self.window(id)?.1)
    }

    /// Start moving the window with the pointer (`xdg_toplevel.move`), as the press that just arrived: call it on a
    /// left-button press over a part of a [`TitlebarStyle::Unified`] window that has no control. The server moves the
    /// window until the button is released.
    pub fn toplevel_move(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        let (window, ..) = self.window(id)?;
        Ok(self.conn.start_move(window)?)
    }

    /// Zoom the window, or restore it when it is zoomed already (`xdg_toplevel.set_maximized` / `unset_maximized`),
    /// as of its last [`Event::Configured`]: the action of a double click on the toolbar of a
    /// [`TitlebarStyle::Unified`] window.
    pub fn toplevel_toggle_maximized(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        let (window, _, maximized) = self.window(id)?;
        Ok(self.conn.set_maximized(window, !maximized)?)
    }

    /// Set the pointer's look over the surface it entered last; call it on [`PointerEvent::Enter`](crate::PointerEvent),
    /// because the server keeps the look of the latest request until another surface sets its own. Fails without a
    /// pointer enter yet, and for [`CursorLook::Arrow`] when the server lacks `wp_cursor_shape_manager_v1`.
    pub fn set_cursor(&mut self, look: CursorLook) -> Result<(), ClientError> {
        match look {
            CursorLook::Hidden => self.conn.hide_cursor()?,
            CursorLook::Arrow => self.conn.set_cursor(CursorShape::Default)?,
        }
        Ok(())
    }

    /// Serial of the latest input event (pointer enter or button, keyboard enter or key), for requests that need one,
    /// such as setting the clipboard selection on a connection of the application's own.
    pub fn input_serial(&self) -> Option<u32> {
        self.conn.input_serial()
    }

    /// Put `text` on the clipboard, as the answer to the latest input event (a copy or cut key). The client serves it
    /// to pasting clients from [`Client::wait`] until another selection replaces it.
    pub fn set_clipboard_text(&mut self, text: String) -> Result<(), ClientError> {
        let contents = TEXT_TYPES
            .iter()
            .map(|mime| (mime.to_string(), text.as_bytes().to_vec()))
            .collect();
        self.conn.set_selection(contents)?;
        self.own_text = Some(text);
        Ok(())
    }

    /// The clipboard's text for a paste, `None` when it holds no text. Reading another client's selection waits up to
    /// a second for it.
    pub fn clipboard_text(&mut self) -> Result<Option<String>, ClientError> {
        let Some(offer) = &self.selection else {
            return Ok(self.own_text.clone());
        };
        let Some(mime) = TEXT_TYPES
            .iter()
            .find(|t| offer.mime_types().iter().any(|m| m == *t))
        else {
            return Ok(None);
        };
        match self.conn.receive(offer, mime) {
            Ok(bytes) => Ok(Some(text_of(&bytes))),
            // replaced while reading, or the copying client did not answer in time
            Err(Error::Cancelled) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// The modifiers held now, as of the last `wl_keyboard.modifiers` event; none before the keymap arrives. Pointer
    /// events carry no modifiers, so a click reads them here.
    pub fn modifiers(&self) -> Modifiers {
        self.conn.modifiers().into()
    }

    /// Create a popup of `parent` (a layer surface, an application window or another popup) covering `rect`, given in
    /// parent-local logical points and rounded outwards to whole points. The popup takes a grab on
    /// the seat with the serial of the latest input event, so the server dismisses it
    /// ([`Event::PopupDone`]) when the user clicks elsewhere. It is mapped and sized by the first
    /// [`Event::Configured`]. Destroy a popup before its parent.
    pub fn popup(&mut self, parent: SurfaceId, rect: &Rect) -> Result<SurfaceId, ClientError> {
        let geometry = PopupGeometry::from_rect(rect)?;
        self.conn.input_serial().ok_or(ClientError::NoInputSerial)?;
        if let Role::Lock = self.surface(parent)?.role {
            return Err(ClientError::NotLayerSurface);
        }
        let popup = self.conn.popup(parent.0, &placement(&geometry, true))?;
        let id = SurfaceId(popup.surface());
        let size = (geometry.width as u32, geometry.height as u32);
        self.surfaces
            .push(Surface::new(id, Role::Popup { popup, parent }, size));
        Ok(id)
    }

    /// Move or resize a popup to `rect` (parent-local, as for [`Client::popup`]). The new size
    /// arrives as [`Event::Configured`].
    pub fn reposition_popup(&mut self, id: SurfaceId, rect: &Rect) -> Result<(), ClientError> {
        let geometry = PopupGeometry::from_rect(rect)?;
        let Role::Popup { popup, .. } = self.surface(id)?.role else {
            return Err(ClientError::NotPopup);
        };
        Ok(self
            .conn
            .reposition_popup(popup, &placement(&geometry, true))?)
    }

    /// Destroy a surface with its role objects. Its popups must be destroyed first. Destroy a
    /// surface's backdrops before the surface.
    pub fn destroy_surface(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        if self
            .surfaces
            .iter()
            .any(|s| matches!(&s.role, Role::Popup { parent, .. } if *parent == id))
        {
            return Err(ClientError::HasPopups);
        }
        let index = self
            .surfaces
            .iter()
            .position(|s| s.id == id)
            .ok_or(ClientError::UnknownSurface)?;
        self.conn.destroy(id.0)?;
        self.surfaces.remove(index);
        if self.key_focus == Some(id) {
            self.key_focus = None;
        }
        Ok(())
    }

    /// Logical size of a configured surface.
    pub fn size(&mut self, id: SurfaceId) -> Result<(u32, u32), ClientError> {
        self.surface(id)?.size.ok_or(ClientError::NotConfigured)
    }

    /// Ask for an [`Event::Draw`] as soon as the surface can take a frame.
    pub fn request_redraw(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        self.surface_mut(id)?.wants_redraw = true;
        Ok(())
    }

    fn layer(&self, id: SurfaceId) -> Result<&ZwlrLayerSurfaceV1, ClientError> {
        match &self.surface(id)?.role {
            Role::Layer(layer) => Ok(layer),
            Role::Popup { .. } | Role::Lock | Role::Toplevel { .. } => {
                Err(ClientError::NotLayerSurface)
            }
        }
    }

    /// Change the keyboard interactivity; applies with the next draw.
    pub fn set_keyboard_interactivity(
        &mut self,
        id: SurfaceId,
        mode: KeyboardInteractivity,
    ) -> Result<(), ClientError> {
        self.layer(id)?.set_keyboard_interactivity(mode);
        Ok(())
    }

    /// Change the distance from the anchored edge that other surfaces keep clear; applies with the
    /// next draw.
    pub fn set_exclusive_zone(&mut self, id: SurfaceId, zone: i32) -> Result<(), ClientError> {
        self.layer(id)?.set_exclusive_zone(zone);
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
        let layer = self.layer(id)?;
        layer.set_anchor(anchor);
        layer.set_size(size.0, size.1);
        self.surface_mut(id)?.requested_size = size;
        Ok(())
    }

    /// Ask the server to lock the session. Create a lock surface for every output right away
    /// ([`Client::lock_surface`]); the server confirms with [`LockState::Locked`] in
    /// [`Client::lock_state`], which changes while [`Client::wait`] runs. Fails when the server lacks
    /// `ext_session_lock_manager_v1` or a lock that is not [`LockState::Finished`] exists.
    pub fn lock_session(&mut self) -> Result<(), ClientError> {
        let manager = self
            .globals
            .session_lock_manager
            .as_ref()
            .ok_or(ClientError::MissingGlobal("ext_session_lock_manager_v1"))?;
        match &self.session_lock {
            Some(lock) if lock.state != LockState::Finished => {
                return Err(ClientError::SessionLockActive)
            }
            Some(lock) => lock.proxy.destroy(),
            None => {}
        }
        self.next_lock += 1;
        let generation = self.next_lock;
        self.session_lock = Some(SessionLock {
            proxy: manager.lock(&self.conn.queue_handle(), LockGeneration(generation)),
            generation,
            state: LockState::Requested,
        });
        Ok(())
    }

    /// Whether Caps Lock is on, as of the last `wl_keyboard.modifiers` event; `false` before the keymap arrives.
    pub fn caps_lock(&self) -> bool {
        self.conn.modifiers().caps_lock
    }

    /// The state of the lock requested with [`Client::lock_session`]; `None` before the first request and after
    /// [`Client::unlock_session`].
    pub fn lock_state(&self) -> Option<LockState> {
        self.session_lock.as_ref().map(|lock| lock.state)
    }

    /// Whether the session is dark, as of the last `cupertino_session_v1.appearance`. The value at connect is read
    /// before [`Client::connect`] returns, so no process starts with a guess; later changes arrive as [`Event::Appearance`].
    pub fn is_dark(&self) -> bool {
        self.session.dark
    }

    /// The accessibility display options, as of the last `cupertino_session_v1.display_options`. The value at connect is
    /// read before [`Client::connect`] returns; later changes arrive as [`Event::DisplayOptions`].
    pub fn display_options(&self) -> DisplayOptions {
        self.session.display
    }

    /// Whether the server holds the session locked, as of the last `locked` or `unlocked` event. The value at connect is
    /// read before [`Client::connect`] returns; later changes arrive as [`Event::SessionLock`].
    pub fn session_locked(&self) -> bool {
        self.session.locked
    }

    /// Create the lock surface of `output`. It is sized by the first [`Event::Configured`], which is followed by
    /// [`Event::Draw`]; it takes keyboard and pointer input once the session is locked. One per output.
    pub fn lock_surface(&mut self, output: OutputId) -> Result<SurfaceId, ClientError> {
        let output = self.wl_output(output)?;
        let lock = match &self.session_lock {
            Some(lock) if lock.state != LockState::Finished => lock.proxy.clone(),
            _ => return Err(ClientError::NoSessionLock),
        };
        let id = self.conn.external_surface(|wl, qh, id| {
            LockRole(lock.get_lock_surface(wl, &output, qh, SurfaceId(id)))
        });
        self.surfaces
            .push(Surface::new(SurfaceId(id), Role::Lock, (0, 0)));
        Ok(SurfaceId(id))
    }

    /// End the lock: destroy every lock surface, tell the server to unlock and wait until it has the request. Only
    /// after the server confirmed the lock; call it only once the user is authenticated.
    pub fn unlock_session(&mut self) -> Result<(), ClientError> {
        let lock = match self.session_lock.take() {
            Some(lock) if lock.state == LockState::Locked => lock,
            other => {
                self.session_lock = other;
                return Err(ClientError::NotLocked);
            }
        };
        let surfaces: Vec<SurfaceId> = self
            .surfaces
            .iter()
            .filter(|s| matches!(s.role, Role::Lock))
            .map(|s| s.id)
            .collect();
        for id in surfaces {
            self.destroy_surface(id)?;
        }
        lock.proxy.unlock_and_destroy();
        self.conn.roundtrip()?;
        Ok(())
    }

    /// Be told when the seat has been idle for `timeout`, honouring idle inhibitors: [`Client::take_idle`] then returns
    /// `true`. Replaces an earlier watch. Fails when the server lacks `ext_idle_notifier_v1` or `timeout` does not
    /// fit the protocol's milliseconds.
    pub fn watch_idle(&mut self, timeout: Duration) -> Result<(), ClientError> {
        let notifier = self
            .globals
            .idle_notifier
            .as_ref()
            .ok_or(ClientError::MissingGlobal("ext_idle_notifier_v1"))?;
        let millis = u32::try_from(timeout.as_millis()).map_err(|_| ClientError::InvalidTimeout)?;
        let seat = self
            .conn
            .seat()
            .ok_or(ClientError::MissingGlobal("wl_seat"))?;
        if let Some(previous) = self.idle_notification.take() {
            previous.destroy();
        }
        self.idled = false;
        self.idle_notification =
            Some(notifier.get_idle_notification(millis, &seat, &self.conn.queue_handle(), Quiet));
        Ok(())
    }

    /// `true` once for every time the seat went idle since the last call, unless input resumed it in between.
    pub fn take_idle(&mut self) -> bool {
        std::mem::take(&mut self.idled)
    }

    /// Restrict pointer input to `rects` (surface-local logical points, rounded outwards); input
    /// elsewhere reaches the surfaces below. An empty slice makes the surface input-transparent.
    /// Applies with the next draw.
    pub fn set_input_region(&mut self, id: SurfaceId, rects: &[Rect]) -> Result<(), ClientError> {
        self.surface(id)?;
        Ok(self.conn.set_input_region(id.0, Some(rects))?)
    }

    /// Create a backdrop below the surface's content.
    pub fn backdrop(&mut self, id: SurfaceId) -> Result<Backdrop, ClientError> {
        let wl = self.conn.wl_surface(id.0)?;
        let proxy = self
            .globals
            .effects
            .get_backdrop(&wl, &self.conn.queue_handle(), Quiet);
        Ok(Backdrop::new(proxy))
    }

    fn shell_roles(&self) -> Result<CupertinoShellV1, ClientError> {
        self.globals
            .shell
            .clone()
            .ok_or(ClientError::MissingGlobal("cupertino_shell_v1"))
    }

    /// Give a layer surface the menu-bar role of `output`. The server then sends the active
    /// application and its menus as [`Event::MenuBar`], the first group right away.
    pub fn menu_bar(
        &mut self,
        surface: SurfaceId,
        output: OutputId,
    ) -> Result<MenuBar, ClientError> {
        let shell = self.shell_roles()?;
        let wl = self.conn.wl_surface(surface.0)?;
        let output = self.wl_output(output)?;
        let data = RoleData {
            surface,
            group: Mutex::default(),
        };
        Ok(MenuBar::new(shell.get_menu_bar(
            &wl,
            &output,
            &self.conn.queue_handle(),
            data,
        )))
    }

    /// Give a layer surface the dock role of `output`. The server then lists the windows of the
    /// session and reports changes as [`Event::Dock`], the first group right away.
    pub fn dock(&mut self, surface: SurfaceId, output: OutputId) -> Result<Dock, ClientError> {
        let shell = self.shell_roles()?;
        let wl = self.conn.wl_surface(surface.0)?;
        let output = self.wl_output(output)?;
        let data = RoleData {
            surface,
            group: Mutex::default(),
        };
        Ok(Dock::new(shell.get_dock(
            &wl,
            &output,
            &self.conn.queue_handle(),
            data,
        )))
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
        self.conn.input_serial().ok_or(ClientError::NoInputSerial)?;
        self.surface(id)?;
        let request = self.conn.request_activation_token(id.0, app_id)?;
        Ok(ActivationRequest(request))
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
    /// other pixel as it is. Only they are reported to the server, which uploads only them. Without a last frame of the
    /// same size and scale it is a whole frame, as [`Client::draw`].
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
        let s = self
            .surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(ClientError::UnknownSurface)?;
        let size = s.size.ok_or(ClientError::NotConfigured)?;
        let mut lease = self.conn.lease(id.0, size, BufferFormat::Argb8888)?;
        let layout = lease.layout();
        let (pw, ph) = (layout.width(), layout.height());
        let scale = Scale::of(lease.scale());
        let kept = s
            .pixmap
            .as_ref()
            .is_some_and(|p| p.width() == pw && p.height() == ph)
            && s.drawn_scale == Some(scale);
        let mut pixmap = match s.pixmap.take() {
            Some(p) if p.width() == pw && p.height() == ph => p,
            _ => Pixmap::new(pw, ph).ok_or(ClientError::BufferSize {
                width: pw,
                height: ph,
            })?,
        };
        let damage: Option<Vec<PixelRect>> = match damage {
            Some(rects) if kept => Some(
                rects
                    .iter()
                    .filter_map(|r| clamp(pixel_rect(r, scale.factor()), pw, ph))
                    .collect(),
            ),
            _ => None,
        };
        match &damage {
            None => pixmap.fill(Color::TRANSPARENT),
            Some(rects) => {
                for r in rects {
                    clear(&mut pixmap, r);
                }
            }
        }
        paint(&mut pixmap, scale);
        // the buffer needs what this frame changed and what it missed of the frames shown since it last held one
        let regions: Option<Vec<PixelRect>> = damage
            .as_ref()
            .zip(lease.missed())
            .map(|(damage, missed)| damage.iter().chain(missed).copied().collect())
            // rectangles that add up to the buffer cost more than converting it once
            .filter(|rects: &Vec<PixelRect>| {
                rects
                    .iter()
                    .map(|r| u64::from(r.width) * u64::from(r.height))
                    .sum::<u64>()
                    < u64::from(pw) * u64::from(ph)
            });
        match regions {
            Some(rects) => {
                convert_rects(pixmap.data(), lease.pixels_mut(), layout.stride(), &rects)
            }
            None => rgba_to_argb8888(pixmap.data(), lease.pixels_mut()),
        }
        lease.present_damage(damage.as_deref())?;
        s.frame_pending = true;
        s.pixmap = Some(pixmap);
        s.drawn_scale = Some(scale);
        Ok(())
    }

    /// Flush requests and block until something needs the application's attention, the
    /// timeout (`None` = no limit) passes, a held key repeats or a [`Waker`] fires. The result may be empty after a
    /// timeout or a wake-up.
    pub fn wait(&mut self, timeout: Option<Duration>) -> Result<Vec<Event>, ClientError> {
        self.update()?;
        if self.events.is_empty() {
            let events = self.conn.wait(timeout)?;
            for event in events {
                self.translate(event);
            }
            self.follow_scroll_counts();
            self.update()?;
        }
        match self.fault.take() {
            Some(e) => Err(e),
            None => Ok(std::mem::take(&mut self.events)),
        }
    }

    /// The `wl_output` of `output`, for a request that names it.
    fn wl_output(&self, output: OutputId) -> Result<WlOutput, ClientError> {
        self.conn
            .outputs()
            .into_iter()
            .find(|o| o.id.registry_name() == output.0)
            .and_then(|o| self.conn.wl_output(o.id))
            .ok_or(ClientError::UnknownOutput)
    }

    fn surface(&self, id: SurfaceId) -> Result<&Surface, ClientError> {
        self.surfaces
            .iter()
            .find(|s| s.id == id)
            .ok_or(ClientError::UnknownSurface)
    }

    fn surface_mut(&mut self, id: SurfaceId) -> Result<&mut Surface, ClientError> {
        self.surfaces
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or(ClientError::UnknownSurface)
    }

    /// Follow the scroll counts once the seat has a pointer and the server offers them.
    fn follow_scroll_counts(&mut self) {
        if self.pointer_scroll.is_some() {
            return;
        }
        if let (Some(counts), Some(pointer)) = (&self.globals.scroll_counts, self.conn.pointer()) {
            self.pointer_scroll =
                Some(counts.get_scroll(&pointer, &self.conn.queue_handle(), Quiet));
        }
    }

    /// Report scale changes and surfaces that can take their requested frame.
    fn update(&mut self) -> Result<(), ClientError> {
        for s in self.surfaces.iter_mut().filter(|s| s.size.is_some()) {
            let scale = Scale::of(self.conn.scale(s.id.0)?);
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
            if s.wants_redraw
                && s.size.is_some()
                && !s.frame_pending
                && self.conn.buffer_free(s.id.0)?
            {
                s.wants_redraw = false;
                self.events.push(Event::Draw(s.id));
            }
        }
        Ok(())
    }

    /// A configure: the surface takes `size` and draws again.
    fn configured(&mut self, id: SurfaceId, size: (u32, u32)) {
        if let Ok(s) = self.surface_mut(id) {
            s.size = Some(size);
            s.wants_redraw = true;
            self.events.push(Event::Configured { surface: id, size });
        }
    }

    /// Turn a transport event into this crate's, keeping the state it carries.
    fn translate(&mut self, event: cupertino_wayland_client::Event) {
        use cupertino_wayland_client::Event as T;
        match event {
            T::Configure { window, configure } => {
                let id = SurfaceId(window.surface());
                let Ok(s) = self.surface_mut(id) else { return };
                if configure.decorations != Decorations::Server {
                    // the toolkit draws no title bar of its own: an undecorated window could not be moved or closed
                    self.fault(ClientError::Protocol(
                        "the server refused server-side decorations",
                    ));
                    return;
                }
                if let Role::Toplevel {
                    activated,
                    maximized,
                    ..
                } = &mut s.role
                {
                    *activated = configure.states.activated;
                    *maximized = configure.states.maximized;
                }
                let current = s.size.unwrap_or(s.requested_size);
                let size = configured_size(configure.size.unwrap_or((0, 0)), current);
                self.configured(id, size);
            }
            T::PopupConfigure { popup, configure } => {
                self.configured(
                    SurfaceId(popup.surface()),
                    (configure.width, configure.height),
                );
            }
            T::Close(window) => self.events.push(Event::Closed(SurfaceId(window.surface()))),
            T::PopupDone(popup) => self
                .events
                .push(Event::PopupDone(SurfaceId(popup.surface()))),
            T::Frame(callback) => {
                if let Ok(s) = self.surface_mut(SurfaceId(callback.surface)) {
                    s.frame_pending = false;
                }
            }
            // the scale is read for every configured surface in `update`
            T::ScaleChanged { .. } => {}
            T::Pointer { surface, event } => self.pointer(SurfaceId(surface), event),
            T::Keyboard { surface, event } => self.keyboard(SurfaceId(surface), event),
            T::Selection(offer) => {
                self.selection = offer;
                self.own_text = None;
            }
            T::OutputsChanged => self.events.push(Event::OutputsChanged),
            T::ActivationToken { request, token } => self.events.push(Event::ActivationToken {
                request: ActivationRequest(request),
                token,
            }),
            T::Extension(event) => {
                if let Ok(emitted) = event.downcast::<Emitted>() {
                    self.extension(&emitted);
                }
            }
        }
    }

    fn pointer(&mut self, surface: SurfaceId, event: cupertino_wayland_client::PointerEvent) {
        use cupertino_wayland_client::PointerEvent as T;
        let events = match event {
            T::Enter(at) => vec![PointerEvent::Enter(at)],
            T::Motion(at) => vec![PointerEvent::Motion(at)],
            T::Leave => vec![PointerEvent::Leave],
            T::Button {
                button,
                pressed,
                position,
            } => vec![PointerEvent::Button {
                button,
                pressed,
                position,
            }],
            T::Scroll { dx, dy, position } => self.scroll.split(dx, dy, position),
        };
        self.events.extend(
            events
                .into_iter()
                .map(|event| Event::Pointer { surface, event }),
        );
    }

    fn keyboard(&mut self, surface: SurfaceId, event: KeyboardEvent) {
        match event {
            KeyboardEvent::Enter => self.key_focus = Some(surface),
            KeyboardEvent::Leave => {
                if self.key_focus.take().is_some() {
                    self.events.push(Event::KeyboardLeave(surface));
                }
            }
            KeyboardEvent::Key {
                code,
                pressed,
                character,
                mods,
                ..
            } => self.events.push(Event::Key {
                surface,
                key: code,
                pressed,
                mods: mods.into(),
                character,
            }),
        }
    }

    fn extension(&mut self, emitted: &Emitted) {
        match emitted {
            Emitted::Configure { surface, size } => {
                let Ok(s) = self.surface(*surface) else {
                    return;
                };
                let size = configured_size(*size, s.requested_size);
                if size.0 == 0 || size.1 == 0 {
                    return self.fault(ClientError::Protocol(
                        "configure without a size for a surface that requested none",
                    ));
                }
                self.configured(*surface, size);
            }
            Emitted::Lock { generation, locked } => {
                // an event of a lock replaced since belongs to no one
                if let Some(lock) = self
                    .session_lock
                    .as_mut()
                    .filter(|lock| lock.generation == *generation)
                {
                    lock.state = if *locked {
                        LockState::Locked
                    } else {
                        LockState::Finished
                    };
                }
            }
            Emitted::Idle(idle) => self.idled = *idle,
            Emitted::ScrollCount { vertical, count } => self.scroll.set(*vertical, *count),
            Emitted::Session(event) => {
                match event {
                    Event::Appearance { dark } => self.session.dark = *dark,
                    Event::DisplayOptions(options) => self.session.display = *options,
                    Event::SessionLock { locked } => self.session.locked = *locked,
                    _ => {}
                }
                if self.session.settled {
                    self.events.push(event.clone());
                }
            }
            Emitted::Event(event) => self.events.push(event.clone()),
        }
    }

    /// End the next wait with `e`; the first error wins.
    fn fault(&mut self, e: ClientError) {
        self.fault.get_or_insert(e);
    }
}

/// The popup's top-left corner at the geometry's, whatever the parent's size.
fn placement(g: &PopupGeometry, grab: bool) -> PopupPlacement {
    PopupPlacement {
        anchor_rect: Rect::new(
            f64::from(g.x),
            f64::from(g.y),
            f64::from(g.width),
            f64::from(g.height),
        ),
        size: (g.width as u32, g.height as u32),
        anchor: PopupAnchor::TopLeft,
        gravity: Gravity::BottomRight,
        constraint_adjustment: ConstraintAdjustment::empty(),
        grab,
    }
}

/// The size a surface takes from its configure: a zero extent leaves it to the client, which keeps the `current` one
/// (the requested size before the first configure).
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

/// `r` cut to a `width` × `height` buffer; `None` when nothing of it is inside.
fn clamp(r: PixelRect, width: u32, height: u32) -> Option<PixelRect> {
    let (x1, y1) = (
        r.x.saturating_add(r.width).min(width),
        r.y.saturating_add(r.height).min(height),
    );
    (r.x < x1 && r.y < y1).then(|| PixelRect {
        x: r.x,
        y: r.y,
        width: x1 - r.x,
        height: y1 - r.y,
    })
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
    (
        left,
        top,
        right.saturating_sub(left),
        bottom.saturating_sub(top),
    )
}

/// `bytes` as text: invalid sequences replaced, a character cut by the read limit dropped.
fn text_of(bytes: &[u8]) -> String {
    let whole = match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        _ => bytes,
    };
    String::from_utf8_lossy(whole).into_owned()
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

    #[test]
    fn damage_is_cut_to_the_buffer() {
        let r = PixelRect {
            x: 90,
            y: 5,
            width: 20,
            height: 10,
        };
        assert_eq!(
            clamp(r, 100, 100),
            Some(PixelRect {
                x: 90,
                y: 5,
                width: 10,
                height: 10
            })
        );
        assert_eq!(clamp(r, 90, 100), None);
    }

    #[test]
    fn a_character_cut_by_the_read_limit_is_dropped() {
        assert_eq!(text_of("aé".as_bytes()), "aé");
        assert_eq!(text_of(&"aé".as_bytes()[..2]), "a");
        assert_eq!(text_of(b"a\xFFb"), "a\u{FFFD}b");
    }
}
