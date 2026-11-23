//! Compositor state: Smithay protocol states, the window model, outputs, shell and input state,
//! plus the glue that turns model effects into protocol requests.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use cupertino_wm::{
    AppId, Command, Effect, MapParams, OutputId, Point as MPoint, Rect, Size, WindowId, Wm,
    WmConfig,
};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::desktop::{layer_map_for_output, PopupManager, Window, WindowSurfaceType};
use smithay::input::keyboard::XkbConfig;
use smithay::input::pointer::{CursorImageStatus, PointerHandle};
use smithay::input::{Seat, SeatState};
use smithay::output::{Mode as OutputMode, Output, PhysicalProperties, Scale as OutputScale};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, LoopHandle, LoopSignal, Mode, PostAction};
use smithay::reexports::wayland_protocols::wp::content_type::v1::server::wp_content_type_v1;
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{
    ClientData, ClientId, DisconnectReason, GlobalId,
};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::BindError;
use smithay::reexports::wayland_server::Resource;
use smithay::reexports::wayland_server::{Display, DisplayHandle};
use smithay::utils::{
    Clock, Logical, Monotonic, Point, Rectangle, Serial, Size as SmSize, Transform, SERIAL_COUNTER,
};
use smithay::wayland::compositor::{with_states, CompositorClientState, CompositorState};
use smithay::wayland::content_type::{ContentTypeState, ContentTypeSurfaceCachedState};
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufGlobal, DmabufState};
use smithay::wayland::foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListState};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_gestures::PointerGesturesState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::shell::wlr_layer::{Layer, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::dialog::XdgDialogState;
use smithay::wayland::shell::xdg::{ToplevelSurface, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconManager;

use crate::app_effects_v1::AppEffectsV1Global;
use crate::authorization::{ClientCapabilitySet, Holder};
use crate::backend::Backend;
use crate::config::Config;
use crate::cursor::CursorLoader;
use crate::dbus::DbusHandle;
use crate::decor::Decor;
use crate::desktop::DesktopIndex;
use crate::dock::Docks;
use crate::fx::Effects;
use crate::input::{ActionContext, ActionPlan, KeyRouter, ShellOp};
use crate::kbd::CapsDebounce;
use crate::menu_bar::{cut_text, MenuBars};
use crate::overlays::Overlays;
use crate::policy;
use crate::protocols::MenuProtocols;
use crate::render::scene::RenderResources;
use crate::render::ElementCache;
use crate::scroll_v1::ScrollObservers;
use crate::session_v1::SessionObservers;
use crate::shell;
use crate::shell_v1::ShellV1Global;
use crate::supervisor::Supervisor;
use crate::toplevel_icon::WindowIcon;
use crate::window_manager::WindowManagers;
use cupertino_assets::Assets;

#[derive(Debug)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    /// When the client connected (the state is made as the connection is accepted).
    pub connected_at: std::time::Instant,
    /// The client has mapped a toplevel: its next one is not the first.
    pub mapped_toplevel: std::sync::atomic::AtomicBool,
    /// The compositor's own connection (the accent picker's input method bridge).
    pub internal: bool,
    /// What the server lets the client do: set only for the processes the server started on a connection of their own
    /// (`crate::shell_connection`), empty for a client of the public socket.
    pub capabilities: ClientCapabilitySet,
    /// The process the connection was made for, held from its spawn ([`crate::shell_connection::bind_holder`]): the
    /// capabilities end with it, so a connection kept alive by another process holds nothing. Until a holder is
    /// bound the connection holds nothing.
    pub holder: std::sync::OnceLock<crate::authorization::Holder>,
    /// How many material backdrops the client holds (`cupertino_app_effects_v1.get_backdrop`).
    pub backdrops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// The client connected through a security context (`wp_security_context_v1`): a sandboxed application. It is not
    /// offered the globals that read or change what other clients own ([`crate::authorization::unsandboxed`]).
    pub sandboxed: bool,
    /// The process that connected, from the socket (`SO_PEERPIDFD`) as the connection is accepted; `None` for a
    /// connection the server made itself, whose process is its holder instead ([`ClientState::process`]). The owner of a
    /// bus name the client hands the server is checked against that process (`crate::owner_check`).
    pub peer: Option<cupertino_peer::ProcessHandle>,
    /// The process behind the connection, read off the loop once it is known ([`ClientState::process_facts`]).
    pub process_lookup: Arc<crate::process::ProcessLookup>,
    /// How many of the client's process lookup threads are alive (`crate::process::CONNECTION_LOOKUP_LIMIT`).
    pub process_lookups: crate::process::Alive,
    /// How many of the client's owner identification threads are alive (`crate::dbus::CLIENT_IDENTIFY_LIMIT`).
    pub owner_identifying: crate::dbus::Identifying,
    /// The commits of the client held back until their buffers are ready ([`crate::acquire_waits`]).
    pub acquire_waits: std::sync::Arc<crate::acquire_waits::AcquireWaits>,
    /// Where the client's id goes when it disconnects, set once it mapped a toplevel ([`Cupertino::forget_gone_processes`]).
    pub gone: std::sync::OnceLock<GoneClients>,
    /// The client bound `cupertino_app_effects_v1`, which every window of the desktop's toolkit binds: it reads the
    /// command key itself, so its chords reach it as pressed and the command key's W stays the compositor's close
    /// (`Cupertino::key_client_class`).
    pub kit: std::sync::atomic::AtomicBool,
    /// The server's own handle of its end of the connection ([`insert_client`]), shut down when the connection ends
    /// ([`ClientState::disconnected`]): a client the server gives up on outside its read loop (its events no longer fit,
    /// [`CLIENT_EVENT_BUFFER`]) is otherwise left with an open socket that never speaks again until it writes, and one
    /// waiting for a frame callback waits for good.
    pub socket: std::sync::Mutex<Option<std::os::fd::OwnedFd>>,
    /// Set when this connection was handed out to a shell component through `cupertino_host_component_v1.connection`.
    pub component: Option<crate::host::ComponentConnection>,
}

/// The clients that mapped a toplevel and have disconnected since the last tick.
pub type GoneClients = std::sync::Arc<std::sync::Mutex<Vec<ClientId>>>;

impl Default for ClientState {
    fn default() -> Self {
        Self {
            compositor_state: CompositorClientState::default(),
            connected_at: std::time::Instant::now(),
            mapped_toplevel: std::sync::atomic::AtomicBool::new(false),
            internal: false,
            capabilities: ClientCapabilitySet::NONE,
            holder: std::sync::OnceLock::new(),
            backdrops: std::sync::Arc::default(),
            sandboxed: false,
            peer: None,
            process_lookup: Arc::default(),
            process_lookups: Arc::default(),
            owner_identifying: std::sync::Arc::default(),
            acquire_waits: std::sync::Arc::default(),
            gone: std::sync::OnceLock::new(),
            kit: std::sync::atomic::AtomicBool::new(false),
            socket: std::sync::Mutex::new(None),
            component: None,
        }
    }
}

impl ClientState {
    /// The process behind the connection: the one that connected to the socket, else the shell process the server
    /// started for it ([`crate::shell_connection::bind_holder`]), or the server itself for a client in its own process.
    /// `None` when none is known, as for a shell connection whose process has not started.
    pub fn process(&self) -> Option<cupertino_peer::ProcessHandle> {
        if let Some(peer) = &self.peer {
            return peer.try_clone().ok();
        }
        match self.holder.get()? {
            Holder::Process(process) => process.try_clone().ok(),
            Holder::Detached(_) => cupertino_peer::ProcessHandle::open(std::process::id()).ok(),
        }
    }

    /// Start reading the process behind the connection off the loop ([`crate::process::read_facts`]), once it is known
    /// ([`ClientState::process`]) and unless a reading started already.
    pub fn look_up_process(&self) {
        if !self.process_lookup.is_unstarted() {
            return;
        }
        let Some(process) = self.process() else {
            return;
        };
        self.process_lookup.start(
            crate::process::Lookups::shared(),
            (
                &self.process_lookups,
                crate::process::CONNECTION_LOOKUP_LIMIT,
            ),
            move || crate::process::read_facts(&process),
        );
    }

    /// The process behind the connection, read once off the loop ([`ClientState::look_up_process`]); the first use while
    /// it is read waits until [`crate::process::IDENTITY_WAIT`] has passed. `None` when it is not known, could not be
    /// named or was not read in time: it then names no process and allows nothing.
    pub fn process_facts(&self) -> Option<Arc<crate::process::ProcessFacts>> {
        self.process_facts_until(std::time::Instant::now() + crate::process::IDENTITY_WAIT)
    }

    /// [`ClientState::process_facts`], waiting until `deadline` at most.
    pub fn process_facts_until(
        &self,
        deadline: std::time::Instant,
    ) -> Option<Arc<crate::process::ProcessFacts>> {
        self.look_up_process();
        self.process_lookup.wait_until(deadline)
    }
}

/// Add the client on `stream` with `state`, which keeps a handle of the server's end of the socket
/// ([`ClientState::socket`]) to shut it down when the connection ends. The process that connected, when the socket names
/// it ([`ClientState::peer`]), is read off the loop from now ([`ClientState::look_up_process`]).
pub fn insert_client(
    display: &DisplayHandle,
    stream: std::os::unix::net::UnixStream,
    state: ClientState,
) -> std::io::Result<Arc<ClientState>> {
    let (_, data) = insert_client_pair(display, stream, state)?;
    Ok(data)
}

/// Like [`insert_client`], returning the inserted Wayland [`Client`] handle.
pub fn insert_client_handle(
    display: &DisplayHandle,
    stream: std::os::unix::net::UnixStream,
    state: ClientState,
) -> std::io::Result<smithay::reexports::wayland_server::Client> {
    let (client, _) = insert_client_pair(display, stream, state)?;
    Ok(client)
}

fn insert_client_pair(
    display: &DisplayHandle,
    stream: std::os::unix::net::UnixStream,
    mut state: ClientState,
) -> std::io::Result<(smithay::reexports::wayland_server::Client, Arc<ClientState>)> {
    state.socket = std::sync::Mutex::new(Some(stream.try_clone()?.into()));
    let state = Arc::new(state);
    let client = display.clone().insert_client(stream, state.clone())?;
    if state.peer.is_some() {
        state.look_up_process();
    }
    Ok((client, state))
}

/// The process that connected `stream`, held by the pidfd the kernel names for the socket.
pub fn client_process(
    stream: &std::os::unix::net::UnixStream,
) -> Option<cupertino_peer::ProcessHandle> {
    cupertino_peer::ProcessHandle::of_peer(stream)
        .inspect_err(
            |e| tracing::debug!(error = %e, "no process for the connection: no bus name it gives is taken for its own"),
        )
        .ok()
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        log_disconnect(self, &client_id, &reason);
        // the client sees the end at once, after what was already sent to it (a protocol error), whether or not the
        // server's read loop closes the connection soon; the read loop then finds it closed too
        let socket = self
            .socket
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(socket) = socket {
            let _ = rustix::net::shutdown(&socket, rustix::net::Shutdown::Both);
        }
        self.acquire_waits.close();
        if let Some(gone) = self.gone.get() {
            gone.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(client_id);
        }
    }
}

/// Says why a client's connection ended, naming its process: a protocol error (the server's or the client's fault)
/// with its object, code and message at WARN; a closed connection at INFO for a shell process the server started (the
/// client closed it, or the server gave up on a client that read nothing for too long, [`CLIENT_EVENT_BUFFER`]), at
/// DEBUG for any other client.
fn log_disconnect(state: &ClientState, client: &ClientId, reason: &DisconnectReason) {
    let facts = state.process_lookup.known();
    let pid = match &facts {
        Some(facts) => Some(facts.process.0),
        None => state.process().map(|p| p.pid),
    };
    let program = facts.as_deref().and_then(program_name);
    let program = program.as_deref().unwrap_or("unknown");
    match reason {
        DisconnectReason::ProtocolError(error) => tracing::warn!(
            ?client,
            ?pid,
            program,
            interface = %error.object_interface,
            object = error.object_id,
            code = error.code,
            message = %error.message,
            "a client was disconnected for a protocol error"
        ),
        DisconnectReason::ConnectionClosed
            if matches!(state.holder.get(), Some(Holder::Process(_))) =>
        {
            tracing::info!(
                ?client,
                ?pid,
                program,
                "a shell process's connection closed"
            )
        }
        DisconnectReason::ConnectionClosed => {
            tracing::debug!(?client, ?pid, program, "a client's connection closed")
        }
    }
}

/// The file name of the program a connection's process was read to run ([`crate::process::ProcessFacts::program`]); a
/// diagnostic names it, never waiting for `/proc`.
pub fn program_name(facts: &crate::process::ProcessFacts) -> Option<String> {
    Some(
        facts
            .program
            .as_ref()?
            .file_name()?
            .to_string_lossy()
            .into_owned(),
    )
}

/// The window model's minimize and restore timing for the minimize effect of `effects`: the effect generators carry their
/// own easing, so the model's progress is linear over the effect's duration each way (the bend into the slot and the
/// unfold back out of it); without an effect the model's own curves.
pub fn set_minimize_timing(cfg: &mut WmConfig, effects: crate::fx::EffectsConfig) {
    let defaults = WmConfig::default().animation;
    let (minimizing, restoring) = match effects.minimize {
        Some(kind) => {
            let linear = |duration| cupertino_animation::Curve::Timed {
                duration,
                bezier: cupertino_theme::animation::LINEAR_CURVE,
            };
            (
                linear(kind.duration_s(false)),
                linear(kind.restore_duration_s(false)),
            )
        }
        None => (defaults.minimizing, defaults.restoring),
    };
    cfg.animation.minimizing = minimizing;
    cfg.animation.restoring = restoring;
}

/// `XDG_CURRENT_DESKTOP` of the clients and shell processes the server starts: the session's
/// desktop-specific files (`cupertino-mimeapps.list`, `OnlyShowIn=Cupertino`) apply to them.
pub const CURRENT_DESKTOP: &str = "Cupertino";

/// The most bytes of events the server holds for a client that is not reading them, beyond what its socket holds, before
/// it gives the client up and closes the connection. A client busy drawing a frame reads nothing meanwhile, and a
/// pointer reporting a thousand times a second sends it some 30 KiB a second, so the socket alone (a few hundred
/// messages) is full within half a second; this holds about half a minute of it.
pub const CLIENT_EVENT_BUFFER: usize = 1 << 20;

/// The layer-shell namespace of the session agent's panels: the log out, restart and shut down confirmations, the power
/// key's panel and the panel that interrupts a log out. They stand above the windows as alerts on glass with an alert's
/// shadow (`render::scene`), take the keyboard when they appear and give it up to a window the person presses
/// ([`Cupertino::release_engaged_layer`]): they are no modal dialogs.
pub const SESSION_PANEL_NAMESPACE: &str = "login-panel";

/// How long a global that went away can still be bound, for the clients that bind it before they hear that it is gone.
pub const RETIRED_GLOBAL_LIFETIME: std::time::Duration = std::time::Duration::from_secs(5);

/// How a window keeps covering the frame a frame animation shrinks it to: its content is drawn at its own size and
/// cropped to the frame, so the client keeps a size that covers every animated frame until the animation ends.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Shrink {
    #[default]
    None,
    /// The model's configure, sent when the animation ends.
    Deferred {
        frame: Rect,
        states: cupertino_wm::ToplevelStates,
    },
    /// The deferred configure went out with this serial: the window stays cropped to its frame until a commit acks it,
    /// whatever size the client then draws (the size is a hint to a floating toplevel).
    Awaiting(smithay::utils::Serial),
}

/// One toplevel as the compositor sees it.
pub struct WindowEntry {
    pub id: WindowId,
    pub window: Window,
    pub toplevel: ToplevelSurface,
    /// Known to the window model (has committed a buffer).
    pub mapped: bool,
    pub server_side: bool,
    /// Title bar style the client chose (`cupertino_app_effects_v1.set_titlebar_style`); it applies while the
    /// window is `server_side`.
    pub titlebar: cupertino_server_render::decorations::TitlebarStyle,
    pub foreign: Option<ForeignToplevelHandle>,
    pub modal: bool,
    /// Space around the surface reserved by server-side decorations (see `WindowStyle::ssd_insets`).
    pub decor_insets: crate::config::Insets,
    /// Configured full screen: the decorations are hidden and the client gets the whole frame.
    pub fullscreen: bool,
    pub shrink: Shrink,
    /// What the client's own frame is as of its last commit ([`crate::frame_fill`]): an opaque rectangle up to corners
    /// no rounder than the window's, or a transparent or shaped window.
    pub frame: crate::frame_fill::Frame,
    /// The application the launch behind the window named: the launch of the activation token it mapped with or was
    /// first activated with ([`crate::handlers::TokenGrant::launch`], a shell process's token only); the docks file the
    /// window under it when its own application id names no application.
    pub launch: Option<String>,
}

impl WindowEntry {
    /// The style of the title bar revealed over a full-screen window: standard, with a background of its own: a
    /// unified bar is drawn over the client's toolbar, which stays at the top of the output under the menu bar.
    pub const REVEALED_TITLEBAR: cupertino_server_render::decorations::TitlebarStyle =
        cupertino_server_render::decorations::TitlebarStyle::Standard;

    /// Whether the client said it draws its own decorations (`xdg-decoration` client-side mode): the compositor draws no
    /// title bar for it, and shapes it as [`Self::styled`] says.
    pub fn draws_own_frame(&self) -> bool {
        !self.server_side
            && self
                .toplevel
                .with_pending_state(|st| st.decoration_mode == Some(DecorationMode::ClientSide))
    }

    /// Whether the compositor masks the window's corners and draws its outline and shadow: a window it decorates, and,
    /// with `style_client_decorated`, one whose client draws its own decorations, whether it said so or not, as long as
    /// that frame is an opaque rectangle ([`Self::frame`]). Such a client's own frame (square corners, tighter
    /// ones, its own shadow) is cut to the window's outline, so every window has the same corners, outline and shadow as
    /// a native one; a transparent or shaped window (an overlay, a floating widget, a pill-shaped bar) is left as it
    /// draws itself.
    pub fn styled(&self, style_client_decorated: bool) -> bool {
        self.server_side || (style_client_decorated && self.frame.opaque)
    }

    /// The space the decorations take now: none while the window is full screen.
    pub fn insets(&self) -> crate::config::Insets {
        if self.fullscreen {
            Default::default()
        } else {
            self.decor_insets
        }
    }

    /// The size the client committed, server-side decorations included, in points.
    pub fn committed_size(&self) -> (f64, f64) {
        let geo = self.window.geometry();
        (
            f64::from(geo.size.w) + self.insets().horizontal(),
            f64::from(geo.size.h) + self.insets().vertical(),
        )
    }

    /// Where the title bar of a full-screen window on `output` shows: below the menu bar revealed over it. Ours also for
    /// a client-decorated window, which hides its own title bar in full screen.
    pub fn revealed_titlebar(
        &self,
        bars: &crate::menu_bar::MenuBars,
        decor: &crate::decor::Decor,
        decorations: bool,
        output: OutputId,
        now: f64,
    ) -> Option<Rect> {
        if !(self.fullscreen && decorations && bars.has_bar(output)) {
            return None;
        }
        let height = decor
            .decorations()
            .config
            .metrics
            .titlebar_height(Self::REVEALED_TITLEBAR);
        bars.set.fullscreen_titlebar(output, height, now)
    }

    /// The wl_surface of the window.
    pub fn surface(&self) -> &WlSurface {
        self.toplevel.wl_surface()
    }

    /// The icon the client set with `xdg_toplevel_icon_v1`, as the server copied it at the commit that applied it.
    pub fn icon(&self) -> Arc<WindowIcon> {
        crate::toplevel_icon::window_icon(self.surface())
    }

    /// The content type the client set with `wp_content_type_v1`, as of its last commit.
    pub fn content_type(&self) -> wp_content_type_v1::Type {
        with_states(self.surface(), |s| {
            *s.cached_state
                .get::<ContentTypeSurfaceCachedState>()
                .current()
                .content_type()
        })
    }

    pub fn has_surface(&self, surface: &WlSurface) -> bool {
        self.toplevel.wl_surface() == surface
    }

    /// Apply a model configure: client size (frame minus insets), model states, activation.
    pub fn configure(
        &mut self,
        frame: Rect,
        states: &cupertino_wm::ToplevelStates,
        activated: bool,
    ) -> Option<smithay::utils::Serial> {
        self.fullscreen = states.fullscreen;
        let ins = self.insets();
        let size = (frame.w > ins.horizontal() && frame.h > ins.vertical()).then(|| {
            (
                (frame.w - ins.horizontal()).round() as i32,
                (frame.h - ins.vertical()).round() as i32,
            )
        });
        let t = &self.toplevel;
        t.with_pending_state(|st| {
            if let Some(sz) = size {
                st.size = Some(sz.into());
            }
            for s in policy::MODEL_STATES {
                st.states.unset(s);
            }
            for s in policy::xdg_states(states, activated) {
                st.states.set(s);
            }
        });
        if t.is_initial_configure_sent() {
            t.send_pending_configure()
        } else {
            None
        }
    }

    pub fn close(&self) {
        self.toplevel.send_close();
    }

    /// Update the activated state; true when it changed.
    pub fn set_activated(&self, want: bool) -> bool {
        let t = &self.toplevel;
        let have = t.with_pending_state(|st| st.states.contains(xdg_toplevel::State::Activated));
        if have == want {
            return false;
        }
        t.with_pending_state(|st| {
            if want {
                st.states.set(xdg_toplevel::State::Activated);
            } else {
                st.states.unset(xdg_toplevel::State::Activated);
            }
        });
        if t.is_initial_configure_sent() {
            t.send_pending_configure();
        }
        true
    }

    /// Tell an activated window again that it is activated, with a configure of the same state; nothing for a window
    /// that is not activated or has not had its first configure. A client that drew itself as inactive when the
    /// keyboard left draws as the window in use again.
    pub fn restate_activated(&self) -> bool {
        let t = &self.toplevel;
        let activated =
            t.with_pending_state(|st| st.states.contains(xdg_toplevel::State::Activated));
        if !activated || !t.is_initial_configure_sent() {
            return false;
        }
        t.send_configure();
        true
    }
}

/// One output with its render state.
pub struct OutputEntry {
    pub output: Output,
    pub id: OutputId,
    /// Rectangle in the global logical space.
    pub rect: Rect,
    pub scale: f64,
    /// The scale the output is drawn at and its surfaces are asked to draw at: the next whole number above a
    /// fractional `scale` when the output is supersampled (`fractional_scale_mode`), else `scale`
    /// ([`Cupertino::refresh_render_scales`]).
    pub render_scale: f64,
    /// Counts the scale changes of the output: whoever keeps something drawn for the output's scale (a raster, a
    /// cursor image, an input transform) compares it with the value it was made at.
    pub scale_generation: u64,
    pub tracker: OutputDamageTracker,
    pub cache: ElementCache,
    pub global: Option<GlobalId>,
    pub lock: crate::session_lock::OutputLock,
    pub scheduler: crate::schedule::FrameScheduler,
    /// The identity of the display it shows (`crate::displays::Head::key`), once its head has it.
    pub identity: Option<String>,
}

/// What the pointer is over inside the window stack.
#[derive(Clone, Debug)]
pub enum WindowTarget {
    Surface {
        id: WindowId,
        surface: WlSurface,
        origin: Point<f64, Logical>,
    },
    /// Compositor-drawn decorations: `local` is frame-local, `content` the surface size.
    Decoration {
        id: WindowId,
        style: cupertino_server_render::decorations::TitlebarStyle,
        hit: cupertino_server_render::decorations::Hit,
        content: (f64, f64),
        local: (f64, f64),
    },
}

/// Pointer interaction owned by the window model (move / resize) or the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerMode {
    Normal,
    /// The model drives a move or resize; the pointer is not delivered to clients.
    Model,
}

pub struct Cupertino {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, Cupertino>,
    pub loop_signal: LoopSignal,
    pub clock: Clock<Monotonic>,
    pub config: Config,
    pub socket_name: Option<String>,
    pub running: bool,
    /// The rendering context was lost (the renderer's error) and the server ends for it: [`crate::cli::run`] then returns
    /// an error, so the session command and the display manager see a failure, not a log out.
    pub rendering_lost: Option<String>,
    /// The backend makes a new renderer when the context is lost ([`crate::gpu_reset`]), instead of ending the session.
    pub gpu_recovers: bool,
    /// The context was lost, or the GPU reset, and the backend has yet to recover: why.
    pub gpu_reset: Option<String>,
    pub gpu_resets: crate::gpu_reset::Resets,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_decoration_state: XdgDecorationState,
    pub xdg_dialog_state: XdgDialogState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Cupertino>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    /// `ext-data-control-v1` and `zwlr-data-control-v1` for clipboard managers (`clipboard`).
    pub security_context_state: smithay::wayland::security_context::SecurityContextState,
    pub ext_data_control_state: smithay::wayland::selection::ext_data_control::DataControlState,
    pub wlr_data_control_state: smithay::wayland::selection::wlr_data_control::DataControlState,
    /// Who may see and bind the data control globals (`data_control`).
    pub data_control_access: Arc<crate::data_control::DataControlAccess>,
    /// The login screen's toggle of its screen reader (`screen_reader`); `None` anywhere else.
    pub screen_reader: Option<crate::screen_reader::ScreenReader>,
    /// The screen reader of a person's session (`screen_reader`), which the compositor runs itself.
    pub session_reader: crate::screen_reader::SessionReader,
    /// The server runs the login screen: it starts no program but its shell processes and Xwayland, so no terminal, screenshot
    /// or `exec:` binding, and no process but the login screen's own descends from it.
    pub login_screen: bool,
    /// The server's copy of the clipboard, kept when its client exits (`clipboard`).
    pub clipboard: crate::clipboard::Clipboard,
    pub viewporter_state: ViewporterState,
    /// `wp_single_pixel_buffer_manager_v1`: one-pixel buffers of a solid colour, stretched by a viewport.
    pub single_pixel_buffer_state: SinglePixelBufferState,
    pub fractional_scale_state: FractionalScaleManagerState,
    /// The outputs each window is on and the output whose scale it draws at (`displays::surface_output`).
    pub surface_outputs: crate::displays::surface_output::SurfaceOutputs,
    /// Which windows were told they are suspended, and when to look again (`suspended`).
    pub suspension: crate::suspended::Suspension,
    pub presentation_state: PresentationState,
    /// `wp_content_type_manager_v1`: the kind of content a window says it shows (`WindowEntry::content_type`).
    pub content_type_state: ContentTypeState,
    pub xdg_activation_state: XdgActivationState,
    /// `zxdg_exporter_v2` and `zxdg_importer_v2`: a toplevel of one client made the parent of another's (a portal
    /// dialog over the window that asked for it).
    pub xdg_foreign_state: smithay::wayland::xdg_foreign::XdgForeignState,
    /// Keyboard shortcuts inhibitors and the releases of the person.
    pub shortcut_inhibit: crate::shortcut_inhibit::ShortcutInhibit,
    /// The surfaces with a pointer constraint (`crate::pointer_constraints`).
    pub pointer_constraints: crate::pointer_constraints::PointerConstraints,
    /// The keyboard focus being given now follows the person's own input (`Cupertino::as_input`).
    pub input_focus: bool,
    pub cursor_shape_state: CursorShapeManagerState,
    pub foreign_toplevel_state: ForeignToplevelListState,
    /// `xdg_toplevel_icon_manager_v1`: the icon a window sets for itself (`WindowEntry::icon_name`, `icon_buffers`).
    pub toplevel_icon_manager: XdgToplevelIconManager,
    pub session_lock: crate::session_lock::SessionLock,
    pub idle: crate::idle::Idle,
    /// Dim, display sleep, the lock after it and system sleep (`power`, `display_power`).
    pub power: crate::power::Power,
    /// The application being quit before a log out (`quit`, `org.cupertino.Session1`).
    pub quit: crate::quit::Quit,
    /// The watched settings files and what they held (`live`).
    pub live: crate::live::LiveSettings,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    /// `linux-drm-syncobj-v1` (explicit sync); present once a DRM backend whose kernel supports it has started, or a
    /// headless backend whose device has a render node.
    pub syncobj_state: Option<smithay::wayland::drm_syncobj::DrmSyncobjState>,
    /// Whether a headless backend withholds `linux-drm-syncobj-v1` though its device could offer it (`--no-explicit-sync`),
    /// as a compositor without the protocol does. Read when the backend starts.
    pub withhold_explicit_sync: bool,
    /// The cache the toolkit's GPU programs are made into when the backend starts ([`crate::toolkit_programs`]),
    /// taken then; `None` makes none. The server's start sets it to the user's cache unless the configuration turns
    /// it off; tests name a directory of their own.
    pub program_cache_base: Option<std::path::PathBuf>,
    /// The thread making them; dropped with the state, which stops and joins it before the process exits.
    pub toolkit_programs: Option<crate::toolkit_programs::ToolkitPrograms>,
    /// Test hook: every dmabuf a client offers is refused, as a renderer that cannot import it would.
    pub refuse_dmabuf_imports: bool,
    /// The waits of the clients that have had a commit held back, until they disconnect ([`crate::acquire_waits::sweep`]).
    pub acquire_registry: Vec<std::sync::Arc<crate::acquire_waits::AcquireWaits>>,
    pub popups: PopupManager,
    /// Which clients answer their pings (`hung.rs`).
    pub hung: crate::hung::Pings,
    /// The unresponsive applications the docks were last told about.
    pub hung_announced: std::collections::BTreeSet<String>,
    /// SIGKILLs promised to applications that were asked to quit (`force_quit.rs`).
    pub force_quits: crate::force_quit::ForceQuits,
    /// Send a signal to a process: `kill(2)`; replaced in tests, which run inside the processes they would signal.
    pub send_signal: crate::force_quit::SendSignal,
    /// The popup chain that holds the pointer and keyboard, with the toplevel it belongs to (`popup_grab.rs`).
    pub popup_grab: Option<crate::popup_grab::ActivePopupGrab>,
    /// When the dismissed popups still drawn reach their limit (`popup_linger.rs`).
    pub popup_linger: crate::popup_linger::Expiries,

    pub seat: Seat<Cupertino>,
    pub pointer: PointerHandle<Cupertino>,
    pub pointer_mode: PointerMode,
    /// Buttons whose press the server consumed and whose release it consumes too, so no client gets a release without its press.
    pub consumed_buttons: Vec<u32>,
    /// Model time of the last key press (not a modifier) or button press: the user event the
    /// activation-token rule compares tokens with.
    pub last_user_event: Option<f64>,
    /// The last key or button press and the client that had it (`None`: no client, the desktop or a server decoration),
    /// for the first-window rule of the focus policy.
    pub last_input: Option<(std::time::Instant, Option<ClientId>)>,
    /// Activation tokens presented for surfaces that have no window yet (a program activates its first surface as it maps),
    /// by surface, until the window maps.
    pub pending_activations:
        HashMap<smithay::reexports::wayland_server::backend::ObjectId, crate::handlers::TokenGrant>,
    /// The processes whose first toplevel has mapped: the first-window rule of the focus policy applies once to a process,
    /// however often it reconnects while a client of it that mapped a toplevel stays connected. A process is forgotten
    /// once the last of those has gone ([`Cupertino::forget_gone_processes`]).
    pub mapped_processes: std::collections::HashSet<crate::process::ProcessIdentity>,
    /// The clients that mapped a toplevel, with their processes.
    mapped_clients: HashMap<
        smithay::reexports::wayland_server::backend::ClientId,
        crate::process::ProcessIdentity,
    >,
    /// Clients of `mapped_clients`, and of the docks' resident applications, that disconnected, put there by their
    /// [`ClientState::gone`].
    pub(crate) gone_clients: GoneClients,
    /// The process behind a client (`crate::process::peer_process`); replaced in tests, whose clients share one process.
    pub process_of: crate::process::ProcessOf,
    /// The program behind a client, when the system installed it (`crate::process::trusted_executable`); replaced in
    /// tests, whose program is the test binary.
    pub executable_of: crate::process::ExecutableOf,
    /// The activation token a client's process was started with (`crate::process::launch_token`); replaced in tests.
    pub launch_token_of: crate::process::LaunchTokenOf,
    /// A server move or resize waiting for the pointer to leave the drag-start box.
    pub pending_drag: Option<crate::events::PendingDrag>,
    pub cursor_status: CursorImageStatus,
    pub cursor_loader: CursorLoader,
    /// Shake to locate (`cursor`).
    pub cursor_shake: crate::cursor::CursorShake,
    pub drag_icon: Option<(WlSurface, Point<i32, Logical>)>,
    /// Where the client's drag under way started (`crate::drag_return`).
    pub client_drag: Option<crate::drag_return::DragStart>,
    /// A client's drag that just ended without a drop, its icon not captured yet.
    pub drag_ended: Option<crate::drag_return::DragEnd>,
    pub key_router: KeyRouter,
    /// The input-source switcher and the accent picker, which the on-screen displays draw.
    pub overlays: Overlays,
    pub caps: CapsDebounce,
    /// Slow keys, sticky keys, mouse keys, the globe key and the compositor's key repeat (`kbd_filters`).
    pub key_filters: crate::kbd_filters::KeyFilters,
    /// `zwp_text_input_v3` / `zwp_input_method_v2` globals (only with an accent table, see `input_method`).
    pub text_input_state: Option<smithay::wayland::text_input::TextInputManagerState>,
    pub input_method_state: Option<smithay::wayland::input_method::InputMethodManagerState>,
    pub input_method: Option<crate::input_method::ImBridge>,
    /// Text cursor rectangle of the focused application (surface-local), from its text input.
    pub text_cursor: Option<Rectangle<i32, Logical>>,
    /// The input method's popup surface, while it has one: it is told the output of the text cursor
    /// (`displays::surface_output`).
    pub input_popup: Option<smithay::wayland::input_method::PopupSurface>,
    /// Time (ms) of the held-back caps-lock press.
    pub caps_press_time: u32,
    pub gestures: cupertino_input::GestureRecognizer,
    /// Acceleration state of the relative pointing devices, by device id.
    pub pointer_devices: HashMap<String, crate::events::PointerDevice>,
    /// Touchpad gesture events are being forwarded to the focused client (not claimed yet).
    pub gesture_forwarded: bool,
    pub gesture_source: crate::events::GestureSource,
    pub gesture_output: Option<OutputId>,
    pub hot_corners: cupertino_input::HotCorners,
    pub bar_edge: cupertino_input::EdgeReveal,
    /// A window dragged up into the top of the screen opens the overview.
    pub top_zone: cupertino_input::TopZone,
    pub desktop: DesktopIndex,
    pub menu_bars: MenuBars,
    /// Checks of who owns a bus name a client handed the server (`crate::owner_check`).
    pub owner_checks: crate::owner_check::OwnerChecks,
    pub docks: Docks,
    pub sessions: SessionObservers,
    /// `cupertino_scroll_v1`: the scroll objects of the clients.
    pub scroll_counts: ScrollObservers,
    /// `cupertino_layers_v1`: the parents that hold layers and the clients' counts.
    pub layers: crate::layers_v1::Layers,
    /// What the axis values sent so far lost to the protocol's fixed point, carried into the next ones.
    pub axis_carry: crate::events::AxisCarry,
    /// Reduce Motion, Reduce Transparency, Increase Contrast, the colour filter (`accessibility.conf`).
    pub accessibility: crate::accessibility::Accessibility,
    /// The window manager process's objects and the stage placements they committed.
    pub window_managers: WindowManagers,
    /// Where new windows open: centring and the per-application cascade.
    pub cascade: policy::Cascade,
    /// The layer surface that asked for the keyboard exclusively (layer-shell) and still has it.
    pub exclusive_layer: Option<WlSurface>,
    /// The engaged shell layer (a dock bar, a menu bar) a press outside it took the keyboard back from
    /// ([`Cupertino::release_engaged_layer`]): it does not take the keyboard again until it stops asking for it, or
    /// until it is engaged again.
    pub released_layer: Option<WlSurface>,
    pub dbus: Option<DbusHandle>,
    pub menu_protocols: MenuProtocols,
    /// The heads the backends found, their configuration and the output-management protocols.
    pub displays: crate::displays::Displays,
    pub shell_v1: ShellV1Global,
    pub app_effects_v1: AppEffectsV1Global,
    /// Shell hosts (`cupertino_host_v1`), per-component connections, liveness, holds and bus identity.
    pub hosts: crate::host::HostManager,
    /// Coordinated shell transitions, gesture bindings and window/workspace holes (`cupertino_transaction_manager_v1`).
    pub transactions: crate::transaction::TransactionManager,
    /// Overview component observers per output (`cupertino_overview_manager_v1`, `cupertino_overview_v1`).
    pub overviews: crate::overview_v1::Overviews,
    pub supervisor: Supervisor,
    /// What wakes the supervision of the shell processes and the satellite.
    pub supervision_wakeups: crate::supervisor::Wakeups,
    /// The X display for X11 programs and its satellite; `None` without `xwayland-satellite` and in headless runs.
    pub xwayland: Option<crate::xwayland::XWayland>,
    /// Screen capture for the screenshot process: its globals, sessions and pending frames.
    pub capture: crate::capture::Capture,
    /// Private desktop-control discovery; no grants or mutation in protocol version 1.
    pub control: crate::control::Control,
    /// The objects that bind the portal's shortcuts; their chords live in [`KeyRouter::global`].
    pub global_shortcuts: crate::global_shortcuts::GlobalShortcuts,

    pub wm: Wm,
    pub decor: Decor,
    /// The window control buttons published to assistive technology; off until a session on the hardware starts it.
    pub button_access: crate::decor_access::ButtonAccessibility,
    pub fx: Effects,
    pub windows: Vec<WindowEntry>,
    pub outputs: Vec<OutputEntry>,
    pub render: Option<RenderResources>,
    /// The tone each glass backdrop's text was last sent ([`crate::app_effects_v1::BackdropTones`]).
    pub backdrop_tones: crate::app_effects_v1::BackdropTones,
    pub backend: Option<Backend>,
    pub next_window: u64,
    pub next_output: u64,
    pub screenshot_requests: Vec<std::path::PathBuf>,
    pub(crate) snapshot_writes: crate::frame::snapshot_writes::Writes,
    pub exit_after_screenshot: bool,
    /// Each screenshot is written for every output: the first output's at the path asked for, every other one's beside
    /// it with the output's name added (`--extra-output` runs).
    pub screenshot_each_output: bool,
    /// The screenshots of [`Cupertino::screenshot_each_output`] still to write, by output.
    pub output_screenshots: Vec<(OutputId, std::path::PathBuf)>,
    /// The press interactive glass grows under ([`crate::glass_press`]).
    pub glass_press: crate::glass_press::GlassPress,
    /// Replaces the clock for the model and animations (deterministic tests, frame stepping).
    pub fixed_time: Option<f64>,
    /// Per-frame timings for the frame-rate benchmark (`--frame-stats`).
    pub frame_stats: Option<crate::frame_stats::FrameStats>,
    /// The DRM backend's trace of flips, planes and latency, when asked for (`CUPERTINO_DRM_TRACE`).
    pub drm_trace: Option<crate::drm_trace::DrmTrace>,
    /// Model time runs at this multiple of the wall clock (slow motion for inspection, 1.0 = real
    /// time) starting from `time_origin`.
    pub time_scale: f64,
    pub(crate) time_origin: f64,
}

/// The socket the server listens on: the first free `wayland-N` of the runtime directory, as every compositor does. Only
/// 32 of them exist, and other sessions and servers running at the same time use them up, so when none is free the
/// socket gets a name of its own (`wayland-cupertino-<pid>-<n>`, which `WAYLAND_DISPLAY` carries like any other).
fn listening_socket() -> Result<ListeningSocketSource> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    match ListeningSocketSource::new_auto() {
        Ok(source) => return Ok(source),
        Err(BindError::AlreadyInUse) => {
            tracing::info!("no free wayland-N socket: using a name of its own")
        }
        Err(e) => return Err(e).context("creating the wayland socket"),
    }
    for _ in 0..OWN_NAME_TRIES {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("wayland-cupertino-{}-{n}", std::process::id());
        match ListeningSocketSource::with_name(&name) {
            Ok(source) => return Ok(source),
            Err(BindError::AlreadyInUse) => {}
            Err(e) => return Err(e).context("creating the wayland socket"),
        }
    }
    anyhow::bail!("creating the wayland socket: no free wayland-N name and {OWN_NAME_TRIES} names of its own are taken")
}

/// How many names of its own a server tries once the `wayland-N` names are gone.
const OWN_NAME_TRIES: u32 = 1024;

/// The keymap the seat starts with when the default layout does not compile: the US layout with every name given,
/// options too, so nothing of the environment reaches it.
const FALLBACK_KEYMAP: XkbConfig<'static> = XkbConfig {
    rules: "evdev",
    model: "pc105",
    layout: "us",
    variant: "",
    options: Some(String::new()),
};

impl Cupertino {
    pub fn new(
        display: Display<Cupertino>,
        loop_handle: LoopHandle<'static, Cupertino>,
        loop_signal: LoopSignal,
        config: Config,
        listen_on_socket: bool,
        assets: &Assets,
    ) -> Result<Self> {
        let dh = display.handle();
        // before the first client: each connection takes the size as it is made
        dh.set_default_max_buffer_size(CLIENT_EVENT_BUFFER);
        let clock = Clock::new();

        let socket_name = if listen_on_socket {
            let source = listening_socket()?;
            let name = source.socket_name().to_string_lossy().into_owned();
            loop_handle
                .insert_source(source, |stream, _, state| {
                    let data = ClientState {
                        peer: client_process(&stream),
                        ..ClientState::default()
                    };
                    if let Err(e) = insert_client(&state.display_handle, stream, data) {
                        tracing::warn!(error = %e, "adding wayland client failed");
                    }
                })
                .map_err(|e| anyhow::anyhow!("inserting socket source: {e}"))?;
            tracing::info!(socket = %name, "listening");
            Some(name)
        } else {
            None
        };
        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    let _dispatch = tracing::trace_span!("server.wayland_dispatch").entered();
                    // Safety: the display is never dropped while the source is installed.
                    unsafe {
                        display
                            .get_mut()
                            .dispatch_clients(state)
                            .expect("dispatching wayland clients");
                    }
                    state.clipboard_check_owner();
                    Ok(PostAction::Continue)
                },
            )
            .map_err(|e| anyhow::anyhow!("inserting display source: {e}"))?;

        let compositor_state = CompositorState::new_v6::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let xdg_dialog_state = XdgDialogState::new::<Self>(&dh);
        let layer_shell_state =
            WlrLayerShellState::new_with_filter::<Self, _>(&dh, crate::authorization::unsandboxed);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        let security_context_state = smithay::wayland::security_context::SecurityContextState::new::<
            Self,
            _,
        >(&dh, crate::authorization::unsandboxed);
        let data_control_access = Arc::new(crate::data_control::DataControlAccess::default());
        let (ext_data_control_state, wlr_data_control_state) =
            crate::clipboard::data_control_states(
                &dh,
                &primary_selection_state,
                &data_control_access,
            );
        let viewporter_state = ViewporterState::new::<Self>(&dh);
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&dh);
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&dh);
        let presentation_state = PresentationState::new::<Self>(&dh, clock.id() as u32);
        let content_type_state = ContentTypeState::new::<Self>(&dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(&dh);
        let xdg_foreign_state = smithay::wayland::xdg_foreign::XdgForeignState::new::<Self>(&dh);
        let shortcut_inhibit = crate::shortcut_inhibit::ShortcutInhibit::new(
            smithay::wayland::keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState::new::<Self>(
                &dh,
            ),
        );
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&dh);
        let foreign_toplevel_state = ForeignToplevelListState::new_with_filter::<Self>(
            &dh,
            crate::authorization::unsandboxed,
        );
        let toplevel_icon_manager = XdgToplevelIconManager::new::<Self>(&dh);
        let session_lock = crate::session_lock::SessionLock::new(&dh);
        let idle = crate::idle::Idle::new(
            smithay::wayland::idle_notify::IdleNotifierState::new(&dh, loop_handle.clone()),
            smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Self>(&dh),
        );
        let power = crate::power::Power::new(crate::display_power::IdlePolicy::new(
            config.power,
            policy::clock_seconds(std::time::Duration::from(clock.now())),
        ));
        let _ = PointerGesturesState::new::<Self>(&dh);
        let _ = RelativePointerManagerState::new::<Self>(&dh);
        let _ = smithay::wayland::pointer_constraints::PointerConstraintsState::new::<Self>(&dh);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "seat0");
        let pointer = seat.add_pointer();
        let repeat = config.keyboard.repeat_info();
        // the default names are the environment's (`XKB_DEFAULT_*`); a layout there that does not compile leaves the
        // seat with the fallback keymap
        if let Err(e) = seat.add_keyboard(XkbConfig::default(), repeat.delay, repeat.rate) {
            tracing::error!(error = ?e, "the default keyboard layout does not compile; the seat starts with the fallback keymap");
            seat.add_keyboard(FALLBACK_KEYMAP, repeat.delay, repeat.rate)
                .map_err(|e| anyhow::anyhow!("initialising the keyboard: {e:?}"))?;
        }

        let mut wm_config = WmConfig::default();
        set_minimize_timing(&mut wm_config, config.effects);
        let wm = Wm::new(wm_config);
        let decor = Decor::new(config.appearance, assets)?;
        let fx = Effects::new(config.effects);
        let key_router = KeyRouter::new(config.keybindings.clone());
        let hot_corner_config = config.hot_corners.clone();
        let menu_protocols = MenuProtocols::new(&dh);
        let owner_checks = crate::owner_check::OwnerChecks::new(&loop_handle)?;
        let displays = crate::displays::Displays::new(&dh);
        let shell_v1 = ShellV1Global::new(&dh);
        let app_effects_v1 = AppEffectsV1Global::new(&dh);
        let hosts = crate::host::HostManager::new(&dh);
        let transactions = crate::transaction::TransactionManager::new(&dh);
        let overviews = crate::overview_v1::Overviews::new(&dh);
        let capture = crate::capture::Capture::new(&dh);
        let control = crate::control::Control::new(&dh);
        let global_shortcuts = crate::global_shortcuts::GlobalShortcuts::new(&dh);
        let scroll_counts = ScrollObservers::new(&dh);
        let layers = crate::layers_v1::Layers::new(&dh);
        let supervisor = Supervisor::default();
        let menu_bars = MenuBars::new(config.shell.menu_bar_height);

        let overlays = Overlays::new(&config, &decor.decorations().config.typography, assets)?;
        let caps = CapsDebounce::new(config.keyboard.caps, config.keyboard.caps_switch_hold());
        let key_filters = crate::kbd_filters::KeyFilters::new(&config.keyboard);
        let cursor_loader = CursorLoader::load(
            assets.cursor_theme(),
            config.pointer.cursor_colors,
            &loop_handle,
            |state: &mut Self| state.request_redraw(),
        );
        let with_bridge = overlays.accent.has_table();
        let text_input_state = with_bridge
            .then(|| smithay::wayland::text_input::TextInputManagerState::new::<Self>(&dh));
        let input_method_state = with_bridge.then(|| {
            smithay::wayland::input_method::InputMethodManagerState::new::<Self, _>(&dh, |client| {
                client.get_data::<ClientState>().is_some_and(|c| c.internal)
            })
        });
        let mut me = Self {
            display_handle: dh,
            loop_handle,
            loop_signal,
            clock,
            config,
            socket_name,
            running: true,
            rendering_lost: None,
            gpu_recovers: false,
            gpu_reset: None,
            gpu_resets: Default::default(),
            compositor_state,
            xdg_shell_state,
            xdg_decoration_state,
            xdg_dialog_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            ext_data_control_state,
            security_context_state,
            wlr_data_control_state,
            data_control_access,
            screen_reader: None,
            session_reader: crate::screen_reader::SessionReader::default(),
            login_screen: false,
            clipboard: crate::clipboard::Clipboard::default(),
            viewporter_state,
            single_pixel_buffer_state,
            fractional_scale_state,
            surface_outputs: Default::default(),
            suspension: Default::default(),
            presentation_state,
            content_type_state,
            xdg_activation_state,
            xdg_foreign_state,
            shortcut_inhibit,
            pointer_constraints: Default::default(),
            input_focus: false,
            cursor_shape_state,
            foreign_toplevel_state,
            toplevel_icon_manager,
            session_lock,
            idle,
            power,
            quit: crate::quit::Quit::default(),
            live: crate::live::LiveSettings::default(),
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            syncobj_state: None,
            withhold_explicit_sync: false,
            program_cache_base: None,
            toolkit_programs: None,
            refuse_dmabuf_imports: false,
            acquire_registry: Vec::new(),
            popups: PopupManager::default(),
            hung: crate::hung::Pings::default(),
            hung_announced: Default::default(),
            force_quits: Default::default(),
            send_signal: Box::new(crate::force_quit::signal_process),
            popup_grab: None,
            popup_linger: Default::default(),
            seat,
            pointer,
            pointer_mode: PointerMode::Normal,
            consumed_buttons: Vec::new(),
            last_user_event: None,
            last_input: None,
            pending_activations: HashMap::new(),
            mapped_processes: std::collections::HashSet::new(),
            mapped_clients: HashMap::new(),
            gone_clients: std::sync::Arc::default(),
            process_of: Box::new(crate::process::peer_process),
            executable_of: Box::new(crate::process::trusted_executable),
            launch_token_of: Box::new(crate::process::launch_token),
            pending_drag: None,
            cursor_status: CursorImageStatus::default_named(),
            cursor_loader,
            cursor_shake: crate::cursor::CursorShake::default(),
            drag_icon: None,
            client_drag: None,
            drag_ended: None,
            key_router,
            overlays,
            caps,
            key_filters,
            text_input_state,
            input_method_state,
            input_method: None,
            text_cursor: None,
            input_popup: None,
            caps_press_time: 0,
            gestures: cupertino_input::GestureRecognizer::new(
                cupertino_input::GestureConfig::default(),
            ),
            pointer_devices: HashMap::new(),
            gesture_forwarded: false,
            gesture_source: Default::default(),
            gesture_output: None,
            hot_corners: cupertino_input::HotCorners::new(hot_corner_config),
            bar_edge: cupertino_input::EdgeReveal::new(cupertino_input::EdgeConfig::menu_bar()),
            top_zone: cupertino_input::TopZone::default(),
            desktop: DesktopIndex::load(),
            menu_bars,
            owner_checks,
            docks: Docks::default(),
            sessions: SessionObservers::default(),
            scroll_counts,
            layers,
            axis_carry: crate::events::AxisCarry::default(),
            accessibility: crate::accessibility::Accessibility::default(),
            window_managers: WindowManagers::new(&cupertino_wm::stage::StageConfig::default()),
            cascade: policy::Cascade::default(),
            exclusive_layer: None,
            released_layer: None,
            dbus: None,
            menu_protocols,
            displays,
            shell_v1,
            app_effects_v1,
            hosts,
            transactions,
            overviews,
            supervisor,
            supervision_wakeups: Default::default(),
            xwayland: None,
            capture,
            control,
            global_shortcuts,
            wm,
            decor,
            button_access: Default::default(),
            fx,
            windows: Vec::new(),
            outputs: Vec::new(),
            render: None,
            backdrop_tones: Default::default(),
            backend: None,
            next_window: 1,
            next_output: 1,
            screenshot_requests: Vec::new(),
            snapshot_writes: Default::default(),
            exit_after_screenshot: false,
            screenshot_each_output: false,
            output_screenshots: Vec::new(),
            glass_press: Default::default(),
            fixed_time: None,
            frame_stats: None,
            drm_trace: None,
            time_scale: 1.0,
            time_origin: policy::clock_seconds(std::time::Duration::from(
                Clock::<Monotonic>::new().now(),
            )),
        };
        me.apply_keyboard_settings();
        Ok(me)
    }

    // ---- time ----------------------------------------------------------------------------

    /// Start of the scaled model clock (seconds on the monotonic clock).
    pub fn time_origin(&self) -> f64 {
        self.time_origin
    }

    /// Model time in seconds on the presentation clock (monotonic).
    pub fn now(&self) -> f64 {
        if let Some(t) = self.fixed_time {
            return t;
        }
        let t = policy::clock_seconds(std::time::Duration::from(self.clock.now()));
        if (self.time_scale - 1.0).abs() < 1e-12 {
            t
        } else {
            self.time_origin + (t - self.time_origin) * self.time_scale
        }
    }

    pub fn request_redraw(&mut self) {
        // what shows changed, perhaps the window buttons too
        self.button_access.invalidate();
        for o in &mut self.outputs {
            o.scheduler.damage();
        }
    }
}

mod lifecycle;
mod windows;

/// An output of `mode` at `scale` and `transform` with its top-left corner at `position` in the global logical space,
/// not yet registered or announced ([`Cupertino::add_output`]).
pub fn new_output(
    name: &str,
    physical: PhysicalProperties,
    mode: OutputMode,
    scale: f64,
    transform: Transform,
    position: (i32, i32),
) -> Output {
    let output = Output::new(name.to_string(), physical);
    output.change_current_state(
        Some(mode),
        Some(transform),
        Some(OutputScale::Fractional(scale)),
        Some(position.into()),
    );
    output.set_preferred(mode);
    output
}

/// The rectangle of an output in the global logical space: its mode turned by `transform`, divided
/// by `scale`, at `position`.
pub use cupertino_server_backend::displays::output_rect;

/// Size constraints of a toplevel from its cached state (zero means unset).
/// The client's size limits (`size_hints`) as limits of the frame the window model resizes: the `insets` around the
/// surface added to every limit that is set (0, no limit, stays 0).
pub fn frame_limits(
    insets: crate::config::Insets,
    (min, max): (Option<Size>, Option<Size>),
) -> (Option<Size>, Option<Size>) {
    let grow = |s: Size| {
        Size::new(
            if s.w > 0.0 {
                s.w + insets.horizontal()
            } else {
                0.0
            },
            if s.h > 0.0 {
                s.h + insets.vertical()
            } else {
                0.0
            },
        )
    };
    (min.map(grow), max.map(grow))
}

pub fn size_hints(surface: &WlSurface) -> (Option<Size>, Option<Size>) {
    use smithay::wayland::shell::xdg::SurfaceCachedState as XdgCached;
    with_states(surface, |s| {
        let mut cached = s.cached_state.get::<XdgCached>();
        let cur = cached.current();
        let conv = |sz: SmSize<i32, Logical>| {
            (sz.w > 0 || sz.h > 0).then(|| Size::new(sz.w as f64, sz.h as f64))
        };
        (conv(cur.min_size), conv(cur.max_size))
    })
}

pub(crate) fn which(program: &str) -> Option<std::path::PathBuf> {
    if program.contains('/') {
        return std::path::Path::new(program)
            .is_file()
            .then(|| program.into());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

/// Map from output name to the model id used by tests.
pub type OutputIds = HashMap<String, OutputId>;
