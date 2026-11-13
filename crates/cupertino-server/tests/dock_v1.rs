//! `cupertino_shell_v1.get_dock` end to end: the compositor runs in-process on an offscreen EGL
//! device, a dock client (a bottom-anchored layer surface with the dock role) and application
//! clients connect through its socket. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::CupertinoBackdropV1;
use cupertino_protocols::client::cupertino_dock_v1::{
    self, AppCommand, CupertinoDockV1, WindowState,
};
use cupertino_protocols::client::cupertino_session_v1::{self, CupertinoSessionV1};
use cupertino_protocols::client::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::config::Config;
use cupertino_server::desktop::DesktopIndex;
use cupertino_server::input::{ActionPlan, ShellOp};
use cupertino_server::render::scene::plan_for;
use cupertino_server::Cupertino;
use cupertino_wm::{Command, LifecycleState, Rect, WindowId};
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::xdg_activation::{
    XdgActivationHandler, XdgActivationToken, XdgActivationTokenData,
};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::WEnum;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols::xdg::toplevel_icon::v1::client::{
    xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1, xdg_toplevel_icon_v1::XdgToplevelIconV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const OUTPUT: (i32, i32) = (1000, 700);
/// Height of the dock surface: full width, anchored to the bottom, reserving its whole height.
const DOCK_HEIGHT: i32 = 100;
/// Slot the dock reports for the next minimized window, surface-local.
const SLOT: (i32, i32, i32, i32) = (100, 20, 48, 48);
/// The same slot in global coordinates: the surface sits at the bottom of the output.
const SLOT_GLOBAL: Rect = Rect {
    x: 100.0,
    y: 620.0,
    w: 48.0,
    h: 48.0,
};
/// The bar's backdrop inside the dock surface (x, y, width, height): narrower than the surface.
const BAR: (i32, i32, i32, i32) = (300, 30, 400, 64);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let cfg = Config::default();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            cfg,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        common::each_connection_a_process(&mut state);
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Step until the dock client has seen `count` groups.
    fn wait_for_groups(&mut self, dock: &DockClient, count: usize) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if dock.groups().len() >= count {
                return;
            }
        }
        panic!(
            "timed out waiting for group {count}, have {:?}",
            dock.groups()
        );
    }

    /// A connection of a shell process the server supervises: the only kind that may be a dock.
    fn shell_stream(&mut self) -> UnixStream {
        cupertino_server::shell_connection::shell_process_connection(
            &self.state.display_handle,
            &[ShellRole::Session, ShellRole::Dock],
        )
        .unwrap()
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Move the model clock forward and settle the animations.
    fn advance(&mut self, dt: f64) {
        let time = self.state.now() + dt;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        for _ in 0..3 {
            self.step();
        }
    }

    /// Wait for `count` application windows and let the opening animations finish.
    fn mapped_window(&mut self, count: usize) -> WindowId {
        self.wait_until("the window to be mapped", |s| {
            s.wm.windows().count() == count
        });
        self.advance(5.0);
        self.state.wm.key_window().expect("focused window")
    }
}

// ---- test clients --------------------------------------------------------------------------

fn tempfile_like(len: usize) -> std::fs::File {
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let file = std::fs::File::from(fd);
    file.set_len(len as u64).unwrap();
    file
}

/// Attach an opaque grey buffer of `size` to `surface` and commit.
fn attach_and_commit(
    shm: &wl_shm::WlShm,
    surface: &wl_surface::WlSurface,
    size: (i32, i32),
    qh: &QueueHandle<ClientState>,
) {
    let (w, h) = size;
    let mut file = tempfile_like((w * h * 4) as usize);
    file.write_all(&vec![128u8; (w * h * 4) as usize]).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w, h);
    surface.commit();
}

/// What the dock object sent, in order.
#[derive(Clone, Debug, PartialEq)]
enum DockEvent {
    Window {
        id: u32,
        app_id: String,
        title: String,
        minimized: bool,
        active: bool,
    },
    /// `window_icon`: the name (empty for none), the side and the pixels the file held.
    WindowIcon {
        id: u32,
        name: String,
        size: u32,
        pixels: Vec<u8>,
    },
    /// `window_thumbnail`: the size and the pixels the file held.
    WindowThumbnail {
        id: u32,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    },
    Closed(u32),
    Launching(String),
    ToggleAutoHide,
    ToggleAppGrid,
    Unresponsive {
        app_id: String,
        unresponsive: bool,
    },
    ForceQuitDialog,
    Engage,
    /// `own`: the output named is the one the dock is on.
    SwitcherOutput {
        own: bool,
    },
    SwitcherEntry(String),
    SwitcherChanged(u32),
    SwitcherClosed(u32),
    Attention(String),
    OverviewWindow {
        id: u32,
        frame: (i32, i32, i32, i32),
    },
    OverviewWorkspace {
        id: u32,
        current: bool,
    },
    /// `app` is true for the application overview.
    OverviewChanged {
        app: bool,
        app_id: String,
        area: (i32, i32, i32, i32),
    },
    OverviewClosed,
    HotCorner(u32),
    Gesture(u32),
    /// `app_resident`: the application id and whether it is resident.
    Resident(String, bool),
    Done,
    /// `cupertino_session_v1.app_activation`: the serial and the result (0 activated, 1 resident, 2 not running).
    Opened(u32, u32),
}

#[derive(Default)]
struct Status {
    events: Vec<DockEvent>,
    /// The protocol error the server posted: object interface and code.
    error: Option<(String, u32)>,
    /// The output a dock client made its dock on.
    own_output: Option<wl_output::WlOutput>,
}

struct ClientState {
    status: Arc<Mutex<Status>>,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    /// The first buffer was committed.
    attached: bool,
    toplevel_size: (i32, i32),
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClientState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
mod gtk_shell {
    #![allow(
        dead_code,
        non_camel_case_types,
        non_upper_case_globals,
        unused_imports,
        missing_docs,
        clippy::all
    )]
    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/gtk-shell.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/gtk-shell.xml");
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for ClientState {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    wl_output::WlOutput,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    CupertinoShellV1,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1,
    xdg_activation_v1::XdgActivationV1,
    gtk_shell::gtk_shell1::GtkShell1,
    gtk_shell::gtk_surface1::GtkSurface1,
    XdgToplevelIconManagerV1,
    XdgToplevelIconV1
);

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, width, .. } = event {
            layer.ack_configure(serial);
            if !state.attached {
                state.attached = true;
                attach_and_commit(&state.shm, &state.surface, (width as i32, DOCK_HEIGHT), qh);
            }
        }
    }
}

impl Dispatch<CupertinoDockV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoDockV1,
        event: cupertino_dock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_dock_v1::Event as E;
        let event = match event {
            E::Window {
                id,
                app_id,
                title,
                state,
            } => {
                let bits = WindowState::from_bits_truncate(u32::from(state));
                DockEvent::Window {
                    id,
                    app_id,
                    title,
                    minimized: bits.contains(WindowState::Minimized),
                    active: bits.contains(WindowState::Active),
                }
            }
            E::WindowIcon {
                id,
                name,
                size,
                pixels,
            } => {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut std::fs::File::from(pixels), &mut bytes).unwrap();
                DockEvent::WindowIcon {
                    id,
                    name,
                    size,
                    pixels: bytes,
                }
            }
            E::WindowThumbnail {
                id,
                width,
                height,
                pixels,
            } => {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut std::fs::File::from(pixels), &mut bytes).unwrap();
                DockEvent::WindowThumbnail {
                    id,
                    width,
                    height,
                    pixels: bytes,
                }
            }
            E::WindowClosed { id } => DockEvent::Closed(id),
            E::Launching { app_id } => DockEvent::Launching(app_id),
            E::ToggleAutoHide => DockEvent::ToggleAutoHide,
            E::ToggleAppGrid => DockEvent::ToggleAppGrid,
            E::Unresponsive { app_id, state } => DockEvent::Unresponsive {
                app_id,
                unresponsive: state == 1,
            },
            E::ForceQuitDialog => DockEvent::ForceQuitDialog,
            E::Engage => DockEvent::Engage,
            E::SwitcherOutput { output } => DockEvent::SwitcherOutput {
                own: state.status.lock().unwrap().own_output.as_ref() == Some(&output),
            },
            E::SwitcherEntry { app_id } => DockEvent::SwitcherEntry(app_id),
            E::SwitcherChanged { selected } => DockEvent::SwitcherChanged(selected),
            E::SwitcherClosed { committed } => DockEvent::SwitcherClosed(committed),
            E::Attention { app_id } => DockEvent::Attention(app_id),
            E::OverviewWindow {
                id,
                x,
                y,
                width,
                height,
            } => DockEvent::OverviewWindow {
                id,
                frame: (x, y, width, height),
            },
            E::OverviewWorkspace { id, current } => DockEvent::OverviewWorkspace {
                id,
                current: current != 0,
            },
            E::OverviewChanged {
                mode,
                app_id,
                x,
                y,
                width,
                height,
            } => DockEvent::OverviewChanged {
                app: mode == WEnum::Value(cupertino_dock_v1::OverviewMode::App),
                app_id,
                area: (x, y, width, height),
            },
            E::OverviewClosed => DockEvent::OverviewClosed,
            E::HotCorner {
                corner: WEnum::Value(c),
            } => DockEvent::HotCorner(c as u32),
            E::Gesture {
                kind: WEnum::Value(k),
            } => DockEvent::Gesture(k as u32),
            E::AppResident { app_id, resident } => DockEvent::Resident(app_id, resident != 0),
            E::Done => DockEvent::Done,
            other => panic!("unexpected event {other:?}"),
        };
        state.status.lock().unwrap().events.push(event);
    }
}

impl Dispatch<CupertinoSessionV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoSessionV1,
        event: cupertino_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // the session's state is not what these tests are about
        if let cupertino_session_v1::Event::AppActivation { serial, result } = event {
            let result = match result {
                WEnum::Value(v) => v as u32,
                WEnum::Unknown(v) => v,
            };
            state
                .status
                .lock()
                .unwrap()
                .events
                .push(DockEvent::Opened(serial, result));
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientState {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for ClientState {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            if !state.attached {
                state.attached = true;
                attach_and_commit(&state.shm, &state.surface, state.toplevel_size, qh);
            }
        }
    }
}

/// A dialog's surface: it takes its buffer on its first configure.
impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for ClientState {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            attach_and_commit(&state.shm, surface, (200, 100), qh);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        _: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Requests a test sends to the dock client thread.
enum Cmd {
    ActivateApp(&'static str),
    RestoreWindow(u32),
    /// `set_minimize_target` followed by a commit of the dock surface.
    SetMinimizeTarget,
    SwitcherSelect(u32),
    SwitcherActivate(u32),
    SwitcherCancel,
    OverviewFrame(u32, (i32, i32, i32, i32)),
    OverviewSelect(u32),
    OverviewClose,
    SwitchWorkspace(u32),
    AddWorkspace,
    RemoveWorkspace(u32),
    MoveWorkspace(u32, u32),
    RunAction(&'static str),
    ForceQuitApp(&'static str),
    AppCommand(&'static str, AppCommand),
    /// An application window asks to be activated with an `xdg_activation_v1` token.
    Activate(String),
    /// An application window calls `gtk_surface1.present`.
    GtkPresent,
    /// An application window calls `gtk_surface1.request_focus` with the given startup id.
    GtkRequestFocus(Option<String>),
    /// An application window sets an icon of this name and one [`ICON_SIDE`] buffer of [`ICON_PIXEL`]s, and commits.
    SetIcon(&'static str),
    /// An application destroys its toplevel and stays connected.
    CloseWindow,
    /// An application maps a dialog, a toplevel whose parent is its window, of this application id.
    OpenDialog(&'static str),
    /// The dock asks for exclusive keyboard interactivity (true) or for none, and commits.
    SetKeyboard(bool),
    /// The launcher's session object asks to bring the application of a desktop file ID forward, with a serial.
    OpenApp(u32, &'static str),
    /// Drop the connection: the dock process died.
    Quit,
}

/// A client thread: the connection, its status and the command channel.
struct TestClient {
    commands: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for TestClient {
    fn drop(&mut self) {
        // The thread may be blocked in a round trip only the (not running) compositor can answer:
        // it is detached and ends when the compositor drops its socket.
        self.thread.take();
    }
}

fn run_client(
    stream: UnixStream,
    setup: impl FnOnce(
            &wayland_client::globals::GlobalList,
            &QueueHandle<ClientState>,
            &Arc<Mutex<Status>>,
        ) -> (ClientState, Box<dyn FnMut(Cmd)>)
        + Send
        + 'static,
) -> TestClient {
    let (commands, inbox): (Sender<Cmd>, Receiver<Cmd>) = channel();
    let status = Arc::new(Mutex::new(Status::default()));
    let st = status.clone();
    let thread = std::thread::spawn(move || {
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
        let qh = queue.handle();
        let (mut client, mut apply) = setup(&globals, &qh, &st);
        loop {
            while let Ok(cmd) = inbox.try_recv() {
                if matches!(cmd, Cmd::Quit) {
                    return;
                }
                apply(cmd);
            }
            if queue.roundtrip(&mut client).is_err() {
                let error = conn
                    .protocol_error()
                    .expect("the roundtrip failed with a protocol error");
                st.lock().unwrap().error = Some((error.object_interface, error.code));
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    TestClient {
        commands,
        status,
        thread: Some(thread),
    }
}

/// The dock process: a bottom-anchored layer surface with the dock role on the first output.
struct DockClient(TestClient);

impl DockClient {
    /// `layer` false makes the dock of a surface that has no layer role.
    fn spawn(stream: UnixStream, layer: bool) -> DockClient {
        Self::spawn_on(stream, layer, 0)
    }

    /// The dock of the `index`th output the server announces.
    fn spawn_on(stream: UnixStream, layer: bool, index: usize) -> DockClient {
        Self::spawn_at(stream, layer, index, 9)
    }

    /// The dock of a shell global bound at `version`.
    fn spawn_at(stream: UnixStream, layer: bool, index: usize, version: u32) -> DockClient {
        DockClient(run_client(stream, move |globals, qh, status| {
            let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
            let shell: CupertinoShellV1 = globals.bind(qh, version..=version, ()).unwrap();
            let effects: CupertinoAppEffectsV1 = globals.bind(qh, 1..=1, ()).unwrap();
            let global = globals
                .contents()
                .clone_list()
                .into_iter()
                .filter(|g| g.interface == "wl_output")
                .nth(index)
                .expect("the output");
            let output: wl_output::WlOutput =
                globals
                    .registry()
                    .bind(global.name, global.version.min(4), qh, ());
            status.lock().unwrap().own_output = Some(output.clone());
            let surface = compositor.create_surface(qh, ());
            let layer_surface = layer.then(|| {
                let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                    globals.bind(qh, 1..=4, ()).unwrap();
                let l = layer_shell.get_layer_surface(
                    &surface,
                    Some(&output),
                    zwlr_layer_shell_v1::Layer::Top,
                    "dock-test".into(),
                    qh,
                    (),
                );
                l.set_size(0, DOCK_HEIGHT as u32);
                l.set_anchor(
                    zwlr_layer_surface_v1::Anchor::Bottom
                        | zwlr_layer_surface_v1::Anchor::Left
                        | zwlr_layer_surface_v1::Anchor::Right,
                );
                l.set_exclusive_zone(DOCK_HEIGHT);
                l
            });
            let dock = shell.get_dock(&surface, &output, qh, ());
            if layer {
                let backdrop = effects.get_backdrop(&surface, qh, ());
                backdrop.set_material("Menu".into());
                backdrop.set_shape(BAR.0, BAR.1, BAR.2, BAR.3, 16.0);
            }
            surface.commit();
            let commit_surface = surface.clone();
            let state = ClientState {
                status: status.clone(),
                shm,
                surface,
                attached: false,
                toplevel_size: (0, 0),
            };
            let apply = Box::new(move |cmd: Cmd| match cmd {
                Cmd::ActivateApp(app_id) => dock.activate_app(app_id.to_string()),
                Cmd::RestoreWindow(id) => dock.restore_window(id, SLOT.0, SLOT.1, SLOT.2, SLOT.3),
                Cmd::SetMinimizeTarget => {
                    dock.set_minimize_target(SLOT.0, SLOT.1, SLOT.2, SLOT.3);
                    commit_surface.commit();
                }
                Cmd::SwitcherSelect(index) => dock.switcher_select(index),
                Cmd::SwitcherActivate(index) => dock.switcher_activate(index),
                Cmd::SwitcherCancel => dock.switcher_cancel(),
                Cmd::OverviewFrame(id, (x, y, w, h)) => dock.overview_frame(id, x, y, w, h),
                Cmd::OverviewSelect(id) => dock.overview_select(id),
                Cmd::OverviewClose => dock.overview_close(),
                Cmd::SwitchWorkspace(id) => dock.switch_workspace(id),
                Cmd::AddWorkspace => dock.add_workspace(),
                Cmd::RemoveWorkspace(id) => dock.remove_workspace(id),
                Cmd::MoveWorkspace(id, index) => dock.move_workspace(id, index),
                Cmd::RunAction(name) => dock.run_action(name.to_string()),
                Cmd::ForceQuitApp(app_id) => dock.force_quit_app(app_id.to_string()),
                Cmd::AppCommand(app_id, command) => dock.app_command(app_id.to_string(), command),
                Cmd::SetKeyboard(exclusive) => {
                    layer_surface
                        .as_ref()
                        .expect("a layer surface")
                        .set_keyboard_interactivity(if exclusive {
                            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive
                        } else {
                            zwlr_layer_surface_v1::KeyboardInteractivity::None
                        });
                    commit_surface.commit();
                }
                Cmd::Activate(_)
                | Cmd::GtkPresent
                | Cmd::GtkRequestFocus(_)
                | Cmd::SetIcon(_)
                | Cmd::CloseWindow
                | Cmd::OpenDialog(_)
                | Cmd::OpenApp(..) => {
                    panic!("the dock does not act as an application window or the launcher")
                }
                Cmd::Quit => unreachable!("handled by the client loop"),
            });
            (state, apply)
        }))
    }

    /// The events received so far, split into groups ended by `done`; a trailing partial group is not listed.
    fn groups(&self) -> Vec<Vec<DockEvent>> {
        let events = self.0.status.lock().unwrap().events.clone();
        let mut groups = vec![Vec::new()];
        for e in events {
            let done = e == DockEvent::Done;
            groups.last_mut().unwrap().push(e);
            if done {
                groups.push(Vec::new());
            }
        }
        groups.pop();
        groups
    }

    fn send(&self, cmd: Cmd) {
        self.0.commands.send(cmd).unwrap();
    }

    fn error(&self) -> Option<(String, u32)> {
        self.0.status.lock().unwrap().error.clone()
    }
}

/// The launcher: a shell process with the Session role and its session object, on a shell global of version 20.
struct LauncherClient(TestClient);

impl LauncherClient {
    fn spawn(stream: UnixStream) -> LauncherClient {
        LauncherClient(run_client(stream, |globals, qh, status| {
            let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
            let shell: CupertinoShellV1 = globals.bind(qh, 20..=20, ()).unwrap();
            let session = shell.get_session(qh, ());
            let state = ClientState {
                status: status.clone(),
                shm,
                surface: compositor.create_surface(qh, ()),
                attached: false,
                toplevel_size: (0, 0),
            };
            let apply = Box::new(move |cmd: Cmd| match cmd {
                Cmd::OpenApp(serial, desktop_id) => {
                    session.activate_app(serial, desktop_id.to_string());
                }
                _ => panic!("the launcher only opens applications"),
            });
            (state, apply)
        }))
    }

    fn send(&self, cmd: Cmd) {
        self.0.commands.send(cmd).unwrap();
    }

    /// The answers so far: (serial, result).
    fn answers(&self) -> Vec<(u32, u32)> {
        self.0
            .status
            .lock()
            .unwrap()
            .events
            .iter()
            .filter_map(|e| match *e {
                DockEvent::Opened(serial, result) => Some((serial, result)),
                _ => None,
            })
            .collect()
    }

    fn error(&self) -> Option<(String, u32)> {
        self.0.status.lock().unwrap().error.clone()
    }
}

/// `app_activation` results.
const ACTIVATED: u32 = 0;
const RESIDENT: u32 = 1;
const NOT_RUNNING: u32 = 2;

const ICON_SIDE: i32 = 4;
/// One ARGB8888 pixel, little-endian: half-transparent premultiplied blue.
const ICON_PIXEL: [u8; 4] = [0x80, 0x00, 0x00, 0x80];

fn icon_buffer(shm: &wl_shm::WlShm, qh: &QueueHandle<ClientState>) -> wl_buffer::WlBuffer {
    let len = ICON_SIDE * ICON_SIDE * 4;
    let mut file = tempfile_like(len as usize);
    file.write_all(&ICON_PIXEL.repeat((ICON_SIDE * ICON_SIDE) as usize))
        .unwrap();
    let pool = shm.create_pool(file.as_fd(), len, qh, ());
    let buffer = pool.create_buffer(
        0,
        ICON_SIDE,
        ICON_SIDE,
        ICON_SIDE * 4,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    buffer
}

/// An application with one 300x200 toplevel.
fn spawn_app(socket: PathBuf, app_id: &'static str) -> TestClient {
    let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
    spawn_app_on(stream, app_id)
}

/// [`spawn_app`] on the connection `stream`.
fn spawn_app_on(stream: UnixStream, app_id: &'static str) -> TestClient {
    run_client(stream, move |globals, qh, status| {
        let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(qh, 1..=5, ()).unwrap();
        let surface = compositor.create_surface(qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, qh, ());
        let toplevel = xdg.get_toplevel(qh, ());
        toplevel.set_app_id(app_id.to_string());
        toplevel.set_title(format!("{app_id} window"));
        surface.commit();
        let activation: xdg_activation_v1::XdgActivationV1 = globals.bind(qh, 1..=1, ()).unwrap();
        let window = surface.clone();
        let gtk: gtk_shell::gtk_shell1::GtkShell1 = globals.bind(qh, 1..=7, ()).unwrap();
        let gtk_surface = gtk.get_gtk_surface(&surface, qh, ());
        let icons: XdgToplevelIconManagerV1 = globals.bind(qh, 1..=1, ()).unwrap();
        let icon_shm = shm.clone();
        let qh = qh.clone();
        let mut kept = Vec::new();
        let mut kept_dialogs = Vec::new();
        let state = ClientState {
            status: status.clone(),
            shm,
            surface,
            attached: false,
            toplevel_size: (300, 200),
        };
        let apply = Box::new(move |cmd: Cmd| match cmd {
            Cmd::Activate(token) => activation.activate(token, &window),
            Cmd::GtkPresent => gtk_surface.present(0),
            Cmd::GtkRequestFocus(startup_id) => gtk_surface.request_focus(startup_id),
            Cmd::SetIcon(name) => {
                let icon = icons.create_icon(&qh, ());
                let buffer = icon_buffer(&icon_shm, &qh);
                icon.set_name(name.into());
                icon.add_buffer(&buffer, 1);
                icons.set_icon(&toplevel, Some(&icon));
                window.commit();
                kept.push((icon, buffer));
            }
            Cmd::OpenDialog(dialog_id) => {
                let dialog = compositor.create_surface(&qh, ());
                let xdg = wm_base.get_xdg_surface(&dialog, &qh, dialog.clone());
                let child = xdg.get_toplevel(&qh, ());
                child.set_parent(Some(&toplevel));
                child.set_app_id(dialog_id.to_string());
                dialog.commit();
                kept_dialogs.push((dialog, xdg, child));
            }
            Cmd::CloseWindow => {
                toplevel.destroy();
                window.attach(None, 0, 0);
                window.commit();
            }
            _ => panic!("an application window takes no dock request"),
        });
        (state, apply)
    })
}

// ---- tests ---------------------------------------------------------------------------------

#[test]
fn a_surface_without_the_layer_role_is_a_role_error() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), false);
    let end = Instant::now() + common::HANG_GUARD;
    while dock.error().is_none() && Instant::now() < end {
        h.step();
    }
    // error 0 of cupertino_shell_v1.error is `role`
    assert_eq!(
        dock.error(),
        Some((
            cupertino_shell_v1::CupertinoShellV1::interface()
                .name
                .to_string(),
            0
        ))
    );
}

/// A shell process that holds other roles but not the dock role, asking for a dock, is refused, so it cannot send `force_quit_app` either.
#[test]
fn an_application_that_is_not_a_shell_process_cannot_be_a_dock_or_end_an_application() {
    let Some(mut h) = Harness::new() else { return };
    let _victim = spawn_app(h.socket_path(), "org.example.victim");
    h.mapped_window(1);
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorder = sent.clone();
    h.state.force_quits.own_pid = u32::MAX;
    h.state.send_signal =
        Box::new(move |process, signal| recorder.borrow_mut().push((process.pid, signal)));

    let intruder = DockClient::spawn(
        cupertino_server::shell_connection::shell_process_connection(
            &h.state.display_handle,
            &[ShellRole::Session, ShellRole::MenuBar],
        )
        .unwrap(),
        true,
    );
    let end = Instant::now() + common::HANG_GUARD;
    while intruder.error().is_none() && Instant::now() < end {
        h.step();
    }
    // error 2 of cupertino_shell_v1.error is `permission`
    assert_eq!(
        intruder.error(),
        Some((
            cupertino_shell_v1::CupertinoShellV1::interface()
                .name
                .to_string(),
            2
        ))
    );
    assert_eq!(intruder.groups(), Vec::<Vec<DockEvent>>::new());
    assert_eq!(*sent.borrow(), vec![], "nothing was signalled");
}

#[test]
fn the_first_group_lists_the_windows_and_the_bar_backdrop_is_the_docks_rectangle() {
    let Some(mut h) = Harness::new() else { return };
    let _app = spawn_app(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    assert_eq!(
        dock.groups(),
        vec![vec![
            DockEvent::Window {
                id: id.0 as u32,
                app_id: "org.example.editor".into(),
                title: "org.example.editor window".into(),
                minimized: false,
                active: true
            },
            DockEvent::Done
        ]]
    );
    let out = h.state.outputs[0].id;
    h.wait_until("the dock surface to be placed", |s| {
        s.dock_rect(out).is_some_and(|r| r.w > 0.0)
    });
    // the dock's rectangle is its backdrop (the surface sits at y 600); its exclusive zone is what windows lose
    assert_eq!(
        h.state.dock_rect(out),
        Some(Rect::new(300.0, 630.0, 400.0, 64.0))
    );
    assert_eq!(h.state.usable_area(out), Rect::new(0.0, 0.0, 1000.0, 600.0));
    assert_eq!(dock.error(), None);
}

/// The dock lists only the main windows of applications: a dialog (a window with a parent, here a portal's chooser
/// over the editor) and a window of a shell process are left out.
#[test]
fn the_dock_lists_only_the_main_windows_of_applications() {
    let Some(mut h) = Harness::new() else { return };
    let app = spawn_app(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    app.commands
        .send(Cmd::OpenDialog("org.example.portal"))
        .unwrap();
    h.mapped_window(2);
    let shell = cupertino_server::shell_connection::shell_process_connection(
        &h.state.display_handle,
        &[ShellRole::MenuBar],
    )
    .unwrap();
    let _part = spawn_app_on(shell, "org.example.shellpart");
    h.mapped_window(3);
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    let listed: Vec<(u32, String)> = dock.groups()[0]
        .iter()
        .filter_map(|e| match e {
            DockEvent::Window { id, app_id, .. } => Some((*id, app_id.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(listed, [(id.0 as u32, "org.example.editor".to_owned())]);
}

#[test]
fn a_window_icon_follows_its_window_event_from_shell_version_17_only() {
    let Some(mut h) = Harness::new() else { return };
    let app = spawn_app(h.socket_path(), "org.example.painter");
    let id = h.mapped_window(1).0 as u32;
    let out = h.state.outputs[0].id;
    let quit = |h: &mut Harness, dock: DockClient| {
        h.wait_until("the dock surface to be placed", |s| {
            s.dock_rect(out).is_some()
        });
        dock.send(Cmd::Quit);
        h.wait_until("the dock to go", |s| s.dock_rect(out).is_none());
    };

    let dock = DockClient::spawn_at(h.shell_stream(), true, 0, 17);
    h.wait_for_groups(&dock, 1);
    app.commands
        .send(Cmd::SetIcon("org.example.painter"))
        .unwrap();
    h.wait_for_groups(&dock, 2);
    let icon = DockEvent::WindowIcon {
        id,
        name: "org.example.painter".into(),
        size: ICON_SIDE as u32,
        pixels: ICON_PIXEL.repeat((ICON_SIDE * ICON_SIDE) as usize),
    };
    assert_eq!(dock.groups()[1], vec![icon.clone(), DockEvent::Done]);

    // a name that could be a path is not sent; the image still is
    app.commands.send(Cmd::SetIcon("../icons/x")).unwrap();
    h.wait_for_groups(&dock, 3);
    assert!(matches!(
        &dock.groups()[2][..],
        [DockEvent::WindowIcon { name, size: 4, .. }, DockEvent::Done] if name.is_empty()
    ));
    assert_eq!(dock.error(), None);
    quit(&mut h, dock);

    // a dock that binds later gets the icon in its first group, after the window
    let late = DockClient::spawn_at(h.shell_stream(), true, 0, 17);
    h.wait_for_groups(&late, 1);
    let first = &late.groups()[0];
    assert!(matches!(first[0], DockEvent::Window { id: w, .. } if w == id));
    assert!(
        matches!(&first[1..], [DockEvent::WindowIcon { id: w, .. }, DockEvent::Done] if *w == id)
    );
    quit(&mut h, late);

    let old = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&old, 1);
    assert!(
        matches!(
            &old.groups()[0][..],
            [DockEvent::Window { .. }, DockEvent::Done]
        ),
        "a version 9 dock is sent no icon"
    );
}

/// From shell version 19 an application is resident while the client that showed its window stays connected, with
/// or without windows; it stops being resident when that client disconnects. An older dock is told nothing of it.
#[test]
fn an_application_stays_resident_after_its_last_window_until_its_client_disconnects() {
    let Some(mut h) = Harness::new() else { return };
    let app = spawn_app(h.socket_path(), "org.example.notes");
    let id = h.mapped_window(1).0 as u32;
    let out = h.state.outputs[0].id;
    let quit = |h: &mut Harness, dock: DockClient| {
        h.wait_until("the dock surface to be placed", |s| {
            s.dock_rect(out).is_some()
        });
        dock.send(Cmd::Quit);
        h.wait_until("the dock to go", |s| s.dock_rect(out).is_none());
    };

    let old = DockClient::spawn_at(h.shell_stream(), true, 0, 18);
    h.wait_for_groups(&old, 1);
    assert!(
        matches!(
            &old.groups()[0][..],
            [DockEvent::Window { .. }, DockEvent::Done]
        ),
        "a version 18 dock is sent no residency"
    );
    quit(&mut h, old);

    let dock = DockClient::spawn_at(h.shell_stream(), true, 0, 19);
    h.wait_for_groups(&dock, 1);
    let first = &dock.groups()[0];
    assert!(matches!(first[0], DockEvent::Window { id: w, .. } if w == id));
    assert_eq!(
        first[1..],
        [
            DockEvent::Resident("org.example.notes".into(), true),
            DockEvent::Done
        ]
    );

    // the last window closes; the process still runs, so the application stays resident
    app.commands.send(Cmd::CloseWindow).unwrap();
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        vec![DockEvent::Closed(id), DockEvent::Done]
    );
    h.advance(0.5);
    assert_eq!(dock.groups().len(), 2, "no change while it runs");
    assert_eq!(
        h.state.resident_apps().into_iter().collect::<Vec<_>>(),
        ["org.example.notes"]
    );

    // the process exits: it is resident no longer
    app.commands.send(Cmd::Quit).unwrap();
    let end = Instant::now() + common::HANG_GUARD;
    while dock.groups().len() < 3 && Instant::now() < end {
        h.advance(0.05);
    }
    assert_eq!(
        dock.groups()[2],
        vec![
            DockEvent::Resident("org.example.notes".into(), false),
            DockEvent::Done
        ]
    );
    assert!(h.state.resident_apps().is_empty());

    // a dock that binds later is told only of the running applications
    quit(&mut h, dock);
    let late = DockClient::spawn_at(h.shell_stream(), true, 0, 19);
    h.wait_for_groups(&late, 1);
    assert_eq!(late.groups()[0], vec![DockEvent::Done]);
    assert_eq!(late.error(), None);
}

/// The X11 satellite shows every X11 application's windows on its one connection, which outlives each of them: its
/// windows make no application resident, so an X11 application leaves the dock with its last window.
#[test]
fn the_x11_satellite_makes_no_application_resident() {
    use std::os::unix::fs::PermissionsExt;
    let Some(mut h) = Harness::new() else { return };
    let root = tempfile::tempdir().unwrap();
    let (tmp, runtime) = (root.path().join("tmp"), root.path().join("run"));
    let sockets = tmp.join(".X11-unix");
    std::fs::create_dir_all(&sockets).unwrap();
    std::fs::create_dir(&runtime).unwrap();
    std::fs::set_permissions(&sockets, std::fs::Permissions::from_mode(0o1777)).unwrap();
    let program = runtime.join("satellite");
    std::fs::write(&program, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let display = cupertino_server::xwayland::XDisplay::open(&tmp, &runtime).unwrap();
    let x_socket = sockets.join(display.name().replace(':', "X"));
    let x = cupertino_server::xwayland::XWayland::start(
        display,
        program,
        "wayland-test".into(),
        &h.event_loop.handle(),
        |s: &mut Cupertino| s.xwayland.as_mut(),
        |_| {},
    )
    .unwrap();
    h.state.xwayland = Some(x);
    // an X11 program connects, and the satellite starts
    let _x_client = UnixStream::connect(&x_socket).unwrap();
    h.wait_until("the satellite to start", |s| {
        s.xwayland.as_ref().and_then(|x| x.pid()).is_some()
    });
    let satellite = h.state.xwayland.as_ref().and_then(|x| x.pid()).unwrap();
    // every connection of the test is the satellite's
    h.state.process_of = Box::new(move |_, client| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        client.id().hash(&mut hasher);
        Some((satellite, hasher.finish()))
    });

    let app = spawn_app(h.socket_path(), "org.example.xterm");
    let id = h.mapped_window(1).0 as u32;
    let dock = DockClient::spawn_at(h.shell_stream(), true, 0, 19);
    h.wait_for_groups(&dock, 1);
    assert!(
        matches!(&dock.groups()[0][..], [DockEvent::Window { id: w, .. }, DockEvent::Done] if *w == id),
        "{:?}",
        dock.groups()
    );
    app.commands.send(Cmd::CloseWindow).unwrap();
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        vec![DockEvent::Closed(id), DockEvent::Done]
    );
    assert!(h.state.resident_apps().is_empty());
}

#[test]
fn an_application_id_and_a_title_longer_than_the_limit_are_cut_at_a_character_boundary() {
    let Some(mut h) = Harness::new() else { return };
    // 4000 bytes, and a two-byte character spans byte 1024; the title appends " window"
    let long_id: &'static str =
        Box::leak(format!("{}é{}", "a".repeat(1023), "b".repeat(4000 - 1025)).into_boxed_str());
    let _app = spawn_app(h.socket_path(), long_id);
    let id = h.mapped_window(1);
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    assert_eq!(
        dock.groups(),
        vec![vec![
            DockEvent::Window {
                id: id.0 as u32,
                app_id: "a".repeat(1023),
                title: "a".repeat(1023),
                minimized: false,
                active: true
            },
            DockEvent::Done
        ]]
    );
    assert_eq!(dock.error(), None, "the dock stays connected");
}

#[test]
fn the_application_ids_of_launch_unresponsive_attention_and_switcher_events_are_cut() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    // 4080 bytes would push `unresponsive` past the wire's 4096; a two-byte character spans byte 1024
    let long = format!("{}é{}", "a".repeat(1023), "b".repeat(4080 - 1025));
    let cut = "a".repeat(1023);
    let named = XdgActivationTokenData {
        app_id: Some(long.clone()),
        ..XdgActivationTokenData::default()
    };
    assert!(h
        .state
        .token_created(XdgActivationToken::from("long".to_string()), named));
    h.state.docks.unresponsive(&long, true);
    h.state.docks.attention(&long);
    h.state.switcher_changed(vec![long.clone()], 0);
    h.wait_for_groups(&dock, 5);
    let groups = dock.groups();
    assert_eq!(
        groups[1],
        vec![DockEvent::Launching(cut.clone()), DockEvent::Done]
    );
    assert_eq!(
        groups[2],
        vec![
            DockEvent::Unresponsive {
                app_id: cut.clone(),
                unresponsive: true
            },
            DockEvent::Done
        ]
    );
    assert_eq!(
        groups[3],
        vec![DockEvent::Attention(cut.clone()), DockEvent::Done]
    );
    assert!(
        groups[4].contains(&DockEvent::SwitcherEntry(cut)),
        "{:?}",
        groups[4]
    );
    assert_eq!(dock.error(), None, "the dock stays connected");
}

#[test]
fn minimizing_sends_the_state_and_animates_toward_the_committed_target() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    let _app = spawn_app(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    assert_eq!(
        h.state.minimize_target(id),
        None,
        "no target before the dock sets one"
    );

    dock.send(Cmd::SetMinimizeTarget);
    h.wait_until("the target to be committed", |s| {
        s.minimize_target(id).is_some()
    });
    assert_eq!(h.state.minimize_target(id), Some(SLOT_GLOBAL));

    let target = h.state.minimize_target(id);
    h.state.command(Command::Minimize { window: id, target });
    h.advance(0.1);
    let (_, pres) = plan_for(
        &h.state.wm,
        h.state.entry(id).unwrap(),
        0.0,
        h.state.now(),
        &h.state.fx,
        h.state.display_options(),
    )
    .expect("drawn while minimizing");
    assert_eq!(pres.minimize_target, SLOT_GLOBAL);

    h.advance(5.0);
    assert_eq!(
        h.state.wm.window(id).unwrap().state(),
        LifecycleState::Minimized
    );
    h.wait_until("the dock to hear the window is minimized", |_| {
        dock.groups().last().is_some_and(|g| {
            g.iter().any(|e| {
                matches!(
                    e,
                    DockEvent::Window {
                        minimized: true,
                        ..
                    }
                )
            })
        })
    });
}

#[test]
fn minimizing_without_a_target_animates_toward_the_dock_slot() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    let _app = spawn_app(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    dock.send(Cmd::SetMinimizeTarget);
    h.wait_until("the target to be committed", |s| {
        s.minimize_target(id).is_some()
    });

    h.state.command(Command::Minimize {
        window: id,
        target: None,
    });
    h.advance(0.1);
    let (_, pres) = plan_for(
        &h.state.wm,
        h.state.entry(id).unwrap(),
        0.0,
        h.state.now(),
        &h.state.fx,
        h.state.display_options(),
    )
    .expect("drawn while minimizing");
    assert_eq!(pres.minimize_target, SLOT_GLOBAL);
}

#[test]
fn activate_app_raises_the_application() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    let _a = spawn_app(h.socket_path(), "org.example.a");
    let first = h.mapped_window(1);
    let _b = spawn_app(h.socket_path(), "org.example.b");
    let second = h.mapped_window(2);
    assert_eq!(h.state.wm.key_window(), Some(second));

    dock.send(Cmd::ActivateApp("org.example.a"));
    h.wait_until("the first application to take focus", |s| {
        s.wm.key_window() == Some(first)
    });
}

/// The launcher opening an application by its desktop file ID: one that shows a window comes forward as the dock's
/// `activate_app` brings it (its minimized window restored, nothing started); one matched through its desktop entry
/// too; a resident one without windows and one that does not run are only reported, for the launcher to start.
#[test]
fn the_launcher_brings_a_running_application_forward_or_learns_it_must_start_it() {
    let Some(mut h) = Harness::new() else { return };
    let data = tempfile::tempdir().unwrap();
    let apps = data.path().join("applications");
    std::fs::create_dir(&apps).unwrap();
    std::fs::write(
        apps.join("org.example.Maps.desktop"),
        "[Desktop Entry]\nType=Application\nName=Maps\nExec=maps\nStartupWMClass=maps-window\n",
    )
    .unwrap();
    h.state.desktop = DesktopIndex::load_from(&[data.path().to_path_buf()]);
    let launcher = LauncherClient::spawn(
        cupertino_server::shell_connection::shell_process_connection(
            &h.state.display_handle,
            &[ShellRole::Session],
        )
        .unwrap(),
    );
    let _a = spawn_app(h.socket_path(), "org.example.a");
    let first = h.mapped_window(1);
    let maps = spawn_app(h.socket_path(), "maps-window");
    let second = h.mapped_window(2);
    let _c = spawn_app(h.socket_path(), "org.example.c");
    let third = h.mapped_window(3);
    assert_eq!(h.state.wm.key_window(), Some(third));

    // by its ID with the suffix, in another case
    launcher.send(Cmd::OpenApp(1, "Org.Example.A.desktop"));
    h.wait_until("the first application to take focus", |s| {
        s.wm.key_window() == Some(first)
    });
    h.wait_until("the answer", |_| !launcher.answers().is_empty());
    assert_eq!(launcher.answers(), [(1, ACTIVATED)]);

    // through its entry's StartupWMClass, out of its minimized window
    h.state.command(Command::Minimize {
        window: second,
        target: None,
    });
    h.advance(5.0);
    assert!(h.state.wm.window(second).unwrap().state().is_minimized());
    launcher.send(Cmd::OpenApp(2, "org.example.Maps.desktop"));
    h.wait_until("the minimized window to take focus", |s| {
        s.wm.key_window() == Some(second)
    });
    h.advance(5.0);
    assert!(!h.state.wm.window(second).unwrap().state().is_minimized());
    assert_eq!(launcher.answers()[1], (2, ACTIVATED));

    // not running: reported, nothing started and nothing raised
    launcher.send(Cmd::OpenApp(3, "org.example.absent.desktop"));
    h.wait_until("the answer", |_| launcher.answers().len() == 3);
    assert_eq!(launcher.answers()[2], (3, NOT_RUNNING));
    assert_eq!(h.state.wm.key_window(), Some(second));

    // resident without windows: reported, for the launcher to ask it for a window
    maps.commands.send(Cmd::CloseWindow).unwrap();
    let end = Instant::now() + common::HANG_GUARD;
    while h.state.wm.windows().count() != 2 && Instant::now() < end {
        h.advance(0.1);
    }
    assert_eq!(h.state.wm.windows().count(), 2, "the window closed");
    launcher.send(Cmd::OpenApp(4, "org.example.Maps.desktop"));
    h.wait_until("the answer", |_| launcher.answers().len() == 4);
    assert_eq!(launcher.answers()[3], (4, RESIDENT));
    assert_eq!(h.state.wm.windows().count(), 2, "nothing was started");
    assert_eq!(launcher.error(), None);
}

/// `activate_app` checks the Session role on every request: once the launcher's process exits, its session object is
/// refused with the permission error, nothing comes forward and nothing is answered.
#[test]
fn a_launcher_whose_process_exited_is_refused_activate_app() {
    let Some(mut h) = Harness::new() else { return };
    let (stream, holder) = common::held_shell_connection(&h.state, &[ShellRole::Session]);
    let launcher = LauncherClient::spawn(stream);
    let _a = spawn_app(h.socket_path(), "org.example.a");
    let first = h.mapped_window(1);
    let _b = spawn_app(h.socket_path(), "org.example.b");
    let second = h.mapped_window(2);
    launcher.send(Cmd::OpenApp(1, "org.example.a"));
    h.wait_until("the first application to take focus", |s| {
        s.wm.key_window() == Some(first)
    });

    holder.end();
    launcher.send(Cmd::OpenApp(2, "org.example.b"));
    h.wait_until("the permission error", |_| launcher.error().is_some());
    // error 2 of cupertino_shell_v1.error is `permission`
    assert_eq!(
        launcher.error(),
        Some(("cupertino_session_v1".to_string(), 2))
    );
    assert_eq!(h.state.wm.key_window(), Some(first));
    assert_ne!(first, second);
    assert_eq!(launcher.answers(), [(1, ACTIVATED)]);
}

/// The role is checked on every request, not only at `get_dock`: once the process the connection was made for exits, a
/// request on the dock it already has is refused with the permission error and not carried out.
#[test]
fn a_dock_whose_process_exited_is_refused_its_next_request() {
    let Some(mut h) = Harness::new() else { return };
    let (stream, holder) =
        common::held_shell_connection(&h.state, &[ShellRole::Session, ShellRole::Dock]);
    let dock = DockClient::spawn(stream, true);
    let _a = spawn_app(h.socket_path(), "org.example.a");
    let first = h.mapped_window(1);
    let _b = spawn_app(h.socket_path(), "org.example.b");
    let second = h.mapped_window(2);
    dock.send(Cmd::ActivateApp("org.example.a"));
    h.wait_until("the first application to take focus", |s| {
        s.wm.key_window() == Some(first)
    });

    holder.end();
    dock.send(Cmd::ActivateApp("org.example.b"));
    h.wait_until("the permission error", |_| dock.error().is_some());
    // error 2 of cupertino_shell_v1.error is `permission`
    assert_eq!(dock.error(), Some(("cupertino_dock_v1".to_string(), 2)));
    assert_eq!(h.state.wm.key_window(), Some(first));
    assert_ne!(first, second);
}

#[test]
fn restore_window_unminimizes_out_of_the_given_tile() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    let _app = spawn_app(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    h.state.command(Command::Minimize {
        window: id,
        target: None,
    });
    h.advance(5.0);
    assert_eq!(
        h.state.wm.window(id).unwrap().state(),
        LifecycleState::Minimized
    );

    dock.send(Cmd::RestoreWindow(id.0 as u32));
    h.wait_until("the window to restore", |s| {
        !s.wm.window(id).unwrap().state().is_minimized()
    });
    h.advance(0.1);
    let (_, pres) = plan_for(
        &h.state.wm,
        h.state.entry(id).unwrap(),
        0.0,
        h.state.now(),
        &h.state.fx,
        h.state.display_options(),
    )
    .expect("drawn while restoring");
    assert_eq!(
        pres.minimize_target, SLOT_GLOBAL,
        "the window flies out of the tile the dock named"
    );
}

#[test]
fn a_minimized_window_s_picture_follows_from_shell_version_22_and_goes_when_it_restores() {
    let Some(mut h) = Harness::new() else { return };
    let out = h.state.outputs[0].id;
    let quit = |h: &mut Harness, dock: DockClient| {
        h.wait_until("the dock surface to be placed", |s| {
            s.dock_rect(out).is_some()
        });
        dock.send(Cmd::Quit);
        h.wait_until("the dock to go", |s| s.dock_rect(out).is_none());
    };
    let pictures = |dock: &DockClient| -> Vec<(u32, u32, u32, Vec<u8>)> {
        dock.groups()
            .into_iter()
            .flatten()
            .filter_map(|e| match e {
                DockEvent::WindowThumbnail {
                    id,
                    width,
                    height,
                    pixels,
                } => Some((id, width, height, pixels)),
                _ => None,
            })
            .collect()
    };
    let until = |h: &mut Harness, dock: &DockClient, count: usize| {
        let end = Instant::now() + common::HANG_GUARD;
        while pictures(dock).len() < count && Instant::now() < end {
            h.step();
        }
    };

    let dock = DockClient::spawn_at(h.shell_stream(), true, 0, 22);
    let _app = spawn_app(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    let key = id.0 as u32;
    assert!(pictures(&dock).is_empty(), "no picture before it minimizes");
    h.state.command(Command::Minimize {
        window: id,
        target: None,
    });
    h.advance(5.0);
    until(&mut h, &dock, 1);
    let got = pictures(&dock);
    let [(w, width, height, pixels)] = &got[..] else {
        panic!("one picture: {got:?}");
    };
    assert_eq!(*w, key);
    // the 300 × 200 window and its frame, brought within 256 pixels on its longer side, its shape kept
    assert!(
        *width == 256 && *height < 256 && *height > 150,
        "{width} × {height}"
    );
    assert_eq!(pixels.len(), (*width * *height * 4) as usize);
    // the middle shows the window's own pixels: its buffer is 128 in every byte, a half-transparent grey
    let mid = ((*height / 2 * *width + *width / 2) * 4) as usize;
    assert!(
        pixels[mid..mid + 4].iter().all(|&c| c.abs_diff(128) <= 2),
        "{:?}",
        &pixels[mid..mid + 4]
    );
    assert_eq!(dock.error(), None);
    quit(&mut h, dock);

    // a dock of version 21 is sent none
    let old = DockClient::spawn_at(h.shell_stream(), true, 0, 21);
    h.wait_for_groups(&old, 1);
    h.advance(0.5);
    assert!(pictures(&old).is_empty());
    quit(&mut h, old);

    // a dock that binds later gets it after the window in its first group
    let late = DockClient::spawn_at(h.shell_stream(), true, 0, 22);
    h.wait_for_groups(&late, 1);
    assert!(matches!(
        &late.groups()[0][..2],
        [DockEvent::Window { id: a, minimized: true, .. }, DockEvent::WindowThumbnail { id: b, .. }]
            if *a == key && *b == key
    ));

    // restored, its picture goes
    late.send(Cmd::RestoreWindow(key));
    h.wait_until("the window to restore", |s| {
        !s.wm.window(id).unwrap().state().is_minimized()
    });
    until(&mut h, &late, 2);
    assert!(matches!(&pictures(&late)[1..], [(w, 0, 0, p)] if *w == key && p.is_empty()));
}

#[test]
fn restoring_a_window_that_is_not_minimized_is_ignored() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    let _app = spawn_app(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    dock.send(Cmd::RestoreWindow(id.0 as u32));
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(
        h.state.wm.window(id).unwrap().state(),
        LifecycleState::Normal
    );
    assert_eq!(dock.error(), None);
}

#[test]
fn a_program_the_server_starts_is_announced_as_launching() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    // the announcement carries the desktop entry id of the program
    let data = std::env::temp_dir().join(format!(
        "cupertino-dock-v1-launching-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(data.join("applications")).unwrap();
    std::fs::write(
        data.join("applications/org.example.Truth.desktop"),
        "[Desktop Entry]\nType=Application\nName=Truth\nExec=true\n",
    )
    .unwrap();
    h.state.desktop = DesktopIndex::load_from(std::slice::from_ref(&data));
    std::fs::remove_dir_all(&data).unwrap();
    h.state.spawn("true", &[]);
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        vec![
            DockEvent::Launching("org.example.Truth".into()),
            DockEvent::Done
        ]
    );
}

#[test]
fn a_token_that_names_an_application_announces_it_as_launching() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    // a launcher or file manager asks for a token for the application it is about to start
    let named = XdgActivationTokenData {
        app_id: Some("org.example.Notes".into()),
        ..XdgActivationTokenData::default()
    };
    assert!(h
        .state
        .token_created(XdgActivationToken::from("named".to_string()), named));
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        vec![
            DockEvent::Launching("org.example.Notes".into()),
            DockEvent::Done
        ]
    );
    // a token for no particular application (a banner's click, an empty id) launches nothing
    let anonymous = XdgActivationTokenData::default();
    assert!(h
        .state
        .token_created(XdgActivationToken::from("anonymous".to_string()), anonymous));
    let empty = XdgActivationTokenData {
        app_id: Some(String::new()),
        ..XdgActivationTokenData::default()
    };
    assert!(h
        .state
        .token_created(XdgActivationToken::from("empty".to_string()), empty));
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(dock.groups().len(), 2);
}

#[test]
fn the_auto_hide_shortcut_reaches_the_dock() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    h.state.docks.toggle_auto_hide();
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        vec![DockEvent::ToggleAutoHide, DockEvent::Done]
    );
}

/// The app-grid action (shortcut, hot corner or gesture) opens the launcher's application browser: the server asks
/// the launcher on the session bus, and the dock hears nothing of it.
#[test]
fn the_app_grid_action_goes_to_the_launcher_not_the_dock() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    for _ in 0..2 {
        h.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![ShellOp::ToggleAppGrid],
        });
    }
    // a later event shows whether anything came before it
    h.state.docks.toggle_auto_hide();
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        vec![DockEvent::ToggleAutoHide, DockEvent::Done]
    );
    assert_eq!(dock.error(), None);
}

/// The Force Quit action is a `ShellOp` the server only forwards: the dock draws the dialog.
#[test]
fn the_force_quit_action_opens_the_dialog_in_the_dock() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    h.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::OpenForceQuit],
    });
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        vec![DockEvent::ForceQuitDialog, DockEvent::Done]
    );
    assert_eq!(dock.error(), None);
}

/// The Focus Dock action engages the dock of the output under the pointer, from shell version 18 only: an older dock
/// is told the next action and nothing before it.
#[test]
fn the_focus_dock_action_engages_the_dock_from_shell_version_18_only() {
    let Some(mut h) = Harness::new() else { return };
    let focus = |h: &mut Harness| {
        h.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![ShellOp::FocusDock],
        })
    };
    let dock = DockClient::spawn_at(h.shell_stream(), true, 0, 18);
    h.wait_for_groups(&dock, 1);
    focus(&mut h);
    h.wait_for_groups(&dock, 2);
    assert_eq!(dock.groups()[1], vec![DockEvent::Engage, DockEvent::Done]);
    assert_eq!(dock.error(), None);
    let out = h.state.outputs[0].id;
    h.wait_until("the dock surface to be placed", |s| {
        s.dock_rect(out).is_some()
    });
    dock.send(Cmd::Quit);
    h.wait_until("the dock to go", |s| s.dock_rect(out).is_none());

    let old = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&old, 1);
    focus(&mut h);
    h.state.execute_plan(ActionPlan {
        commands: Vec::new(),
        ops: vec![ShellOp::OpenForceQuit],
    });
    h.wait_for_groups(&old, 2);
    assert_eq!(
        old.groups()[1],
        vec![DockEvent::ForceQuitDialog, DockEvent::Done]
    );
    assert_eq!(old.error(), None);
}

/// An engaged dock holds the keyboard until a press lands outside it: the keyboard then goes to the window, the dock is
/// not given it back while it still asks, and it takes it again when it asks anew or is engaged again.
#[test]
fn a_press_outside_the_engaged_dock_gives_the_keyboard_back() {
    let Some(mut h) = Harness::new() else { return };
    let _app = spawn_app(h.socket_path(), "org.example.a");
    let window = h.mapped_window(1);
    let dock = DockClient::spawn_at(h.shell_stream(), true, 0, 18);
    h.wait_for_groups(&dock, 1);
    let out = h.state.outputs[0].id;
    h.wait_until("the dock surface to be placed", |s| {
        s.dock_rect(out).is_some()
    });
    let window_surface = h.state.entry(window).unwrap().surface().clone();
    let focus = |h: &Harness| h.state.seat.get_keyboard().unwrap().current_focus();
    let press_at = |h: &mut Harness, x: f64, y: f64| {
        h.state.pointer_moved_to((x, y).into(), 0, None);
        h.state
            .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 0);
        h.state
            .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Released, 0);
        h.step();
    };
    let engage = |h: &mut Harness| {
        h.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![ShellOp::FocusDock],
        })
    };

    engage(&mut h);
    dock.send(Cmd::SetKeyboard(true));
    h.wait_until("the dock to take the keyboard", |s| {
        s.exclusive_layer.is_some()
    });
    let held = h.state.exclusive_layer.clone();
    assert_eq!(focus(&h), held);

    let bar = h.state.dock_rect(out).unwrap();
    press_at(&mut h, bar.x + bar.w / 2.0, bar.y + bar.h / 2.0);
    assert_eq!(
        h.state.exclusive_layer, held,
        "a press on the dock keeps it"
    );
    assert_eq!(focus(&h), held);

    let f = h.state.wm.window(window).unwrap().geometry();
    press_at(&mut h, f.x + 40.0, f.y + 80.0);
    assert_eq!(h.state.exclusive_layer, None, "a press outside releases it");
    assert_eq!(focus(&h), Some(window_surface.clone()));
    // the dock commits again while it still asks for the keyboard: it is not given it back
    dock.send(Cmd::SetMinimizeTarget);
    h.advance(0.1);
    assert_eq!(h.state.exclusive_layer, None);
    assert_eq!(focus(&h), Some(window_surface.clone()));

    // a new claim after letting go takes the keyboard again
    dock.send(Cmd::SetKeyboard(false));
    h.advance(0.1);
    dock.send(Cmd::SetKeyboard(true));
    h.wait_until("the dock to take the keyboard again", |s| {
        s.exclusive_layer.is_some()
    });
    assert_eq!(focus(&h), held);

    // released again, then engaged again before it let go: the next commit takes the keyboard
    press_at(&mut h, f.x + 40.0, f.y + 80.0);
    assert_eq!(focus(&h), Some(window_surface));
    engage(&mut h);
    dock.send(Cmd::SetMinimizeTarget);
    h.wait_until("the engaged dock to take the keyboard", |s| {
        s.exclusive_layer.is_some()
    });
    assert_eq!(focus(&h), held);
    assert_eq!(dock.error(), None);
}

#[test]
fn an_application_going_unresponsive_is_announced_to_the_dock() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    h.state.docks.unresponsive("org.example.a", true);
    h.state.docks.unresponsive("org.example.a", false);
    h.wait_for_groups(&dock, 3);
    assert_eq!(
        dock.groups()[1],
        vec![
            DockEvent::Unresponsive {
                app_id: "org.example.a".into(),
                unresponsive: true
            },
            DockEvent::Done
        ]
    );
    assert_eq!(
        dock.groups()[2],
        vec![
            DockEvent::Unresponsive {
                app_id: "org.example.a".into(),
                unresponsive: false
            },
            DockEvent::Done
        ]
    );
}

#[test]
fn force_quit_app_from_the_dock_asks_the_application_to_quit() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    let _a = spawn_app(h.socket_path(), "org.example.a");
    h.mapped_window(1);
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorder = sent.clone();
    h.state.force_quits.own_pid = u32::MAX;
    h.state.send_signal =
        Box::new(move |process, signal| recorder.borrow_mut().push((process.pid, signal)));

    dock.send(Cmd::ForceQuitApp("org.example.a"));
    h.wait_until("SIGTERM", |_| !sent.borrow().is_empty());
    assert_eq!(*sent.borrow(), vec![(std::process::id(), 15)]);
}

// ---- application switcher ------------------------------------------------------------------

/// xkb keycodes (evdev code + 8) of the keys the switcher is driven with.
const KEY_SUPER: u32 = 125 + 8;
const KEY_ALT: u32 = 56 + 8;
const KEY_TAB: u32 = 15 + 8;
const KEY_ESCAPE: u32 = 1 + 8;
const KEY_H: u32 = 35 + 8;
const KEY_HOME: u32 = 102 + 8;
const KEY_LEFT: u32 = 105 + 8;
const KEY_UP: u32 = 103 + 8;
const KEY_DOWN: u32 = 108 + 8;
const KEY_RIGHT: u32 = 106 + 8;
const KEY_END: u32 = 107 + 8;

impl Harness {
    /// One key through the compositor's keyboard path, as a device would deliver it.
    fn key(&mut self, code: u32, pressed: bool) {
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.state.keyboard_event_now(Keycode::new(code), state, 0);
        self.step();
    }

    /// One key through the filters of the keyboard path (sticky keys), as a device would deliver it.
    fn device_key(&mut self, code: u32, pressed: bool) {
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.state.key_after_delays(Keycode::new(code), state, 0);
        self.step();
    }

    fn device_tap(&mut self, code: u32) {
        self.device_key(code, true);
        self.device_key(code, false);
    }

    /// Tap Tab: opens the switcher while Super is held, or steps it when it is open.
    fn tap(&mut self, code: u32) {
        self.key(code, true);
        self.key(code, false);
    }

    /// Three applications mapped in turn, so the most recently used order is `three`, `two`, `one`;
    /// returns the clients (keep them alive) and their windows in that order.
    fn three_apps(&mut self) -> ([TestClient; 3], [WindowId; 3]) {
        let mut clients = Vec::new();
        let mut ids = Vec::new();
        for (i, app) in ["org.example.one", "org.example.two", "org.example.three"]
            .into_iter()
            .enumerate()
        {
            clients.push(spawn_app(self.socket_path(), app));
            ids.push(self.mapped_window(i + 1));
        }
        let [one, two, three]: [WindowId; 3] = ids.try_into().expect("three windows");
        let clients: [TestClient; 3] = clients.try_into().ok().expect("three clients");
        (clients, [three, two, one])
    }
}

/// The group the server sends for the open switcher: the panel's output (the dock's own), the
/// whole list, then the selection.
fn switcher_group(selected: u32) -> Vec<DockEvent> {
    let mut group = vec![DockEvent::SwitcherOutput { own: true }];
    group.extend(
        ["org.example.three", "org.example.two", "org.example.one"]
            .iter()
            .map(|a| DockEvent::SwitcherEntry(a.to_string())),
    );
    group.extend([DockEvent::SwitcherChanged(selected), DockEvent::Done]);
    group
}

#[test]
fn the_switcher_opens_steps_and_commits_without_a_dock() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, _two, one]) = h.three_apps();
    assert_eq!(h.state.wm.key_window(), Some(three));
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    assert_eq!(
        h.state.wm.switcher().map(|s| s.selected),
        Some(1),
        "the first Tab selects the previous application"
    );
    h.tap(KEY_TAB);
    assert_eq!(h.state.wm.switcher().map(|s| s.selected), Some(2));
    assert_eq!(
        h.state.wm.key_window(),
        Some(three),
        "nothing changes until the release"
    );
    h.key(KEY_SUPER, false);
    assert!(h.state.wm.switcher().is_none());
    assert_eq!(h.state.wm.key_window(), Some(one));
}

#[test]
fn under_classic_pc_alt_tab_switches_and_releasing_alt_commits() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, _two, one]) = h.three_apps();
    let tiled = |h: &Harness| {
        matches!(
            h.state.wm.window(three).map(|w| w.state()),
            Some(LifecycleState::Tiling(_) | LifecycleState::Tiled(_))
        )
    };
    // under cupertino Super+Left is not a tile
    h.key(KEY_SUPER, true);
    h.tap(KEY_LEFT);
    h.key(KEY_SUPER, false);
    assert!(!tiled(&h));
    h.state.apply_keyboard_change(
        &cupertino_defaults::keyboard::KeyboardSettings::default(),
        &cupertino_defaults::keyboard::KeyboardSettings {
            modifier_profile: Some(cupertino_defaults::keyboard::ModifierProfile::ClassicPc),
            ..Default::default()
        },
    );
    // Super+Left tiles the window
    h.key(KEY_SUPER, true);
    h.tap(KEY_LEFT);
    h.key(KEY_SUPER, false);
    assert!(tiled(&h));
    // Super+Tab is the overview here, not the switcher
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    assert!(h.state.wm.switcher().is_none());
    h.key(KEY_SUPER, false);
    h.key(KEY_ESCAPE, true);
    h.key(KEY_ESCAPE, false);

    h.key(KEY_ALT, true);
    h.tap(KEY_TAB);
    assert_eq!(h.state.wm.switcher().map(|s| s.selected), Some(1));
    h.tap(KEY_TAB);
    assert_eq!(h.state.wm.switcher().map(|s| s.selected), Some(2));
    assert_eq!(h.state.wm.key_window(), Some(three));
    h.key(KEY_ALT, false);
    assert!(h.state.wm.switcher().is_none());
    assert_eq!(h.state.wm.key_window(), Some(one));
}

#[test]
fn under_classic_pc_super_down_restores_a_filled_window_and_then_minimizes_it() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, _two, _one]) = h.three_apps();
    h.state.apply_keyboard_change(
        &cupertino_defaults::keyboard::KeyboardSettings::default(),
        &cupertino_defaults::keyboard::KeyboardSettings {
            modifier_profile: Some(cupertino_defaults::keyboard::ModifierProfile::ClassicPc),
            ..Default::default()
        },
    );
    let state = |h: &Harness| h.state.wm.window(three).map(|w| w.state());
    h.key(KEY_SUPER, true);
    h.tap(KEY_UP);
    assert!(matches!(
        state(&h),
        Some(LifecycleState::Tiling(_) | LifecycleState::Tiled(_))
    ));
    h.advance(1.0);
    h.tap(KEY_DOWN);
    h.advance(1.0);
    assert_eq!(state(&h), Some(LifecycleState::Normal));
    h.tap(KEY_DOWN);
    h.advance(1.0);
    assert_eq!(state(&h), Some(LifecycleState::Minimized));
    h.key(KEY_SUPER, false);
}

#[test]
fn under_classic_pc_the_super_key_alone_runs_its_tap_and_a_chord_or_a_click_does_not() {
    use cupertino_input::{Action, ShortcutMap};
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, _two, _one]) = h.three_apps();
    h.state.apply_keyboard_change(
        &cupertino_defaults::keyboard::KeyboardSettings::default(),
        &cupertino_defaults::keyboard::KeyboardSettings {
            modifier_profile: Some(cupertino_defaults::keyboard::ModifierProfile::ClassicPc),
            ..Default::default()
        },
    );
    // a tap that is visible: the key window minimizes
    let mut map = ShortcutMap::classic_pc_defaults();
    map.bind_tap(Action::MinimizeWindow);
    h.state.key_router.bindings =
        cupertino_server::input::KeyBindings::from_shortcuts(&map, "classic-pc");
    let state = |h: &Harness| h.state.wm.window(three).map(|w| w.state());
    let minimized = |h: &Harness| {
        matches!(
            state(h),
            Some(LifecycleState::Minimized | LifecycleState::Minimizing)
        )
    };

    // Super+Left is a chord
    h.device_key(KEY_SUPER, true);
    h.device_tap(KEY_LEFT);
    h.device_key(KEY_SUPER, false);
    h.advance(1.0);
    assert!(!minimized(&h));
    // Super and a click
    h.device_key(KEY_SUPER, true);
    h.state.pointer_button(0x110, ButtonState::Pressed, 0);
    h.state.pointer_button(0x110, ButtonState::Released, 0);
    h.device_key(KEY_SUPER, false);
    h.advance(1.0);
    assert!(!minimized(&h));
    // Super alone
    h.device_tap(KEY_SUPER);
    h.advance(1.0);
    assert!(minimized(&h));
}

/// A key of the pointer keypad, a gesture, and a click are all things the Super key held at the time is a modifier of;
/// the capture command's scripted modifiers go the way of a device's keys and make a tap.
#[test]
fn a_keypad_key_a_gesture_or_a_click_cancels_the_super_tap_and_scripted_modifiers_make_one() {
    use cupertino_input::{Action, ShortcutMap};
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, _two, _one]) = h.three_apps();
    h.state.apply_keyboard_change(
        &cupertino_defaults::keyboard::KeyboardSettings::default(),
        &cupertino_defaults::keyboard::KeyboardSettings {
            modifier_profile: Some(cupertino_defaults::keyboard::ModifierProfile::ClassicPc),
            mouse_keys: Some(true),
            ..Default::default()
        },
    );
    let mut map = ShortcutMap::classic_pc_defaults();
    map.bind_tap(Action::MinimizeWindow);
    h.state.key_router.bindings =
        cupertino_server::input::KeyBindings::from_shortcuts(&map, "classic-pc");
    let minimized = |h: &Harness| {
        matches!(
            h.state.wm.window(three).map(|w| w.state()),
            Some(LifecycleState::Minimized | LifecycleState::Minimizing)
        )
    };
    // Super and a key of the pointer keypad (the 8 moves the pointer, which no button or scroll shows)
    h.device_key(KEY_SUPER, true);
    h.device_tap(72 + 8);
    h.device_key(KEY_SUPER, false);
    h.advance(1.0);
    assert!(!minimized(&h), "a keypad key");
    // Super and a touchpad gesture
    h.device_key(KEY_SUPER, true);
    h.state.gesture_begun();
    h.device_key(KEY_SUPER, false);
    h.advance(1.0);
    assert!(!minimized(&h), "a gesture");
    // a scripted Super press and release, as the capture command sends them, is a tap
    h.state.scripted_modifier("super", true).unwrap();
    h.state.scripted_modifier("super", false).unwrap();
    h.advance(1.0);
    assert!(minimized(&h), "scripted modifiers");
}

/// The shortcut match comes after sticky keys: a latched modifier counts as held for the system chords, under either
/// profile. The latch is freed with the release of the key that used it, so the switcher the chord opened commits at
/// once: a latched Alt and Tab switch to the previous application.
#[test]
fn a_latched_modifier_takes_part_in_the_system_chords() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, two, _one]) = h.three_apps();
    let settings = |profile| cupertino_defaults::keyboard::KeyboardSettings {
        modifier_profile: Some(profile),
        sticky_keys: Some(true),
        ..Default::default()
    };
    h.state.apply_keyboard_change(
        &cupertino_defaults::keyboard::KeyboardSettings::default(),
        &settings(cupertino_defaults::keyboard::ModifierProfile::ClassicPc),
    );
    // Tab alone does nothing
    h.device_tap(KEY_TAB);
    assert!(h.state.wm.switcher().is_none());
    assert_eq!(h.state.wm.key_window(), Some(three));
    // Alt tapped and released, then Tab: the Alt is latched, so this is Alt+Tab
    h.device_tap(KEY_ALT);
    h.device_tap(KEY_TAB);
    assert!(h.state.wm.switcher().is_none());
    assert_eq!(h.state.wm.key_window(), Some(two));
}

#[test]
fn a_dock_is_told_the_switcher_in_order_and_picks_an_entry() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [_three, _two, one]) = h.three_apps();
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    h.wait_for_groups(&dock, 2);
    assert_eq!(dock.groups()[1], switcher_group(1));
    dock.send(Cmd::SwitcherSelect(2));
    h.wait_for_groups(&dock, 3);
    assert_eq!(dock.groups()[2], switcher_group(2));
    dock.send(Cmd::SwitcherSelect(7));
    dock.send(Cmd::SwitcherSelect(2));
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(
        dock.groups().len(),
        3,
        "an index outside the list and the selected entry change nothing"
    );
    h.key(KEY_SUPER, false);
    h.wait_for_groups(&dock, 4);
    assert_eq!(
        dock.groups()[3],
        [DockEvent::SwitcherClosed(1), DockEvent::Done]
    );
    assert_eq!(
        h.state.wm.key_window(),
        Some(one),
        "the pointer's pick is what the release commits"
    );
    assert_eq!(dock.error(), None);
}

#[test]
fn a_dock_that_binds_mid_switch_gets_the_switcher_and_a_cancel_reaches_it() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, ..]) = h.three_apps();
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 2);
    assert_eq!(dock.groups()[1], switcher_group(1));
    h.tap(KEY_ESCAPE);
    h.wait_for_groups(&dock, 3);
    assert_eq!(
        dock.groups()[2],
        [DockEvent::SwitcherClosed(0), DockEvent::Done]
    );
    h.key(KEY_SUPER, false);
    assert_eq!(
        h.state.wm.key_window(),
        Some(three),
        "a cancel keeps the focus"
    );
}

/// The QEMU switcher check failed now and then with "the commit did not move the focus": a Tab
/// released late (a loaded guest) let the compositor's repeat run on, wrap round to the current
/// application and commit that. A repeat now stops at the last entry.
#[test]
fn holding_tab_stops_at_the_last_entry_instead_of_wrapping_to_the_current_application() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [_three, _two, one]) = h.three_apps();
    h.key(KEY_SUPER, true);
    h.key(KEY_TAB, true);
    assert_eq!(h.state.wm.switcher().map(|s| s.selected), Some(1));
    for _ in 0..30 {
        h.advance(0.1);
        assert_ne!(
            h.state.wm.switcher().map(|s| s.selected),
            Some(0),
            "the repeat wrapped"
        );
    }
    assert_eq!(
        h.state.wm.switcher().map(|s| s.selected),
        Some(2),
        "the repeat ran to the last entry"
    );
    h.key(KEY_TAB, false);
    h.key(KEY_SUPER, false);
    assert_eq!(h.state.wm.key_window(), Some(one));
}

#[test]
fn the_open_switcher_takes_its_key_table_and_h_hides_the_selected_application() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, two, _one]) = h.three_apps();
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    let selected = |h: &Harness| h.state.wm.switcher().map(|s| s.selected);
    h.tap(KEY_END);
    assert_eq!(selected(&h), Some(2), "End jumps to the last entry");
    h.tap(KEY_HOME);
    assert_eq!(selected(&h), Some(0), "Home jumps to the first");
    h.tap(KEY_RIGHT);
    assert_eq!(selected(&h), Some(1), "Right steps forward");
    h.tap(KEY_H);
    let app = h.state.wm.window(two).unwrap().app;
    assert!(
        h.state.wm.app(app).unwrap().hidden,
        "H hides the selected application"
    );
    assert!(h.state.wm.switcher().is_some(), "and the switch stays open");
    h.tap(KEY_ESCAPE);
    assert!(h.state.wm.switcher().is_none());
    h.key(KEY_SUPER, false);
    assert_eq!(h.state.wm.key_window(), Some(three));
}

#[test]
fn a_click_off_the_panel_cancels_the_switch_and_reaches_no_window() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, two, _one]) = h.three_apps();
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    let f = h.state.wm.window(two).unwrap().geometry();
    h.state
        .pointer_moved_to((f.x + 20.0, f.y + 100.0).into(), 0, None);
    h.state
        .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 0);
    assert!(
        h.state.wm.switcher().is_some(),
        "the press alone keeps the switch"
    );
    h.state
        .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Released, 0);
    assert!(h.state.wm.switcher().is_none(), "the release cancels it");
    h.key(KEY_SUPER, false);
    assert_eq!(
        h.state.wm.key_window(),
        Some(three),
        "the window under the click is not focused"
    );
}

#[test]
fn a_release_on_a_panel_entry_switches_at_once_and_one_off_the_entries_cancels() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [_three, _two, one]) = h.three_apps();
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    h.wait_for_groups(&dock, 2);
    dock.send(Cmd::SwitcherActivate(2));
    h.wait_until("the switch to be committed", |_| {
        dock.groups()
            .iter()
            .any(|g| g.contains(&DockEvent::SwitcherClosed(1)))
    });
    assert_eq!(
        dock.groups()[2],
        switcher_group(2),
        "the entry is selected, then committed"
    );
    assert_eq!(
        h.state.wm.key_window(),
        Some(one),
        "the entry is committed while Super is still held"
    );
    h.key(KEY_SUPER, false);

    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    assert!(h.state.wm.switcher().is_some());
    dock.send(Cmd::SwitcherCancel);
    h.wait_until("the switch to be cancelled", |_| {
        dock.groups()
            .iter()
            .any(|g| g.contains(&DockEvent::SwitcherClosed(0)))
    });
    h.key(KEY_SUPER, false);
    assert_eq!(
        h.state.wm.key_window(),
        Some(one),
        "a cancel keeps the focus"
    );
    assert_eq!(dock.error(), None);
}

#[test]
fn the_panel_goes_to_the_dock_of_the_output_under_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    headless::add_display(
        &mut h.state,
        &HeadlessDisplay {
            name: "headless-2".into(),
            size: OUTPUT,
            physical_mm: (0, 0),
        },
        Some(1.0),
    )
    .unwrap();
    h.step();
    let (_apps, _) = h.three_apps();
    let first = DockClient::spawn_on(h.shell_stream(), true, 0);
    let second = DockClient::spawn_on(h.shell_stream(), true, 1);
    h.wait_for_groups(&first, 1);
    h.wait_for_groups(&second, 1);
    let other = h
        .state
        .outputs
        .iter()
        .find(|o| o.rect.x > 0.0 || o.rect.y > 0.0)
        .expect("the second output")
        .rect;
    h.state
        .pointer_moved_to((other.x + 50.0, other.y + 50.0).into(), 0, None);
    h.key(KEY_SUPER, true);
    h.tap(KEY_TAB);
    h.wait_for_groups(&second, 2);
    assert_eq!(
        second.groups()[1],
        switcher_group(1),
        "the dock of the pointer's output draws the panel"
    );
    h.key(KEY_SUPER, false);
    h.wait_until("the end of the switch", |_| {
        first
            .groups()
            .iter()
            .any(|g| g.contains(&DockEvent::SwitcherClosed(1)))
    });
    assert!(
        !first.groups().iter().flatten().any(|e| matches!(
            e,
            DockEvent::SwitcherOutput { .. } | DockEvent::SwitcherEntry(_)
        )),
        "the other dock only hears the end"
    );
}

/// `gtk_surface1.present` and `request_focus` are activation requests: a client with no input behind it cannot take the
/// keyboard from another, and its tile bounces instead.
#[test]
fn a_gtk_present_or_focus_request_from_a_background_client_only_bounces_its_tile() {
    let Some(mut h) = Harness::new() else { return };
    let ([_one_client, two_client, _three_client], [_three, _two, one]) = h.three_apps();
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    h.state.command(Command::FocusWindow { window: one });
    h.advance(1.0);
    assert_eq!(h.state.wm.key_window(), Some(one));

    two_client.commands.send(Cmd::GtkPresent).unwrap();
    h.wait_until("the attention request", |_| {
        dock.groups()
            .iter()
            .flatten()
            .any(|e| *e == DockEvent::Attention("org.example.two".into()))
    });
    assert_eq!(h.state.wm.key_window(), Some(one), "present");

    let attention = |dock: &DockClient| {
        dock.groups()
            .iter()
            .flatten()
            .filter(|e| **e == DockEvent::Attention("org.example.two".into()))
            .count()
    };
    let before = attention(&dock);
    two_client
        .commands
        .send(Cmd::GtkRequestFocus(None))
        .unwrap();
    h.wait_until("the second attention request", |_| {
        attention(&dock) > before
    });
    assert_eq!(h.state.wm.key_window(), Some(one), "request_focus");

    // a token the server made for a program it started is a grant, as for xdg_activation
    let fresh = h.state.server_activation_token();
    h.advance(1.5);
    two_client
        .commands
        .send(Cmd::GtkRequestFocus(Some(fresh)))
        .unwrap();
    h.wait_until("the focus request with a token", |s| {
        s.wm.key_window() != Some(one)
    });
}

#[test]
fn an_activation_token_older_than_the_applications_last_activation_only_bounces_its_tile() {
    let Some(mut h) = Harness::new() else { return };
    let ([_one_client, two_client, three_client], [three, two, one]) = h.three_apps();
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    let old = h.state.server_activation_token();
    h.advance(1.0);
    h.state.command(Command::FocusWindow { window: two });
    h.advance(1.0);
    h.state.command(Command::FocusWindow { window: one });
    h.advance(1.0);

    // `two` was activated after the token was made: refused, its tile bounces
    two_client.commands.send(Cmd::Activate(old)).unwrap();
    h.wait_until("the attention request", |_| {
        dock.groups()
            .iter()
            .flatten()
            .any(|e| *e == DockEvent::Attention("org.example.two".into()))
    });
    assert_eq!(h.state.wm.key_window(), Some(one));

    // a token newer than the application's last activation, no user event since: accepted
    let fresh = h.state.server_activation_token();
    h.advance(1.5);
    three_client
        .commands
        .send(Cmd::Activate(fresh.clone()))
        .unwrap();
    h.wait_until("the activation", |s| s.wm.key_window() == Some(three));

    // a token is used once
    h.state.command(Command::FocusWindow { window: one });
    three_client.commands.send(Cmd::Activate(fresh)).unwrap();
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(
        h.state.wm.key_window(),
        Some(one),
        "a used token does nothing"
    );

    // a mapped window is not brought forward by a token more than 2 s old
    let stale = h.state.server_activation_token();
    h.advance(2.5);
    two_client.commands.send(Cmd::Activate(stale)).unwrap();
    h.wait_until("the attention request", |_| {
        dock.groups()
            .iter()
            .flatten()
            .filter(|e| **e == DockEvent::Attention("org.example.two".into()))
            .count()
            == 2
    });
    assert_eq!(h.state.wm.key_window(), Some(one));
}

#[test]
fn a_refused_activation_bounces_the_tile_of_an_inactive_application_only() {
    let Some(mut h) = Harness::new() else { return };
    let (_apps, [three, two, _one]) = h.three_apps();
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    h.state.command(Command::DemandAttention { window: two });
    h.wait_for_groups(&dock, 2);
    assert_eq!(
        dock.groups()[1],
        [
            DockEvent::Attention("org.example.two".into()),
            DockEvent::Done
        ]
    );
    h.state.command(Command::DemandAttention { window: three });
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(
        dock.groups().len(),
        2,
        "the active application is not bounced"
    );
    assert_eq!(
        h.state.wm.key_window(),
        Some(three),
        "attention takes no focus"
    );
}

// ---- overview, workspaces, hot corners and gestures ------------------------------------------------

/// A 300×200 window maps at the model's default place: its frame as the dock is told it.
fn frame_of(h: &Harness, id: WindowId) -> (i32, i32, i32, i32) {
    let r = h.state.wm.window(id).unwrap().geometry();
    (
        r.x.round() as i32,
        r.y.round() as i32,
        r.w.round() as i32,
        r.h.round() as i32,
    )
}

/// The group that opened the overview, if the dock has one.
fn overview_group(dock: &DockClient) -> Option<Vec<DockEvent>> {
    dock.groups().into_iter().find(|g| {
        g.iter()
            .any(|e| matches!(e, DockEvent::OverviewChanged { .. }))
    })
}

impl Harness {
    fn toggle_overview(&mut self, app: bool) {
        self.state.execute_plan(ActionPlan {
            commands: Vec::new(),
            ops: vec![if app {
                ShellOp::ToggleAppOverview
            } else {
                ShellOp::ToggleOverview
            }],
        });
        self.step();
    }

    fn overview_target(&self, id: WindowId) -> Option<Rect> {
        self.state
            .shell
            .overview
            .as_ref()?
            .targets()
            .into_iter()
            .find(|f| f.id == id)
            .map(|f| f.frame)
    }
}

#[test]
fn opening_the_overview_sends_the_windows_and_workspaces_and_the_docks_frames_place_them() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    let _a = spawn_app(h.socket_path(), "org.example.a");
    let a = h.mapped_window(1);
    let _b = spawn_app(h.socket_path(), "org.example.b");
    let b = h.mapped_window(2);
    let out = h.state.outputs[0].id;
    h.wait_until("the dock to reserve its zone", |s| {
        s.usable_area(out).h < f64::from(OUTPUT.1)
    });
    let ws = h.state.wm.current_workspace(out).unwrap().0 as u32;

    h.toggle_overview(false);
    assert!(h.state.shell.overview_open());
    h.wait_until("the overview group", |_| overview_group(&dock).is_some());
    assert_eq!(
        overview_group(&dock).unwrap(),
        vec![
            DockEvent::OverviewWindow {
                id: a.0 as u32,
                frame: frame_of(&h, a)
            },
            DockEvent::OverviewWindow {
                id: b.0 as u32,
                frame: frame_of(&h, b)
            },
            DockEvent::OverviewWorkspace {
                id: ws,
                current: true
            },
            DockEvent::OverviewChanged {
                app: false,
                app_id: String::new(),
                area: (0, 0, OUTPUT.0, OUTPUT.1 - DOCK_HEIGHT)
            },
            DockEvent::Done,
        ],
        "stacking order, then the workspaces, then the usable area"
    );
    assert_eq!(
        h.overview_target(a),
        Some(h.state.wm.window(a).unwrap().geometry()),
        "a window stays at its desktop frame until the dock lays it out"
    );

    let target = Rect::new(20.0, 20.0, 200.0, 150.0);
    dock.send(Cmd::OverviewFrame(a.0 as u32, (20, 20, 200, 150)));
    dock.send(Cmd::OverviewFrame(9999, (0, 0, 10, 10)));
    h.wait_until("the frame to arrive", |s| {
        s.shell
            .overview
            .as_ref()
            .is_some_and(|o| o.targets().iter().any(|f| f.id == a && f.frame == target))
    });
    h.advance(5.0);
    let frames = h
        .state
        .shell
        .overview
        .as_ref()
        .unwrap()
        .frames(h.state.now());
    assert_eq!(
        frames.iter().find(|f| f.id == a).map(|f| f.frame),
        Some(target),
        "the window glided to the dock's frame"
    );
    assert!(
        h.state.shell.overview.as_ref().unwrap().windows.len() == 2,
        "an unknown id changed nothing"
    );

    dock.send(Cmd::OverviewSelect(a.0 as u32));
    h.wait_until("the overview to close", |s| !s.shell.overview_open());
    // the focus change that follows sends a window group too, so the closed group need not be the last
    h.wait_until("the dock to hear it closed", |_| {
        dock.groups()
            .iter()
            .any(|g| g == &[DockEvent::OverviewClosed, DockEvent::Done])
    });
    h.advance(5.0);
    assert_eq!(h.state.wm.key_window(), Some(a), "the pick is focused");
    assert!(
        h.state.shell.overview.is_none(),
        "the closing session is dropped once at rest"
    );
    assert_eq!(dock.error(), None);
}

#[test]
fn a_dock_that_dies_mid_overview_closes_it_and_the_windows_come_back() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    let _a = spawn_app(h.socket_path(), "org.example.a");
    let a = h.mapped_window(1);
    let desktop = h.state.wm.window(a).unwrap().geometry();
    h.wait_for_groups(&dock, 1);
    h.toggle_overview(false);
    h.wait_until("the overview group", |_| overview_group(&dock).is_some());
    dock.send(Cmd::OverviewFrame(a.0 as u32, (20, 20, 200, 150)));
    h.wait_until("the frame to arrive", |s| {
        s.shell.overview.as_ref().is_some_and(|o| {
            o.targets()
                .iter()
                .any(|f| f.id == a && f.frame == Rect::new(20.0, 20.0, 200.0, 150.0))
        })
    });
    h.advance(5.0);
    dock.send(Cmd::Quit);
    h.wait_until("the dock's death to close the overview", |s| {
        !s.shell.overview_open()
    });
    h.advance(5.0);
    assert!(h.state.shell.overview.is_none());
    let (plan, _) = plan_for(
        &h.state.wm,
        h.state.entry(a).unwrap(),
        0.0,
        h.state.now(),
        &h.state.fx,
        h.state.display_options(),
    )
    .expect("drawn on the desktop");
    assert_eq!(
        plan.visual, desktop,
        "the window is back at its desktop frame"
    );
    let out = h.state.outputs[0].id;
    assert_eq!(
        h.state.usable_area(out).h,
        f64::from(OUTPUT.1),
        "the dead dock's zone is released"
    );
}

#[test]
fn workspace_requests_edit_the_list_and_a_switch_leaves_the_overview() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    let out = h.state.outputs[0].id;
    let first = h.state.wm.current_workspace(out).unwrap();
    h.toggle_overview(false);
    h.wait_until("the overview group", |_| overview_group(&dock).is_some());
    let groups_before = dock.groups().len();

    dock.send(Cmd::AddWorkspace);
    h.wait_until("a second workspace", |s| s.wm.workspaces(out).len() == 2);
    let second = h.state.wm.workspaces(out)[1].id;
    h.wait_for_groups(&dock, groups_before + 1);
    let bar: Vec<DockEvent> = dock.groups()[groups_before]
        .iter()
        .filter(|e| matches!(e, DockEvent::OverviewWorkspace { .. }))
        .cloned()
        .collect();
    assert_eq!(
        bar,
        vec![
            DockEvent::OverviewWorkspace {
                id: first.0 as u32,
                current: true
            },
            DockEvent::OverviewWorkspace {
                id: second.0 as u32,
                current: false
            }
        ],
        "the bar is resent with the new workspace"
    );

    dock.send(Cmd::MoveWorkspace(second.0 as u32, 0));
    h.wait_until("the reorder", |s| s.wm.workspaces(out)[0].id == second);
    assert_eq!(
        h.state.wm.current_workspace(out),
        Some(first),
        "reordering keeps the screen"
    );
    h.wait_for_groups(&dock, groups_before + 2);
    assert!(
        h.state.shell.overview_open(),
        "workspace edits keep the overview open"
    );

    dock.send(Cmd::SwitchWorkspace(second.0 as u32));
    h.wait_until("the switch", |s| {
        s.wm.current_workspace(out) == Some(second)
    });
    assert!(
        !h.state.shell.overview_open(),
        "a switch leaves the overview"
    );
    h.wait_until("the dock to hear it closed", |_| {
        dock.groups()
            .iter()
            .any(|g| g == &[DockEvent::OverviewClosed, DockEvent::Done])
    });
    h.advance(5.0);

    dock.send(Cmd::RemoveWorkspace(second.0 as u32));
    h.wait_until("the removal", |s| s.wm.workspaces(out).len() == 1);
    dock.send(Cmd::RemoveWorkspace(first.0 as u32));
    dock.send(Cmd::SwitchWorkspace(777));
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(
        h.state.wm.workspaces(out).len(),
        1,
        "the last workspace stays"
    );
    assert_eq!(
        h.state.wm.current_workspace(out),
        Some(first),
        "an unknown id switches nothing"
    );
    assert_eq!(dock.error(), None);
}

#[test]
fn named_actions_and_tile_commands_from_the_dock_act_as_their_shortcuts() {
    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    let _a = spawn_app(h.socket_path(), "org.example.a");
    let a = h.mapped_window(1);
    h.wait_for_groups(&dock, 1);

    dock.send(Cmd::RunAction("show-desktop"));
    h.wait_until("the desktop to show", |s| s.fx.show_desktop_active());
    dock.send(Cmd::RunAction("show-desktop"));
    h.wait_until("the desktop to come back", |s| !s.fx.show_desktop_active());
    dock.send(Cmd::RunAction("no-such-action"));

    dock.send(Cmd::AppCommand("org.example.a", AppCommand::ShowWindows));
    h.wait_until("the application overview", |s| {
        s.shell.overview.as_ref().is_some_and(|o| {
            !o.closing
                && o.mode
                    == cupertino_overview::OverviewMode::AppOverview(s.wm.window(a).unwrap().app)
        })
    });
    h.wait_until("its group", |_| overview_group(&dock).is_some());
    assert!(
        matches!(overview_group(&dock).unwrap()[..], [DockEvent::OverviewWindow { .. }, DockEvent::OverviewWorkspace { .. }, DockEvent::OverviewChanged { app: true, ref app_id, .. }, DockEvent::Done] if app_id == "org.example.a")
    );
    dock.send(Cmd::OverviewClose);
    h.wait_until("the overview to close", |s| !s.shell.overview_open());

    dock.send(Cmd::AppCommand("org.example.a", AppCommand::Hide));
    h.wait_until("the application to hide", |s| {
        s.wm.window(a)
            .is_some_and(|w| s.wm.app(w.app).unwrap().hidden)
    });
    dock.send(Cmd::AppCommand("org.example.gone", AppCommand::Quit));
    for _ in 0..10 {
        h.step();
    }
    assert_eq!(
        h.state.wm.windows().count(),
        1,
        "an unknown application is ignored"
    );
    assert_eq!(dock.error(), None);
}

#[test]
fn a_hot_corner_and_a_gesture_reach_the_dock_of_the_output() {
    use cupertino_input::GestureEvent;

    let Some(mut h) = Harness::new() else { return };
    let dock = DockClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&dock, 1);
    let groups = dock.groups().len();
    // the default dwell is 0.0 s: the corner fires on arrival
    h.state.pointer_moved_to(
        (f64::from(OUTPUT.0) - 1.0, f64::from(OUTPUT.1) - 1.0).into(),
        0,
        None,
    );
    h.state.tick();
    h.wait_for_groups(&dock, groups + 1);
    assert_eq!(
        dock.groups()[groups],
        vec![
            DockEvent::HotCorner(cupertino_dock_v1::Corner::BottomRight as u32),
            DockEvent::Done
        ]
    );
    h.state.pointer_moved_to((500.0, 300.0).into(), 0, None);

    let t = h.state.now();
    h.state.gesture(GestureEvent::PinchBegin {
        time: t,
        fingers: 4,
    });
    for i in 1..=10 {
        h.state.gesture(GestureEvent::PinchUpdate {
            time: t + 0.016 * i as f64,
            dx: 0.0,
            dy: 0.0,
            scale: 1.0 + 0.1 * i as f64,
            rotation: 0.0,
        });
    }
    h.state.gesture(GestureEvent::PinchEnd {
        time: t + 0.3,
        cancelled: false,
    });
    h.wait_for_groups(&dock, groups + 2);
    assert_eq!(
        dock.groups()[groups + 1],
        vec![
            DockEvent::Gesture(cupertino_dock_v1::Gesture::Spread as u32),
            DockEvent::Done
        ]
    );
    assert!(
        !h.state.fx.show_desktop_active(),
        "the server runs nothing itself: the dock maps the gesture"
    );
    assert_eq!(dock.error(), None);
}

/// Kinds of the layer-shell `top` pieces in the composed frame of the first output.
fn top_layer(h: &mut Harness) -> Vec<cupertino_scene::SceneKind> {
    let mut backend = h.state.backend.take().unwrap();
    let kinds = h
        .state
        .with_scene(backend.renderer(), 0, |r, mut p| {
            cupertino_server::render::scene::compose_scene(r, &mut p)
                .frame
                .items()
                .iter()
                .filter(|(layer, _)| *layer == cupertino_scene::SceneLayer::LayerTop)
                .map(|(_, item)| item.id.kind)
                .collect()
        })
        .expect("render resources");
    h.state.backend = Some(backend);
    kinds
}

#[test]
fn the_dock_gives_way_to_a_full_screen_window_in_front_and_comes_back() {
    let Some(mut h) = Harness::new() else { return };
    let _one = spawn_app(h.socket_path(), "org.example.one");
    let one = h.mapped_window(1);
    let _two = spawn_app(h.socket_path(), "org.example.two");
    let two = h.mapped_window(2);
    let dock = DockClient::spawn(h.shell_stream(), true);
    let out = h.state.outputs[0].id;
    h.wait_until("the dock surface to be placed", |s| {
        s.dock_rect(out).is_some_and(|r| r.w > 0.0)
    });
    h.advance(1.0);
    let on_dock = (500.0, 660.0).into();
    let dock_under = |s: &Cupertino| {
        let window = s.window_surface_under(on_dock).map(|(_, s, _)| s);
        s.surface_under(on_dock)
            .is_some_and(|(s, _)| window.as_ref() != Some(&s))
    };
    assert!(!top_layer(&mut h).is_empty(), "the dock is drawn");
    // a placed dock has not necessarily committed its input region yet (seen under the full gate's load)
    h.wait_until("the dock to take the pointer", dock_under);

    h.state.command(Command::Fullscreen {
        requested_by: cupertino_wm::Requester::Client,
        window: two,
        output: None,
    });
    h.advance(5.0);
    h.advance(1.0);
    assert_eq!(
        h.state.wm.window(two).unwrap().state(),
        LifecycleState::FullScreen
    );
    assert_eq!(
        top_layer(&mut h),
        vec![],
        "nothing of the dock over the window"
    );
    assert!(
        !dock_under(&h.state),
        "the window takes the pointer where the dock was"
    );

    h.state.command(Command::FocusWindow { window: one });
    h.advance(5.0);
    assert!(
        !top_layer(&mut h).is_empty(),
        "back with another window in front"
    );

    h.state.command(Command::FocusWindow { window: two });
    h.advance(5.0);
    assert_eq!(top_layer(&mut h), vec![]);
    h.state.command(Command::ExitFullscreen { window: two });
    h.advance(5.0);
    h.advance(1.0);
    assert!(!top_layer(&mut h).is_empty(), "back after full screen");
    assert!(dock_under(&h.state));
    assert_eq!(dock.error(), None);
}

#[test]
fn a_panel_outside_click_catcher_still_takes_the_click_over_a_full_screen_window() {
    let Some(mut h) = Harness::new() else { return };
    let _app = spawn_app(h.socket_path(), "org.example.one");
    let window = h.mapped_window(1);
    h.state.command(Command::Fullscreen {
        requested_by: cupertino_wm::Requester::Client,
        window,
        output: None,
    });
    h.advance(5.0);
    h.advance(1.0);
    assert_eq!(
        h.state.wm.fullscreen_front(h.state.outputs[0].id),
        Some(window)
    );
    let _catcher = common::Client::catcher(UnixStream::connect(h.socket_path()).unwrap());
    let outside = (500.0, 350.0).into();
    h.wait_until("the catcher to take a click outside its panel", |s| {
        let window = s.window_surface_under(outside).map(|(_, s, _)| s);
        s.surface_under(outside)
            .is_some_and(|(s, _)| window.as_ref() != Some(&s))
    });
}
