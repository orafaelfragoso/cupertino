//! The shell processes' client API on the transport (`cupertino-wayland-client`): the protocols are extensions on its
//! queue (`cupertino-protocol-app`, `cupertino-shell-client`), surfaces draw through `cupertino-ui-wayland`, and the
//! events are translated into this crate's. The API stays until the shell processes move to those crates, and goes
//! away then.

use crate::event::{ActivationRequest, Event, PointerEvent, ScrollCounts};
use crate::keyboard::Modifiers;
use crate::output::OutputInfo;
use crate::scale::Scale;
use crate::{Anchor, ClientError, KeyboardInteractivity, LayerSurfaceConfig};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_geometry::Rect;
use cupertino_protocol_app::{
    AppEffects, AppEvent, Appearance, Backdrop, PointerScroll, ScrollCounts as ScrollGlobal,
};
use cupertino_shell_client::{
    Dock, IdleNotifier, IdleWatch, LayerShell, LayerSurface, MenuBar, Session as SessionObserver,
    SessionChange, SessionLockManager, SessionState, Shell, ShellEvent,
};
use cupertino_ui_wayland::{clipboard::Clipboard, popup, Ready, ShmPresenter};
use cupertino_wayland_client::{
    Capability, Connection, CursorShape, Decorations, Error, KeyboardEvent, OutputId, Popup,
    Toplevel, ToplevelConfig as WindowConfig,
};
use std::time::Duration;
use tiny_skia::Pixmap;

pub use cupertino_wayland_client::Waker;

/// Handle of a surface of a [`Client`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub(crate) cupertino_wayland_client::SurfaceId);

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
    Layer(LayerSurface),
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
    presenter: ShmPresenter,
}

impl Surface {
    fn new(id: SurfaceId, role: Role, requested_size: (u32, u32)) -> Self {
        Surface {
            id,
            role,
            requested_size,
            presenter: ShmPresenter::new(id.0),
        }
    }
}

struct SessionLock {
    lock: cupertino_shell_client::SessionLock,
    state: LockState,
}

/// What the server last said about the session: `cupertino_session_v1` on a shell connection, else the appearance and
/// display options of `cupertino_appearance_v1` (the lock state stays false).
struct Session {
    /// Kept alive: the server stops sending when the object is destroyed.
    _session: Option<SessionObserver>,
    _appearance: Option<Appearance>,
    state: SessionState,
    /// The state at connect is read; later changes also queue an [`Event`].
    settled: bool,
}

/// The extension globals.
struct Globals {
    /// Absent from a server that does not offer it to this connection (a sandboxed application); only
    /// [`Client::layer_surface`] needs it.
    layer_shell: Option<LayerShell>,
    effects: AppEffects,
    /// Bound on a shell connection only ([`Client::connect_shell`]).
    shell: Option<Shell>,
    session_lock_manager: Option<SessionLockManager>,
    idle_notifier: Option<IdleNotifier>,
    /// Offered by the server: the scroll count of the pointer's scrolls (`cupertino_scroll_v1`).
    scroll_counts: Option<ScrollGlobal>,
}

/// Connection to the server and the surfaces of one shell process.
pub struct Client {
    conn: Connection,
    globals: Globals,
    session: Session,
    session_lock: Option<SessionLock>,
    idle_notification: Option<IdleWatch>,
    /// The seat went idle since [`Client::take_idle`] last ran.
    idled: bool,
    /// Kept alive: the server stops sending when the object is destroyed.
    pointer_scroll: Option<PointerScroll>,
    scroll: ScrollCounts,
    surfaces: Vec<Surface>,
    key_focus: Option<SurfaceId>,
    clipboard: Clipboard,
    events: Vec<Event>,
    fault: Option<ClientError>,
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
    /// observer reports the lock state and the roles the server gave the process can be asked for. Unlike
    /// [`Client::connect`] it requires the layer shell: a shell process fails at start on a server without it.
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
        let effects = AppEffects::bind(&mut conn)
            .ok_or(ClientError::MissingGlobal("cupertino_app_effects_v1"))?;
        // the session's state at connect is read before the process draws anything
        let (shell, session, appearance, state, early) = if shell_roles {
            let shell =
                Shell::bind(&mut conn).ok_or(ClientError::MissingGlobal("cupertino_shell_v1"))?;
            let (session, state, early) = shell.observe(&mut conn)?;
            (Some(shell), Some(session), None, state, early)
        } else {
            let appearance = effects.appearance(&conn);
            conn.roundtrip()?;
            let early = conn.wait(Some(Duration::ZERO))?;
            (None, None, Some(appearance), SessionState::default(), early)
        };
        let layer_shell = LayerShell::bind(&mut conn);
        if shell_roles && layer_shell.is_none() {
            return Err(ClientError::MissingGlobal("zwlr_layer_shell_v1"));
        }
        let globals = Globals {
            layer_shell,
            effects,
            shell,
            session_lock_manager: SessionLockManager::bind(&mut conn),
            idle_notifier: IdleNotifier::bind(&mut conn),
            scroll_counts: ScrollGlobal::bind(&mut conn),
        };
        let mut client = Client {
            conn,
            globals,
            session: Session {
                _session: session,
                _appearance: appearance,
                state,
                settled: false,
            },
            session_lock: None,
            idle_notification: None,
            idled: false,
            pointer_scroll: None,
            scroll: ScrollCounts::default(),
            surfaces: Vec::new(),
            key_focus: None,
            clipboard: Clipboard::default(),
            events: Vec::new(),
            fault: None,
        };
        for event in early {
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
        Ok(self.conn.surface_outputs(id.0)?)
    }

    /// Create a layer surface. It is mapped and sized by the first [`Event::Configured`], which
    /// is followed by [`Event::Draw`].
    pub fn layer_surface(&mut self, config: &LayerSurfaceConfig) -> Result<SurfaceId, ClientError> {
        let layer = self
            .globals
            .layer_shell
            .as_ref()
            .ok_or(ClientError::MissingGlobal("zwlr_layer_shell_v1"))?
            .layer_surface(&mut self.conn, config)?;
        let id = SurfaceId(layer.surface());
        self.surfaces
            .push(Surface::new(id, Role::Layer(layer), config.size));
        Ok(id)
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
            self.globals
                .effects
                .unify_titlebar(&self.conn, window.surface())?;
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
        Ok(self.clipboard.set_text(&mut self.conn, text)?)
    }

    /// The clipboard's text for a paste, `None` when it holds no text. Reading another client's selection waits up to
    /// a second for it.
    pub fn clipboard_text(&mut self) -> Result<Option<String>, ClientError> {
        Ok(self.clipboard.text(&mut self.conn)?)
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
        let placement = popup::placement(rect, true)?;
        self.conn.input_serial().ok_or(ClientError::NoInputSerial)?;
        if let Role::Lock = self.surface(parent)?.role {
            return Err(ClientError::NotLayerSurface);
        }
        let popup = self.conn.popup(parent.0, &placement)?;
        let id = SurfaceId(popup.surface());
        self.surfaces.push(Surface::new(
            id,
            Role::Popup { popup, parent },
            placement.size,
        ));
        Ok(id)
    }

    /// Move or resize a popup to `rect` (parent-local, as for [`Client::popup`]). The new size
    /// arrives as [`Event::Configured`].
    pub fn reposition_popup(&mut self, id: SurfaceId, rect: &Rect) -> Result<(), ClientError> {
        let placement = popup::placement(rect, true)?;
        let Role::Popup { popup, .. } = self.surface(id)?.role else {
            return Err(ClientError::NotPopup);
        };
        Ok(self.conn.reposition_popup(popup, &placement)?)
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
        self.surface(id)?
            .presenter
            .size()
            .ok_or(ClientError::NotConfigured)
    }

    /// Ask for an [`Event::Draw`] as soon as the surface can take a frame.
    pub fn request_redraw(&mut self, id: SurfaceId) -> Result<(), ClientError> {
        self.surface_mut(id)?.presenter.request_redraw();
        Ok(())
    }

    fn layer(&self, id: SurfaceId) -> Result<&LayerSurface, ClientError> {
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
        self.layer(id)?;
        if let Role::Layer(layer) = &mut self.surface_mut(id)?.role {
            layer.set_placement(anchor, size);
        }
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
            .as_mut()
            .ok_or(ClientError::MissingGlobal("ext_session_lock_manager_v1"))?;
        if let Some(lock) = &self.session_lock {
            if lock.state != LockState::Finished {
                return Err(ClientError::SessionLockActive);
            }
        }
        if let Some(previous) = self.session_lock.take() {
            previous.lock.destroy();
        }
        self.session_lock = Some(SessionLock {
            lock: manager.lock(&self.conn),
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
        self.session.state.dark
    }

    /// The accessibility display options, as of the last `cupertino_session_v1.display_options`. The value at connect is
    /// read before [`Client::connect`] returns; later changes arrive as [`Event::DisplayOptions`].
    pub fn display_options(&self) -> DisplayOptions {
        self.session.state.display
    }

    /// Whether the server holds the session locked, as of the last `locked` or `unlocked` event. The value at connect is
    /// read before [`Client::connect`] returns; later changes arrive as [`Event::SessionLock`].
    pub fn session_locked(&self) -> bool {
        self.session.state.locked
    }

    /// Create the lock surface of `output`. It is sized by the first [`Event::Configured`], which is followed by
    /// [`Event::Draw`]; it takes keyboard and pointer input once the session is locked. One per output.
    pub fn lock_surface(&mut self, output: OutputId) -> Result<SurfaceId, ClientError> {
        self.conn
            .wl_output(output)
            .ok_or(ClientError::UnknownOutput)?;
        let lock = match &self.session_lock {
            Some(lock) if lock.state != LockState::Finished => &lock.lock,
            _ => return Err(ClientError::NoSessionLock),
        };
        let id = lock.lock_surface(&mut self.conn, output)?;
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
        lock.lock.unlock();
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
        self.idle_notification = None;
        self.idled = false;
        self.idle_notification = Some(notifier.watch(&self.conn, &seat, millis));
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
        Ok(self.globals.effects.backdrop(&self.conn, id.0)?)
    }

    fn shell_roles(&self) -> Result<&Shell, ClientError> {
        self.globals
            .shell
            .as_ref()
            .ok_or(ClientError::MissingGlobal("cupertino_shell_v1"))
    }

    /// Give a layer surface the menu-bar role of `output`. The server then sends the active
    /// application and its menus as [`Event::MenuBar`], the first group right away.
    pub fn menu_bar(
        &mut self,
        surface: SurfaceId,
        output: OutputId,
    ) -> Result<MenuBar, ClientError> {
        Ok(self
            .shell_roles()?
            .menu_bar(&self.conn, surface.0, output)?)
    }

    /// Give a layer surface the dock role of `output`. The server then lists the windows of the
    /// session and reports changes as [`Event::Dock`], the first group right away.
    pub fn dock(&mut self, surface: SurfaceId, output: OutputId) -> Result<Dock, ClientError> {
        Ok(self.shell_roles()?.dock(&self.conn, surface.0, output)?)
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
        Ok(s.presenter.draw(&mut self.conn, damage, |pixmap, scale| {
            paint(pixmap, Scale::of(scale))
        })?)
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
            self.pointer_scroll = Some(counts.watch(&self.conn, &pointer));
        }
    }

    /// Report scale changes and surfaces that can take their requested frame.
    fn update(&mut self) -> Result<(), ClientError> {
        let mut draws = Vec::new();
        for s in &mut self.surfaces {
            loop {
                match s.presenter.poll(&self.conn)? {
                    Ready::Idle => break,
                    Ready::ScaleChanged(scale) => self.events.push(Event::ScaleChanged {
                        surface: s.id,
                        scale: Scale::of(scale),
                    }),
                    Ready::Draw => draws.push(Event::Draw(s.id)),
                }
            }
        }
        self.events.extend(draws);
        Ok(())
    }

    /// A configure: the surface takes `size` and draws again.
    fn configured(&mut self, id: SurfaceId, size: (u32, u32)) {
        if let Ok(s) = self.surface_mut(id) {
            s.presenter.configured(size);
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
                let current = s.presenter.size().unwrap_or(s.requested_size);
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
                    s.presenter.frame_done();
                }
            }
            // the scale is read for every configured surface in `update`
            T::ScaleChanged { .. } => {}
            T::Pointer { surface, event } => self.pointer(SurfaceId(surface), event),
            T::Keyboard { surface, event } => self.keyboard(SurfaceId(surface), event),
            T::Selection(offer) => self.clipboard.selection_changed(offer),
            T::OutputsChanged => self.events.push(Event::OutputsChanged),
            T::ActivationToken { request, token } => self.events.push(Event::ActivationToken {
                request: ActivationRequest(request),
                token,
            }),
            T::Extension(event) => {
                if let Some(change) = self.session.state.apply(&event) {
                    return self.session_changed(match change {
                        SessionChange::Appearance { dark } => Event::Appearance { dark },
                        SessionChange::DisplayOptions(options) => Event::DisplayOptions(options),
                        SessionChange::Lock { locked } => Event::SessionLock { locked },
                    });
                }
                match event.downcast::<ShellEvent>() {
                    Ok(event) => self.shell_event(&event),
                    Err(event) => {
                        if let Ok(event) = event.downcast::<AppEvent>() {
                            self.app_event(*event);
                        }
                    }
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

    fn shell_event(&mut self, event: &ShellEvent) {
        match event {
            ShellEvent::Configure { surface, size } => {
                let surface = SurfaceId(*surface);
                let Ok(s) = self.surface(surface) else {
                    return;
                };
                let size = match &s.role {
                    Role::Layer(layer) => match layer.configured(*size) {
                        Ok(size) => size,
                        Err(e) => return self.fault(e.into()),
                    },
                    _ => *size,
                };
                self.configured(surface, size);
            }
            ShellEvent::Closed(surface) => self.events.push(Event::Closed(SurfaceId(*surface))),
            ShellEvent::Lock { generation, locked } => {
                // an event of a lock replaced since belongs to no one
                if let Some(lock) = self
                    .session_lock
                    .as_mut()
                    .filter(|lock| lock.lock.generation() == *generation)
                {
                    lock.state = if *locked {
                        LockState::Locked
                    } else {
                        LockState::Finished
                    };
                }
            }
            ShellEvent::Idle(idle) => self.idled = *idle,
            // taken by the session state in `translate`
            ShellEvent::SessionLock { .. } => {}
            ShellEvent::LockRequested { explicit } => self.session_changed(Event::LockRequested {
                explicit: *explicit,
            }),
            ShellEvent::LogOutRequested => self.session_changed(Event::LogOutRequested),
            ShellEvent::MenuBar { surface, update } => self.events.push(Event::MenuBar {
                surface: SurfaceId(*surface),
                update: update.clone(),
            }),
            ShellEvent::Dock { surface, update } => self.events.push(Event::Dock {
                surface: SurfaceId(*surface),
                update: update.clone(),
            }),
        }
    }

    fn app_event(&mut self, event: AppEvent) {
        match event {
            // taken by the session state in `translate`
            AppEvent::Appearance { .. } | AppEvent::DisplayOptions(_) => {}
            AppEvent::ScrollCount { vertical, count } => self.scroll.set(vertical, count),
        }
    }

    /// The session's state at connect is read silently; later changes queue `event`.
    fn session_changed(&mut self, event: Event) {
        if self.session.settled {
            self.events.push(event);
        }
    }

    /// End the next wait with `e`; the first error wins.
    fn fault(&mut self, e: ClientError) {
        self.fault.get_or_insert(e);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_extent_in_a_configure_keeps_the_current_size() {
        assert_eq!(configured_size((0, 0), (920, 436)), (920, 436));
        assert_eq!(configured_size((1200, 0), (920, 436)), (1200, 436));
        assert_eq!(configured_size((640, 480), (920, 436)), (640, 480));
    }
}
