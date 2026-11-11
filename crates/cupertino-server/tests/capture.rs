//! Screen capture against a headless compositor: a privileged client (inserted through `Cupertino::privileged_client`,
//! the connection the screenshot process gets) captures the output and a toplevel through `ext-image-copy-capture-v1`
//! and reads the window feed of `cupertino_screenshot_v1`; a client of the public socket sees none of those globals.
//! Server and clients run on one thread: each step dispatches the server, then every client's queue. Tests skip
//! (pass) when no EGL device is available.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod common;

use cupertino_protocols::client::screenshot::cupertino_screenshot_v1::{
    self, CupertinoScreenshotV1,
};
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1, ext_foreign_toplevel_list_v1,
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1, ext_image_capture_source_v1,
    ext_output_image_capture_source_manager_v1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1, ext_image_copy_capture_manager_v1,
    ext_image_copy_capture_session_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
const TOPLEVEL: (i32, i32) = (400, 300);
const APP_ID: &str = "org.example.captured";
/// Memory order of an Argb8888 pixel is blue, green, red, alpha.
const RED: [u8; 4] = [0, 0, 255, 255];
const BLUE: [u8; 4] = [255, 0, 0, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];
/// The globals only privileged clients see.
const CAPTURE_GLOBALS: [&str; 4] = [
    "ext_output_image_capture_source_manager_v1",
    "ext_foreign_toplevel_image_capture_source_manager_v1",
    "ext_image_copy_capture_manager_v1",
    "cupertino_screenshot_v1",
];

// ---- a client run on the test thread -------------------------------------------------------------

/// A connection whose queue the harness dispatches after every server step.
struct TestClient<S> {
    conn: Connection,
    queue: EventQueue<S>,
    state: S,
    globals: GlobalList,
}

impl<S: Dispatch<wl_registry::WlRegistry, GlobalListContents> + 'static> TestClient<S> {
    /// Connect over `stream`; the registry roundtrip runs on a helper thread because the server must dispatch it.
    fn connect(h: &mut Harness, stream: UnixStream, state: S) -> TestClient<S> {
        let conn = Connection::from_socket(stream).unwrap();
        let init = std::thread::spawn({
            let conn = conn.clone();
            move || registry_queue_init::<S>(&conn).map_err(|e| e.to_string())
        });
        while !init.is_finished() {
            h.step();
        }
        let (globals, queue) = init.join().unwrap().unwrap();
        TestClient {
            conn,
            queue,
            state,
            globals,
        }
    }

    fn qh(&self) -> QueueHandle<S> {
        self.queue.handle()
    }

    fn interfaces(&self) -> Vec<String> {
        self.globals
            .contents()
            .clone_list()
            .into_iter()
            .map(|g| g.interface)
            .collect()
    }
}

fn shm_buffer<
    S: Dispatch<wl_shm_pool::WlShmPool, ()> + Dispatch<wl_buffer::WlBuffer, ()> + 'static,
>(
    shm: &wl_shm::WlShm,
    size: (i32, i32),
    pixel: impl Fn(i32, i32) -> [u8; 4],
    qh: &QueueHandle<S>,
) -> (wl_buffer::WlBuffer, std::fs::File) {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("cupertino-capture-test", rustix::fs::MemfdFlags::CLOEXEC)
        .unwrap();
    let mut file = std::fs::File::from(fd);
    let mut bytes = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            bytes.extend_from_slice(&pixel(x, y));
        }
    }
    file.write_all(&bytes).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    (buffer, file)
}

macro_rules! ignore_events {
    ($state:ty: $($t:ty),*) => {$(
        impl Dispatch<$t, ()> for $state {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

#[path = "capture/control.rs"]
mod desktop_control;

// ---- the application: one toplevel, red above blue -------------------------------------------------

#[derive(Default)]
struct App {
    shm: Option<wl_shm::WlShm>,
    control_window: Option<cupertino_wm::WindowId>,
    control_serial: u32,
    control_keys: Vec<(u32, bool)>,
    /// The pointer buttons the window got, in order.
    control_buttons: Vec<(u32, bool)>,
    control_request: Option<(u64, u32, bool)>,
    /// The app the last consent request named: epoch, desktop entry or not, identity, name, services, remembered.
    control_app: Option<(u64, bool, String, String, u32, bool)>,
    /// Keeps the shm file of the attached buffer open.
    _file: Option<std::fs::File>,
    _window: Option<(
        xdg_wm_base::XdgWmBase,
        xdg_surface::XdgSurface,
        xdg_toplevel::XdgToplevel,
    )>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
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
ignore_events!(App: wl_compositor::WlCompositor, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_buffer::WlBuffer, wl_surface::WlSurface, xdg_toplevel::XdgToplevel);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
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

impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for App {
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
            let shm = state.shm.as_ref().unwrap();
            let (buffer, file) = shm_buffer(
                shm,
                TOPLEVEL,
                |_, y| if y < TOPLEVEL.1 / 2 { RED } else { BLUE },
                qh,
            );
            state._file = Some(file);
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, TOPLEVEL.0, TOPLEVEL.1);
            surface.commit();
        }
    }
}

/// Map the application's toplevel and let its opening animation settle; returns the client and the window frame.
fn mapped_app(h: &mut Harness) -> (TestClient<App>, cupertino_wm::Rect) {
    mapped_app_named(h, APP_ID, None)
}

/// [`mapped_app`] with another application id, and a title when one is given.
fn mapped_app_named(
    h: &mut Harness,
    app_id: &str,
    title: Option<&str>,
) -> (TestClient<App>, cupertino_wm::Rect) {
    let stream = UnixStream::connect(h.socket_path()).unwrap();
    mapped_app_stream(h, stream, app_id, title)
}

fn mapped_app_stream(
    h: &mut Harness,
    stream: UnixStream,
    app_id: &str,
    title: Option<&str>,
) -> (TestClient<App>, cupertino_wm::Rect) {
    let mut app = TestClient::connect(h, stream, App::default());
    let qh = app.qh();
    app.state.shm = Some(app.globals.bind(&qh, 1..=1, ()).unwrap());
    let compositor: wl_compositor::WlCompositor = app.globals.bind(&qh, 4..=6, ()).unwrap();
    let wm_base: xdg_wm_base::XdgWmBase = app.globals.bind(&qh, 1..=5, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_app_id(app_id.to_string());
    if let Some(title) = title {
        toplevel.set_title(title.to_string());
    }
    surface.commit();
    app.state._window = Some((wm_base, xdg, toplevel));
    h.clients_until(&mut [&mut app], "the toplevel to be mapped", |s| {
        s.wm.windows().any(|w| {
            s.entry(w.id).is_some_and(|e| e.mapped)
                && s.wm.app(w.app).is_some_and(|app| app.identity == app_id)
        })
    });
    let id = h
        .state
        .wm
        .windows()
        .find(|w| {
            h.state
                .wm
                .app(w.app)
                .is_some_and(|app| app.identity == app_id)
        })
        .unwrap()
        .id;
    h.state
        .command(cupertino_wm::Command::FocusWindow { window: id });
    let time = h.state.now() + 5.0;
    common::set_time_with(&mut h.state, time, |state| {
        app.pump();
        headless::step(state, &mut h.event_loop, Duration::from_millis(2)).unwrap();
        app.pump();
    });
    h.state.tick();
    let id = h.state.wm.key_window().unwrap();
    let frame = h.state.wm.window(id).unwrap().geometry();
    (app, frame)
}

// ---- the capturing client ---------------------------------------------------------------------------

#[derive(Default)]
struct SessionSeen {
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    done: bool,
    stopped: bool,
}

#[derive(Default)]
struct FrameSeen {
    damage: Vec<(i32, i32, i32, i32)>,
    transform: Option<wl_output::Transform>,
    presented: bool,
    ready: bool,
    failed: Option<ext_image_copy_capture_frame_v1::FailureReason>,
}

/// A window of the feed: identifier and frame `(x, y, width, height)`.
type FeedWindow = (String, (i32, i32, i32, i32));

#[derive(Default)]
struct Capturer {
    /// Foreign toplevel handles with their app id and identifier.
    toplevels: Vec<(
        ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        String,
        String,
    )>,
    /// Every title the handles were sent, in order.
    titles: Vec<String>,
    control_seen: desktop_control::Seen,
    /// What each run of a connection holding several sessions was told, by the session's slot.
    control_runs: Vec<desktop_control::Seen>,
    session: SessionSeen,
    frame: FrameSeen,
    /// The window feed's last complete group, and the group being received.
    windows: Option<Vec<FeedWindow>>,
    windows_pending: Vec<FeedWindow>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Capturer {
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
ignore_events!(Capturer:
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_output::WlOutput,
    ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
    ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1
);

impl Dispatch<ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, ()> for Capturer {
    fn event(
        state: &mut Self,
        _: &ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            state
                .toplevels
                .push((toplevel, String::new(), String::new()));
        }
    }

    event_created_child!(Capturer, ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1, ()> for Capturer {
    fn event(
        state: &mut Self,
        handle: &ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let entry = state
            .toplevels
            .iter_mut()
            .find(|(h, _, _)| h == handle)
            .unwrap();
        match event {
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => entry.1 = app_id,
            ext_foreign_toplevel_handle_v1::Event::Identifier { identifier } => {
                entry.2 = identifier
            }
            ext_foreign_toplevel_handle_v1::Event::Title { title } => state.titles.push(title),
            _ => {}
        }
    }
}

impl Dispatch<ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, ()> for Capturer {
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let s = &mut state.session;
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => {
                s.size = Some((width, height))
            }
            ext_image_copy_capture_session_v1::Event::ShmFormat { format } => {
                s.formats.push(format.into_result().unwrap())
            }
            ext_image_copy_capture_session_v1::Event::Done => s.done = true,
            ext_image_copy_capture_session_v1::Event::Stopped => s.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, ()> for Capturer {
    fn event(
        state: &mut Self,
        _: &ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let f = &mut state.frame;
        match event {
            ext_image_copy_capture_frame_v1::Event::Damage {
                x,
                y,
                width,
                height,
            } => f.damage.push((x, y, width, height)),
            ext_image_copy_capture_frame_v1::Event::Transform { transform } => {
                f.transform = Some(transform.into_result().unwrap())
            }
            ext_image_copy_capture_frame_v1::Event::PresentationTime { .. } => f.presented = true,
            ext_image_copy_capture_frame_v1::Event::Ready => f.ready = true,
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => {
                f.failed = Some(reason.into_result().unwrap())
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoScreenshotV1, ()> for Capturer {
    fn event(
        state: &mut Self,
        _: &CupertinoScreenshotV1,
        event: cupertino_screenshot_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_screenshot_v1::Event::Window {
                identifier,
                x,
                y,
                width,
                height,
            } => state
                .windows_pending
                .push((identifier, (x, y, width, height))),
            cupertino_screenshot_v1::Event::Done => {
                state.windows = Some(std::mem::take(&mut state.windows_pending))
            }
            _ => {}
        }
    }
}

/// A capture session in progress: the session and its shm buffer, mapped for reading.
struct Capture {
    session: ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
    buffer: wl_buffer::WlBuffer,
    file: std::fs::File,
    size: (i32, i32),
}

impl Capture {
    /// The buffer's pixel at `(x, y)`, blue, green, red, alpha.
    fn pixel(&self, x: i32, y: i32) -> [u8; 4] {
        use std::os::unix::fs::FileExt;
        let mut p = [0u8; 4];
        self.file
            .read_exact_at(&mut p, ((y * self.size.0 + x) * 4) as u64)
            .unwrap();
        p
    }

    /// Every pixel of the buffer, row by row.
    fn pixels(&self) -> Vec<[u8; 4]> {
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0u8; (self.size.0 * self.size.1 * 4) as usize];
        self.file.read_exact_at(&mut bytes, 0).unwrap();
        bytes.as_chunks::<4>().0.to_vec()
    }
}

impl TestClient<Capturer> {
    fn privileged(h: &mut Harness) -> TestClient<Capturer> {
        let stream = h.state.privileged_client().unwrap();
        TestClient::connect(h, stream, Capturer::default())
    }

    /// Start a session on `source` and allocate a buffer of the size it announces.
    fn session(
        &mut self,
        h: &mut Harness,
        source: &ext_image_capture_source_v1::ExtImageCaptureSourceV1,
        paint_cursors: bool,
    ) -> Capture {
        let qh = self.qh();
        let manager: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 =
            self.globals.bind(&qh, 1..=1, ()).unwrap();
        let options = if paint_cursors {
            ext_image_copy_capture_manager_v1::Options::PaintCursors
        } else {
            ext_image_copy_capture_manager_v1::Options::empty()
        };
        self.state.session = SessionSeen::default();
        let session = manager.create_session(source, options, &qh, ());
        h.pump_until(self, "the buffer constraints", |c| {
            c.session.done || c.session.stopped
        });
        assert!(!self.state.session.stopped, "the session stopped");
        assert!(
            self.state
                .session
                .formats
                .contains(&wl_shm::Format::Argb8888),
            "formats {:?}",
            self.state.session.formats
        );
        let (w, h_px) = self.state.session.size.unwrap();
        let size = (w as i32, h_px as i32);
        let shm: wl_shm::WlShm = self.globals.bind(&qh, 1..=1, ()).unwrap();
        let (buffer, file) = shm_buffer(&shm, size, |_, _| [0, 0, 0, 0], &qh);
        Capture {
            session,
            buffer,
            file,
            size,
        }
    }

    /// Ask for a frame into the capture's buffer.
    fn request_frame(
        &mut self,
        capture: &Capture,
    ) -> ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1 {
        let qh = self.qh();
        self.state.frame = FrameSeen::default();
        let frame = capture.session.create_frame(&qh, ());
        frame.attach_buffer(&capture.buffer);
        frame.damage_buffer(0, 0, capture.size.0, capture.size.1);
        frame.capture();
        frame
    }

    fn output_source(&mut self) -> ext_image_capture_source_v1::ExtImageCaptureSourceV1 {
        let qh = self.qh();
        let output: wl_output::WlOutput = self.globals.bind(&qh, 1..=4, ()).unwrap();
        let manager: ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1 = self.globals.bind(&qh, 1..=1, ()).unwrap();
        manager.create_source(&output, &qh, ())
    }
}

// ---- harness ----------------------------------------------------------------------------------------

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

trait Pump {
    /// Send what is queued, read what arrived and dispatch it, without blocking.
    fn pump(&mut self);
}

impl<S> Pump for TestClient<S> {
    fn pump(&mut self) {
        self.conn.flush().unwrap();
        if let Some(guard) = self.queue.prepare_read() {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("reading from the server: {e}"),
            }
        }
        self.queue.dispatch_pending(&mut self.state).unwrap();
    }
}

impl Harness {
    fn new() -> Option<Harness> {
        Harness::with_output(OUTPUT, 1.0)
    }

    /// A server with a headless output of `size` pixels at `scale`.
    fn with_output(size: (i32, i32), scale: f64) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        match headless::init(&mut state, size, scale) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format_args!("{e:#}"));
                None
            }
        }
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Step the server and the clients until `cond` holds on the server; panics after [`common::HANG_GUARD`].
    fn clients_until(
        &mut self,
        clients: &mut [&mut dyn Pump],
        what: &str,
        mut cond: impl FnMut(&Cupertino) -> bool,
    ) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            for c in clients.iter_mut() {
                c.pump();
            }
            self.step();
            for c in clients.iter_mut() {
                c.pump();
            }
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Step the server and one client until `cond` holds on the client's state; panics after [`common::HANG_GUARD`].
    fn pump_until<S>(
        &mut self,
        client: &mut TestClient<S>,
        what: &str,
        mut cond: impl FnMut(&S) -> bool,
    ) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            client.pump();
            self.step();
            client.pump();
            if cond(&client.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Step `steps` times, pumping the client.
    fn idle<S>(&mut self, client: &mut TestClient<S>, steps: usize) {
        for _ in 0..steps {
            client.pump();
            self.step();
            client.pump();
        }
    }
}

// ---- tests ------------------------------------------------------------------------------------------

#[test]
fn a_client_of_the_public_socket_sees_no_capture_global() {
    let Some(mut h) = Harness::new() else { return };
    let stream = UnixStream::connect(h.socket_path()).unwrap();
    let public = TestClient::connect(&mut h, stream, App::default());
    let privileged = TestClient::privileged(&mut h);
    for name in CAPTURE_GLOBALS {
        assert!(
            !public.interfaces().iter().any(|i| i == name),
            "the public client sees {name}"
        );
        assert!(
            privileged.interfaces().iter().any(|i| i == name),
            "the privileged client does not see {name}"
        );
    }
}

#[test]
fn the_output_is_captured_at_its_mode_size_with_full_damage_and_the_cursor_only_on_request() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, frame) = mapped_app(&mut h);
    // the pointer rests on the blue half of the window
    let pointer = (frame.x + frame.w / 2.0, frame.y + frame.h * 0.75);
    h.state.pointer_moved_to(pointer.into(), 0, None);
    h.idle(&mut app, 5);
    let mut c = TestClient::privileged(&mut h);

    let source = c.output_source();
    let capture = c.session(&mut h, &source, false);
    assert_eq!(capture.size, OUTPUT);
    let _frame = c.request_frame(&capture);
    h.pump_until(&mut c, "the first output frame", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    assert_eq!(
        c.state.frame.damage,
        vec![(0, 0, OUTPUT.0, OUTPUT.1)],
        "the first frame is all damage"
    );
    assert_eq!(c.state.frame.transform, Some(wl_output::Transform::Normal));
    assert!(c.state.frame.presented);
    let (px, py) = (pointer.0 as i32, pointer.1 as i32);
    assert_eq!(
        capture.pixel(px, (frame.y + frame.h * 0.25) as i32),
        RED,
        "the window's upper half"
    );
    // the cursor's arrow extends below and to the right of the pointer
    let cursor_area: Vec<[u8; 4]> = (0..8)
        .flat_map(|d| {
            [
                capture.pixel(px + 2, py + 2 + d),
                capture.pixel(px + 1 + d, py + 4),
            ]
        })
        .collect();
    assert!(
        cursor_area.iter().all(|p| *p == BLUE),
        "no cursor without paint_cursors: {cursor_area:?}"
    );

    let with_cursor = c.session(&mut h, &source, true);
    let _frame = c.request_frame(&with_cursor);
    h.pump_until(&mut c, "the frame with the cursor", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    let cursor_area: Vec<[u8; 4]> = (0..8)
        .flat_map(|d| {
            [
                with_cursor.pixel(px + 2, py + 2 + d),
                with_cursor.pixel(px + 1 + d, py + 4),
            ]
        })
        .collect();
    assert!(
        cursor_area.iter().any(|p| *p != BLUE),
        "paint_cursors draws the cursor"
    );
}

#[test]
fn a_quarter_turned_output_is_captured_upright_as_it_is_shown() {
    let Some(mut h) = Harness::new() else { return };
    let id = h.state.outputs[0].id;
    h.state.configure_output(
        id,
        smithay::output::Mode {
            size: OUTPUT.into(),
            refresh: 60_000,
        },
        1.0,
        smithay::utils::Transform::_90,
        (0, 0),
    );
    let (_app, frame) = mapped_app(&mut h);
    let mut c = TestClient::privileged(&mut h);
    let source = c.output_source();
    let capture = c.session(&mut h, &source, false);
    assert_eq!(
        capture.size,
        (OUTPUT.1, OUTPUT.0),
        "the buffer is the output as shown, its mode on its side"
    );
    let _frame = c.request_frame(&capture);
    h.pump_until(&mut c, "the frame of the turned output", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    assert_eq!(
        c.state.frame.transform,
        Some(wl_output::Transform::Normal),
        "the buffer is upright"
    );
    let x = (frame.x + frame.w / 2.0) as i32;
    assert_eq!(
        capture.pixel(x, (frame.y + frame.h * 0.25) as i32),
        RED,
        "the window's upper half, where it is shown"
    );
    assert_eq!(
        capture.pixel(x, (frame.y + frame.h * 0.75) as i32),
        BLUE,
        "the window's lower half"
    );
}

#[test]
fn a_second_frame_waits_for_a_change_and_reports_only_the_changed_area() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, frame) = mapped_app(&mut h);
    h.state
        .pointer_moved_to((frame.x + 20.0, frame.y + 200.0).into(), 0, None);
    h.idle(&mut app, 5);
    let mut c = TestClient::privileged(&mut h);
    let source = c.output_source();
    let capture = c.session(&mut h, &source, true);
    let first = c.request_frame(&capture);
    h.pump_until(&mut c, "the first frame", |s| s.frame.ready);
    first.destroy();

    let _second = c.request_frame(&capture);
    h.state.request_redraw();
    h.idle(&mut c, 30);
    assert!(
        !c.state.frame.ready && c.state.frame.failed.is_none(),
        "an unchanged output keeps the frame waiting"
    );

    let moved = (frame.x + 300.0, frame.y + 60.0);
    h.state.pointer_moved_to(moved.into(), 1, None);
    h.pump_until(&mut c, "the frame after the pointer moved", |s| {
        s.frame.ready
    });
    let damage = c.state.frame.damage.clone();
    assert_eq!(damage.len(), 1);
    let (x, y, w, dh) = damage[0];
    assert!(
        w < OUTPUT.0 && dh < OUTPUT.1,
        "only the cursor's old and new places changed: {damage:?}"
    );
    let contains = |p: (f64, f64)| {
        (x..x + w).contains(&(p.0 as i32 + 2)) && (y..y + dh).contains(&(p.1 as i32 + 2))
    };
    assert!(
        contains(moved) && contains((frame.x + 20.0, frame.y + 200.0)),
        "the damage covers both cursor places: {damage:?}"
    );
    assert_eq!(capture.pixel(moved.0 as i32 - 40, moved.1 as i32), RED);
}

#[test]
fn a_foreign_toplevel_title_and_application_id_longer_than_the_limit_are_cut_at_a_character_boundary(
) {
    let Some(mut h) = Harness::new() else { return };
    // 4000 bytes, and a two-byte character spans byte 1024
    let long = format!("{}é{}", "a".repeat(1023), "b".repeat(4000 - 1025));
    let (mut app, _) = mapped_app_named(&mut h, &long, Some(&long));
    let mut c = TestClient::privileged(&mut h);
    let qh = c.qh();
    let _list: ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 =
        c.globals.bind(&qh, 1..=1, ()).unwrap();
    let cut = "a".repeat(1023);
    h.pump_until(&mut c, "the cut identity of the toplevel", |s| {
        s.toplevels.iter().any(|t| t.1 == cut) && s.titles.contains(&cut)
    });
    // a later change is cut the same way
    let changed = format!("{}é{}", "c".repeat(1023), "d".repeat(2000));
    app.state._window.as_ref().unwrap().2.set_title(changed);
    app.pump();
    let cut_changed = "c".repeat(1023);
    h.pump_until(&mut c, "the changed title", |s| {
        s.titles.contains(&cut_changed)
    });
    assert!(c.state.titles.iter().all(|t| t.len() <= 1024));
    // so is a later change of the application id
    let changed_id = format!("{}é{}", "e".repeat(1023), "f".repeat(2000));
    app.state._window.as_ref().unwrap().2.set_app_id(changed_id);
    app.pump();
    h.pump_until(&mut c, "the changed application id", |s| {
        s.toplevels.iter().any(|t| t.1 == "e".repeat(1023))
    });
}

#[test]
fn a_toplevel_is_captured_at_its_frame_size_and_listed_in_the_window_feed() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, frame) = mapped_app(&mut h);
    let mut c = TestClient::privileged(&mut h);
    let qh = c.qh();
    let feed: CupertinoScreenshotV1 = c.globals.bind(&qh, 1..=1, ()).unwrap();
    let list: ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 =
        c.globals.bind(&qh, 1..=1, ()).unwrap();
    h.pump_until(&mut c, "the foreign toplevel and the window feed", |s| {
        s.toplevels.iter().any(|t| t.1 == APP_ID && !t.2.is_empty()) && s.windows.is_some()
    });
    app.pump();
    let (handle, _, identifier) = c
        .state
        .toplevels
        .iter()
        .find(|t| t.1 == APP_ID)
        .unwrap()
        .clone();
    let expected_frame = (
        frame.x as i32,
        frame.y as i32,
        frame.w as i32,
        frame.h as i32,
    );
    assert_eq!(
        c.state.windows.as_deref(),
        Some(&[(identifier.clone(), expected_frame)][..]),
        "the feed lists the window with its frame"
    );

    let manager: ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 = c.globals.bind(&qh, 1..=1, ()).unwrap();
    let source = manager.create_source(&handle, &qh, ());
    let capture = c.session(&mut h, &source, false);
    assert_eq!(
        capture.size,
        (frame.w as i32, frame.h as i32),
        "a toplevel is captured at its frame size"
    );
    let _frame = c.request_frame(&capture);
    h.pump_until(&mut c, "the toplevel frame", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    let (w, hh) = capture.size;
    assert_eq!(capture.pixel(w / 2, hh / 4), RED, "the top half is on top");
    assert_eq!(capture.pixel(w / 2, hh * 3 / 4), BLUE);
    drop((feed, list));
}

/// Lock the session through a lock screen client and wait until it is locked.
fn locked(h: &mut Harness) -> common::Client {
    let locker = common::Client::locker(common::session_agent(&h.state));
    let end = std::time::Instant::now() + common::HANG_GUARD;
    while !locker.locked() && std::time::Instant::now() < end {
        h.step();
    }
    assert!(locker.locked(), "the session lock was not granted");
    locker
}

/// Ask for a frame of `capture` and wait until it is ready, failed or the session stopped.
fn next_frame(
    h: &mut Harness,
    c: &mut TestClient<Capturer>,
    capture: &Capture,
    what: &str,
) -> ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1 {
    let frame = c.request_frame(capture);
    h.state.request_redraw();
    h.pump_until(c, what, |s| {
        s.frame.ready || s.frame.failed.is_some() || s.session.stopped
    });
    frame
}

/// SEC-05: under the lock an output session goes on and every frame is black: neither the lock screen nor what it
/// covers reaches the buffer. After the unlock the same session shows the desktop again.
#[test]
fn under_the_lock_an_output_session_goes_on_and_is_copied_black() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, frame) = mapped_app(&mut h);
    h.idle(&mut app, 5);
    let red = (
        (frame.x + frame.w / 2.0) as i32,
        (frame.y + frame.h * 0.25) as i32,
    );
    let mut c = TestClient::privileged(&mut h);
    let source = c.output_source();
    let capture = c.session(&mut h, &source, true);
    next_frame(&mut h, &mut c, &capture, "the frame before the lock").destroy();
    assert_eq!(capture.pixel(red.0, red.1), RED);

    let locker = locked(&mut h);
    let second = next_frame(&mut h, &mut c, &capture, "the frame while locked");
    assert!(!c.state.session.stopped, "the lock stopped the session");
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    assert!(h.state.capture_running());
    assert!(
        capture.pixels().iter().all(|p| *p == BLACK),
        "a frame of a locked output that is not all black"
    );

    second.destroy();
    locker.unlock();
    h.idle(&mut c, 40);
    next_frame(&mut h, &mut c, &capture, "the frame after the unlock");
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    assert_eq!(
        capture.pixel(red.0, red.1),
        RED,
        "the desktop after the unlock"
    );
}

/// SEC-05: a console switch away and back while the session is locked does not open the screen to capture, and a
/// session made while locked is served black frames.
#[test]
fn a_session_resumed_or_made_while_locked_is_copied_black() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, _) = mapped_app(&mut h);
    h.idle(&mut app, 5);
    let mut c = TestClient::privileged(&mut h);
    let source = c.output_source();
    let capture = c.session(&mut h, &source, true);
    let locker = locked(&mut h);
    // the lock screen is drawn and the fade from black after the resume is over, so only the black copy keeps its
    // colour out of the frames below
    h.clients_until(&mut [&mut c], "the lock surface to be drawn", |s| {
        s.lock_surface_of(0).is_some_and(|surface| {
            with_renderer_surface_state(&surface, |state| state.buffer().is_some()).unwrap_or(false)
        })
    });

    h.state.session_activated();
    let after_fade = h.state.now() + 1.0;
    let event_loop = &mut h.event_loop;
    common::set_time_with(&mut h.state, after_fade, |state| {
        app.pump();
        headless::step(state, event_loop, Duration::from_millis(2)).unwrap();
        app.pump();
    });
    assert_eq!(h.state.activation_overlay_alpha(), None);
    next_frame(
        &mut h,
        &mut c,
        &capture,
        "the frame asked for after the resume",
    );
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    assert!(
        capture.pixels().iter().all(|p| *p == BLACK),
        "a frame after the resume that is not all black"
    );
    assert!(
        h.state.session_lock.is_active(),
        "the resume ended the lock"
    );

    let late = c.session(&mut h, &source, false);
    assert_eq!(
        late.size, OUTPUT,
        "a session made while locked gets its constraints"
    );
    next_frame(
        &mut h,
        &mut c,
        &late,
        "the frame of a session made while locked",
    );
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    assert!(late.pixels().iter().all(|p| *p == BLACK));
    locker.unlock();
}

/// SEC-05: under the lock a window session goes on and its frame waits: it is copied after the unlock.
#[test]
fn under_the_lock_a_window_frame_waits_for_the_unlock() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, _) = mapped_app(&mut h);
    let mut c = TestClient::privileged(&mut h);
    let qh = c.qh();
    let list: ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 =
        c.globals.bind(&qh, 1..=1, ()).unwrap();
    h.pump_until(&mut c, "the foreign toplevel", |s| {
        s.toplevels.iter().any(|t| t.1 == APP_ID)
    });
    app.pump();
    let handle = c
        .state
        .toplevels
        .iter()
        .find(|t| t.1 == APP_ID)
        .unwrap()
        .0
        .clone();
    let manager: ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 = c.globals.bind(&qh, 1..=1, ()).unwrap();
    let source = manager.create_source(&handle, &qh, ());
    let capture = c.session(&mut h, &source, false);
    let locker = locked(&mut h);

    let _frame = c.request_frame(&capture);
    h.state.request_redraw();
    h.idle(&mut c, 40);
    assert!(
        !c.state.frame.ready && c.state.frame.failed.is_none() && !c.state.session.stopped,
        "a window frame under the lock was answered: ready {}, failed {:?}, stopped {}",
        c.state.frame.ready,
        c.state.frame.failed,
        c.state.session.stopped
    );

    locker.unlock();
    h.pump_until(&mut c, "the window frame after the unlock", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    let (w, hh) = capture.size;
    assert_eq!(capture.pixel(w / 2, hh / 4), RED);
    drop(list);
}

/// The authentication prompt is never captured: where its overlay surface is shown, a capture has the pixels it had
/// before the prompt came.
#[test]
fn the_authentication_prompt_is_not_in_an_output_capture() {
    let Some(mut h) = Harness::new() else { return };
    let mut c = TestClient::privileged(&mut h);
    let source = c.output_source();
    let before = c.session(&mut h, &source, false);
    next_frame(&mut h, &mut c, &before, "the frame before the prompt");
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);

    let _prompt = common::Client::prompt(common::prompt_agent(&h.state));
    h.clients_until(&mut [&mut c], "the prompt to be shown", |s| {
        s.prompt_shown()
    });
    let after = c.session(&mut h, &source, false);
    next_frame(&mut h, &mut c, &after, "the frame with the prompt");
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    // the prompt's 120 x 80 points in the top left corner
    let area: Vec<(i32, i32)> = (0..80)
        .flat_map(|y| (0..120).map(move |x| (x, y)))
        .collect();
    assert!(
        area.iter().all(|&(x, y)| after.pixel(x, y) != BLUE),
        "the prompt's pixels are in the capture"
    );
    assert!(area
        .iter()
        .all(|&(x, y)| after.pixel(x, y) == before.pixel(x, y)));
}

/// The prompt draws on layer surfaces only: a toplevel it asks for ends its connection and never becomes a window that a
/// capture or the window list could show.
#[test]
fn a_toplevel_of_the_authentication_prompt_ends_its_connection_and_is_no_window() {
    let Some(mut h) = Harness::new() else { return };
    let stream = common::prompt_agent(&h.state);
    let mut prompt = TestClient::connect(&mut h, stream, App::default());
    let qh = prompt.qh();
    prompt.state.shm = Some(prompt.globals.bind(&qh, 1..=1, ()).unwrap());
    let compositor: wl_compositor::WlCompositor = prompt.globals.bind(&qh, 4..=6, ()).unwrap();
    let wm_base: xdg_wm_base::XdgWmBase = prompt.globals.bind(&qh, 1..=5, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title("Authenticate".to_string());
    surface.commit();
    let end = Instant::now() + common::HANG_GUARD;
    while prompt.conn.protocol_error().is_none() {
        assert!(Instant::now() < end, "the prompt's toplevel was accepted");
        let _ = prompt.conn.flush();
        h.step();
        if let Some(guard) = prompt.queue.prepare_read() {
            let _ = guard.read();
        }
        let _ = prompt.queue.dispatch_pending(&mut prompt.state);
    }
    assert_eq!(
        prompt.conn.protocol_error().unwrap().object_interface,
        "xdg_toplevel"
    );
    for _ in 0..5 {
        h.step();
    }
    assert_eq!(h.state.wm.windows().count(), 0, "a window of the prompt");
}

/// The portal's connection holds Capture alone: it sees the capture globals and the window list but not the shell
/// protocol, and it captures a window.
#[test]
fn a_connection_with_the_capture_role_alone_captures_a_window() {
    let Some(mut h) = Harness::new() else { return };
    let (_app, frame) = mapped_app(&mut h);
    let (stream, _holder) = common::held_shell_connection(
        &h.state,
        &[cupertino_server::authorization::ShellRole::Capture],
    );
    let mut c = TestClient::connect(&mut h, stream, Capturer::default());
    let interfaces = c.interfaces();
    for name in CAPTURE_GLOBALS
        .into_iter()
        .chain(["ext_foreign_toplevel_list_v1"])
    {
        assert!(
            interfaces.iter().any(|i| i == name),
            "{name} is not offered"
        );
    }
    assert!(!interfaces.iter().any(|i| i == "cupertino_shell_v1"));

    let qh = c.qh();
    let _list: ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1 =
        c.globals.bind(&qh, 1..=1, ()).unwrap();
    h.pump_until(&mut c, "the foreign toplevel", |s| {
        s.toplevels.iter().any(|t| t.1 == APP_ID)
    });
    let handle = c.state.toplevels[0].0.clone();
    let manager: ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1 = c.globals.bind(&qh, 1..=1, ()).unwrap();
    let source = manager.create_source(&handle, &qh, ());
    let capture = c.session(&mut h, &source, false);
    assert_eq!(capture.size, (frame.w as i32, frame.h as i32));
    next_frame(&mut h, &mut c, &capture, "the window frame");
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    assert_eq!(capture.pixel(capture.size.0 / 2, capture.size.1 / 4), RED);
}

/// A `sleep` process holding a connection with the Capture role.
fn holder_with_capture_role(h: &Harness) -> (std::process::Child, UnixStream) {
    let process = std::process::Command::new("sleep")
        .arg("60")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let (stream, state) = cupertino_server::shell_connection::connect_bound_client(
        &h.state.display_handle,
        cupertino_server::shell_connection::shell_process_state(&[
            cupertino_server::authorization::ShellRole::Capture,
        ]),
    )
    .unwrap();
    cupertino_server::shell_connection::bind_holder(&state, process.id()).unwrap();
    (process, stream)
}

/// SEC-05: a capture session whose connection no longer holds the Capture role (its process exited) is stopped, and a
/// frame it asks for fails, even though the connection is still open.
#[test]
fn a_session_of_a_process_that_exited_is_stopped() {
    let Some(mut h) = Harness::new() else { return };
    let (mut process, stream) = holder_with_capture_role(&h);
    let mut c = TestClient::connect(&mut h, stream, Capturer::default());
    let source = c.output_source();
    let capture = c.session(&mut h, &source, true);
    let first = c.request_frame(&capture);
    h.pump_until(&mut c, "the first frame", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    first.destroy();

    process.kill().unwrap();
    process.wait().unwrap();
    let _second = c.request_frame(&capture);
    h.pump_until(&mut c, "the frame of the exited process", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(
        !c.state.frame.ready,
        "a frame was delivered after the process exited"
    );
    assert_eq!(
        c.state.frame.failed,
        Some(ext_image_copy_capture_frame_v1::FailureReason::Stopped)
    );
    assert!(c.state.session.stopped);
    assert!(!h.state.capture_running());
}

#[test]
fn a_frame_already_waiting_when_the_process_exits_is_stopped_at_the_next_render() {
    let Some(mut h) = Harness::new() else { return };
    let (mut process, stream) = holder_with_capture_role(&h);
    let mut c = TestClient::connect(&mut h, stream, Capturer::default());
    let source = c.output_source();
    let capture = c.session(&mut h, &source, true);
    let first = c.request_frame(&capture);
    h.pump_until(&mut c, "the first frame", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
    first.destroy();

    let _waiting = c.request_frame(&capture);
    h.state.request_redraw();
    h.idle(&mut c, 30);
    assert!(
        !c.state.frame.ready && c.state.frame.failed.is_none(),
        "an unchanged output keeps the frame waiting"
    );

    process.kill().unwrap();
    process.wait().unwrap();
    h.state.pointer_moved_to((300.0, 200.0).into(), 1, None);
    h.pump_until(&mut c, "the render after the process exited", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(
        !c.state.frame.ready,
        "a waiting frame was delivered after the process exited"
    );
    assert_eq!(
        c.state.frame.failed,
        Some(ext_image_copy_capture_frame_v1::FailureReason::Stopped)
    );
    assert!(!h.state.capture_running());
}

/// The server's time for the steps that draw one pointer move, `moves` times, with an output frame captured on each
/// move or not: the pointer moves each step, so every step draws the output.
fn move_steps(
    h: &mut Harness,
    c: &mut TestClient<Capturer>,
    capture: Option<&Capture>,
    moves: u32,
) -> Vec<Duration> {
    let mut times = Vec::new();
    for i in 0..moves {
        let x = 100.0 + f64::from(i % 20) * 30.0;
        h.state.pointer_moved_to((x, 300.0).into(), i, None);
        let frame = capture.map(|capture| c.request_frame(capture));
        c.pump();
        let mut time = Duration::ZERO;
        loop {
            let start = Instant::now();
            headless::step(&mut h.state, &mut h.event_loop, Duration::ZERO).unwrap();
            time += start.elapsed();
            c.pump();
            if frame.is_none() || c.state.frame.ready {
                break;
            }
            assert!(c.state.frame.failed.is_none(), "the frame failed");
        }
        if let Some(frame) = frame {
            frame.destroy();
        }
        times.push(time);
    }
    times
}

fn median_ms(mut times: Vec<Duration>) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1e3
}

/// The cost of capturing the output on each of 60 pointer moves at 1080p @1 and 4K @2, against the same moves drawn
/// without a capture: medians in ms. A captured frame draws the output again into an image, reads it back, compares it
/// with the session's last frame and copies it into the client's buffer.
#[test]
#[ignore = "timing matrix; run with --release --ignored --nocapture"]
fn capture_matrix() {
    const MOVES: u32 = 60;
    println!("output | scale | frame MB | move ms | move + capture ms | capture ms");
    for (size, scale) in [((1920, 1080), 1.0), ((3840, 2160), 2.0)] {
        let Some(mut h) = Harness::with_output(size, scale) else {
            return;
        };
        let (mut app, _) = mapped_app(&mut h);
        h.idle(&mut app, 5);
        let mut c = TestClient::privileged(&mut h);
        let source = c.output_source();
        let capture = c.session(&mut h, &source, true);
        assert_eq!(capture.size, size);
        move_steps(&mut h, &mut c, Some(&capture), 5);
        let plain = median_ms(move_steps(&mut h, &mut c, None, MOVES));
        let captured = median_ms(move_steps(&mut h, &mut c, Some(&capture), MOVES));
        let bytes = f64::from(size.0) * f64::from(size.1) * 4.0;
        println!(
            "{}x{} | {scale} | {:.1} | {plain:.2} | {captured:.2} | {:.2}",
            size.0,
            size.1,
            bytes / 1e6,
            captured - plain
        );
    }
}
