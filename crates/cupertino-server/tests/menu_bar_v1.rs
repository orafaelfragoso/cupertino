//! `cupertino_shell_v1.get_menu_bar` end to end: the compositor runs in-process on an offscreen EGL
//! device, a menu bar client (a top-anchored layer surface with the menu-bar role) and application
//! clients connect through its socket. Tests skip (pass) when no EGL device is available.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_defaults::compositor::CompositorSettings;
use cupertino_defaults::keyboard::{KeyboardSettings, ModifierProfile};
use cupertino_protocols::client::cupertino_dock_v1::{self, CupertinoDockV1};
use cupertino_protocols::client::cupertino_menu_bar_v1::{
    self, Command as BarCommand, CupertinoMenuBarV1, SessionAction,
};
use cupertino_protocols::client::cupertino_shell_v1::{self, CupertinoShellV1};
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_wm::{Command, LifecycleState, Rect, TileKind, WindowId};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1, ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_manager_v1, ext_image_copy_capture_session_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_plasma::appmenu::client::{
    org_kde_kwin_appmenu, org_kde_kwin_appmenu_manager,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
mod common;

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

use gtk_shell::gtk_shell1::{self, GtkShell1};

const OUTPUT: (i32, i32) = (1000, 700);
/// Height of the bar surface; the server reserves the height of the design tokens (24 pt).
const BAR_HEIGHT: i32 = 24;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let cfg = Config::default();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            cfg,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .ok()?;
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
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
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Step until the server has handled every request `bar` sent so far, or posted it an error.
    fn settle_requests(&mut self, bar: &BarClient) {
        let from = bar.roundtrips();
        self.wait_until("the requests to be handled", |_| {
            bar.roundtrips() >= from + 2 || bar.error().is_some()
        });
    }

    /// Step until the bar client has seen `count` groups.
    fn wait_for_groups(&mut self, bar: &BarClient, count: usize) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            self.step();
            if bar.groups().len() >= count {
                return;
            }
        }
        panic!(
            "timed out waiting for group {count}, have {:?}",
            bar.groups()
        );
    }

    /// A connection of a shell process the server supervises, holding the roles of the menu bar process.
    fn shell_stream(&mut self) -> UnixStream {
        cupertino_server::shell_connection::shell_process_connection(
            &self.state.display_handle,
            &[ShellRole::Session, ShellRole::MenuBar],
        )
        .unwrap()
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Move the model clock forward and settle the animations.
    fn advance(&mut self, dt: f64) {
        self.state.fixed_time = Some(self.state.now() + dt);
        self.state.tick();
        for _ in 0..3 {
            self.step();
        }
    }

    /// Wait for `count` application windows and let the opening animations finish.
    fn mapped_window(&mut self, count: usize) -> cupertino_wm::WindowId {
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

/// What the menu bar object sent, in order.
#[derive(Clone, Debug, PartialEq)]
enum BarEvent {
    ActiveApp {
        app_id: String,
        title: String,
    },
    MenuNone,
    MenuDbusmenu {
        service: String,
        path: String,
    },
    WindowState {
        can_zoom: u32,
        workspace: u32,
        workspaces: u32,
    },
    Engage,
    ScreenCapture(u32),
    SessionAgent(u32, u32),
    AppWindow {
        id: u32,
        title: String,
        flags: u32,
    },
    Shortcut(u32, String),
    PrimaryModifier(u32),
    Done,
}

#[derive(Default)]
struct Status {
    events: Vec<BarEvent>,
    /// The protocol error the server posted: object interface and code.
    error: Option<(String, u32)>,
    /// The `gtk_shell1` capabilities received, in order.
    gtk_capabilities: Vec<u32>,
    /// Round trips completed: the events of every request sent before the last one have arrived.
    roundtrips: usize,
    /// `force_quit_dialog` events a dock received.
    force_quit_dialogs: usize,
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
    org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager,
    org_kde_kwin_appmenu::OrgKdeKwinAppmenu,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
    ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1
);

impl Dispatch<GtkShell1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &GtkShell1,
        event: gtk_shell1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let gtk_shell1::Event::Capabilities { capabilities } = event;
        let mut status = state.status.lock().unwrap();
        status.gtk_capabilities.push(capabilities);
    }
}

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
                attach_and_commit(&state.shm, &state.surface, (width as i32, BAR_HEIGHT), qh);
            }
        }
    }
}

impl Dispatch<CupertinoMenuBarV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoMenuBarV1,
        event: cupertino_menu_bar_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use cupertino_menu_bar_v1::Event as E;
        let event = match event {
            E::ActiveApp { app_id, title } => BarEvent::ActiveApp { app_id, title },
            E::MenuNone => BarEvent::MenuNone,
            E::MenuDbusmenu { service, path } => BarEvent::MenuDbusmenu { service, path },
            E::WindowState {
                can_zoom,
                workspace,
                workspaces,
            } => BarEvent::WindowState {
                can_zoom,
                workspace,
                workspaces,
            },
            E::Engage => BarEvent::Engage,
            E::ScreenCapture { active } => BarEvent::ScreenCapture(active),
            E::SessionAgent { trust, pid } => BarEvent::SessionAgent(raw(trust), pid),
            E::AppWindow { id, title, flags } => BarEvent::AppWindow {
                id,
                title,
                flags: raw(flags),
            },
            E::Shortcut { action, chord } => BarEvent::Shortcut(raw(action), chord),
            E::PrimaryModifier { key } => BarEvent::PrimaryModifier(raw(key)),
            E::Done => BarEvent::Done,
            other => panic!("unexpected event {other:?}"),
        };
        state.status.lock().unwrap().events.push(event);
    }
}

/// The raw value of an enum or bitfield argument.
fn raw<T: Into<u32>>(value: wayland_client::WEnum<T>) -> u32 {
    match value {
        wayland_client::WEnum::Value(v) => v.into(),
        wayland_client::WEnum::Unknown(raw) => raw,
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
        if let cupertino_dock_v1::Event::ForceQuitDialog = event {
            state.status.lock().unwrap().force_quit_dialogs += 1;
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

/// A further toplevel of a client: its surface, which gets its buffer at the first configure.
struct OtherWindow {
    surface: wl_surface::WlSurface,
    size: (i32, i32),
    attached: std::sync::atomic::AtomicBool,
}

impl Dispatch<xdg_surface::XdgSurface, OtherWindow> for ClientState {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        window: &OtherWindow,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            if !window
                .attached
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                attach_and_commit(&state.shm, &window.surface, window.size, qh);
            }
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

/// Requests a test sends to a client thread.
enum Cmd {
    SetMenuOpen(bool),
    WindowCommand(BarCommand),
    MoveToWorkspace(u32),
    SessionAction(SessionAction),
    /// Announce the KDE application menu address of an application window.
    SetAddress(&'static str, &'static str),
    /// Map a second toplevel of the same application client.
    OpenSecondWindow,
    /// Announce the KDE application menu address of the second toplevel.
    SetSecondAddress(&'static str, &'static str),
    /// Ask for exclusive keyboard interactivity (true) or none (false), and commit.
    SetKeyboard(bool),
    /// Choose a row of the window list.
    ActivateWindow(u32),
    /// Retitle the first toplevel of an application client.
    SetTitle(String),
    /// Close the second toplevel of an application client.
    CloseSecondWindow,
    /// Destroy the menu bar, its layer surface and the surface, as an exiting bar process does.
    Destroy,
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

impl TestClient {
    fn send(&self, cmd: Cmd) {
        self.commands.send(cmd).unwrap();
    }
}

type Setup = Box<
    dyn FnOnce(
            &wayland_client::globals::GlobalList,
            &QueueHandle<ClientState>,
            &Arc<Mutex<Status>>,
        ) -> (ClientState, Box<dyn FnMut(Cmd)>)
        + Send,
>;

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
    run_client_on(stream, Box::new(setup))
}

/// A client thread on a connected stream (a privileged client's stream comes from the server itself).
fn run_client_on(stream: UnixStream, setup: Setup) -> TestClient {
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
                apply(cmd);
            }
            if queue.roundtrip(&mut client).is_err() {
                let error = conn
                    .protocol_error()
                    .expect("the roundtrip failed with a protocol error");
                st.lock().unwrap().error = Some((error.object_interface, error.code));
                return;
            }
            st.lock().unwrap().roundtrips += 1;
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    TestClient {
        commands,
        status,
        thread: Some(thread),
    }
}

/// The menu bar process: a top-anchored layer surface with the menu-bar role on the first output.
struct BarClient(TestClient);

impl BarClient {
    /// `layer` false makes the menu bar of a surface that has no layer role.
    fn spawn(stream: UnixStream, layer: bool) -> BarClient {
        Self::spawn_at(stream, layer, 1)
    }

    /// A bar on a shell global bound at `version`.
    fn spawn_at(stream: UnixStream, layer: bool, version: u32) -> BarClient {
        BarClient(run_client(stream, move |globals, qh, status| {
            let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
            let shell: CupertinoShellV1 = globals.bind(qh, version..=version, ()).unwrap();
            let output: wl_output::WlOutput = globals.bind(qh, 1..=4, ()).unwrap();
            let surface = compositor.create_surface(qh, ());
            let layer_surface = layer.then(|| {
                let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                    globals.bind(qh, 1..=4, ()).unwrap();
                let l = layer_shell.get_layer_surface(
                    &surface,
                    Some(&output),
                    zwlr_layer_shell_v1::Layer::Top,
                    "menu-bar-test".into(),
                    qh,
                    (),
                );
                l.set_size(0, BAR_HEIGHT as u32);
                l.set_anchor(
                    zwlr_layer_surface_v1::Anchor::Top
                        | zwlr_layer_surface_v1::Anchor::Left
                        | zwlr_layer_surface_v1::Anchor::Right,
                );
                l.set_exclusive_zone(0);
                l
            });
            let bar = shell.get_menu_bar(&surface, &output, qh, ());
            surface.commit();
            let bar_surface = surface.clone();
            let state = ClientState {
                status: status.clone(),
                shm,
                surface,
                attached: false,
                toplevel_size: (0, 0),
            };
            let apply = Box::new(move |cmd: Cmd| match cmd {
                Cmd::SetMenuOpen(open) => bar.set_menu_open(u32::from(open)),
                Cmd::WindowCommand(c) => bar.window_command(c),
                Cmd::MoveToWorkspace(n) => bar.move_to_workspace(n),
                Cmd::SessionAction(a) => bar.session_action(a),
                Cmd::ActivateWindow(id) => bar.activate_window(id),
                Cmd::SetAddress(..)
                | Cmd::OpenSecondWindow
                | Cmd::SetSecondAddress(..)
                | Cmd::SetTitle(_)
                | Cmd::CloseSecondWindow => {
                    panic!("a menu bar has no application windows")
                }
                Cmd::SetKeyboard(exclusive) => {
                    let mode = if exclusive {
                        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive
                    } else {
                        zwlr_layer_surface_v1::KeyboardInteractivity::None
                    };
                    layer_surface
                        .as_ref()
                        .expect("a layer surface")
                        .set_keyboard_interactivity(mode);
                    bar_surface.commit();
                }
                Cmd::Destroy => {
                    bar.destroy();
                    if let Some(l) = &layer_surface {
                        l.destroy();
                    }
                    bar_surface.destroy();
                }
            });
            (state, apply)
        }))
    }

    /// The events received so far, split into groups ended by `done`; a trailing partial group is not listed.
    fn groups(&self) -> Vec<Vec<BarEvent>> {
        let events = self.0.status.lock().unwrap().events.clone();
        let mut groups = vec![Vec::new()];
        for e in events {
            let done = e == BarEvent::Done;
            groups.last_mut().unwrap().push(e);
            if done {
                groups.push(Vec::new());
            }
        }
        groups.pop();
        groups
    }

    fn send(&self, cmd: Cmd) {
        self.0.send(cmd);
    }

    fn error(&self) -> Option<(String, u32)> {
        self.0.status.lock().unwrap().error.clone()
    }

    fn roundtrips(&self) -> usize {
        self.0.status.lock().unwrap().roundtrips
    }
}

/// The dock process, as far as the menu bar tests need it: a layer surface at the bottom edge with the dock role that counts the
/// `force_quit_dialog` events.
struct DockClient(TestClient);

impl DockClient {
    fn spawn(h: &mut Harness) -> DockClient {
        let stream = cupertino_server::shell_connection::shell_process_connection(
            &h.state.display_handle,
            &[ShellRole::Session, ShellRole::Dock],
        )
        .unwrap();
        DockClient(run_client(stream, move |globals, qh, status| {
            let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
            let shell: CupertinoShellV1 = globals.bind(qh, 10..=10, ()).unwrap();
            let output: wl_output::WlOutput = globals.bind(qh, 1..=4, ()).unwrap();
            let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(qh, 1..=4, ()).unwrap();
            let surface = compositor.create_surface(qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                Some(&output),
                zwlr_layer_shell_v1::Layer::Top,
                "dock-test".into(),
                qh,
                (),
            );
            layer.set_size(0, BAR_HEIGHT as u32);
            layer.set_anchor(
                zwlr_layer_surface_v1::Anchor::Bottom
                    | zwlr_layer_surface_v1::Anchor::Left
                    | zwlr_layer_surface_v1::Anchor::Right,
            );
            let dock = shell.get_dock(&surface, &output, qh, ());
            surface.commit();
            let state = ClientState {
                status: status.clone(),
                shm,
                surface,
                attached: false,
                toplevel_size: (0, 0),
            };
            let apply = Box::new(move |_: Cmd| {
                let _ = (&dock, &layer);
                panic!("the dock takes no command")
            });
            (state, apply)
        }))
    }

    fn force_quit_dialogs(&self) -> usize {
        self.0.status.lock().unwrap().force_quit_dialogs
    }

    fn roundtrips(&self) -> usize {
        self.0.status.lock().unwrap().roundtrips
    }
}

/// The screenshot process: a privileged client that captures the output while it runs; `Cmd::Destroy` ends the session.
struct CaptureClient(TestClient);

impl CaptureClient {
    fn spawn(h: &mut Harness) -> CaptureClient {
        let stream = h.state.privileged_client().unwrap();
        CaptureClient(run_client_on(
            stream,
            Box::new(|globals, qh, status| {
                let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
                let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
                let output: wl_output::WlOutput = globals.bind(qh, 1..=4, ()).unwrap();
                let sources: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 = globals.bind(qh, 1..=1, ()).unwrap();
                let manager: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 =
                    globals.bind(qh, 1..=1, ()).unwrap();
                let source = sources.create_source(&output, qh, ());
                let session = manager.create_session(
                    &source,
                    ext_image_copy_capture_manager_v1::Options::empty(),
                    qh,
                    (),
                );
                let state = ClientState {
                    status: status.clone(),
                    shm,
                    surface: compositor.create_surface(qh, ()),
                    attached: true,
                    toplevel_size: (0, 0),
                };
                let apply = Box::new(move |cmd: Cmd| match cmd {
                    Cmd::Destroy => session.destroy(),
                    _ => panic!("a capture client takes no other command"),
                });
                (state, apply)
            }),
        ))
    }

    fn send(&self, cmd: Cmd) {
        self.0.send(cmd);
    }
}

/// An application with one 300x200 toplevel.
/// A GTK application's connection: it binds `gtk_shell1` and nothing more.
struct GtkClient(TestClient);

impl GtkClient {
    fn spawn(socket: PathBuf) -> GtkClient {
        let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
        GtkClient(run_client(stream, move |globals, qh, status| {
            let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
            let shell: GtkShell1 = globals.bind(qh, 1..=7, ()).unwrap();
            let state = ClientState {
                status: status.clone(),
                shm,
                surface: compositor.create_surface(qh, ()),
                attached: false,
                toplevel_size: (0, 0),
            };
            let apply = Box::new(move |_: Cmd| {
                let _ = &shell;
                panic!("a GTK client takes no command")
            });
            (state, apply)
        }))
    }

    fn capabilities(&self) -> Vec<u32> {
        self.0.status.lock().unwrap().gtk_capabilities.clone()
    }

    fn roundtrips(&self) -> usize {
        self.0.status.lock().unwrap().roundtrips
    }
}

struct AppClient(TestClient);

impl AppClient {
    fn spawn(socket: PathBuf, app_id: &'static str) -> AppClient {
        let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
        AppClient(run_client(stream, move |globals, qh, status| {
            let compositor: wl_compositor::WlCompositor = globals.bind(qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(qh, 1..=5, ()).unwrap();
            let manager: org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager =
                globals.bind(qh, 1..=2, ()).unwrap();
            let surface = compositor.create_surface(qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, qh, ());
            let toplevel = xdg.get_toplevel(qh, ());
            toplevel.set_app_id(app_id.to_string());
            toplevel.set_title(format!("{app_id} window"));
            let appmenu = manager.create(&surface, qh, ());
            surface.commit();
            let state = ClientState {
                status: status.clone(),
                shm,
                surface,
                attached: false,
                toplevel_size: (300, 200),
            };
            let qh = qh.clone();
            let mut second = None;
            let apply = Box::new(move |cmd: Cmd| match cmd {
                Cmd::SetAddress(service, path) => {
                    appmenu.set_address(service.to_string(), path.to_string())
                }
                Cmd::OpenSecondWindow => {
                    let surface = compositor.create_surface(&qh, ());
                    let window = OtherWindow {
                        surface: surface.clone(),
                        size: (200, 150),
                        attached: Default::default(),
                    };
                    let xdg = wm_base.get_xdg_surface(&surface, &qh, window);
                    let toplevel = xdg.get_toplevel(&qh, ());
                    toplevel.set_app_id(app_id.to_string());
                    toplevel.set_title(format!("{app_id} second window"));
                    let appmenu = manager.create(&surface, &qh, ());
                    surface.commit();
                    second = Some((xdg, toplevel, appmenu, surface));
                }
                Cmd::SetSecondAddress(service, path) => second
                    .as_ref()
                    .expect("the second window is open")
                    .2
                    .set_address(service.to_string(), path.to_string()),
                Cmd::SetTitle(title) => toplevel.set_title(title),
                Cmd::CloseSecondWindow => {
                    let (xdg, toplevel, appmenu, surface) =
                        second.take().expect("the second window is open");
                    appmenu.release();
                    toplevel.destroy();
                    xdg.destroy();
                    surface.destroy();
                }
                _ => panic!("an application window takes no menu bar command"),
            });
            (state, apply)
        }))
    }

    fn send(&self, cmd: Cmd) {
        self.0.send(cmd);
    }
}

// ---- tests ---------------------------------------------------------------------------------

/// The group sent while no window is active.
fn empty_group() -> Vec<BarEvent> {
    vec![
        BarEvent::ActiveApp {
            app_id: String::new(),
            title: String::new(),
        },
        BarEvent::MenuNone,
        BarEvent::WindowState {
            can_zoom: 0,
            workspace: 0,
            workspaces: 0,
        },
        BarEvent::Done,
    ]
}

#[test]
fn the_bar_gets_a_first_group_and_the_server_reserves_its_height() {
    let Some(mut h) = Harness::new() else { return };
    let out = h.state.outputs[0].id;
    assert_eq!(h.state.usable_area(out), Rect::new(0.0, 0.0, 1000.0, 700.0));
    let bar = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&bar, 1);
    assert_eq!(bar.groups(), vec![empty_group()]);
    // the client's exclusive zone is 0: the reservation is the server's
    assert_eq!(
        h.state.usable_area(out),
        Rect::new(0.0, 24.0, 1000.0, 676.0)
    );
    assert_eq!(bar.error(), None);
}

#[test]
fn a_version_3_bar_shows_whether_a_capture_session_runs() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn_at(h.shell_stream(), true, 3);
    h.wait_for_groups(&bar, 2);
    assert_eq!(
        bar.groups(),
        vec![
            empty_group(),
            vec![BarEvent::ScreenCapture(0), BarEvent::Done]
        ]
    );

    let capture = CaptureClient::spawn(&mut h);
    h.wait_for_groups(&bar, 3);
    assert_eq!(
        bar.groups()[2],
        [BarEvent::ScreenCapture(1), BarEvent::Done]
    );
    assert!(h.state.capture_running());

    capture.send(Cmd::Destroy);
    h.wait_for_groups(&bar, 4);
    assert_eq!(
        bar.groups()[3],
        [BarEvent::ScreenCapture(0), BarEvent::Done]
    );
    // a bar of version 1 is never told: the other tests see exactly their groups
    assert_eq!(bar.error(), None);
}

#[test]
fn a_version_8_bar_is_told_which_process_is_the_session_agent() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn_at(h.shell_stream(), true, 8);
    h.wait_for_groups(&bar, 3);
    assert_eq!(
        bar.groups()[2],
        [BarEvent::SessionAgent(0, 0), BarEvent::Done],
        "no login process is supervised: nobody is the agent"
    );

    h.state.config.dev_shell = true;
    h.state.refresh_session_agent();
    h.wait_for_groups(&bar, 4);
    assert_eq!(
        bar.groups()[3],
        [BarEvent::SessionAgent(2, 0), BarEvent::Done]
    );
    h.state.refresh_session_agent();
    h.step();
    assert_eq!(
        bar.groups().len(),
        4,
        "an unchanged answer is not sent again"
    );
    assert_eq!(bar.error(), None);
}

#[test]
fn a_surface_without_the_layer_role_is_a_role_error() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), false);
    let end = Instant::now() + Duration::from_secs(10);
    while bar.error().is_none() && Instant::now() < end {
        h.step();
    }
    // error 0 of cupertino_shell_v1.error is `role`
    assert_eq!(
        bar.error(),
        Some((
            cupertino_shell_v1::CupertinoShellV1::interface()
                .name
                .to_string(),
            0
        ))
    );
    assert_eq!(
        h.state.usable_area(h.state.outputs[0].id),
        Rect::new(0.0, 0.0, 1000.0, 700.0)
    );
}

#[test]
fn the_active_window_its_menu_and_commands_go_through_the_bar() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&bar, 1);

    // a second client maps and takes focus: its app id, no menu, its window state
    let app = AppClient::spawn(h.socket_path(), "org.example.editor");
    let id = h.mapped_window(1);
    h.wait_for_groups(&bar, 2);
    assert_eq!(
        bar.groups()[1],
        vec![
            BarEvent::ActiveApp {
                app_id: "org.example.editor".into(),
                title: "org.example.editor window".into()
            },
            BarEvent::MenuNone,
            BarEvent::WindowState {
                can_zoom: 1,
                workspace: 1,
                workspaces: 1
            },
            BarEvent::Done,
        ]
    );

    // the window announces a DBusMenu address: the next group names it
    app.send(Cmd::SetAddress(":1.99", "/MenuBar/1"));
    h.wait_for_groups(&bar, 3);
    assert_eq!(
        bar.groups()[2],
        vec![
            BarEvent::ActiveApp {
                app_id: "org.example.editor".into(),
                title: "org.example.editor window".into()
            },
            BarEvent::MenuDbusmenu {
                service: ":1.99".into(),
                path: "/MenuBar/1".into()
            },
            BarEvent::WindowState {
                can_zoom: 1,
                workspace: 1,
                workspaces: 1
            },
            BarEvent::Done,
        ]
    );

    // the window command acts on the active window
    bar.send(Cmd::WindowCommand(BarCommand::Minimize));
    h.wait_until("the window to minimize", |s| {
        s.wm.window(id).is_some_and(|w| w.state().is_minimized())
    });
    assert_eq!(
        h.state.wm.window(id).unwrap().state(),
        LifecycleState::Minimizing
    );
}

/// The title and the menu the bar was told in its latest group, once a window is active.
fn latest_title_and_menu(bar: &BarClient) -> Option<(String, BarEvent)> {
    let group = bar.groups().pop()?;
    let title = group.iter().find_map(|e| match e {
        BarEvent::ActiveApp { title, .. } => Some(title.clone()),
        _ => None,
    })?;
    let menu = group
        .into_iter()
        .find(|e| matches!(e, BarEvent::MenuNone | BarEvent::MenuDbusmenu { .. }))?;
    Some((title, menu))
}

/// Two windows of one client each export their own menus: the bar is told the menus of the window with the focus
/// only, never the other window's, and the focus moving back swaps them back.
#[test]
fn the_bar_gets_the_menus_of_the_focused_window_of_a_client_with_two() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&bar, 1);
    let first_menu = BarEvent::MenuDbusmenu {
        service: ":1.42".into(),
        path: "/MenuBar/1".into(),
    };
    let second_menu = BarEvent::MenuDbusmenu {
        service: ":1.42".into(),
        path: "/MenuBar/2".into(),
    };
    let first_title = "org.example.office window".to_owned();
    let second_title = "org.example.office second window".to_owned();

    let app = AppClient::spawn(h.socket_path(), "org.example.office");
    let first = h.mapped_window(1);
    app.send(Cmd::SetAddress(":1.42", "/MenuBar/1"));
    h.wait_until("the first window's menus", |_| {
        latest_title_and_menu(&bar) == Some((first_title.clone(), first_menu.clone()))
    });

    // the second window takes the focus with no menus of its own: the first window's are not offered for it
    app.send(Cmd::OpenSecondWindow);
    let second = h.mapped_window(2);
    assert_ne!(second, first);
    h.wait_until("the second window to be active", |_| {
        latest_title_and_menu(&bar).is_some_and(|(title, _)| title == second_title)
    });
    assert_eq!(
        latest_title_and_menu(&bar),
        Some((second_title.clone(), BarEvent::MenuNone))
    );
    app.send(Cmd::SetSecondAddress(":1.42", "/MenuBar/2"));
    h.wait_until("the second window's menus", |_| {
        latest_title_and_menu(&bar) == Some((second_title.clone(), second_menu.clone()))
    });

    // the second window goes: the focus and the menus are the first window's again
    bar.send(Cmd::WindowCommand(BarCommand::Minimize));
    h.wait_until("the first window's menus again", |s| {
        s.wm.key_window() == Some(first)
            && latest_title_and_menu(&bar) == Some((first_title.clone(), first_menu.clone()))
    });
    assert_eq!(bar.error(), None);
}

/// GTK hides a window's own menu bar on `global_menu_bar` (2): the server announces it only while a menu bar shows
/// the menus, to clients already connected and to new ones, and withdraws it (0) when the bar goes.
#[test]
fn gtk_clients_are_told_of_a_global_menu_bar_only_while_a_bar_exists() {
    let Some(mut h) = Harness::new() else { return };
    let early = GtkClient::spawn(h.socket_path());
    h.wait_until("the client to have bound", |_| early.roundtrips() > 0);
    assert_eq!(early.capabilities(), Vec::<u32>::new(), "no bar yet");

    let bar = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&bar, 1);
    h.wait_until("the connected client to be told", |_| {
        early.capabilities() == [2]
    });
    let late = GtkClient::spawn(h.socket_path());
    h.wait_until("a new client to be told on bind", |_| {
        late.capabilities() == [2]
    });

    bar.send(Cmd::Destroy);
    h.wait_until("both clients to be told the bar is gone", |_| {
        early.capabilities() == [2, 0] && late.capabilities() == [2, 0]
    });
}

/// The output of the last bar unplugged: the bar goes with it, and so does `global_menu_bar`.
#[test]
fn gtk_clients_are_told_the_bar_is_gone_when_its_output_is() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&bar, 1);
    let gtk = GtkClient::spawn(h.socket_path());
    h.wait_until("the client to be told", |_| gtk.capabilities() == [2]);

    let output = h.state.outputs[0].id;
    h.state.remove_output(output);
    h.wait_until("the client to be told the bar is gone", |_| {
        gtk.capabilities() == [2, 0]
    });
}

#[test]
fn move_to_workspace_moves_the_active_window_and_updates_the_state() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), true);
    let _app = AppClient::spawn(h.socket_path(), "org.example.mover");
    let id = h.mapped_window(1);
    let out = h.state.outputs[0].id;
    h.state.command(Command::AddWorkspace { output: out });
    h.wait_until("both workspaces to exist", |s| {
        s.wm.workspaces(out).len() == 2
    });
    let second = h.state.wm.workspaces(out)[1].id;
    bar.send(Cmd::MoveToWorkspace(2));
    h.wait_until("the window to change workspace", |s| {
        s.wm.window(id).is_some_and(|w| w.workspace == second)
    });
    h.advance(5.0);
    let last = bar.groups().pop().unwrap();
    assert_eq!(
        last[2],
        BarEvent::WindowState {
            can_zoom: 1,
            workspace: 2,
            workspaces: 2
        }
    );
}

#[test]
fn the_menu_bar_shortcut_engages_the_bar_of_the_active_output() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&bar, 1);
    h.state.focus_menu_bar();
    // the client applies events per group: engage must end with done to take effect
    h.wait_for_groups(&bar, 2);
    assert_eq!(bar.groups()[1], vec![BarEvent::Engage, BarEvent::Done]);
}

/// The bar answers `engage` by asking for exclusive keyboard interactivity; it then holds the
/// keyboard, a window taking focus does not take it away, and letting go returns it to the key
/// window.
#[test]
fn an_engaged_bar_holds_the_keyboard_over_window_focus_changes() {
    let Some(mut h) = Harness::new() else { return };
    let _first = AppClient::spawn(h.socket_path(), "org.example.first");
    let first = h.mapped_window(1);
    let _second = AppClient::spawn(h.socket_path(), "org.example.second");
    let second = h.mapped_window(2);
    let bar = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&bar, 1);
    let surface = |h: &Harness, id| {
        h.state
            .entry(id)
            .map(|e| e.surface().clone())
            .expect("window surface")
    };
    let focus = |h: &Harness| h.state.seat.get_keyboard().unwrap().current_focus();
    assert_eq!(focus(&h), Some(surface(&h, second)));

    h.state.focus_menu_bar();
    bar.send(Cmd::SetKeyboard(true));
    h.wait_until("the bar to take the keyboard", |s| {
        s.exclusive_layer.is_some()
    });
    let held = h.state.exclusive_layer.clone();
    assert_eq!(focus(&h), held);

    // another window becomes key while the bar holds the keyboard
    h.state.command(Command::FocusWindow { window: first });
    h.advance(0.1);
    assert_eq!(h.state.wm.key_window(), Some(first));
    assert_eq!(
        focus(&h),
        held,
        "a window focus change leaves an exclusive layer the keyboard"
    );

    bar.send(Cmd::SetKeyboard(false));
    h.wait_until("the bar to let go", |s| s.exclusive_layer.is_none());
    assert_eq!(
        focus(&h),
        Some(surface(&h, first)),
        "the keyboard returns to the key window"
    );
}

/// A bar process that exits with a menu open must not leave the output's next bar pinned
/// revealed in full screen.
#[test]
fn a_bar_gone_with_a_menu_open_does_not_keep_the_next_bar_revealed() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), true);
    let _app = AppClient::spawn(h.socket_path(), "org.example.full");
    let id = h.mapped_window(1);
    let out = h.state.outputs[0].id;
    let visible = |h: &Harness| {
        h.state
            .menu_bars
            .set
            .presentation(out, h.state.now() + 5.0)
            .unwrap()
            .visible
    };
    h.state.command(Command::Fullscreen {
        window: id,
        output: None,
    });
    h.advance(6.0);
    bar.send(Cmd::SetMenuOpen(true));
    let end = Instant::now() + Duration::from_secs(10);
    while visible(&h) != 1.0 && Instant::now() < end {
        h.step();
    }
    assert_eq!(visible(&h), 1.0, "revealed while a menu is open");

    bar.send(Cmd::Destroy);
    h.wait_until("the first bar to go", |s| !s.menu_bars.has_bar(out));
    let next = BarClient::spawn(h.shell_stream(), true);
    h.wait_for_groups(&next, 1);
    h.advance(1.0);
    assert_eq!(visible(&h), 0.0, "the next bar is hidden in full screen");
}

#[test]
fn full_screen_hides_the_bar_and_an_open_menu_or_the_top_edge_reveals_it() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn(h.shell_stream(), true);
    let _app = AppClient::spawn(h.socket_path(), "org.example.full");
    let id = h.mapped_window(1);
    let out = h.state.outputs[0].id;
    let visible =
        |h: &Harness, at: f64| h.state.menu_bars.set.presentation(out, at).unwrap().visible;
    assert_eq!(visible(&h, h.state.now()), 1.0);
    assert_eq!(h.state.usable_area(out).y, 24.0);
    let top = (500.0, 10.0).into();
    assert!(
        h.state.surface_under(top).is_some(),
        "the shown bar takes the pointer at the top edge"
    );

    h.state.command(Command::Fullscreen {
        window: id,
        output: None,
    });
    h.advance(5.0);
    h.advance(1.0);
    assert_eq!(visible(&h, h.state.now()), 0.0, "hidden in full screen");
    assert_eq!(
        h.state.usable_area(out).y,
        0.0,
        "a hidden bar reserves no space"
    );
    assert!(
        h.state.surface_under(top).is_none(),
        "a hidden bar does not take the pointer"
    );

    // an open menu keeps the bar shown, closing it hides the bar again
    bar.send(Cmd::SetMenuOpen(true));
    let end = Instant::now() + Duration::from_secs(10);
    while !h
        .state
        .menu_bars
        .set
        .presentation(out, h.state.now() + 5.0)
        .unwrap()
        .visible
        .eq(&1.0)
        && Instant::now() < end
    {
        h.step();
    }
    assert_eq!(
        visible(&h, h.state.now() + 5.0),
        1.0,
        "revealed while a menu is open"
    );
    bar.send(Cmd::SetMenuOpen(false));
    let end = Instant::now() + Duration::from_secs(10);
    while h
        .state
        .menu_bars
        .set
        .presentation(out, h.state.now() + 5.0)
        .unwrap()
        .visible
        != 0.0
        && Instant::now() < end
    {
        h.step();
    }
    assert_eq!(visible(&h, h.state.now() + 5.0), 0.0);

    // the edge dwell reveals it
    let t = h.state.now();
    h.state.pointer_moved_to((500.0, 0.0).into(), 0, None);
    h.state.fixed_time = Some(t + 0.3);
    h.state.tick();
    assert_eq!(visible(&h, t + 2.0), 1.0, "revealed by the edge dwell");
}

/// A private session bus (`dbus-daemon --session`), killed on drop.
struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    /// `None` when `dbus-daemon` is not installed.
    fn start() -> Option<PrivateBus> {
        let out = std::process::Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        assert!(out.status.success(), "dbus-daemon failed: {}", out.status);
        let text = String::from_utf8(out.stdout).unwrap();
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next().unwrap().to_string(),
            pid: lines.next().unwrap().to_string(),
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill").arg(&self.pid).status();
    }
}

/// The session agent's side: records which methods the server called.
struct FakeLogin(Arc<Mutex<Vec<&'static str>>>);

#[zbus::interface(name = "org.cupertino.Login1")]
impl FakeLogin {
    fn sleep(&self) {
        self.0.lock().unwrap().push("Sleep");
    }
    fn request_restart(&self) {
        self.0.lock().unwrap().push("RequestRestart");
    }
    fn request_shut_down(&self) {
        self.0.lock().unwrap().push("RequestShutDown");
    }
    fn lock_now(&self) {
        self.0.lock().unwrap().push("LockNow");
    }
    fn request_log_out(&self) {
        self.0.lock().unwrap().push("RequestLogOut");
    }
    fn switch_to_login_window(&self) {
        self.0.lock().unwrap().push("SwitchToLoginWindow");
    }
    fn refresh_capabilities(&self) {
        self.0.lock().unwrap().push("RefreshCapabilities");
    }
}

/// The menu bar does not call the session agent itself: it asks the server, which calls as the agent's parent.
#[test]
fn the_session_menu_entries_reach_the_session_agent_through_the_server() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: no dbus-daemon");
        return;
    };
    let Some(mut h) = Harness::new() else { return };
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address);
    let handle = cupertino_server::dbus::DbusHandle::start().expect("worker thread");
    handle.set_login(cupertino_server::dbus::LoginTrust::Agent(std::process::id()));
    h.state.dbus = Some(handle);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let _login = zbus::block_on(
        zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.cupertino.Login1")
            .unwrap()
            .serve_at("/org/cupertino/Login1", FakeLogin(calls.clone()))
            .unwrap()
            .build(),
    )
    .expect("the fake agent");
    let bar = BarClient::spawn_at(h.shell_stream(), true, 9);
    h.wait_for_groups(&bar, 1);
    for action in [
        SessionAction::Sleep,
        SessionAction::Restart,
        SessionAction::ShutDown,
        SessionAction::Lock,
        SessionAction::LogOut,
        SessionAction::SwitchUser,
        SessionAction::RefreshCapabilities,
    ] {
        bar.send(Cmd::SessionAction(action));
    }
    let end = Instant::now() + Duration::from_secs(10);
    while calls.lock().unwrap().len() < 7 && Instant::now() < end {
        h.step();
    }
    assert_eq!(
        *calls.lock().unwrap(),
        [
            "Sleep",
            "RequestRestart",
            "RequestShutDown",
            "LockNow",
            "RequestLogOut",
            "SwitchToLoginWindow",
            "RefreshCapabilities"
        ]
    );
}

// ---- version 10: window list, shortcuts, the commands of the fallback menus --------------------

/// `app_window_flags`.
const ACTIVE: u32 = 1;
const MINIMIZED: u32 = 2;

/// A bar on a shell global of version 10, with its first groups (window state, screen capture, session agent and
/// shortcuts) received.
fn bar_v10(h: &mut Harness) -> BarClient {
    let bar = BarClient::spawn_at(h.shell_stream(), true, 10);
    h.wait_for_groups(&bar, 4);
    bar
}

fn list_id(window: WindowId) -> u32 {
    window.0 as u32
}

/// The window list of the latest group with `window_state`: id, title and flags per row.
fn window_list(bar: &BarClient) -> Option<Vec<(u32, String, u32)>> {
    let group = bar
        .groups()
        .into_iter()
        .rev()
        .find(|g| g.iter().any(|e| matches!(e, BarEvent::WindowState { .. })))?;
    Some(
        group
            .into_iter()
            .filter_map(|e| match e {
                BarEvent::AppWindow { id, title, flags } => Some((id, title, flags)),
                _ => None,
            })
            .collect(),
    )
}

/// The chords and the primary modifier of the latest shortcut group.
fn shortcuts(bar: &BarClient) -> Option<(Vec<(u32, String)>, u32)> {
    let group = bar
        .groups()
        .into_iter()
        .rev()
        .find(|g| g.iter().any(|e| matches!(e, BarEvent::PrimaryModifier(_))))?;
    let chords = group
        .iter()
        .filter_map(|e| match e {
            BarEvent::Shortcut(action, chord) => Some((*action, chord.clone())),
            _ => None,
        })
        .collect();
    let primary = group.iter().find_map(|e| match e {
        BarEvent::PrimaryModifier(key) => Some(*key),
        _ => None,
    })?;
    Some((chords, primary))
}

/// One application client with two toplevels, the second one key: (client, first, second).
fn two_windows(h: &mut Harness, app_id: &'static str) -> (AppClient, WindowId, WindowId) {
    let app = AppClient::spawn(h.socket_path(), app_id);
    let first = h.mapped_window(1);
    app.send(Cmd::OpenSecondWindow);
    let second = h.mapped_window(2);
    assert_ne!(first, second);
    (app, first, second)
}

#[test]
fn the_window_list_follows_the_windows_of_the_active_application() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let (app, first, second) = two_windows(&mut h, "org.example.list");
    let row = |w: WindowId, title: &str, flags: u32| (list_id(w), title.to_string(), flags);
    let first_title = "org.example.list window";
    let second_title = "org.example.list second window";
    h.wait_until("both windows to be listed, the key one on top", |_| {
        window_list(&bar)
            == Some(vec![
                row(second, second_title, ACTIVE),
                row(first, first_title, 0),
            ])
    });

    h.state.command(Command::Minimize {
        window: first,
        target: None,
    });
    h.wait_until("the minimized flag", |_| {
        window_list(&bar)
            == Some(vec![
                row(second, second_title, ACTIVE),
                row(first, first_title, MINIMIZED),
            ])
    });

    h.state.command(Command::FocusWindow { window: first });
    app.send(Cmd::SetTitle("Renamed".into()));
    h.wait_until("the restored window on top with its new title", |_| {
        window_list(&bar)
            == Some(vec![
                row(first, "Renamed", ACTIVE),
                row(second, second_title, 0),
            ])
    });

    app.send(Cmd::CloseSecondWindow);
    h.wait_until("the closed window's row to go", |_| {
        window_list(&bar) == Some(vec![row(first, "Renamed", ACTIVE)])
    });
    assert_eq!(bar.error(), None);
}

#[test]
fn a_title_and_an_application_id_longer_than_the_limit_are_cut_at_a_character_boundary() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    // the application id is 4000 bytes, and a two-byte character spans byte 1024 of it
    let long_id: &'static str =
        Box::leak(format!("{}é{}", "a".repeat(1023), "b".repeat(4000 - 1025)).into_boxed_str());
    assert_eq!(long_id.len(), 4000);
    let app = AppClient::spawn(h.socket_path(), long_id);
    h.mapped_window(1);
    // a two-byte character spans byte 1024: the cut keeps none of it
    let title = format!("{}é{}", "a".repeat(1023), "b".repeat(4000 - 1025));
    assert_eq!(title.len(), 4000);
    app.send(Cmd::SetTitle(title.clone()));
    h.wait_until("the cut title in the active app", |_| {
        bar.groups().last().is_some_and(|g| {
            g.contains(&BarEvent::ActiveApp {
                app_id: "a".repeat(1023),
                title: "a".repeat(1023),
            })
        })
    });
    let list = window_list(&bar).unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].1.len() <= 1024, "{} bytes", list[0].1.len());
    assert_eq!(list[0].1, "a".repeat(1023));
    h.settle_requests(&bar);
    assert_eq!(bar.error(), None, "the bar stays connected");
}

#[test]
fn activate_window_shows_a_window_of_the_active_application() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let (_app, first, second) = two_windows(&mut h, "org.example.activate");
    h.wait_until("both windows listed", |_| {
        window_list(&bar).is_some_and(|l| l.len() == 2)
    });

    bar.send(Cmd::ActivateWindow(list_id(first)));
    h.wait_until("the first window to be key", |s| {
        s.wm.key_window() == Some(first)
    });
    assert_eq!(h.state.wm.stacking_order().last(), Some(&first));

    h.state.command(Command::Minimize {
        window: first,
        target: None,
    });
    h.wait_until("the minimized row", |_| {
        window_list(&bar).is_some_and(|l| {
            l.contains(&(
                list_id(first),
                "org.example.activate window".into(),
                MINIMIZED,
            ))
        })
    });
    assert_eq!(h.state.wm.key_window(), Some(second));
    bar.send(Cmd::ActivateWindow(list_id(first)));
    h.wait_until("the minimized window to be restored and key", |s| {
        s.wm.key_window() == Some(first)
            && s.wm
                .window(first)
                .is_some_and(|w| !w.state().is_minimized())
    });
    assert_eq!(bar.error(), None);
}

#[test]
fn activate_window_ignores_a_window_of_another_application() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let _a = AppClient::spawn(h.socket_path(), "org.example.first");
    let a = h.mapped_window(1);
    let _b = AppClient::spawn(h.socket_path(), "org.example.second");
    h.mapped_window(2);
    let b = h.state.wm.windows().find(|w| w.id != a).unwrap().id;
    h.state.command(Command::FocusWindow { window: b });
    h.wait_until("the second application's list", |_| {
        window_list(&bar).is_some_and(|l| l.len() == 1 && l[0].0 == list_id(b))
    });

    bar.send(Cmd::ActivateWindow(list_id(a)));
    h.settle_requests(&bar);
    h.advance(0.5);
    assert_eq!(h.state.wm.key_window(), Some(b));
    assert_eq!(bar.error(), None);
}

#[test]
fn activate_window_ignores_the_id_of_a_window_that_closed_since_the_list() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let (app, first, second) = two_windows(&mut h, "org.example.stale");
    h.wait_until("both windows listed", |_| {
        window_list(&bar).is_some_and(|l| l.len() == 2)
    });
    app.send(Cmd::CloseSecondWindow);
    h.wait_until("the second window to go", |s| s.wm.window(second).is_none());

    bar.send(Cmd::ActivateWindow(list_id(second)));
    h.settle_requests(&bar);
    h.advance(0.5);
    assert_eq!(h.state.wm.key_window(), Some(first));
    assert_eq!(bar.error(), None, "a stale id is not a protocol error");
}

#[test]
fn activate_window_is_ignored_while_the_session_is_locked() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let (_app, first, second) = two_windows(&mut h, "org.example.locked");
    h.wait_until("both windows listed", |_| {
        window_list(&bar).is_some_and(|l| l.len() == 2)
    });
    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());

    bar.send(Cmd::ActivateWindow(list_id(first)));
    h.settle_requests(&bar);
    h.advance(0.5);
    assert_eq!(h.state.wm.key_window(), Some(second));
    assert_eq!(h.state.wm.stacking_order().last(), Some(&second));
    assert_eq!(bar.error(), None);
}

#[test]
fn window_commands_are_ignored_while_the_session_is_locked() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let (_app, first, second) = two_windows(&mut h, "org.example.locked");
    h.wait_until("both windows listed", |_| {
        window_list(&bar).is_some_and(|l| l.len() == 2)
    });
    let locker = common::Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());

    bar.send(Cmd::WindowCommand(BarCommand::Minimize));
    bar.send(Cmd::WindowCommand(BarCommand::MinimizeAll));
    h.settle_requests(&bar);
    h.advance(0.5);
    for w in [first, second] {
        assert!(!h.state.wm.window(w).unwrap().state().is_minimized());
    }
    assert_eq!(bar.error(), None);
}

#[test]
fn the_bar_is_told_the_chords_of_the_bindings_and_the_primary_modifier() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let groups = bar.groups();
    assert!(
        matches!(groups[3].last(), Some(BarEvent::Done)),
        "the fourth group is the shortcut group: {:?}",
        groups[3]
    );
    let (chords, primary) = shortcuts(&bar).unwrap();
    assert_eq!(primary, 0, "super under the cupertino profile");
    assert_eq!(
        chords.iter().map(|(a, _)| *a).collect::<Vec<_>>(),
        (0..13).collect::<Vec<_>>(),
        "one event per bound action, in order"
    );
    let chord = |chords: &[(u32, String)], action: u32| chords[action as usize].1.clone();
    assert_eq!(chord(&chords, 6), "Super+q", "quit_app");
    assert_eq!(chord(&chords, 0), "Super+w", "close_window");
    assert_eq!(chord(&chords, 1), "Super+m", "minimize");

    // a binding of config.json: the bar is told the new chord
    let rebound = CompositorSettings {
        keybindings: Some([("Super+t".to_string(), "quit-app".to_string())].into()),
        ..CompositorSettings::default()
    };
    h.state
        .apply_compositor_change(&CompositorSettings::default(), &rebound);
    h.wait_for_groups(&bar, 5);
    let (chords, primary) = shortcuts(&bar).unwrap();
    assert_eq!(chord(&chords, 6), "Super+t");
    assert_eq!(primary, 0);

    // the classic-pc profile: Ctrl makes the primary modifier
    let classic = KeyboardSettings {
        modifier_profile: Some(ModifierProfile::ClassicPc),
        ..KeyboardSettings::default()
    };
    h.state
        .apply_keyboard_change(&KeyboardSettings::default(), &classic);
    h.wait_for_groups(&bar, 6);
    let (chords, primary) = shortcuts(&bar).unwrap();
    assert_eq!(primary, 1, "ctrl under the classic-pc profile");
    assert_eq!(chord(&chords, 6), "Ctrl+t");
    assert_eq!(bar.error(), None);
}

#[test]
fn a_version_9_bar_gets_none_of_the_version_10_events_or_commands() {
    let Some(mut h) = Harness::new() else { return };
    let bar = BarClient::spawn_at(h.shell_stream(), true, 9);
    h.wait_for_groups(&bar, 3);
    let (_app, first, _second) = two_windows(&mut h, "org.example.old");
    h.state.command(Command::Minimize {
        window: first,
        target: None,
    });
    h.state.apply_keyboard_change(
        &KeyboardSettings::default(),
        &KeyboardSettings {
            modifier_profile: Some(ModifierProfile::ClassicPc),
            ..KeyboardSettings::default()
        },
    );
    h.advance(1.0);
    h.settle_requests(&bar);
    let new = bar.groups().into_iter().flatten().find(|e| {
        matches!(
            e,
            BarEvent::AppWindow { .. } | BarEvent::Shortcut(..) | BarEvent::PrimaryModifier(_)
        )
    });
    assert_eq!(new, None);

    bar.send(Cmd::WindowCommand(BarCommand::TileLeft));
    let end = Instant::now() + Duration::from_secs(10);
    while bar.error().is_none() && Instant::now() < end {
        h.step();
    }
    assert_eq!(
        bar.error(),
        Some((
            CupertinoMenuBarV1::interface().name.to_string(),
            0 // invalid_command
        ))
    );
}

#[test]
fn full_screen_and_tiling_commands_act_on_the_active_window() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let _app = AppClient::spawn(h.socket_path(), "org.example.tiles");
    let id = h.mapped_window(1);
    let state = |h: &Harness| h.state.wm.window(id).unwrap().state();

    bar.send(Cmd::WindowCommand(BarCommand::ToggleFullscreen));
    h.wait_until("full screen", |s| {
        s.wm.window(id).unwrap().state().is_fullscreen()
    });
    h.advance(5.0);
    bar.send(Cmd::WindowCommand(BarCommand::ToggleFullscreen));
    h.wait_until("out of full screen", |s| {
        !s.wm.window(id).unwrap().state().is_fullscreen()
    });
    h.advance(5.0);
    assert_eq!(state(&h), LifecycleState::Normal);

    let out = h.state.outputs[0].id;
    let margin = h.state.wm.config().snap.tiled_margin;
    for (command, kind) in [
        (BarCommand::TileLeft, TileKind::Left),
        (BarCommand::TileRight, TileKind::Right),
        (BarCommand::TileFill, TileKind::Fill),
    ] {
        bar.send(Cmd::WindowCommand(command));
        h.wait_until("the tile", |s| {
            s.wm.window(id).unwrap().state() == LifecycleState::Tiled(kind)
                || s.wm.window(id).unwrap().state() == LifecycleState::Tiling(kind)
        });
        h.advance(5.0);
        let usable = h.state.usable_area(out);
        assert_eq!(
            h.state.wm.window(id).unwrap().geometry(),
            cupertino_wm::tile_rect(kind, usable, margin),
            "{kind:?}"
        );
    }
    assert_eq!(bar.error(), None);
}

#[test]
fn the_all_windows_commands_act_on_every_window_of_the_active_application() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let (_a, a1, a2) = two_windows(&mut h, "org.example.all");
    let _b = AppClient::spawn(h.socket_path(), "org.example.other");
    h.mapped_window(3);
    let b = h
        .state
        .wm
        .windows()
        .find(|w| ![a1, a2].contains(&w.id))
        .unwrap()
        .id;
    h.state.command(Command::FocusWindow { window: b });
    h.state.command(Command::FocusWindow { window: a1 });
    h.advance(1.0);
    assert_eq!(h.state.wm.stacking_order(), &[a2, b, a1]);

    bar.send(Cmd::WindowCommand(BarCommand::BringAllToFront));
    h.wait_until("the application's windows in front", |s| {
        s.wm.stacking_order() == [b, a2, a1]
    });
    assert_eq!(h.state.wm.key_window(), Some(a1), "the focus stays");

    let origin = h.state.wm.window(a1).unwrap().geometry();
    bar.send(Cmd::WindowCommand(BarCommand::CascadeAll));
    let behind = Rect::new(origin.x - 24.0, origin.y - 24.0, 200.0, 150.0);
    h.wait_until("the window behind to step up and left", |s| {
        s.wm.window(a2).unwrap().geometry() == behind
    });
    h.advance(5.0);
    assert_eq!(h.state.wm.window(a1).unwrap().geometry(), origin);
    assert_eq!(h.state.wm.stacking_order(), &[b, a2, a1]);

    bar.send(Cmd::WindowCommand(BarCommand::ZoomAll));
    h.wait_until("both windows to zoom", |s| {
        [a1, a2].iter().all(|w| {
            matches!(
                s.wm.window(*w).unwrap().state(),
                LifecycleState::Zooming | LifecycleState::Zoomed
            )
        })
    });
    assert_eq!(
        h.state.wm.window(b).unwrap().state(),
        LifecycleState::Normal
    );
    h.advance(5.0);

    bar.send(Cmd::WindowCommand(BarCommand::MinimizeAll));
    h.wait_until("both windows to minimize", |s| {
        [a1, a2]
            .iter()
            .all(|w| s.wm.window(*w).unwrap().state().is_minimized())
    });
    assert!(!h.state.wm.window(b).unwrap().state().is_minimized());
    assert_eq!(bar.error(), None);
}

#[test]
fn the_force_quit_command_opens_the_dialog_in_the_dock() {
    let Some(mut h) = Harness::new() else { return };
    let bar = bar_v10(&mut h);
    let dock = DockClient::spawn(&mut h);
    h.wait_until("the dock to be created", |_| dock.roundtrips() >= 2);
    bar.send(Cmd::WindowCommand(BarCommand::ForceQuitDialog));
    h.wait_until("the dialog", |_| dock.force_quit_dialogs() == 1);
    assert_eq!(bar.error(), None);
}
