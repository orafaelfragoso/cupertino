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

// ---- the application: one toplevel, red above blue -------------------------------------------------

#[derive(Default)]
struct App {
    shm: Option<wl_shm::WlShm>,
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
    let stream = UnixStream::connect(h.socket_path()).unwrap();
    let mut app = TestClient::connect(h, stream, App::default());
    let qh = app.qh();
    app.state.shm = Some(app.globals.bind(&qh, 1..=1, ()).unwrap());
    let compositor: wl_compositor::WlCompositor = app.globals.bind(&qh, 4..=6, ()).unwrap();
    let wm_base: xdg_wm_base::XdgWmBase = app.globals.bind(&qh, 1..=5, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_app_id(APP_ID.to_string());
    surface.commit();
    app.state._window = Some((wm_base, xdg, toplevel));
    h.clients_until(&mut [&mut app], "the toplevel to be mapped", |s| {
        s.wm.key_window()
            .is_some_and(|w| s.entry(w).is_some_and(|e| e.mapped))
    });
    h.state.fixed_time = Some(h.state.now() + 5.0);
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
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
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
            Duration::from_millis(2),
        )
        .unwrap();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Step the server and the clients until `cond` holds on the server; panics after ten seconds.
    fn clients_until(
        &mut self,
        clients: &mut [&mut dyn Pump],
        what: &str,
        mut cond: impl FnMut(&Cupertino) -> bool,
    ) {
        let end = Instant::now() + Duration::from_secs(10);
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

    /// Step the server and one client until `cond` holds on the client's state; panics after ten seconds.
    fn pump_until<S>(
        &mut self,
        client: &mut TestClient<S>,
        what: &str,
        mut cond: impl FnMut(&S) -> bool,
    ) {
        let end = Instant::now() + Duration::from_secs(10);
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

/// SEC-05: locking the session ends every capture session: it is told `stopped`, a frame asked for while locked fails,
/// and the session does not resume at the unlock (a new one does).
#[test]
fn locking_the_session_stops_the_capture_sessions_for_good() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, frame) = mapped_app(&mut h);
    h.state
        .pointer_moved_to((frame.x + 20.0, frame.y + 200.0).into(), 0, None);
    h.idle(&mut app, 5);
    let mut c = TestClient::privileged(&mut h);
    let source = c.output_source();
    let capture = c.session(&mut h, &source, true);
    let first = c.request_frame(&capture);
    h.pump_until(&mut c, "the first frame before the lock", |s| s.frame.ready);
    first.destroy();
    c.state.frame = Default::default();

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.pump_until(&mut c, "the session to be stopped by the lock", |s| {
        s.session.stopped
    });
    for _ in 0..200 {
        if locker.locked() {
            break;
        }
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(locker.locked(), "the session lock was not granted");
    assert!(
        !h.state.capture_running(),
        "a stopped session still shows as running"
    );

    let second = c.request_frame(&capture);
    h.state
        .pointer_moved_to((frame.x + 300.0, frame.y + 60.0).into(), 1, None);
    h.state.request_redraw();
    h.pump_until(&mut c, "the frame asked for while locked", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(
        !c.state.frame.ready,
        "a frame was delivered while the session was locked"
    );
    assert_eq!(
        c.state.frame.failed,
        Some(ext_image_copy_capture_frame_v1::FailureReason::Stopped)
    );

    second.destroy();
    locker.unlock();
    h.idle(&mut c, 40);
    let third = c.request_frame(&capture);
    h.pump_until(&mut c, "the frame after the unlock", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(
        !c.state.frame.ready,
        "a session stopped by the lock delivered after the unlock"
    );

    third.destroy();
    let renewed = c.session(&mut h, &source, true);
    let _fourth = c.request_frame(&renewed);
    h.pump_until(&mut c, "a new session after the unlock", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(c.state.frame.ready, "failed: {:?}", c.state.frame.failed);
}

/// SEC-05: a console switch away and back while the session is locked does not open the screen to capture, and a
/// session made while locked is stopped at once.
#[test]
fn a_session_resumed_while_locked_still_delivers_no_capture_buffer() {
    let Some(mut h) = Harness::new() else { return };
    let (mut app, frame) = mapped_app(&mut h);
    h.state
        .pointer_moved_to((frame.x + 20.0, frame.y + 200.0).into(), 0, None);
    h.idle(&mut app, 5);
    let mut c = TestClient::privileged(&mut h);
    let source = c.output_source();
    let capture = c.session(&mut h, &source, true);

    let locker = common::Client::locker(common::session_agent(&h.state));
    h.pump_until(&mut c, "the session to be stopped by the lock", |s| {
        s.session.stopped
    });
    for _ in 0..200 {
        if locker.locked() {
            break;
        }
        h.step();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(locker.locked(), "the session lock was not granted");

    h.state.session_activated();
    let _frame = c.request_frame(&capture);
    h.state.request_redraw();
    h.pump_until(&mut c, "the frame asked for after the resume", |s| {
        s.frame.ready || s.frame.failed.is_some()
    });
    assert!(
        !c.state.frame.ready,
        "a frame was delivered after the session resumed while locked"
    );
    assert!(
        h.state.session_lock.is_active(),
        "the resume ended the lock"
    );

    c.state.session = SessionSeen::default();
    let qh = c.qh();
    let manager: ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1 =
        c.globals.bind(&qh, 1..=1, ()).unwrap();
    let _late = manager.create_session(
        &source,
        ext_image_copy_capture_manager_v1::Options::empty(),
        &qh,
        (),
    );
    h.pump_until(&mut c, "a session made while locked to be stopped", |s| {
        s.session.stopped || s.session.done
    });
    assert!(
        c.state.session.stopped,
        "a session made while locked was served"
    );
    assert!(
        c.state.session.size.is_none(),
        "a session made while locked got constraints"
    );
    locker.unlock();
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
