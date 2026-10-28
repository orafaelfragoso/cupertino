//! Headless smoke tests: the compositor runs in-process on an offscreen EGL device, a Wayland
//! client (shm buffer + xdg toplevel) connects through its socket, and the rendered output is
//! checked pixel-wise. Tests skip (pass) when no EGL device is available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::displays::HeadMode;
use cupertino_server::input::{translate_action, Action};
use cupertino_server::render::motion::WorkspaceTransition;
use cupertino_server::render::CupertinoElement;
use cupertino_server::Cupertino;
use cupertino_wm::{Command, LifecycleState, Rect};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols_plasma::appmenu::client::{
    org_kde_kwin_appmenu, org_kde_kwin_appmenu_manager,
};

const OUTPUT: (i32, i32) = (1000, 700);

/// A slot at the bottom of the output a window minimizes into.
const DOCK_SLOT: Rect = Rect {
    x: 600.0,
    y: 640.0,
    w: 48.0,
    h: 48.0,
};

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        Self::with_config(|_| {})
    }

    fn with_config(tweak: impl FnOnce(&mut Config)) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut cfg = Config::default();
        tweak(&mut cfg);
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

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn screenshot(&mut self, name: &str) -> image::RgbaImage {
        let path =
            std::env::temp_dir().join(format!("cupertino-test-{}-{name}.png", std::process::id()));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }
}

// ---- test client ---------------------------------------------------------------------------

#[derive(Default)]
struct Status {
    configured: bool,
    closed: bool,
    committed: bool,
}

struct ClientState {
    status: Arc<Mutex<Status>>,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    /// Size of the window geometry; the buffer is `margin` larger on every side (a client-drawn shadow).
    size: (i32, i32),
    margin: i32,
    color: [u8; 4],
    buffer: Option<wl_buffer::WlBuffer>,
    /// The popup, once requested: its surfaces and size.
    popup: Option<(wl_surface::WlSurface, xdg_surface::XdgSurface, (i32, i32))>,
    popup_buffer: Option<wl_buffer::WlBuffer>,
}

/// An opaque buffer of `size` in `color`, attached to `surface` and committed.
fn attach_buffer(
    shm: &wl_shm::WlShm,
    surface: &wl_surface::WlSurface,
    size: (i32, i32),
    color: [u8; 4],
    qh: &QueueHandle<ClientState>,
) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let mut file = tempfile_like(w as usize * h as usize * 4);
    let row: Vec<u8> = color.iter().copied().cycle().take(w as usize * 4).collect();
    for _ in 0..h {
        file.write_all(&row).unwrap();
    }
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w, h);
    surface.commit();
    buffer
}

impl ClientState {
    fn attach(&mut self, qh: &QueueHandle<ClientState>) {
        let (w, h) = self.size;
        let m = self.margin;
        self.buffer = Some(attach_buffer(
            &self.shm,
            &self.surface,
            (w + 2 * m, h + 2 * m),
            self.color,
            qh,
        ));
        self.status.lock().unwrap().committed = true;
    }
}

fn tempfile_like(len: usize) -> std::fs::File {
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let file = std::fs::File::from(fd);
    file.set_len(len as u64).unwrap();
    file
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
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface
);

ignore_events!(
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager,
    org_kde_kwin_appmenu::OrgKdeKwinAppmenu
);

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
            if let Some((popup_surface, popup_xdg, size)) = &state.popup {
                if popup_xdg == surface {
                    if state.popup_buffer.is_none() {
                        state.popup_buffer = Some(attach_buffer(
                            &state.shm,
                            popup_surface,
                            *size,
                            [0, 255, 0, 255],
                            qh,
                        ));
                    }
                    return;
                }
            }
            let first = !state.status.lock().unwrap().configured;
            state.status.lock().unwrap().configured = true;
            if first {
                state.attach(qh);
            }
        }
    }
}

ignore_events!(xdg_positioner::XdgPositioner, xdg_popup::XdgPopup);

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Close = event {
            state.status.lock().unwrap().closed = true;
        }
    }
}

#[derive(Default)]
struct Options {
    decorate: bool,
    /// A client-drawn shadow this many points wide around the window geometry, which the buffer and the input region
    /// include (`xdg_surface.set_window_geometry` at the margin).
    margin: i32,
    /// A popup of this size anchored at this point of the window geometry, opened once the window has drawn.
    popup: Option<((i32, i32), (i32, i32))>,
    /// `xdg_toplevel.set_min_size` and `set_max_size` before the first commit (0 is no limit).
    min_size: (i32, i32),
    max_size: (i32, i32),
    /// KDE appmenu address (service, path) announced before the first commit.
    menu_address: Option<(String, String)>,
}

struct TestClient {
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

impl TestClient {
    fn spawn(
        socket: PathBuf,
        app_id: &'static str,
        size: (i32, i32),
        color: [u8; 4],
    ) -> TestClient {
        Self::spawn_with(socket, app_id, size, color, Options::default())
    }

    fn spawn_decorated(
        socket: PathBuf,
        app_id: &'static str,
        size: (i32, i32),
        color: [u8; 4],
    ) -> TestClient {
        Self::spawn_with(
            socket,
            app_id,
            size,
            color,
            Options {
                decorate: true,
                ..Options::default()
            },
        )
    }

    fn spawn_with(
        socket: PathBuf,
        app_id: &'static str,
        size: (i32, i32),
        color: [u8; 4],
        options: Options,
    ) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status::default()));
        let (s2, st2) = (stop.clone(), status.clone());
        let thread = std::thread::spawn(move || {
            let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            toplevel.set_title(format!("{app_id} window"));
            let _decoration = options.decorate.then(|| {
                let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
                    globals.bind(&qh, 1..=1, ()).unwrap();
                let d = manager.get_toplevel_decoration(&toplevel, &qh, ());
                d.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
                d
            });
            let _menu = options.menu_address.as_ref().map(|(service, path)| {
                let manager: org_kde_kwin_appmenu_manager::OrgKdeKwinAppmenuManager =
                    globals.bind(&qh, 1..=2, ()).unwrap();
                let m = manager.create(&surface, &qh, ());
                m.set_address(service.clone(), path.clone());
                m
            });
            if options.margin > 0 {
                xdg.set_window_geometry(options.margin, options.margin, size.0, size.1);
            }
            toplevel.set_min_size(options.min_size.0, options.min_size.1);
            toplevel.set_max_size(options.max_size.0, options.max_size.1);
            surface.commit();
            let mut client = ClientState {
                status: st2.clone(),
                shm,
                surface,
                size,
                margin: options.margin,
                color,
                buffer: None,
                popup: None,
                popup_buffer: None,
            };
            while !s2.load(Ordering::SeqCst) && !st2.lock().unwrap().closed {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                if let (Some((popup_size, (ax, ay))), None, true) =
                    (options.popup, &client.popup, client.buffer.is_some())
                {
                    let positioner = wm_base.create_positioner(&qh, ());
                    positioner.set_size(popup_size.0, popup_size.1);
                    positioner.set_anchor_rect(ax, ay, 1, 1);
                    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
                    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
                    let popup_surface = compositor.create_surface(&qh, ());
                    let popup_xdg = wm_base.get_xdg_surface(&popup_surface, &qh, ());
                    let _popup = popup_xdg.get_popup(Some(&xdg), &positioner, &qh, ());
                    popup_surface.commit();
                    client.popup = Some((popup_surface, popup_xdg, popup_size));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        TestClient {
            stop,
            status,
            thread: Some(thread),
        }
    }

    fn committed(&self) -> bool {
        self.status.lock().unwrap().committed
    }

    fn closed(&self) -> bool {
        self.status.lock().unwrap().closed
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        // The client may be blocked in a round trip that only the (not running) compositor can
        // answer, so the thread is detached: it ends when the compositor drops its socket.
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take();
    }
}

fn luma(p: &image::Rgba<u8>) -> f64 {
    0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64
}

// ---- tests ---------------------------------------------------------------------------------

#[test]
fn client_window_is_mapped_styled_and_closed() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.smoke",
        (400, 300),
        [0, 0, 255, 255],
    );
    h.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);

    // model state: one window, focused, opening or normal, grouped under its application id
    let id = h.state.wm.key_window().expect("the new window takes focus");
    let w = h.state.wm.window(id).unwrap();
    assert_eq!(w.geometry().w, 400.0);
    assert_eq!(w.geometry().h, 300.0);
    assert!(matches!(
        w.state(),
        LifecycleState::Opening | LifecycleState::Normal
    ));
    assert!(h.state.wm.app_for_identity("org.example.smoke").is_some());
    assert!(client.committed());

    // finish the opening animation deterministically, then render
    let frame = {
        let time = h.state.now() + 5.0;
        common::set_time(&mut h.state, &mut h.event_loop, time);
        h.state.tick();
        h.state.wm.window(id).unwrap().geometry()
    };
    assert_eq!(
        h.state.wm.window(id).unwrap().state(),
        LifecycleState::Normal
    );
    for _ in 0..20 {
        h.step();
    }
    let img = h.screenshot("styled");
    assert_eq!(
        (img.width(), img.height()),
        (OUTPUT.0 as u32, OUTPUT.1 as u32)
    );
    let (x, y, w, hgt) = (
        frame.x as u32,
        frame.y as u32,
        frame.w as u32,
        frame.h as u32,
    );

    // content: red pixels of the client buffer (ARGB 0,0,255,255 as B,G,R,A => red)
    let centre = img.get_pixel(x + w / 2, y + hgt / 2);
    assert!(
        centre[0] > 240 && centre[1] < 20 && centre[2] < 20,
        "centre pixel {centre:?}"
    );
    // rounded corner: the very corner pixel is not window content
    let corner = img.get_pixel(x, y);
    assert!(
        !(corner[0] > 200 && corner[1] < 40),
        "corner pixel should be masked, got {corner:?}"
    );
    let inside = img.get_pixel(x + 20, y + 20);
    assert!(
        inside[0] > 240 && inside[1] < 20,
        "pixel inside the corner radius {inside:?}"
    );
    // shadow: darker just below the window than far below it (same wallpaper region)
    let near = luma(img.get_pixel(x + w / 2, y + hgt + 8));
    let far = luma(img.get_pixel(x + w / 2, (y + hgt + 120).min(OUTPUT.1 as u32 - 1)));
    assert!(near < far - 6.0, "shadow near {near} vs far {far}");

    // close through the action path: the client receives the close request and leaves
    let ctx = h.state.action_context();
    h.state
        .execute_plan(translate_action(Action::CloseWindow, &ctx));
    h.wait_until("the client to see close", |_| client.closed());
    drop(client);
    h.wait_until("the window to be destroyed", |s| {
        s.wm.windows().count() == 0
    });
}

#[test]
fn element_order_is_wallpaper_windows_shell_cursor() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.order",
        (300, 200),
        [255, 255, 255, 255],
    );
    h.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    for _ in 0..10 {
        h.step();
    }
    let mut backend = h.state.backend.take().unwrap();
    let elements = h.state.build_output_elements(backend.renderer(), 0);
    h.state.backend = Some(backend);
    let kinds: Vec<&str> = elements
        .iter()
        .map(|e| match e {
            CupertinoElement::Image(_) => "cursor",
            CupertinoElement::Decoration(_) => "decoration",
            CupertinoElement::Mesh(_) => "mesh",
            CupertinoElement::Material(_) => "material",
            CupertinoElement::Window(_) => "window",
            CupertinoElement::Popup(_) => "popup",
            CupertinoElement::Surface(_) => "surface",
            CupertinoElement::Shader(_) => "shader",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds.first(), Some(&"cursor"), "{kinds:?}");
    assert_eq!(
        kinds.last(),
        Some(&"shader"),
        "wallpaper is the bottom element: {kinds:?}"
    );
    let pos = |k: &str| kinds.iter().position(|x| *x == k);
    assert!(
        pos("window") < kinds.iter().rposition(|k| *k == "shader"),
        "windows are above the shadow and wallpaper: {kinds:?}"
    );
    drop(client);
}

#[test]
fn workspace_switch_slides_windows_and_overview_toggles() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.slide",
        (300, 200),
        [0, 255, 0, 255],
    );
    h.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let out = h.state.outputs[0].id;
    h.state.command(Command::AddWorkspace { output: out });
    let ctx = h.state.action_context();
    h.state
        .execute_plan(translate_action(Action::WorkspaceRight, &ctx));
    // half way through the slide the first workspace is partially off screen
    let t0 = h.state.now();
    let time = t0 + 0.15;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let pos = h.state.wm.workspace_position(out, h.state.now());
    assert!(pos > 0.0 && pos < 1.0, "slide position {pos}");
    let list = cupertino_server::render::scene::windows_on_output(
        &h.state.wm,
        out,
        1000.0,
        WorkspaceTransition::Slide,
        h.state.now(),
    );
    assert_eq!(list.len(), 1);
    assert!(list[0].1 < 0.0, "window offset {:?}", list[0]);
    let time = t0 + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    assert!(cupertino_server::render::scene::windows_on_output(
        &h.state.wm,
        out,
        1000.0,
        WorkspaceTransition::Slide,
        h.state.now()
    )
    .is_empty());

    // overview opens and closes
    h.state.execute_plan(translate_action(
        Action::WorkspaceLeft,
        &h.state.action_context(),
    ));
    let time = t0 + 10.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    h.state.execute_plan(translate_action(
        Action::Overview,
        &h.state.action_context(),
    ));
    assert!(h.state.shell.overview_open());
    let time = t0 + 12.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.execute_plan(translate_action(
        Action::Overview,
        &h.state.action_context(),
    ));
    assert!(!h.state.shell.overview_open());
    let time = t0 + 20.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    assert!(h.state.shell.overview.is_none());
    drop(client);
}

/// Server-side insets reserve space around the surface: the frame is the surface geometry plus
/// the insets and the content is drawn below the top inset.
#[test]
fn insets_reserve_space_around_the_surface() {
    let Some(mut h) = Harness::with_config(|c| {
        c.window.decorations = false;
        c.window.ssd_insets.top = 28.0;
    }) else {
        return;
    };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.deco",
        (400, 300),
        [0, 0, 255, 255],
    );
    h.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    for _ in 0..20 {
        h.step();
    }
    let id = h.state.wm.key_window().unwrap();
    let frame = h.state.wm.window(id).unwrap().geometry();
    assert_eq!(
        (frame.w, frame.h),
        (400.0, 328.0),
        "frame = surface geometry + insets"
    );
    let img = h.screenshot("deco");
    let (x, y) = (frame.x as u32, frame.y as u32);
    let content = img.get_pixel(x + 200, y + 28 + 100);
    assert!(
        content[0] > 240 && content[1] < 20 && content[2] < 20,
        "content pixel {content:?}"
    );
    drop(client);
}

/// Writes the image into `$CUPERTINO_ARTIFACTS` when that variable names a directory.
fn artifact(img: &image::RgbaImage, name: &str) {
    if let Some(dir) = std::env::var_os("CUPERTINO_ARTIFACTS") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        img.save(dir.join(name)).unwrap();
    }
}

#[test]
fn minimize_collapses_the_window_towards_its_target() {
    use cupertino_server::render::scene::plan_for;
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.min",
        (420, 300),
        [255, 128, 0, 255],
    );
    h.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);
    let t0 = h.state.now() + 5.0;
    let time = t0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    for _ in 0..10 {
        h.step();
    }
    let id = h.state.wm.key_window().unwrap();
    let rest = h.state.wm.window(id).unwrap().geometry();
    artifact(&h.screenshot("min-rest"), "min-0-rest.png");
    h.state.command(Command::Minimize {
        window: id,
        target: Some(DOCK_SLOT),
    });
    let time = t0 + 0.25;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let entry = h.state.entry(id).unwrap();
    let (plan, pres) = plan_for(
        &h.state.wm,
        entry,
        0.0,
        h.state.now(),
        &h.state.fx,
        h.state.display_options(),
    )
    .expect("still drawn while minimizing");
    assert!(
        pres.minimize_progress > 0.05 && pres.minimize_progress < 0.95,
        "progress {}",
        pres.minimize_progress
    );
    assert!(
        plan.visual.w < rest.w && plan.visual.y > rest.y,
        "visual {:?} vs rest {:?}",
        plan.visual,
        rest
    );
    assert!(!plan.exact);
    for _ in 0..3 {
        h.step();
    }
    artifact(&h.screenshot("min-mid"), "min-1-mid.png");
    let time = t0 + 3.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    assert!(h.state.wm.window(id).unwrap().state().is_minimized());
    assert!(
        !h.state.wm.window_on_screen(id),
        "a minimized window is not drawn"
    );
    for _ in 0..5 {
        h.step();
    }
    artifact(&h.screenshot("min-done"), "min-2-done.png");
    drop(client);
}

/// Reduce Motion drops the full-screen frame zoom: mid-transition the window is drawn
/// at its full-screen frame, where without it the frame is still between the two.
#[test]
fn under_reduce_motion_full_screen_draws_the_new_frame_at_once() {
    use cupertino_defaults::accessibility::DisplayOption;
    use cupertino_server::render::scene::plan_for;
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.fs",
        (420, 300),
        [0, 128, 255, 255],
    );
    h.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);
    let t0 = h.state.now() + 5.0;
    let time = t0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let id = h.state.wm.key_window().unwrap();
    h.state.command(Command::Fullscreen {
        window: id,
        output: None,
    });
    let time = t0 + 0.1;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let w = h.state.wm.window(id).unwrap();
    assert_eq!(w.state(), LifecycleState::EnterFullScreen);
    let target = w.lifecycle.target_frame();
    let entry = h.state.entry(id).unwrap();
    let (zoomed, _) = plan_for(
        &h.state.wm,
        entry,
        0.0,
        h.state.now(),
        &h.state.fx,
        h.state.display_options(),
    )
    .expect("drawn");
    assert_ne!(
        zoomed.visual, target,
        "the frame zooms without Reduce Motion"
    );
    h.state
        .set_display_option(DisplayOption::ReduceMotion, true);
    let entry = h.state.entry(id).unwrap();
    let (reduced, _) = plan_for(
        &h.state.wm,
        entry,
        0.0,
        h.state.now(),
        &h.state.fx,
        h.state.display_options(),
    )
    .expect("drawn");
    assert_eq!(reduced.visual, target);
    drop(client);
}

/// The display filter pass recolours the whole output: grayscale mixes linear light with the
/// reference's weights (pure red becomes 0.3 of linear white), invert flips the encoded values,
/// and turning the settings off gives the unfiltered frame back.
#[test]
fn the_display_filter_recolours_the_whole_output() {
    use cupertino_defaults::accessibility::{AccessibilitySettings, ColorFilter};
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.filter",
        (400, 300),
        [0, 0, 255, 255],
    );
    h.wait_until("the window to be mapped", |s| s.wm.windows().count() == 1);
    let time = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let id = h.state.wm.key_window().unwrap();
    let f = h.state.wm.window(id).unwrap().geometry();
    let (cx, cy) = ((f.x + f.w / 2.0) as u32, (f.y + f.h / 2.0) as u32);
    let shot = |h: &mut Harness, name: &str| {
        for _ in 0..5 {
            h.step();
        }
        h.screenshot(name)
    };
    let plain = shot(&mut h, "filter-off");
    assert_eq!(
        plain.get_pixel(cx, cy).0,
        [255, 0, 0, 255],
        "the red window"
    );
    let elsewhere = (20, OUTPUT.1 as u32 - 20);

    h.state.set_accessibility(AccessibilitySettings {
        color_filter: Some(ColorFilter::Grayscale),
        ..AccessibilitySettings::default()
    });
    let gray = shot(&mut h, "filter-grayscale");
    let g = gray.get_pixel(cx, cy).0;
    assert!(
        g[..3].iter().all(|v| v.abs_diff(149) <= 1),
        "pure red through grayscale: {g:?}"
    );
    let w = gray.get_pixel(elsewhere.0, elsewhere.1).0;
    assert!(
        w[0] == w[1] && w[1] == w[2],
        "the wallpaper is gray too: {w:?}"
    );

    h.state.set_accessibility(AccessibilitySettings {
        invert_colors: true,
        ..AccessibilitySettings::default()
    });
    let inverted = shot(&mut h, "filter-invert");
    assert_eq!(inverted.get_pixel(cx, cy).0, [0, 255, 255, 255]);
    let (p, q) = (
        plain.get_pixel(elsewhere.0, elsewhere.1).0,
        inverted.get_pixel(elsewhere.0, elsewhere.1).0,
    );
    assert!(
        (0..3).all(|i| (p[i] as i32 + q[i] as i32 - 255).abs() <= 1),
        "inverted wallpaper {q:?} of {p:?}"
    );

    h.state.set_accessibility(AccessibilitySettings::default());
    assert_eq!(
        shot(&mut h, "filter-off-again").as_raw(),
        plain.as_raw(),
        "no pass without settings"
    );
    drop(client);
}

/// The mesh element draws a captured texture upright through an effect mesh; the capture of an
/// element list has the same orientation as the live scene.
#[test]
fn capture_orientation_and_mesh_drawing() {
    use cupertino_effects::Mesh;
    use cupertino_server::render::mesh::{capture, MeshElement};
    use cupertino_server::render::scene::RenderResources;
    use cupertino_server::render::screenshot::render_to_image;
    use cupertino_server::render::shader_rect::{ElementCache, ElementKey};
    use cupertino_server::render::ui::{rounded_rect, RoundedRectSpec};
    use smithay::backend::renderer::element::Id;
    use smithay::backend::renderer::utils::CommitCounter;
    use smithay::utils::{Logical, Rectangle};

    let Ok(mut renderer) = headless::create_renderer() else {
        eprintln!("skipping: no EGL device");
        return;
    };
    let res = RenderResources::new(&mut renderer).unwrap();
    let mut cache = ElementCache::default();
    let mut rect = |y: f64, h: f64, c: [f32; 4], key: u64| {
        let spec = RoundedRectSpec {
            rect: Rectangle::<f64, Logical>::new((0.0, y).into(), (100.0, h).into()),
            radius: 0.0,
            continuous: false,
            fill: c,
            border: None,
            alpha: 1.0,
        };
        CupertinoElement::from(rounded_rect(
            &res.programs,
            &mut cache,
            1.0,
            ElementKey::new("t", key),
            &spec,
        ))
    };
    // top half red, bottom half blue
    let src = vec![
        rect(0.0, 50.0, [1.0, 0.0, 0.0, 1.0], 1),
        rect(50.0, 50.0, [0.0, 0.0, 1.0, 1.0], 2),
    ];
    let tex = capture(&mut renderer, &src, (100, 100).into(), 1.0).unwrap();
    {
        use smithay::backend::renderer::{Bind, ExportMem};
        let mut t2 = tex.clone();
        let target = renderer.bind(&mut t2).unwrap();
        let m = renderer
            .copy_framebuffer(
                &target,
                smithay::utils::Rectangle::from_size((100, 100).into()),
                smithay::backend::allocator::Fourcc::Abgr8888,
            )
            .unwrap();
        let bytes = renderer.map_texture(&m).unwrap().to_vec();
        for y in (0..100).step_by(20) {
            eprintln!(
                "tex row {y}: {:?}",
                &bytes[(y * 100 + 50) * 4..(y * 100 + 50) * 4 + 4]
            );
        }
    }
    let live = render_to_image(
        &mut renderer,
        &src,
        (100, 100).into(),
        1.0,
        smithay::backend::renderer::Color32F::new(0.0, 0.0, 0.0, 0.0),
    )
    .unwrap();
    assert!(
        live.get_pixel(50, 20)[0] > 200 && live.get_pixel(50, 80)[2] > 200,
        "live scene: red above blue"
    );

    let mesh = Mesh::quad(cupertino_geometry::Rect::new(200.0, 100.0, 100.0, 100.0));
    let el = MeshElement::new(
        Id::new(),
        CommitCounter::default(),
        tex,
        res.mesh.clone().expect("mesh pass"),
        &mesh,
        1.0,
        1.0,
        None,
    );
    let img = render_to_image(
        &mut renderer,
        &[CupertinoElement::from(el)],
        (500, 400).into(),
        1.0,
        smithay::backend::renderer::Color32F::new(0.0, 0.0, 0.0, 1.0),
    )
    .unwrap();
    for y in (95..205).step_by(10) {
        eprintln!("y={y}: {:?}", img.get_pixel(250, y));
    }
    let top = img.get_pixel(250, 120);
    let bottom = img.get_pixel(250, 180);
    assert!(
        top[0] > 200 && top[2] < 40,
        "mesh top should be red, got {top:?}"
    );
    assert!(
        bottom[2] > 200 && bottom[0] < 40,
        "mesh bottom should be blue, got {bottom:?}"
    );
    let outside = img.get_pixel(150, 150);
    assert!(
        outside[0] < 10 && outside[2] < 10,
        "nothing outside the mesh: {outside:?}"
    );
    res.destroy(&mut renderer);
}

// ---- interaction tests ------------------------------------------------------------------------

use smithay::backend::input::ButtonState;

impl Harness {
    /// Advance model time by `dt` seconds and run the compositor a few iterations.
    fn advance(&mut self, dt: f64) {
        let t = self.state.now() + dt;
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        for _ in 0..3 {
            self.step();
        }
    }

    fn move_to(&mut self, x: f64, y: f64) {
        self.state.pointer_moved_to((x, y).into(), 0, None);
    }

    fn press(&mut self) {
        self.state
            .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 0);
    }

    fn release(&mut self) {
        self.state
            .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Released, 0);
    }

    fn click(&mut self, x: f64, y: f64) {
        self.move_to(x, y);
        self.press();
        self.release();
    }

    /// Open window of a freshly spawned client: settled model time, focused window id and frame.
    fn mapped_window(
        &mut self,
        client_count: usize,
    ) -> (cupertino_wm::WindowId, cupertino_wm::Rect) {
        self.wait_until("the window to be mapped", |s| {
            s.wm.windows().count() == client_count
        });
        let t = self.state.now() + 5.0;
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        for _ in 0..10 {
            self.step();
        }
        let id = self.state.wm.key_window().expect("focused window");
        (id, self.state.wm.window(id).unwrap().geometry())
    }
}

#[test]
fn double_clicking_a_resize_edge_moves_that_edge_to_the_screen_edge() {
    let Some(mut h) = Harness::new() else { return };
    let _client = TestClient::spawn_decorated(
        h.socket_path(),
        "org.example.deco",
        (400, 300),
        [0, 0, 255, 255],
    );
    let (id, frame) = h.mapped_window(1);
    let usable = h
        .state
        .wm
        .output(h.state.wm.window(id).unwrap().output)
        .expect("the output")
        .usable;
    h.move_to(frame.right() + 2.0, frame.y + 150.0);
    h.press();
    h.release();
    h.press();
    h.release();
    h.advance(3.0);
    let snapped = h.state.wm.window(id).unwrap().geometry();
    assert!(
        (snapped.right() - usable.right()).abs() < 1.5,
        "right edge at the screen: {snapped:?} in {usable:?}"
    );
    assert_eq!(
        (snapped.x, snapped.y, snapped.h),
        (frame.x, frame.y, frame.h),
        "only the double-clicked edge moves"
    );
}

#[test]
fn decorations_buttons_title_bar_and_resize_zones_drive_the_window() {
    use cupertino_decorations::ButtonKind;
    use cupertino_geometry::{Point, Size};

    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn_decorated(
        h.socket_path(),
        "org.example.deco",
        (400, 300),
        [0, 0, 255, 255],
    );
    let (id, frame) = h.mapped_window(1);
    assert!(
        h.state.entry(id).unwrap().server_side,
        "the client accepted server-side decorations"
    );
    assert_eq!(
        (frame.w, frame.h),
        (400.0, 328.0),
        "frame = surface + title bar"
    );
    let layout = h.state.decor.decorations().layout(Size::new(400.0, 300.0));
    let center = |frame: cupertino_wm::Rect, k: ButtonKind| {
        let c: Point = layout.button(k).center;
        (frame.x + c.x, frame.y + c.y)
    };

    // hovering the button group lights up the glyphs (re-rendered title bar, group state set)
    let (cx, cy) = center(frame, ButtonKind::Minimize);
    h.move_to(cx, cy);
    assert_eq!(h.state.decor.hover_group, Some(id));
    let renders = h.state.decor.renders;
    h.step();
    h.step();
    assert!(
        h.state.decor.renders > renders,
        "title bar re-rendered for the hover state"
    );
    h.move_to(frame.x + 200.0, frame.y + 200.0);
    assert_eq!(h.state.decor.hover_group, None);

    // title bar drag moves the window once the pointer leaves the drag-start box
    h.move_to(frame.x + 250.0, frame.y + 10.0);
    h.press();
    assert!(!h.state.wm.is_interacting(), "a press alone is no drag yet");
    h.move_to(frame.x + 350.0, frame.y + 70.0);
    assert!(h.state.wm.is_interacting());
    h.release();
    h.advance(3.0);
    let moved = h.state.wm.window(id).unwrap().geometry();
    assert!(
        (moved.x - frame.x - 100.0).abs() < 1.5 && (moved.y - frame.y - 60.0).abs() < 1.5,
        "moved {moved:?} from {frame:?}"
    );

    // the right edge resizes (zone outside the frame geometry)
    h.advance(1.0);
    h.move_to(moved.right() + 2.0, moved.y + 150.0);
    assert!(
        h.state.decor.cursor.is_some(),
        "resize cursor over the edge"
    );
    h.press();
    h.move_to(moved.right() + 52.0, moved.y + 150.0);
    assert!(h.state.wm.is_interacting());
    h.release();
    h.advance(3.0);
    let resized = h.state.wm.window(id).unwrap().geometry();
    assert!(
        (resized.w - moved.w - 50.0).abs() < 1.5,
        "resized {resized:?} from {moved:?}"
    );

    // double click on the title bar zooms
    h.advance(1.0);
    let (tx, ty) = (resized.x + 200.0, resized.y + 10.0);
    h.click(tx, ty);
    h.advance(0.1);
    h.click(tx, ty);
    h.advance(0.2);
    assert!(
        matches!(
            h.state.wm.window(id).unwrap().state(),
            LifecycleState::Zooming | LifecycleState::Zoomed
        ),
        "state {:?}",
        h.state.wm.window(id).unwrap().state()
    );
    h.advance(3.0);
    let zoomed = h.state.wm.window(id).unwrap().geometry();

    // the close button asks the client to close
    let zlayout = h
        .state
        .decor
        .decorations()
        .layout(Size::new(zoomed.w, zoomed.h - 28.0));
    let c = zlayout.button(ButtonKind::Close).center;
    h.click(zoomed.x + c.x, zoomed.y + c.y);
    h.wait_until("the client to see close", |_| client.closed());
    drop(client);
}

#[test]
fn clients_that_ask_for_client_side_decorations_keep_them() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.csd",
        (400, 300),
        [0, 255, 0, 255],
    );
    let (id, frame) = h.mapped_window(1);
    assert!(!h.state.entry(id).unwrap().server_side);
    assert_eq!(
        (frame.w, frame.h),
        (400.0, 300.0),
        "no title bar inset without negotiation"
    );
    drop(client);
}

#[test]
fn plain_windows_close_at_once_unless_the_fade_is_enabled() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn_decorated(
        h.socket_path(),
        "org.example.close",
        (400, 300),
        [255, 255, 0, 255],
    );
    h.mapped_window(1);
    drop(client);
    h.wait_until("the window to be destroyed", |s| {
        s.wm.windows().count() == 0
    });
    assert!(
        h.state.fx.closing.is_empty(),
        "plain windows have no close animation by default"
    );

    h.state.fx.cfg.closing = true;
    let client = TestClient::spawn_decorated(
        h.socket_path(),
        "org.example.close",
        (400, 300),
        [255, 255, 0, 255],
    );
    let (_, frame) = h.mapped_window(1);
    drop(client);
    h.wait_until("the window to be destroyed", |s| {
        s.wm.windows().count() == 0
    });
    assert_eq!(
        h.state.fx.closing.len(),
        1,
        "with the fade enabled a ghost of the closed window exists"
    );
    let img = h.screenshot("ghost");
    let (x, y) = ((frame.x + 200.0) as u32, (frame.y + 150.0) as u32);
    let px = img.get_pixel(x, y);
    assert!(
        px[0] < 40 && px[1] > 200 && px[2] > 200,
        "the ghost still shows the cyan content, got {px:?}"
    );
    let t = h.state.now() + 5.0;
    let time = t;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    assert!(
        h.state.fx.closing.is_empty(),
        "the ghost is gone after the fade"
    );
}

#[test]
fn output_resize_refits_a_fullscreen_window() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.fs",
        (300, 200),
        [10, 10, 10, 255],
    );
    let (id, _) = h.mapped_window(1);
    h.state.command(Command::Fullscreen {
        window: id,
        output: None,
    });
    h.advance(5.0);
    let out = h.state.outputs[0].id;
    assert_eq!(h.state.wm.window(id).unwrap().geometry().w, OUTPUT.0 as f64);
    let head = h
        .state
        .displays
        .head_of_output(out)
        .expect("the output's display")
        .id;
    h.state.head_mode_replaced(
        head,
        HeadMode {
            width: 800,
            height: 600,
            refresh_mhz: 60_000,
        },
    );
    let g = h.state.wm.window(id).unwrap().geometry();
    assert_eq!((g.w, g.h), (800.0, 600.0), "full screen follows the output");
    assert_eq!(h.state.wm.output(out).unwrap().rect.w, 800.0);
    drop(client);
}

#[test]
fn touchpad_gestures_switch_workspaces_and_a_spread_without_a_dock_only_leaves_the_overview() {
    use cupertino_input::GestureEvent;

    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(
        h.socket_path(),
        "org.example.gesture",
        (300, 200),
        [0, 0, 255, 255],
    );
    let (id, _) = h.mapped_window(1);
    let out = h.state.outputs[0].id;
    h.state.command(Command::AddWorkspace { output: out });
    h.advance(1.0);
    let start = h.state.wm.current_workspace(out).unwrap();
    // three fingers moving left: towards the next workspace
    let t0 = h.state.now();
    h.state.gesture(GestureEvent::SwipeBegin {
        time: t0,
        fingers: 3,
    });
    for i in 1..=12 {
        h.state.gesture(GestureEvent::SwipeUpdate {
            time: t0 + 0.016 * i as f64,
            dx: -40.0,
            dy: 0.0,
        });
    }
    assert!(
        h.state.wm.workspace_position(out, h.state.now()) > 0.0,
        "the content follows the fingers"
    );
    h.state.gesture(GestureEvent::SwipeEnd {
        time: t0 + 0.3,
        cancelled: false,
    });
    h.advance(5.0);
    assert_ne!(
        h.state.wm.current_workspace(out).unwrap(),
        start,
        "a committed swipe switched the workspace"
    );

    // a four-finger spread is the dock's to map (tests/dock_v1.rs); without a dock the server only
    // uses it to leave an open overview, so a dead dock never leaves the overview stuck
    h.state.command(Command::SwitchWorkspace {
        output: out,
        workspace: start,
    });
    h.advance(5.0);
    h.state.toggle_overview(false);
    assert!(h.state.shell.overview_open());
    let spread = |h: &mut Harness| {
        let t1 = h.state.now();
        h.state.gesture(GestureEvent::PinchBegin {
            time: t1,
            fingers: 4,
        });
        for i in 1..=10 {
            h.state.gesture(GestureEvent::PinchUpdate {
                time: t1 + 0.016 * i as f64,
                dx: 0.0,
                dy: 0.0,
                scale: 1.0 + 0.1 * i as f64,
                rotation: 0.0,
            });
        }
        h.state.gesture(GestureEvent::PinchEnd {
            time: t1 + 0.3,
            cancelled: false,
        });
    };
    spread(&mut h);
    assert!(
        !h.state.shell.overview_open(),
        "the spread left the overview"
    );
    h.advance(3.0);
    spread(&mut h);
    assert!(
        !h.state.fx.show_desktop_active(),
        "without a dock nothing maps the spread to an action"
    );
    assert!(h.state.wm.window_on_screen(id));
    drop(client);
}

#[test]
fn every_minimize_effect_draws_a_mesh_and_restore_runs_it_backwards() {
    use cupertino_effects::MinimizeKind;
    for kind in [
        MinimizeKind::Warp,
        MinimizeKind::Funnel,
        MinimizeKind::Scale,
    ] {
        let Some(mut h) = Harness::with_config(|c| {
            c.effects.minimize = Some(kind);
        }) else {
            return;
        };
        let client = TestClient::spawn(
            h.socket_path(),
            "org.example.fx",
            (420, 300),
            [0, 255, 0, 255],
        );
        let (id, rest) = h.mapped_window(1);
        h.state.command(Command::Minimize {
            window: id,
            target: Some(DOCK_SLOT),
        });
        let duration = kind.duration_s(false);
        let t0 = h.state.now();
        let time = t0 + duration * 0.5;
        common::set_time(&mut h.state, &mut h.event_loop, time);
        h.state.tick();
        for _ in 0..3 {
            h.step();
        }
        let mid = h.screenshot("fx-mid");
        // the window content (green) is still visible somewhere in the animation frame, but not at its rest position corner
        let green = |p: &image::Rgba<u8>| p[1] > 200 && p[0] < 60 && p[2] < 60;
        let count = mid.pixels().filter(|p| green(p)).count();
        assert!(
            count > 300,
            "{kind:?}: the mesh draws the window content mid-animation ({count} px)"
        );
        let corner = mid.get_pixel(rest.x as u32 + 2, rest.y as u32 + 2);
        assert!(
            !green(corner),
            "{kind:?}: the window left its rest rectangle"
        );
        let time = t0 + duration + 1.0;
        common::set_time(&mut h.state, &mut h.event_loop, time);
        h.state.tick();
        assert!(h.state.wm.window(id).unwrap().state().is_minimized());

        // restore: the reverse run starts at the tile and shows the mesh again
        h.state.command(Command::Restore { window: id });
        let t1 = h.state.now();
        let time = t1 + duration * 0.5;
        common::set_time(&mut h.state, &mut h.event_loop, time);
        h.state.tick();
        for _ in 0..3 {
            h.step();
        }
        let back = h.screenshot("fx-back");
        assert!(
            back.pixels().filter(|p| green(p)).count() > 300,
            "{kind:?}: restore draws the mesh"
        );
        let time = t1 + duration + 1.0;
        common::set_time(&mut h.state, &mut h.event_loop, time);
        h.state.tick();
        assert_eq!(
            h.state.wm.window(id).unwrap().geometry(),
            rest,
            "restore returns the exact frame"
        );
        drop(client);
    }
}

// ---- resize band (3 pt inside, 4 pt outside the frame, corner zones 12 / 15 pt) -------------------------------

use cupertino_server::state::WindowTarget;
use smithay::input::pointer::CursorIcon;

#[test]
fn the_inner_resize_band_of_a_server_decorated_window_wins_over_the_client_surface() {
    let Some(mut h) = Harness::new() else { return };
    let _client = TestClient::spawn_decorated(
        h.socket_path(),
        "org.example.ssd",
        (400, 300),
        [0, 0, 255, 255],
    );
    let (_, frame) = h.mapped_window(1);
    h.move_to(frame.right() - 2.0, frame.bottom() - 8.0);
    assert_eq!(
        h.state.decor.cursor,
        Some(CursorIcon::NwseResize),
        "2 pt inside, in the 15 pt bottom-right corner zone"
    );
    h.move_to(frame.right() - 2.0, frame.y + frame.h / 2.0);
    assert_eq!(
        h.state.decor.cursor,
        Some(CursorIcon::EwResize),
        "inside band on the right edge"
    );
    h.move_to(frame.right() - 4.0, frame.y + frame.h / 2.0);
    assert_eq!(h.state.decor.cursor, None, "past the 3 pt band: the client");
    assert!(matches!(
        h.state
            .window_target_under((frame.right() - 4.0, frame.y + frame.h / 2.0).into()),
        Some(WindowTarget::Surface { .. })
    ));
}

#[test]
fn a_client_decorated_window_gets_the_band_from_its_geometry_and_its_shadow_passes_input_through() {
    let Some(mut h) = Harness::new() else { return };
    let _client = TestClient::spawn_with(
        h.socket_path(),
        "org.example.csd",
        (400, 300),
        [0, 0, 255, 255],
        Options {
            margin: 12,
            ..Options::default()
        },
    );
    let (id, frame) = h.mapped_window(1);
    assert!(!h.state.entry(id).unwrap().server_side);
    assert_eq!(
        (frame.w, frame.h),
        (400.0, 300.0),
        "the frame is the window geometry, not the buffer"
    );
    let mid = frame.y + frame.h / 2.0;
    for (dx, expected) in [
        (-4.0, None),
        (-3.0, Some(CursorIcon::EwResize)),
        (0.0, Some(CursorIcon::EwResize)),
        (3.0, Some(CursorIcon::EwResize)),
        (4.0, None),
        (11.0, None),
    ] {
        h.move_to(frame.right() + dx, mid);
        assert_eq!(h.state.decor.cursor, expected, "right edge {dx:+}");
    }
    h.move_to(frame.right() - 2.0, frame.bottom() - 8.0);
    assert_eq!(h.state.decor.cursor, Some(CursorIcon::NwseResize));
    // the shadow margin beyond the band is no part of the window: the desktop below gets the pointer
    for p in [
        (frame.right() + 4.0, mid),
        (frame.right() + 11.0, mid),
        (frame.x + frame.w / 2.0, frame.y - 8.0),
    ] {
        assert!(
            h.state.window_target_under(p.into()).is_none(),
            "{p:?} passes through"
        );
    }
    h.move_to(frame.right() + 8.0, mid);
    assert_eq!(
        h.state.pointer.current_focus(),
        None,
        "the client does not get the pointer in its shadow"
    );
    assert!(
        matches!(
            h.state
                .window_target_under((frame.right() - 4.0, mid).into()),
            Some(WindowTarget::Surface { .. })
        ),
        "inside: the client"
    );
    // a press in the band resizes through the compositor, as for a server-decorated window
    h.move_to(frame.right() + 1.0, mid);
    h.press();
    h.move_to(frame.right() + 41.0, mid);
    h.release();
    h.advance(1.0);
    assert!(
        h.state.wm.window(id).unwrap().geometry().w > frame.w,
        "the drag resized the window"
    );
}

#[test]
fn a_popup_over_the_resize_band_takes_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    // a 60 x 40 popup anchored 10 pt left of the right edge of the content, 100 pt down
    let _client = TestClient::spawn_with(
        h.socket_path(),
        "org.example.popup",
        (400, 300),
        [0, 0, 255, 255],
        Options {
            decorate: true,
            popup: Some(((60, 40), (390, 100))),
            ..Options::default()
        },
    );
    let (id, frame) = h.mapped_window(1);
    let top = frame.y + h.state.entry(id).unwrap().insets.top;
    h.wait_until("the popup to draw", |s| {
        let p = (frame.right() - 2.0, top + 110.0).into();
        matches!(s.window_target_under(p), Some(WindowTarget::Surface { .. }))
    });
    h.move_to(frame.right() - 2.0, top + 110.0);
    assert_eq!(h.state.decor.cursor, None, "the popup, not the band");
    h.move_to(frame.right() - 2.0, top + 200.0);
    assert_eq!(
        h.state.decor.cursor,
        Some(CursorIcon::EwResize),
        "the band where no popup is"
    );
}

#[test]
fn a_fixed_size_window_shows_no_resize_cursor_and_keeps_its_title_bar() {
    let Some(mut h) = Harness::new() else { return };
    let _client = TestClient::spawn_with(
        h.socket_path(),
        "org.example.fixed",
        (400, 300),
        [0, 0, 255, 255],
        Options {
            decorate: true,
            min_size: (400, 300),
            max_size: (400, 300),
            ..Options::default()
        },
    );
    let (id, frame) = h.mapped_window(1);
    let mid = frame.y + frame.h / 2.0;
    for p in [
        (frame.right() - 2.0, mid),
        (frame.right() + 2.0, mid),
        (frame.right() - 2.0, frame.bottom() - 8.0),
        (frame.x + 1.0, mid),
    ] {
        h.move_to(p.0, p.1);
        assert_eq!(
            h.state.decor.cursor, None,
            "{p:?}: the window cannot resize"
        );
    }
    // the top band is the title bar's again
    let top = (frame.x + frame.w / 2.0, frame.y + 1.0);
    assert!(
        matches!(
            h.state.window_target_under(top.into()),
            Some(WindowTarget::Decoration {
                hit: cupertino_decorations::Hit::Titlebar,
                ..
            })
        ),
        "the title bar under the band"
    );
    assert_eq!(
        h.state
            .resize_cursor(id, cupertino_decorations::ResizeEdge::Right),
        None
    );
}

#[test]
fn a_window_at_its_minimum_width_shows_the_outward_arrows_only() {
    let Some(mut h) = Harness::new() else { return };
    // the minimum is the size it opens at: the width can only grow, the height has no limit
    let _client = TestClient::spawn_with(
        h.socket_path(),
        "org.example.minwidth",
        (400, 300),
        [0, 0, 255, 255],
        Options {
            decorate: true,
            min_size: (400, 0),
            ..Options::default()
        },
    );
    let (_, frame) = h.mapped_window(1);
    let mid = frame.y + frame.h / 2.0;
    h.move_to(frame.right() - 2.0, mid);
    assert_eq!(
        h.state.decor.cursor,
        Some(CursorIcon::EResize),
        "the right edge can only move out"
    );
    h.move_to(frame.x + 1.0, mid);
    assert_eq!(
        h.state.decor.cursor,
        Some(CursorIcon::WResize),
        "the left edge can only move out"
    );
    h.move_to(frame.x + frame.w / 2.0, frame.bottom() - 1.0);
    assert_eq!(
        h.state.decor.cursor,
        Some(CursorIcon::NsResize),
        "the height is free"
    );
    h.move_to(frame.right() - 2.0, frame.bottom() - 8.0);
    assert_eq!(
        h.state.decor.cursor,
        Some(CursorIcon::SeResize),
        "the corner only grows"
    );
}

#[test]
fn an_untitled_window_has_the_inside_band_only() {
    // without our decorations a window has no title bar: untitled
    let Some(mut h) = Harness::with_config(|c| c.window.decorations = false) else {
        return;
    };
    let _client = TestClient::spawn_decorated(
        h.socket_path(),
        "org.example.untitled",
        (400, 300),
        [0, 0, 255, 255],
    );
    let (id, frame) = h.mapped_window(1);
    assert!(h.state.entry(id).unwrap().server_side);
    let mid = frame.y + frame.h / 2.0;
    for (dx, expected) in [
        (-4.0, None),
        (-3.0, Some(CursorIcon::EwResize)),
        (-1.0, Some(CursorIcon::EwResize)),
        (0.0, None),
        (2.0, None),
    ] {
        h.move_to(frame.right() + dx, mid);
        assert_eq!(h.state.decor.cursor, expected, "right edge {dx:+}");
    }
}
