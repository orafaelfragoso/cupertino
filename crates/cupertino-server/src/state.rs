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
use crate::shell::{self, Shell};
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
    /// How many of the client's owner identification threads are alive (`crate::dbus::CLIENT_IDENTIFY_LIMIT`).
    pub owner_identifying: crate::dbus::Identifying,
    /// The commits of the client held back until their buffers are ready ([`crate::acquire_waits`]).
    pub acquire_waits: std::sync::Arc<crate::acquire_waits::AcquireWaits>,
    /// Where the client's id goes when it disconnects, set once it mapped a toplevel ([`Cupertino::forget_gone_processes`]).
    pub gone: std::sync::OnceLock<GoneClients>,
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
            owner_identifying: std::sync::Arc::default(),
            acquire_waits: std::sync::Arc::default(),
            gone: std::sync::OnceLock::new(),
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
    fn disconnected(&self, client_id: ClientId, _reason: DisconnectReason) {
        self.acquire_waits.close();
        if let Some(gone) = self.gone.get() {
            gone.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(client_id);
        }
    }
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
                bezier: cupertino_animation::CubicBezier::LINEAR,
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
    pub titlebar: cupertino_decorations::TitlebarStyle,
    pub foreign: Option<ForeignToplevelHandle>,
    pub modal: bool,
    /// Space around the surface reserved by server-side decorations (see `WindowStyle::ssd_insets`).
    pub decor_insets: crate::config::Insets,
    /// Configured full screen: the decorations are hidden and the client gets the whole frame.
    pub fullscreen: bool,
    pub shrink: Shrink,
}

impl WindowEntry {
    /// The style of the title bar revealed over a full-screen window: standard, with a background of its own: a
    /// unified bar is drawn over the client's toolbar, which stays at the top of the output under the menu bar.
    pub const REVEALED_TITLEBAR: cupertino_decorations::TitlebarStyle =
        cupertino_decorations::TitlebarStyle::Standard;

    /// Whether the client said it draws its own decorations (`xdg-decoration` client-side mode): its window keeps its
    /// own shape, with no corner mask and no shadow of the compositor's.
    pub fn draws_own_frame(&self) -> bool {
        !self.server_side
            && self
                .toplevel
                .with_pending_state(|st| st.decoration_mode == Some(DecorationMode::ClientSide))
    }

    /// Whether the compositor masks the window's corners and draws its shadow: a window it decorates, and, with
    /// `style_client_decorated`, one whose client never said it draws its own decorations.
    pub fn styled(&self, style_client_decorated: bool) -> bool {
        self.server_side || (style_client_decorated && !self.draws_own_frame())
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
        style: cupertino_decorations::TitlebarStyle,
        hit: cupertino_decorations::Hit,
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
    /// Input-source HUD and accent popup.
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
    pub supervisor: Supervisor,
    /// What wakes the supervision of the shell processes and the satellite.
    pub supervision_wakeups: crate::supervisor::Wakeups,
    /// The X display for X11 programs and its satellite; `None` without `xwayland-satellite` and in headless runs.
    pub xwayland: Option<crate::xwayland::XWayland>,
    /// Screen capture for the screenshot process: its globals, sessions and pending frames.
    pub capture: crate::capture::Capture,
    /// Private desktop-control discovery; no grants or mutation in protocol version 1.
    pub control: crate::control::Control,

    pub wm: Wm,
    pub decor: Decor,
    /// The window control buttons published to assistive technology; off until a session on the hardware starts it.
    pub button_access: crate::decor_access::ButtonAccessibility,
    pub fx: Effects,
    pub windows: Vec<WindowEntry>,
    pub outputs: Vec<OutputEntry>,
    pub shell: Shell,
    pub render: Option<RenderResources>,
    /// The tone each glass backdrop's text was last sent ([`crate::app_effects_v1::BackdropTones`]).
    pub backdrop_tones: crate::app_effects_v1::BackdropTones,
    pub backend: Option<Backend>,
    pub next_window: u64,
    pub next_output: u64,
    pub screenshot_requests: Vec<std::path::PathBuf>,
    pub exit_after_screenshot: bool,
    /// Each screenshot is written for every output: the first output's at the path asked for, every other one's beside
    /// it with the output's name added (`--extra-output` runs).
    pub screenshot_each_output: bool,
    /// The screenshots of [`Cupertino::screenshot_each_output`] still to write, by output.
    pub output_screenshots: Vec<(OutputId, std::path::PathBuf)>,
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
                    if let Err(e) = state.display_handle.insert_client(stream, Arc::new(data)) {
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
        seat.add_keyboard(XkbConfig::default(), repeat.delay, repeat.rate)
            .map_err(|e| anyhow::anyhow!("initialising the keyboard: {e:?}"))?;

        let mut wm_config = WmConfig::default();
        set_minimize_timing(&mut wm_config, config.effects);
        let wm = Wm::new(wm_config);
        let decor = Decor::new(config.appearance, assets)?;
        let fx = Effects::new(config.effects);
        let key_router = KeyRouter::new(config.keybindings.clone());
        let hot_corner_config = config.hot_corners.clone();
        let shell = Shell::default();
        let menu_protocols = MenuProtocols::new(&dh);
        let owner_checks = crate::owner_check::OwnerChecks::new(&loop_handle)?;
        let displays = crate::displays::Displays::new(&dh);
        let shell_v1 = ShellV1Global::new(&dh);
        let app_effects_v1 = AppEffectsV1Global::new(&dh);
        let capture = crate::capture::Capture::new(&dh);
        let control = crate::control::Control::new(&dh);
        let scroll_counts = ScrollObservers::new(&dh);
        let mut supervisor = Supervisor::new(&config.shell_processes, std::time::Instant::now());
        supervisor.set_dev_folder(config.dev_folder.as_ref());
        let menu_bars = MenuBars::new(config.shell.menu_bar_height);

        let overlays = Overlays::new(&config, &decor.decorations().config.typography, assets)?;
        let caps = CapsDebounce::new(config.keyboard.caps, config.keyboard.caps_switch_hold());
        let key_filters = crate::kbd_filters::KeyFilters::new(&config.keyboard);
        let cursor_loader = CursorLoader::load(assets.cursor_theme(), config.pointer.cursor_colors);
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
            acquire_registry: Vec::new(),
            popups: PopupManager::default(),
            hung: crate::hung::Pings::default(),
            hung_announced: Default::default(),
            force_quits: Default::default(),
            send_signal: Box::new(crate::force_quit::signal_process),
            popup_grab: None,
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
            axis_carry: crate::events::AxisCarry::default(),
            accessibility: crate::accessibility::Accessibility::default(),
            window_managers: WindowManagers::new(&cupertino_stage::StageConfig::default()),
            cascade: policy::Cascade::default(),
            exclusive_layer: None,
            released_layer: None,
            dbus: None,
            menu_protocols,
            displays,
            shell_v1,
            app_effects_v1,
            supervisor,
            supervision_wakeups: Default::default(),
            xwayland: None,
            capture,
            control,
            wm,
            decor,
            button_access: Default::default(),
            fx,
            windows: Vec::new(),
            outputs: Vec::new(),
            shell,
            render: None,
            backdrop_tones: Default::default(),
            backend: None,
            next_window: 1,
            next_output: 1,
            screenshot_requests: Vec::new(),
            exit_after_screenshot: false,
            screenshot_each_output: false,
            output_screenshots: Vec::new(),
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

    // ---- outputs -------------------------------------------------------------------------

    /// Register `output` in its current state, which [`new_output`] set, and announce its global. Returns its model id.
    /// Outputs are created for the enabled heads of `crate::displays`, once their backend shows them.
    pub fn add_output(&mut self, output: Output) -> OutputId {
        let global = output.create_global::<Self>(&self.display_handle);
        let mode = output.current_mode().expect("a new output has a mode");
        let scale = output.current_scale().fractional_scale();
        let transform = output.current_transform();
        let position = output.current_location();
        let id = OutputId(self.next_output);
        self.next_output += 1;
        let rect = output_rect(mode, scale, transform, (position.x, position.y));
        let tracker = OutputDamageTracker::from_output(&output);
        self.outputs.push(OutputEntry {
            output: output.clone(),
            id,
            rect,
            scale,
            scale_generation: 0,
            tracker,
            cache: ElementCache::default(),
            global: Some(global),
            lock: Default::default(),
            scheduler: Default::default(),
            identity: None,
        });
        let now = self.now();
        self.menu_bars.set.add_output(id, rect, now);
        let usable = self.usable_area(id);
        let fx = self.wm.handle(
            Command::AddOutput {
                output: id,
                rect,
                usable,
            },
            now,
        );
        self.apply_effects(fx);
        self.refresh_usable_area(id);
        id
    }

    /// Remove an output (hot-unplug): its windows move to the first remaining output and its layer surfaces let go of
    /// the input ([`Cupertino::release_output_layers`]). What needs the person stays reachable
    /// (`crate::displays::recovery`): a menu of a window on it closes, an overview on it ends, the pointer comes onto a
    /// remaining output and a locked session's keyboard onto a remaining lock surface.
    pub fn remove_output(&mut self, id: OutputId) {
        let Some(idx) = self.outputs.iter().position(|o| o.id == id) else {
            return;
        };
        if let Some(identity) = self.outputs[idx].identity.clone() {
            let from = self.outputs[idx].rect;
            self.note_departures(id, &identity, from);
        }
        self.end_popup_grab_on(id);
        self.output_screenshots.retain(|(o, _)| *o != id);
        // the overview of this output has nothing left to show on; one open elsewhere takes the windows that arrive
        if self.shell.overview.as_ref().is_some_and(|o| o.output == id) {
            tracing::info!(output = id.0, "the overview's output went away");
            self.shell.overview = None;
        }
        // told before the global goes, so a client hears `leave` for an output it can still name
        self.leave_departing_output(idx);
        let entry = self.outputs.remove(idx);
        self.release_output_layers(&entry.output);
        let now = self.now();
        self.menu_bars.remove_output(id, now);
        self.refresh_gtk_capabilities();
        self.refresh_app_menus();
        self.docks.remove_output(id);
        self.window_managers.remove_output(id);
        if let Some(g) = entry.global {
            self.retire_global(g);
        }
        let now = self.now();
        let fx = self.wm.handle(Command::RemoveOutput { output: id }, now);
        self.apply_effects(fx);
        self.note_placements();
        self.bring_pointer_onto_outputs();
        self.refocus_lock_keyboard();
        self.refresh_surface_outputs();
        self.request_redraw();
    }

    /// The layer surfaces of an output that goes leave its layer map: a popup grab of one of them ends, and one that
    /// holds the keyboard exclusively gives it back to the key window. Their clients destroy them later, after the
    /// `closed` that [`Cupertino::drop_head_output`] sent, or never; meanwhile no output shows them and none of them
    /// keeps the keyboard or the pointer.
    fn release_output_layers(&mut self, output: &Output) {
        let layers: Vec<_> = layer_map_for_output(output).layers().cloned().collect();
        if layers.is_empty() {
            return;
        }
        if self
            .popup_grab_root()
            .is_some_and(|root| layers.iter().any(|l| l.wl_surface() == root))
        {
            self.end_popup_grab(
                SERIAL_COUNTER.next_serial(),
                std::time::Duration::from(self.clock.now()).as_millis() as u32,
            );
        }
        for layer in &layers {
            self.layer_keyboard_exclusivity(layer.wl_surface(), false);
        }
        {
            let mut map = layer_map_for_output(output);
            for layer in &layers {
                map.unmap_layer(layer);
            }
        }
        self.refresh_holds();
    }

    /// Take a global away without disconnecting a client that binds it before it hears that it is gone: clients are
    /// told now, and the global is destroyed after [`RETIRED_GLOBAL_LIFETIME`].
    fn retire_global(&mut self, global: GlobalId) {
        self.display_handle.disable_global::<Self>(global.clone());
        let timer = self.loop_handle.insert_source(
            Timer::from_duration(RETIRED_GLOBAL_LIFETIME),
            move |_, _, state| {
                state.display_handle.remove_global::<Self>(global.clone());
                TimeoutAction::Drop
            },
        );
        if let Err(e) = timer {
            tracing::warn!(error = %e, "cannot time the removal of a global; it stays disabled");
        }
    }

    /// Give an output a new mode, scale, transform or position (a nested window resized, an
    /// output configuration applied): its layer surfaces, lock surface, menu bar and windows follow.
    pub fn configure_output(
        &mut self,
        id: OutputId,
        mode: OutputMode,
        scale: f64,
        transform: Transform,
        position: (i32, i32),
    ) {
        let Some(idx) = self.outputs.iter().position(|o| o.id == id) else {
            return;
        };
        let entry = &mut self.outputs[idx];
        entry.output.change_current_state(
            Some(mode),
            Some(transform),
            Some(OutputScale::Fractional(scale)),
            Some(position.into()),
        );
        entry.output.set_preferred(mode);
        if entry.scale != scale {
            entry.scale_generation += 1;
            tracing::debug!(output = ?id, from = entry.scale, to = scale, "output scale changed");
        }
        entry.scale = scale;
        entry.rect = output_rect(mode, scale, transform, position);
        entry.tracker = OutputDamageTracker::from_output(&entry.output);
        entry.scheduler.damage();
        let rect = entry.rect;
        layer_map_for_output(&entry.output).arrange();
        self.configure_lock_surface(idx);
        self.menu_bars.set.set_output_rect(id, rect);
        let usable = self.usable_area(id);
        self.command(Command::SetOutputRect {
            output: id,
            rect,
            usable,
        });
        self.refresh_usable_area(id);
        self.request_redraw();
    }

    /// The output's quantities kept apart: its mode in panel pixels, how it is turned, its scale and its logical
    /// extent. `None` for an unknown output or one without a mode.
    pub fn output_geometry(&self, id: OutputId) -> Option<cupertino_displays::OutputGeometry> {
        let entry = self.output_entry(id)?;
        let mode = entry.output.current_mode()?;
        let extent = cupertino_displays::Extent::new(
            u32::try_from(mode.size.w).ok()?,
            u32::try_from(mode.size.h).ok()?,
        );
        let turned = matches!(
            entry.output.current_transform(),
            Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
        );
        cupertino_displays::OutputGeometry::new(extent, entry.scale, turned).ok()
    }

    /// A position in an output's turned panel pixels (a pointer that reports pixels) in the global logical space.
    pub fn output_pixels_to_global(
        &self,
        id: OutputId,
        pixels: (f64, f64),
    ) -> Option<Point<f64, Logical>> {
        let entry = self.output_entry(id)?;
        let (x, y) = self.output_geometry(id)?.to_points(pixels);
        Some(Point::from((entry.rect.x + x, entry.rect.y + y)))
    }

    pub fn output_entry(&self, id: OutputId) -> Option<&OutputEntry> {
        self.outputs.iter().find(|o| o.id == id)
    }

    pub fn output_by_wl(&self, output: &Output) -> Option<&OutputEntry> {
        self.outputs.iter().find(|o| &o.output == output)
    }

    /// Output containing `p` (global logical coordinates), else the nearest one.
    pub fn output_at(&self, p: Point<f64, Logical>) -> Option<&OutputEntry> {
        let pt = MPoint::new(p.x, p.y);
        self.outputs
            .iter()
            .find(|o| o.rect.contains(pt))
            .or_else(|| {
                self.outputs.iter().min_by(|a, b| {
                    let d = |o: &OutputEntry| {
                        let dx = (pt.x - pt.x.clamp(o.rect.x, o.rect.right())).abs();
                        let dy = (pt.y - pt.y.clamp(o.rect.y, o.rect.bottom())).abs();
                        dx + dy
                    };
                    d(a).total_cmp(&d(b))
                })
            })
    }

    pub fn primary_output(&self) -> Option<&OutputEntry> {
        self.outputs.first()
    }

    /// Area available to windows on `id` (menu bar and layer-shell zones removed; the dock's
    /// exclusive zone is one of them).
    pub fn usable_area(&self, id: OutputId) -> Rect {
        let Some(o) = self.output_entry(id) else {
            return Rect::default();
        };
        let menu = if self.config.shell.show_menu_bar {
            self.menu_bars.reserved_height(id)
        } else {
            0.0
        };
        let zone = {
            let map = layer_map_for_output(&o.output);
            let z = map.non_exclusive_zone();
            (z.size.w > 0 && z.size.h > 0).then(|| {
                Rect::new(
                    o.rect.x + z.loc.x as f64,
                    o.rect.y + z.loc.y as f64,
                    z.size.w as f64,
                    z.size.h as f64,
                )
            })
        };
        shell::usable_area(o.rect, menu, zone)
    }

    pub fn refresh_usable_area(&mut self, id: OutputId) {
        let usable = self.usable_area(id);
        let now = self.now();
        let fx = self
            .wm
            .handle(Command::SetUsableArea { output: id, usable }, now);
        self.apply_effects(fx);
        let fx = self.wm.handle(
            Command::SetDockZone {
                output: id,
                zone: self.dock_rect(id),
            },
            now,
        );
        self.apply_effects(fx);
        self.refresh_window_managers();
    }

    // ---- windows -------------------------------------------------------------------------

    pub fn entry(&self, id: WindowId) -> Option<&WindowEntry> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Send the configures that shrinking frame animations held back once they end.
    fn send_deferred_configures(&mut self, now: f64) {
        let key = self.activated_window();
        for e in &mut self.windows {
            let Shrink::Deferred { frame, states } = e.shrink else {
                continue;
            };
            if self
                .wm
                .window(e.id)
                .is_some_and(|w| w.lifecycle.is_animating(now))
            {
                continue;
            }
            e.shrink = e
                .configure(frame, &states, key == Some(e.id))
                .map_or(Shrink::None, Shrink::Awaiting);
        }
    }

    pub fn entry_for_surface(&self, surface: &WlSurface) -> Option<&WindowEntry> {
        self.windows.iter().find(|w| w.has_surface(surface))
    }

    pub fn window_id_for_surface(&self, surface: &WlSurface) -> Option<WindowId> {
        self.entry_for_surface(surface).map(|w| w.id)
    }

    /// Toplevel root of any surface in a window's tree (subsurface or popup child).
    pub fn root_window_surface(&self, surface: &WlSurface) -> Option<WlSurface> {
        let mut root = surface.clone();
        while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
            root = parent;
        }
        if self.entry_for_surface(&root).is_some() {
            return Some(root);
        }
        self.popups
            .find_popup(surface)
            .and_then(|p| smithay::desktop::find_popup_root_surface(&p).ok())
    }

    /// Called on a toplevel commit: sends the initial configure, maps the window once it has a
    /// buffer and tracks unmapping.
    pub fn toplevel_committed(&mut self, surface: &WlSurface) {
        let Some(idx) = self.windows.iter().position(|w| w.has_surface(surface)) else {
            return;
        };
        if crate::toplevel_icon::icon_committed(surface) && self.windows[idx].mapped {
            self.refresh_docks();
        }
        let initial_sent = with_states(surface, |s| {
            s.data_map
                .get::<XdgToplevelSurfaceData>()
                .map(|d| d.lock().unwrap().initial_configure_sent)
                .unwrap_or(false)
        });
        if !initial_sent {
            use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
            let t = &self.windows[idx].toplevel;
            if self.windows[idx].server_side {
                t.with_pending_state(|st| st.decoration_mode = Some(Mode::ServerSide));
            }
            t.send_configure();
            return;
        }
        let has_buffer =
            smithay::backend::renderer::utils::with_renderer_surface_state(surface, |s| {
                s.buffer().is_some()
            })
            .unwrap_or(false);
        let (mapped, id) = (self.windows[idx].mapped, self.windows[idx].id);
        match (mapped, has_buffer) {
            (false, true) => self.map_window(idx),
            (true, false) => {
                self.windows[idx].mapped = false;
                self.windows[idx].shrink = Shrink::None;
                let now = self.now();
                let fx = self.wm.handle(Command::Destroy { window: id }, now);
                self.apply_effects(fx);
            }
            (true, true) => {
                self.windows[idx].window.on_commit();
                if let Shrink::Awaiting(sent) = self.windows[idx].shrink {
                    let acked = with_states(surface, |s| {
                        s.data_map
                            .get::<XdgToplevelSurfaceData>()
                            .and_then(|d| d.lock().unwrap().current_serial)
                    });
                    if acked.is_some_and(|serial| serial.is_no_older_than(&sent)) {
                        self.windows[idx].shrink = Shrink::None;
                    }
                }
                self.refresh_snapshot(id);
                let (min, max) = frame_limits(self.windows[idx].insets(), size_hints(surface));
                let now = self.now();
                let fx = self.wm.handle(
                    Command::SetSizeHints {
                        window: id,
                        min,
                        max,
                        content: None,
                    },
                    now,
                );
                self.apply_effects(fx);
                let disabled = cupertino_wm::DisabledControls::from(
                    crate::app_effects_v1::disabled_controls(surface),
                );
                if self.wm.window(id).is_some_and(|w| w.disabled != disabled) {
                    let fx = self.wm.handle(
                        Command::SetDisabledControls {
                            window: id,
                            disabled,
                        },
                        now,
                    );
                    self.apply_effects(fx);
                    self.refresh_menu_bars();
                }
            }
            (false, false) => {}
        }
        self.request_redraw_for(surface);
    }

    pub(crate) fn map_window(&mut self, idx: usize) {
        let surface = self.windows[idx].surface().clone();
        let id = self.windows[idx].id;
        let (app_id, title) = with_states(&surface, |s| {
            let d = s
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap();
            (d.app_id.clone(), d.title.clone())
        });
        let launch = self.take_launch(&surface);
        let identity = match app_id.as_deref().filter(|a| !a.is_empty()) {
            Some(app_id) => app_id.to_string(),
            None => self
                .client_identity(&surface, launch.as_ref())
                .unwrap_or_else(|| policy::app_identity(None, title.as_deref())),
        };
        self.windows[idx].window.on_commit();
        let geo = self.windows[idx].window.geometry();
        let (min, max) = frame_limits(self.windows[idx].insets(), size_hints(&surface));
        // the protocol facts the window kind is chosen from (`cupertino_wm::window_kind`): parent and
        // modality; Wayland has no utility-window signal
        let parent = self.windows[idx]
            .toplevel
            .parent()
            .and_then(|p| self.window_id_for_surface(&p));
        let Some(output) = self.active_output() else {
            return;
        };
        let (screen, usable) = self
            .wm
            .output(output)
            .map(|o| (o.rect, o.usable))
            .unwrap_or_default();
        // the visible frame new windows are placed in: the stage frame while the stage is on
        let visible = if self.window_managers.stage_active(output) {
            cupertino_stage::StageConfig::default().stage_frame(usable)
        } else {
            usable
        };
        let ins = self.windows[idx].insets();
        let size = (geo.size.w > 0 && geo.size.h > 0).then(|| {
            Size::new(
                geo.size.w as f64 + ins.horizontal(),
                geo.size.h as f64 + ins.vertical(),
            )
        });
        let size = policy::new_window_size(visible, size, &self.config);
        let frame = if parent.is_some() {
            // the model places child windows itself (sheets, dialogs)
            policy::centred(visible, size)
        } else {
            self.cascade.retain(|w| self.wm.window(w).is_some());
            self.cascade.place(
                &identity,
                id,
                size,
                visible,
                screen,
                self.config.cascade_step,
            )
        };
        let mut params = MapParams::new(id, &identity, frame);
        params.title = title.unwrap_or_default();
        params.min_size = min;
        params.max_size = max;
        params.disabled = cupertino_wm::DisabledControls::from(
            crate::app_effects_v1::disabled_controls(&surface),
        );
        params.parent = parent;
        params.modal = self.windows[idx].modal;
        if self.windows[idx].server_side {
            params.sheet_top = self.decor.sheet_top(self.windows[idx].titlebar);
        }
        // the model turns a modal child into a sheet and places dialogs
        params.kind = cupertino_wm::window_kind(params.modal, false);
        params.output = Some(output);
        params.activation = cupertino_wm::Activation::Policy(self.map_focus(&surface, launch));
        self.windows[idx].mapped = true;
        self.apply_pending_menu_addresses(id.0, &surface);
        let handle = self
            .foreign_toplevel_state
            .new_toplevel::<Self>(cut_text(&params.title), cut_text(&identity));
        self.windows[idx].foreign = Some(handle);
        let now = self.now();
        let fx = self.wm.handle(Command::Map(params), now);
        self.apply_effects(fx);
    }

    /// The grant of the launch behind a toplevel that maps, used up: the token it presented for its surface, else the
    /// `XDG_ACTIVATION_TOKEN` its process was started with, for a program that never presents it.
    fn take_launch(&mut self, surface: &WlSurface) -> Option<crate::handlers::TokenGrant> {
        if let Some(grant) = self.pending_activations.remove(&surface.id()) {
            return Some(grant);
        }
        let client = surface.client()?;
        let token = smithay::wayland::xdg_activation::XdgActivationToken::from((self
            .launch_token_of)(
            &self.display_handle,
            &client,
        )?);
        let grant = self
            .xdg_activation_state
            .data_for_token(&token)?
            .user_data
            .get::<crate::handlers::TokenGrant>()
            .cloned();
        self.xdg_activation_state.remove_token(&token);
        grant
    }

    /// The application of a toplevel that maps without an application id (Chromium's first-run dialog), from its client:
    /// the application of another mapped window of that client, else the application its launch named, else the desktop
    /// entry of the program it runs when the system installed that program ([`crate::process::trusted_executable`]). A
    /// client cannot name an application this way that it could not name with `set_app_id`.
    fn client_identity(
        &self,
        surface: &WlSurface,
        launch: Option<&crate::handlers::TokenGrant>,
    ) -> Option<String> {
        let client = surface.client()?;
        let sibling = self.windows.iter().filter(|e| e.mapped).find_map(|e| {
            let same = e.surface().client().is_some_and(|c| c.id() == client.id());
            same.then(|| self.wm.window(e.id).and_then(|w| self.wm.app(w.app)))
                .flatten()
                .map(|a| a.identity.clone())
        });
        sibling
            .or_else(|| launch.and_then(|l| l.app_id.clone()))
            .or_else(|| {
                let program = (self.executable_of)(&self.display_handle, &client)?;
                self.desktop.for_program(&program).map(|e| e.id.clone())
            })
    }

    /// Forgets the first-window grant of processes whose last client that mapped a toplevel has disconnected, and the
    /// resident applications of the clients that disconnected (the docks hear of it).
    pub fn forget_gone_processes(&mut self) {
        let gone = std::mem::take(
            &mut *self
                .gone_clients
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        if gone.is_empty() {
            return;
        }
        let mut residents_changed = false;
        for id in &gone {
            self.mapped_clients.remove(id);
            residents_changed |= self.docks.client_gone(id);
        }
        let mapped = &self.mapped_clients;
        self.mapped_processes
            .retain(|p| mapped.values().any(|q| q == p));
        if residents_changed {
            self.refresh_docks();
        }
    }

    /// The facts the focus policy needs about a window that maps (`cupertino_wm::MapFocus`): whether it is its client's
    /// first toplevel, whether the person has given input elsewhere since the client connected, and the grant of its launch
    /// ([`Self::take_launch`]).
    fn map_focus(
        &mut self,
        surface: &WlSurface,
        launch: Option<crate::handlers::TokenGrant>,
    ) -> cupertino_wm::MapFocus {
        use std::sync::atomic::Ordering;
        let (first_of_client, input_elsewhere) = match surface.client() {
            Some(client) => match client.get_data::<ClientState>() {
                Some(data) => {
                    let id = client.id();
                    let first_of_connection = !data.mapped_toplevel.swap(true, Ordering::SeqCst);
                    // a process that cannot be named gets no first-window grant
                    let process = (self.process_of)(&self.display_handle, &client);
                    if let Some(process) = process {
                        self.mapped_clients.insert(id.clone(), process);
                        data.gone.get_or_init(|| self.gone_clients.clone());
                    }
                    let first_of_process =
                        process.is_some_and(|process| self.mapped_processes.insert(process));
                    // nobody is at the screen while the session is locked: what comes up then waits behind
                    let first =
                        first_of_connection && first_of_process && !self.session_lock.is_active();
                    let elsewhere = self.last_input.as_ref().is_some_and(|(at, who)| {
                        *at > data.connected_at && who.as_ref() != Some(&id)
                    });
                    (first, elsewhere)
                }
                None => (false, false),
            },
            None => (false, false),
        };
        let grant = launch.map(|g| cupertino_wm::Grant {
            created: g.created,
            valid: g.valid,
        });
        cupertino_wm::MapFocus {
            first_of_client,
            input_elsewhere,
            grant,
            last_user_event: self.last_user_event,
        }
    }

    /// Note that a key or button press went to `focus` (its client, if any).
    pub fn note_input(&mut self, focus: Option<&WlSurface>) {
        self.last_input = Some((
            std::time::Instant::now(),
            focus.and_then(|s| s.client()).map(|c| c.id()),
        ));
    }

    /// Insets for a window decorated by the compositor with a title bar of `style`.
    pub fn decoration_insets(
        &self,
        style: cupertino_decorations::TitlebarStyle,
    ) -> crate::config::Insets {
        if self.config.window.decorations {
            self.decor.insets(style)
        } else {
            self.config.window.ssd_insets
        }
    }

    /// The cursor of the resize zone `edge` of window `id` at its size now, `None` where it cannot resize that way
    /// (`decor::resize_cursor`).
    pub fn resize_cursor(
        &self,
        id: WindowId,
        edge: cupertino_decorations::ResizeEdge,
    ) -> Option<smithay::input::pointer::CursorIcon> {
        let w = self.wm.window(id)?;
        let (min, max) = w.size_limits();
        let f = w.geometry();
        crate::decor::resize_cursor(
            edge,
            crate::decor::AxisFreedom::of(f.w, min.w, max.w),
            crate::decor::AxisFreedom::of(f.h, min.h, max.h),
        )
    }

    /// The windows on the output under `pos` that take input, topmost first, each with its plan; `None` while the
    /// session is locked or off every output. Strip thumbnails and hidden windows take no input of their clients
    /// (`window_manager_press`).
    fn input_windows(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<Vec<(WindowId, &WindowEntry, crate::render::scene::WindowPlan)>> {
        if self.session_lock.is_active() {
            return None;
        }
        let out = self.output_at(pos)?;
        let now = self.now();
        let list = crate::render::scene::windows_on_output(
            &self.wm,
            out.id,
            out.rect.w,
            crate::render::motion::WorkspaceTransition::for_options(self.display_options()),
            now,
        );
        Some(
            list.into_iter()
                .rev()
                .filter(|(id, _, _)| !self.window_managers.passes_input(*id))
                .filter_map(|(id, slide, _)| {
                    let entry = self.entry(id)?;
                    let (plan, _) = crate::render::scene::plan_for(
                        &self.wm,
                        entry,
                        slide,
                        now,
                        &self.fx,
                        self.display_options(),
                    )?;
                    Some((id, entry, plan))
                })
                .collect(),
        )
    }

    /// The surface of one of `entry`'s popups under `pos` (global logical), every surface of a popup's tree:
    /// `Window::surface_under` with `POPUP` alone finds only the popups' roots.
    fn popup_of_window_under(
        id: WindowId,
        entry: &WindowEntry,
        plan: &crate::render::scene::WindowPlan,
        pos: Point<f64, Logical>,
    ) -> Option<WindowTarget> {
        let geo = entry.window.geometry();
        let ins = entry.insets();
        let origin = Point::<f64, Logical>::from((
            plan.visual.x + ins.left - geo.loc.x as f64,
            plan.visual.y + ins.top - geo.loc.y as f64,
        ));
        let root = entry.window.toplevel().map(|t| t.wl_surface().clone())?;
        for (popup, location) in PopupManager::popups_for_surface(&root) {
            let offset = geo.loc + location - popup.geometry().loc;
            if let Some((surface, loc)) = smithay::desktop::utils::under_from_surface_tree(
                popup.wl_surface(),
                pos - origin,
                offset,
                WindowSurfaceType::ALL,
            ) {
                return Some(WindowTarget::Surface {
                    id,
                    surface,
                    origin: origin + loc.to_f64(),
                });
            }
        }
        None
    }

    /// The popup surface under `pos` (global logical) of the window whose popups are drawn above every window and the
    /// bars (`SceneLayer::Popups`, [`crate::render::scene::raised_popups_window`]), so the pointer finds them before
    /// either. The popups of the other windows are part of their window ([`Self::window_target_under`]).
    /// The window target under `pos` when nothing drawn above the windows (an overlay or top layer surface such as the
    /// dock, a bar's popup, a lock surface) takes the point first: what hover cursors and presses on the decorations
    /// act on, so a resize band under the dock neither changes the cursor nor starts a resize.
    pub fn window_target_on_top(&self, pos: Point<f64, Logical>) -> Option<WindowTarget> {
        let target = self.window_target_under(pos)?;
        let above = self.surface_under(pos);
        let on_top = match (&target, &above) {
            (WindowTarget::Decoration { .. }, None) => true,
            (WindowTarget::Surface { surface, .. }, Some((s, _))) => s == surface,
            _ => false,
        };
        on_top.then_some(target)
    }

    pub fn window_popup_under(&self, pos: Point<f64, Logical>) -> Option<WindowTarget> {
        let out = self.output_at(pos)?;
        let raised = crate::render::scene::raised_popups_window(&self.wm, out.id)?;
        let (id, entry, plan) = self
            .input_windows(pos)?
            .into_iter()
            .find(|(id, _, _)| *id == raised)?;
        Self::popup_of_window_under(id, entry, &plan, pos)
    }

    /// What is under `pos` (global logical), from the top down: the raised popups of the key window
    /// ([`Self::window_popup_under`]), then per window, in this order: the window control buttons of a unified bar, the
    /// window's own popups (any other window's), the resize zones (3 pt inside to 4 pt outside the frame, also for a
    /// client-decorated window, measured from its window geometry), the client's surface inside the frame, the title
    /// bar and buttons of a server-decorated window. Outside the frame and its resize band a window takes no input: a
    /// client-drawn shadow passes the pointer to what lies below.
    pub fn window_target_under(&self, pos: Point<f64, Logical>) -> Option<WindowTarget> {
        if let Some(popup) = self.window_popup_under(pos) {
            return Some(popup);
        }
        let out = self.output_at(pos)?;
        let now = self.now();
        let raised = crate::render::scene::raised_popups_window(&self.wm, out.id);
        for (id, entry, plan) in self.input_windows(pos)? {
            let geo = entry.window.geometry();
            let ins = entry.insets();
            let origin = Point::<f64, Logical>::from((
                plan.visual.x + ins.left - geo.loc.x as f64,
                plan.visual.y + ins.top - geo.loc.y as f64,
            ));
            let style = entry.titlebar;
            let decorated = entry.server_side
                && !entry.fullscreen
                && self.config.window.decorations
                && plan.exact
                && (ins.top > 0.0 || style.client_drawn());
            let decoration_hit = |resize: bool| {
                let content = (f64::from(geo.size.w), f64::from(geo.size.h));
                let local = (pos.x - plan.visual.x, pos.y - plan.visual.y);
                (
                    self.decor.hit_test(style, content, local, resize),
                    content,
                    local,
                )
            };
            // the window control buttons of a unified bar lie over the client's surface and take the pointer first
            if decorated && style.client_drawn() {
                let (hit, content, local) = decoration_hit(false);
                if matches!(hit, cupertino_decorations::Hit::Button(_)) {
                    return Some(WindowTarget::Decoration {
                        id,
                        style,
                        hit,
                        content,
                        local,
                    });
                }
            }
            // the popups of a window other than the one whose popups are raised lie above it and its resize band
            if Some(id) != raised {
                if let Some(popup) = Self::popup_of_window_under(id, entry, &plan, pos) {
                    return Some(popup);
                }
            }
            // the title bar revealed with the menu bar over a full-screen window: its buttons act, the rest takes the
            // pointer from the client
            if let Some(strip) = entry
                .revealed_titlebar(
                    &self.menu_bars,
                    &self.decor,
                    self.config.window.decorations,
                    out.id,
                    now,
                )
                .filter(|r| plan.exact && r.contains(MPoint::new(pos.x, pos.y)))
            {
                let content = (strip.w, strip.h);
                let local = (pos.x - strip.x, pos.y - strip.y);
                let style = WindowEntry::REVEALED_TITLEBAR;
                let hit = match self.decor.hit_test(style, content, local, false) {
                    hit @ cupertino_decorations::Hit::Button(_) => hit,
                    _ => cupertino_decorations::Hit::None,
                };
                return Some(WindowTarget::Decoration {
                    id,
                    style,
                    hit,
                    content,
                    local,
                });
            }
            // the resize zones, where the window can resize that way (the cursor follows; dropping the zone with the cursor is)
            let edge = if decorated {
                match decoration_hit(true).0 {
                    cupertino_decorations::Hit::Resize(edge) => Some(edge),
                    _ => None,
                }
            } else if plan.exact
                && (!entry.server_side || !self.config.window.decorations || ins.top == 0.0)
            {
                // a client-decorated toplevel draws its own title bar: titled, the band reaches outside its window
                // geometry; a window of ours without a title bar is untitled and has the inside band only
                let local = (pos.x - plan.visual.x, pos.y - plan.visual.y);
                self.decor
                    .resize_hit((plan.visual.w, plan.visual.h), local, !entry.server_side)
            } else {
                None
            };
            if let Some(edge) = edge.filter(|e| self.resize_cursor(id, *e).is_some()) {
                let content = (f64::from(geo.size.w), f64::from(geo.size.h));
                let local = (pos.x - plan.visual.x, pos.y - plan.visual.y);
                return Some(WindowTarget::Decoration {
                    id,
                    style,
                    hit: cupertino_decorations::Hit::Resize(edge),
                    content,
                    local,
                });
            }
            let v = plan.visual;
            let in_frame = pos.x >= v.x && pos.x < v.x + v.w && pos.y >= v.y && pos.y < v.y + v.h;
            if in_frame {
                if let Some((surface, loc)) = entry.window.surface_under(
                    pos - origin,
                    WindowSurfaceType::TOPLEVEL | WindowSurfaceType::SUBSURFACE,
                ) {
                    return Some(WindowTarget::Surface {
                        id,
                        surface,
                        origin: origin + loc.to_f64(),
                    });
                }
            }
            if decorated {
                let (hit, content, local) = decoration_hit(false);
                if matches!(
                    hit,
                    cupertino_decorations::Hit::Button(_) | cupertino_decorations::Hit::Titlebar
                ) {
                    return Some(WindowTarget::Decoration {
                        id,
                        style,
                        hit,
                        content,
                        local,
                    });
                }
            }
        }
        None
    }

    /// Window id under `pos` (global logical), with the surface and its origin, from the topmost
    /// window down. Popups of a window are part of it.
    pub fn window_surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WindowId, WlSurface, Point<f64, Logical>)> {
        match self.window_target_under(pos)? {
            WindowTarget::Surface {
                id,
                surface,
                origin,
            } => Some((id, surface, origin)),
            WindowTarget::Decoration { .. } => None,
        }
    }

    /// Pointer focus target under `pos`: overlay layer surfaces, the popups of the other layer surfaces and of the key
    /// window, top layer surfaces, windows (with the popups of the others), layer surfaces below (the drawing order).
    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.session_lock.is_active() {
            return self.lock_surface_under(pos);
        }
        let out = self.output_at(pos)?;
        let now = self.now();
        let rel = pos - Point::from((out.rect.x, out.rect.y));
        let origin = Point::<f64, Logical>::from((out.rect.x, out.rect.y));
        let covered = crate::render::scene::top_layer_covered(&self.wm, &self.shell, out.id);
        let layer_hit = |layers: &[Layer]| {
            let map = layer_map_for_output(&out.output);
            // every surface of a layer, topmost first: one whose box holds the point but whose input region does not
            // (an overlay strip with an empty region) must not hide the surfaces below it
            for l in layers {
                for surface in crate::authorization::layers_stacked(&map, *l).rev() {
                    if *l == Layer::Top
                        && covered
                        && crate::render::scene::gives_way(surface, &self.menu_bars)
                    {
                        continue;
                    }
                    let geo = map.layer_geometry(surface)?;
                    // a menu bar is offset while it hides or reveals
                    let shift = Point::<f64, Logical>::from((
                        0.0,
                        self.menu_bars
                            .look(surface.wl_surface(), out.rect.y, now)
                            .map_or(0.0, |l| l.dy),
                    ));
                    if let Some((s, loc)) = surface
                        .surface_under(rel - geo.loc.to_f64() - shift, WindowSurfaceType::ALL)
                    {
                        return Some((s, origin + (geo.loc + loc).to_f64() + shift));
                    }
                }
            }
            None
        };
        if let Some(hit) = layer_hit(&[Layer::Overlay]) {
            return Some(hit);
        }
        // the popups of the bars and of the key window are drawn above every window and bar (`SceneLayer::Popups`): a
        // menu over the dock takes the pointer there
        let layer_popup_hit = || {
            let map = layer_map_for_output(&out.output);
            for l in [Layer::Top, Layer::Bottom, Layer::Background] {
                for surface in crate::authorization::layers_stacked(&map, l).rev() {
                    if l == Layer::Top
                        && covered
                        && crate::render::scene::gives_way(surface, &self.menu_bars)
                    {
                        continue;
                    }
                    let Some(geo) = map.layer_geometry(surface) else {
                        continue;
                    };
                    let shift = Point::<f64, Logical>::from((
                        0.0,
                        self.menu_bars
                            .look(surface.wl_surface(), out.rect.y, now)
                            .map_or(0.0, |l| l.dy),
                    ));
                    let base = geo.loc.to_f64() + shift;
                    for (popup, location) in PopupManager::popups_for_surface(surface.wl_surface())
                    {
                        let offset = location - popup.geometry().loc;
                        if let Some((s, loc)) = smithay::desktop::utils::under_from_surface_tree(
                            popup.wl_surface(),
                            rel - base,
                            offset,
                            WindowSurfaceType::ALL,
                        ) {
                            return Some((s, origin + base + loc.to_f64()));
                        }
                    }
                }
            }
            None
        };
        if let Some(hit) = layer_popup_hit() {
            return Some(hit);
        }
        if !self.shell.overview_open() {
            if let Some(WindowTarget::Surface {
                surface, origin, ..
            }) = self.window_popup_under(pos)
            {
                return Some((surface, origin));
            }
        }
        if let Some(hit) = layer_hit(&[Layer::Top]) {
            return Some(hit);
        }
        if self.shell.overview_open() {
            return None;
        }
        match self.window_target_under(pos) {
            Some(WindowTarget::Surface {
                surface, origin, ..
            }) => return Some((surface, origin)),
            Some(WindowTarget::Decoration { .. }) => return None,
            None => {}
        }
        layer_hit(&[Layer::Bottom, Layer::Background])
    }

    // ---- model effects ---------------------------------------------------------------------

    pub fn command(&mut self, cmd: Command) {
        // the model may change a window's buttons: its title, its state, what it can do
        self.button_access.invalidate();
        let now = self.now();
        // a minimize without a target (key binding, action) flies into the dock's slot
        let cmd = match cmd {
            Command::Minimize {
                window,
                target: None,
            } => Command::Minimize {
                window,
                target: self.minimize_target(window),
            },
            other => other,
        };
        let fx = self.wm.handle(cmd, now);
        self.apply_effects(fx);
    }

    pub fn commands(&mut self, cmds: Vec<Command>) {
        for c in cmds {
            self.command(c);
        }
    }

    /// Advance the model to now and apply the resulting effects.
    pub fn tick(&mut self) {
        self.flush_publications();
        self.refresh_control();
        self.forget_gone_processes();
        let now = self.now();
        self.tick_session_lock();
        self.idle.refresh_inhibition();
        self.tick_display_power();
        self.tick_quit();
        crate::acquire_waits::sweep(&mut self.acquire_registry, &self.loop_handle);
        self.tick_hung(now);
        self.tick_force_quit(now);
        let fx = self.wm.tick(now);
        self.apply_effects(fx);
        self.send_deferred_configures(now);
        // a drag that ended elsewhere than at a button (the session locked, the source went away)
        self.drag_ended();
        // the cursor, a drag icon and an input method's popup follow the pointer and the text cursor
        self.refresh_loose_surface_outputs();
        if self.fx.tick(now) {
            // an effect that ended leaves the screen; its last frame may still show it
            self.request_redraw();
        }
        self.tick_window_managers(now);
        self.tick_menu_bars(now);
        self.tick_input_timers(now);
        self.tick_overlays(now);
        self.sync_button_access();
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        self.refresh_shortcut_inhibitors(focus.as_ref());
        self.refresh_pointer_constraints(focus.as_ref());
        // windows without a close transform are skipped by `refresh_snapshot`
        let missing: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|e| e.mapped && self.fx.snapshot_age(e.id, now).is_infinite())
            .map(|e| e.id)
            .collect();
        for id in missing {
            self.refresh_snapshot(id);
        }
        if self
            .shell
            .overview
            .as_ref()
            .is_some_and(|o| o.closing && !o.is_animating(now))
        {
            self.shell.overview = None;
            self.request_redraw();
        }
    }

    /// The earliest model time at which [`Cupertino::tick`] has time-driven work that no event and no frame brings
    /// about: a ping or the pong timeout, a promised SIGKILL, the idle countdowns, the end of the wait for lock
    /// surfaces, a hot corner's dwell, the menu bar edge's reveal or hide, a stale menu edit, and the keyboard filters'
    /// repeat, slow keys and mouse keys, and pending native-control action replies. Animation is not here: it draws frames ([`Cupertino::animating`]).
    pub fn next_tick_deadline(&self) -> Option<f64> {
        let filters = &self.key_filters;
        let ms = |t: u64| t as f64 / 1000.0;
        [
            self.control_deadline(),
            self.hung.next_deadline(),
            self.force_quits.next_deadline(),
            self.display_power_deadline(),
            self.lock_wait_deadline(),
            self.hot_corners.next_deadline(),
            self.bar_edge.next_deadline(),
            self.menu_bars.set.next_deadline(),
            self.menu_bars.edit_deadline(),
            filters.slow.as_ref().and_then(|s| s.deadline()).map(ms),
            filters.mouse.deadline().map(ms),
            filters.repeat.next_deadline().map(ms),
        ]
        .into_iter()
        .flatten()
        .reduce(f64::min)
    }

    /// Whether something animates: every output then draws a frame per its own refresh.
    pub fn animating(&self) -> bool {
        let now = self.now();
        self.wm.wants_tick(now)
            || self.shell.wants_tick(now)
            || self.fx.wants_tick(now)
            || self.window_managers.wants_tick(now)
            || self.menu_bars_want_tick(now)
            || self.overlays.wants_tick()
            || self.power.policy().animating(now)
    }

    pub fn wants_frames(&self) -> bool {
        self.animating() || self.outputs.iter().any(|o| o.scheduler.damaged())
    }

    pub fn apply_effects(&mut self, effects: Vec<Effect>) {
        let mut redraw = false;
        for fx in effects {
            redraw = true;
            match fx {
                Effect::Configure {
                    window,
                    size,
                    states,
                } => {
                    let activated = self.activated_window() == Some(window);
                    let now = self.now();
                    let (pos, animating) = self
                        .wm
                        .window(window)
                        .map(|w| (w.geometry(), w.lifecycle.is_animating(now)))
                        .unwrap_or_default();
                    if let Some(e) = self.windows.iter_mut().find(|e| e.id == window) {
                        let frame = Rect::new(pos.x, pos.y, size.w, size.h);
                        let (w, h) = e.committed_size();
                        if animating && e.mapped && (frame.w < w || frame.h < h) {
                            let covering =
                                Rect::new(frame.x, frame.y, frame.w.max(w), frame.h.max(h));
                            e.configure(covering, &states, activated);
                            e.shrink = Shrink::Deferred { frame, states };
                        } else {
                            e.configure(frame, &states, activated);
                            e.shrink = Shrink::None;
                        }
                    }
                }
                // a window that moved is told its outputs and scale below
                Effect::Place { .. } => {}
                Effect::RequestClose { window } => {
                    if let Some(e) = self.entry(window) {
                        e.close();
                    }
                }
                Effect::Focus { window } => {
                    // an exclusive layer surface keeps the keyboard; the key window gets it when
                    // the layer lets go (`layer_keyboard_exclusivity`)
                    if self.exclusive_layer.is_none() {
                        let surface = window
                            .and_then(|w| self.entry(w))
                            .map(|e| e.surface().clone());
                        self.set_keyboard_focus(surface);
                    }
                    // focus moved elsewhere: the accent popup loses its reason to stay open
                    self.accent_dismiss();
                    self.refresh_activation();
                    if let Some(window) = window {
                        self.recheck_menu_owners(window);
                        self.follow_focus_to_output(window);
                    }
                }
                Effect::WindowDestroyed { window } => {
                    if let Some(h) = self.entry(window).and_then(|e| e.foreign.clone()) {
                        h.send_closed();
                    }
                }
                Effect::SwitcherChanged { apps, selected } => {
                    let names = apps
                        .iter()
                        .filter_map(|a| self.wm.app(*a))
                        .map(|a| a.identity.clone())
                        .collect();
                    self.switcher_changed(names, selected);
                }
                Effect::AppAttention { app } => {
                    if let Some(a) = self.wm.app(app) {
                        self.docks.attention(&a.identity);
                    }
                }
                Effect::SwitcherClosed { committed } => self.docks.switcher_closed(committed),
                // the overview does not survive a workspace switch (the dock's, a shortcut's or a
                // swipe's); workspace edits reach the dock's bar
                Effect::WorkspaceSwitched { output, .. } => {
                    if self.shell.overview_on(output).is_some() {
                        self.close_overview();
                    }
                }
                Effect::WorkspaceCreated { .. }
                | Effect::WorkspaceRemoved { .. }
                | Effect::WorkspacesReordered { .. } => self.send_overview_to_docks(),
                Effect::Activate { .. }
                | Effect::AppRegistered { .. }
                | Effect::WindowStateChanged { .. }
                | Effect::Attention { .. }
                | Effect::Raise { .. }
                | Effect::SetVisible { .. }
                | Effect::StartAnimation { .. }
                | Effect::SnapPreview { .. } => {}
            }
        }
        // the outputs each window is on and the scale it draws at follow its frame (`displays::surface_output`)
        self.refresh_window_outputs(false);
        if redraw {
            self.request_redraw();
            self.refresh_overview_windows();
            self.refresh_menu_bars();
            self.refresh_docks();
            self.refresh_window_managers();
        }
    }

    /// Keep the open overview in sync with windows that opened, closed or changed state: a changed
    /// set goes to the dock for a new layout.
    fn refresh_overview_windows(&mut self) {
        let Some(out) = self
            .shell
            .overview
            .as_ref()
            .filter(|s| !s.closing)
            .map(|s| s.output)
        else {
            return;
        };
        let now = self.now();
        let windows = shell::overview_windows(&self.wm, out, now, self.dock_rect(out));
        let changed = self
            .shell
            .overview
            .as_mut()
            .is_some_and(|s| s.set_windows(windows));
        if changed {
            self.send_overview_to_docks();
        }
    }

    /// The toplevel whose `activated` state is on: the model's key window, and none while the session is locked or about to
    /// be, so no application behaves as the one in front behind the lock screen.
    pub fn activated_window(&self) -> Option<WindowId> {
        self.wm
            .key_window()
            .filter(|_| !self.session_lock.is_active())
    }

    /// Make every toplevel's `activated` state match [`Self::activated_window`].
    pub fn refresh_activation(&mut self) {
        let key = self.activated_window();
        for e in &self.windows {
            e.set_activated(Some(e.id) == key);
        }
    }

    /// Give the keyboard to `surface`. While the session is locked only a lock surface gets it, and while the
    /// authentication prompt is shown only the prompt. A popup grab whose chain `surface` is outside of ends first, its
    /// menus dismissed ([`Cupertino::end_popup_grab_for_focus`]).
    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        if surface
            .as_ref()
            .is_some_and(|s| self.control_sensitive_focus(s))
        {
            self.suspend_controls(crate::control::PauseReason::ProtectedFocus);
        }
        let allowed = if self.session_lock.is_active() {
            surface.as_ref().is_some_and(|s| self.is_lock_surface(s))
        } else {
            !self.prompt_shown()
                || surface
                    .as_ref()
                    .is_some_and(crate::authorization::is_prompt)
        };
        if !allowed {
            return;
        }
        self.end_popup_grab_for_focus(surface.as_ref());
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.set_focus(self, surface, SERIAL_COUNTER.next_serial());
    }

    /// Give the keyboard back to the key window.
    pub fn restore_keyboard_focus(&mut self) {
        let surface = self
            .wm
            .key_window()
            .and_then(|w| self.entry(w))
            .map(|e| e.surface().clone());
        self.set_keyboard_focus(surface);
    }

    /// A layer surface above the windows that asks for exclusive keyboard interactivity takes the
    /// keyboard; when it stops asking or goes away the key window gets it back. A dock bar or menu bar a press outside
    /// it released ([`Cupertino::release_engaged_layer`]) is not given it again until it stops asking.
    ///
    /// While the session is locked only a shell process can take it: an application's layer surface must not collect
    /// what is typed at the lock screen, nor wait to take the keyboard at the unlock. The authentication prompt keeps its
    /// claim under the lock, which holds the keyboard, and gets it back at the unlock. While the authentication prompt is
    /// shown only the prompt can take it, and an open menu of another surface is closed as it does.
    pub fn layer_keyboard_exclusivity(&mut self, surface: &WlSurface, exclusive: bool) {
        if exclusive && self.control_sensitive_focus(surface) {
            self.suspend_controls(crate::control::PauseReason::ProtectedFocus);
        }
        if !exclusive && self.released_layer.as_ref() == Some(surface) {
            // the layer let go after a press outside it: its next claim is a new one
            self.released_layer = None;
        }
        let prompt = crate::authorization::is_prompt(surface);
        let exclusive = exclusive
            && self.released_layer.as_ref() != Some(surface)
            && (!self.session_lock.is_active()
                || prompt
                || surface
                    .client()
                    .is_some_and(|c| crate::authorization::capabilities(&c).holds_shell_role()))
            && (prompt || !self.prompt_shown());
        let holds = self.exclusive_layer.as_ref() == Some(surface);
        if exclusive && !holds {
            self.exclusive_layer = Some(surface.clone());
            self.set_keyboard_focus(Some(surface.clone()));
        } else if !exclusive && holds {
            self.exclusive_layer = None;
            // another surface may have taken the focus since: leave it there
            if self.seat.get_keyboard().unwrap().current_focus().as_ref() == Some(surface) {
                self.restore_keyboard_focus();
            }
        }
    }

    /// A press at `pos` outside the dock bar or menu bar that holds the keyboard exclusively, which it asks for only
    /// while engaged (Focus Dock, the menu bar's keyboard mode), ends that: the layer loses the keyboard, which goes back
    /// to the key window, and the layer answers the leave by letting go. A press on the layer or on one of its popups
    /// keeps it. Other exclusive layers (the authentication prompt, the launcher, the overview) are left as they are,
    /// and the lock screen never gets here.
    ///
    /// A menu open from the layer closes first, so the keyboard is back on the layer when it leaves and the layer is
    /// told; the press is then consumed, as any press that closes a menu is. True when it was.
    pub fn release_engaged_layer(
        &mut self,
        pos: Point<f64, Logical>,
        serial: Serial,
        time: u32,
    ) -> bool {
        let Some(layer) = self
            .exclusive_layer
            .clone()
            .filter(|l| self.docks.has_surface(l) || self.menu_bars.has_surface(l))
        else {
            return false;
        };
        if self
            .surface_under(pos)
            .is_some_and(|(surface, _)| self.shell_surface_root(&surface) == layer)
        {
            return false;
        }
        let menus = self.popup_grab_root() == Some(&layer)
            && self.dismiss_popup_grab_for_press(pos, serial, time);
        self.exclusive_layer = None;
        self.released_layer = Some(layer);
        self.restore_keyboard_focus();
        menus
    }

    /// The surface whose tree `surface` is in: the parent of a subsurface, the surface a popup chain grows from.
    pub(crate) fn shell_surface_root(&self, surface: &WlSurface) -> WlSurface {
        let top = |s: &WlSurface| {
            let mut top = s.clone();
            while let Some(parent) = smithay::wayland::compositor::get_parent(&top) {
                top = parent;
            }
            top
        };
        let surface = top(surface);
        self.popups
            .find_popup(&surface)
            .and_then(|p| smithay::desktop::find_popup_root_surface(&p).ok())
            .map_or(surface, |root| top(&root))
    }

    // ---- actions ---------------------------------------------------------------------------

    pub fn action_context(&self) -> ActionContext {
        let output = self.active_output();
        let (workspaces, current) = output
            .map(|o| {
                let list: Vec<_> = self.wm.workspaces(o).iter().map(|w| w.id).collect();
                let cur = self
                    .wm
                    .current_workspace(o)
                    .and_then(|c| list.iter().position(|w| *w == c))
                    .unwrap_or(0);
                (list, cur)
            })
            .unwrap_or_default();
        let front_app = self.wm.front_app();
        let front_app_windows = front_app
            .and_then(|a| self.wm.app(a))
            .map(|a| a.windows_mru.clone())
            .unwrap_or_default();
        ActionContext {
            key_window: self.wm.key_window(),
            front_app,
            front_app_windows,
            output,
            switcher_open: self.wm.switcher().is_some(),
            overview_open: self.shell.overview_open(),
            workspaces,
            current_workspace: current,
            locked: self.session_lock.is_active(),
            input_held: self.shortcut_inhibit.holding(),
            pointer_held: self.pointer_held(),
            key_window_fullscreen: self
                .wm
                .key_window()
                .and_then(|w| self.wm.window(w))
                .is_some_and(|w| w.state().is_fullscreen()),
        }
    }

    /// Run what a key, a gesture or a menu asked for: the keyboard focus it gives is the person's own input.
    pub fn execute_plan(&mut self, plan: ActionPlan) {
        self.as_input(|s| s.run_plan(plan));
    }

    fn run_plan(&mut self, plan: ActionPlan) {
        self.commands(plan.commands);
        for op in plan.ops {
            match op {
                ShellOp::ToggleOverview => self.toggle_overview(false),
                ShellOp::ToggleAppOverview => self.toggle_overview(true),
                ShellOp::CloseOverview => self.close_overview(),
                ShellOp::SpawnTerminal => self.spawn_terminal(),
                ShellOp::Screenshot(request) => self.start_screenshot(request),
                ShellOp::Quit => self.quit_compositor(),
                ShellOp::SwitchVt(n) => {
                    if let Some(b) = self.backend.as_mut() {
                        b.switch_vt(n);
                    }
                }
                ShellOp::ToggleShowDesktop => {
                    let now = self.now();
                    self.fx.toggle_show_desktop(now);
                    self.request_redraw();
                }
                ShellOp::ToggleAppGrid => self.toggle_applications(),
                ShellOp::OpenForceQuit => self.docks.force_quit_dialog(),
                ShellOp::ToggleLauncher => self.toggle_launcher(),
                ShellOp::ToggleNotificationCenter => self.toggle_notification_center(),
                ShellOp::ToggleControlPanel => self.toggle_control_panel(),
                ShellOp::ToggleAppearance => self.toggle_appearance(),
                ShellOp::ToggleDisplayOption(option) => self.toggle_display_option(option),
                ShellOp::Osd { key, fine } => self.osd_key(key, fine),
                ShellOp::InputSource { menu, step } => self.input_source_step(menu, step),
                ShellOp::FocusMenuBar => self.focus_menu_bar(),
                ShellOp::HelpSearch => self.focus_help_search(),
                ShellOp::FocusDock => self.focus_dock(),
                ShellOp::ToggleDockAutoHide => self.docks.toggle_auto_hide(),
                ShellOp::ToggleStage => self.window_managers.toggle_stage(),
                ShellOp::Exec(i) => {
                    if let Some(cmd) = self.key_router.bindings.exec_command(i).map(str::to_string)
                    {
                        self.spawn_command_line(&cmd);
                    }
                }
                ShellOp::Lock => self.lock_session_now(),
                ShellOp::LogOut => self.log_out_session(),
                ShellOp::PowerDialog => self.power_dialog(),
                ShellOp::ReleaseShortcuts => {
                    self.release_shortcuts();
                }
                ShellOp::PowerKey { pressed } => self.power_key(pressed),
            }
        }
    }

    /// End the server: the event loop stops after this iteration.
    pub fn quit_compositor(&mut self) {
        self.running = false;
        self.loop_signal.stop();
    }

    pub fn spawn_terminal(&mut self) {
        let candidates = self.config.terminals.clone();
        match cupertino_defaults::terminal::resolve_installed(&candidates) {
            Some(terminal) => self.spawn(&terminal, &[]),
            None => tracing::warn!(?candidates, "no terminal emulator found"),
        }
    }

    /// Start a client on this compositor's socket. The application receives an activation token
    /// so its first window takes focus, and the docks learn which application is launching when
    /// the program has a desktop entry.
    pub fn spawn(&mut self, program: &str, args: &[&str]) {
        let mut cmd = std::process::Command::new(program);
        cmd.args(args);
        let token = self.server_activation_token();
        cmd.env("XDG_ACTIVATION_TOKEN", &token)
            .env("DESKTOP_STARTUP_ID", &token);
        self.spawn_prepared(cmd);
    }

    /// Start a prepared command on this compositor's socket.
    pub fn spawn_prepared(&mut self, mut cmd: std::process::Command) {
        if self.login_screen {
            tracing::warn!(program = %cmd.get_program().to_string_lossy(), "the login screen starts no programs");
            return;
        }
        if let Some(s) = &self.socket_name {
            cmd.env("WAYLAND_DISPLAY", s);
        }
        cmd.env("XDG_SESSION_TYPE", "wayland")
            .env("XDG_CURRENT_DESKTOP", CURRENT_DESKTOP);
        // clients that draw their own cursors draw them in the asset theme (the only one on their path) at the user cursor
        // size; without a theme they keep the cursor theme of their environment
        cmd.env(
            "XCURSOR_SIZE",
            self.cursor_loader
                .client_size(self.config.pointer.cursor_scale)
                .to_string(),
        );
        if let Some(theme) = self.cursor_loader.theme() {
            cmd.env("XCURSOR_THEME", &theme.name)
                .env("XCURSOR_PATH", &theme.search_dir);
        }
        crate::xwayland::set_client_env(&mut cmd, self.xwayland.as_ref().map(|x| x.display()));
        cmd.env_remove("WAYLAND_SOCKET");
        let program = cmd.get_program().to_string_lossy().into_owned();
        // the docks key tiles by desktop entry id; a program without an entry (a script, a shell)
        // is not an application launch
        let app_id = std::path::Path::new(&program)
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| self.desktop.find(n))
            .map(|e| e.id.clone());
        crate::file_limit::restore_in_child(&mut cmd);
        match cmd.spawn() {
            Ok(mut child) => {
                if let Some(app_id) = app_id {
                    self.docks.launching(&app_id);
                }
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => tracing::warn!(program, error = %e, "spawning failed"),
        }
    }

    /// Run a command line (program and arguments split at whitespace).
    pub fn spawn_command_line(&mut self, line: &str) {
        let parts: Vec<String> = line.split_whitespace().map(String::from).collect();
        if let Some((prog, args)) = parts.split_first() {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            self.spawn(prog, &args);
        }
    }

    pub fn app_identity_of(&self, app: AppId) -> Option<String> {
        self.wm.app(app).map(|a| a.identity.clone())
    }

    /// Serial helper.
    pub fn next_serial(&self) -> Serial {
        SERIAL_COUNTER.next_serial()
    }

    pub fn rect_to_logical(r: Rect) -> Rectangle<i32, Logical> {
        Rectangle::new(
            (r.x.round() as i32, r.y.round() as i32).into(),
            (r.w.round() as i32, r.h.round() as i32).into(),
        )
    }

    /// The primary output's scale. A surface is told its own output's (`Cupertino::surface_scale`).
    pub fn preferred_scale(&self) -> f64 {
        self.outputs.first().map_or(1.0, |o| o.scale)
    }
}

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
pub fn output_rect(
    mode: OutputMode,
    scale: f64,
    transform: Transform,
    position: (i32, i32),
) -> Rect {
    let size = transform.transform_size(mode.size);
    Rect::new(
        f64::from(position.0),
        f64::from(position.1),
        (f64::from(size.w) / scale).round(),
        (f64::from(size.h) / scale).round(),
    )
}

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
