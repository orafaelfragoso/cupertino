//! `cupertino_app_effects_v1` backdrops end to end: the compositor runs in-process on an offscreen EGL
//! device, a client binds the global, creates a layer surface with a backdrop, and the test
//! reads the committed backdrops the scene draws. Tests skip (pass) when no EGL device is available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::{self, CupertinoBackdropV1};
use cupertino_server::app_effects_v1::{layer_backdrops, BackdropArea};
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Rectangle;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const OUTPUT: (i32, i32) = (1000, 700);
/// Layer surface size and its margins from the top-left corner of the output.
const SURFACE: (i32, i32) = (400, 200);
const MARGIN_TOP: i32 = 50;
const MARGIN_LEFT: i32 = 60;

/// What a test client creates: the layer surface and the first backdrop state it commits.
#[derive(Clone, Copy)]
struct Layout {
    size: (i32, i32),
    margin_top: i32,
    margin_left: i32,
    material: &'static str,
    /// Backdrop region: x, y, width, height, radius.
    shape: (i32, i32, i32, i32, f64),
    alpha: f64,
    layer: zwlr_layer_shell_v1::Layer,
    /// The surface's pixels (straight RGBA by surface-local position); none is fully transparent.
    paint: Option<fn(i32, i32) -> [u8; 4]>,
}

/// A backdrop inside a smaller surface, with a rounded corner and half opacity.
const CARD: Layout = Layout {
    size: SURFACE,
    margin_top: MARGIN_TOP,
    margin_left: MARGIN_LEFT,
    material: "Menu",
    shape: (10, 20, 300, 80, 16.0),
    alpha: 0.5,
    layer: zwlr_layer_shell_v1::Layer::Top,
    paint: None,
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
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
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
        .expect("create test compositor");
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

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn screenshot(&mut self, name: &str) -> image::RgbaImage {
        // the tests run in parallel in one process and two of them shoot a "wall": every shot needs a file of its own
        static SHOTS: AtomicUsize = AtomicUsize::new(0);
        let shot = SHOTS.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-{shot}-{name}.png",
            std::process::id()
        ));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }

    /// Backdrops of the layer surfaces of the first output.
    fn backdrops(&self) -> Vec<BackdropArea> {
        layer_backdrops(&self.state.outputs[0].output)
    }
}

// ---- test client ---------------------------------------------------------------------------

/// Requests the test sends to the backdrop from the client thread.
enum Cmd {
    SetMaterial(&'static str),
    SetShape {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        radius: f64,
    },
    SetAlpha(f64),
    /// `set_glass(style, container, spacing)`.
    SetGlass(cupertino_backdrop_v1::GlassStyle, u32, f64),
    SetKeyboard(zwlr_layer_surface_v1::KeyboardInteractivity),
    Commit,
}

#[derive(Default)]
struct Status {
    /// Code of the protocol error the server posted on the backdrop, if any.
    backdrop_error: Option<u32>,
}

struct ClientState {
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    backdrop: CupertinoBackdropV1,
    layout: Layout,
    attached: bool,
}

impl ClientState {
    /// Attach a fully transparent buffer and commit it together with the backdrop state.
    fn attach_and_commit(&mut self, qh: &QueueHandle<ClientState>) {
        let (w, h) = self.layout.size;
        let mut file = tempfile_like((w * h * 4) as usize);
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        if let Some(paint) = self.layout.paint {
            for y in 0..h {
                for x in 0..w {
                    let [r, g, b, a] = paint(x, y);
                    // ARGB8888 is B, G, R, A in memory, premultiplied
                    let pm = |c: u8| (u16::from(c) * u16::from(a) / 255) as u8;
                    let i = ((y * w + x) * 4) as usize;
                    pixels[i..i + 4].copy_from_slice(&[pm(b), pm(g), pm(r), a]);
                }
            }
        }
        file.write_all(&pixels).unwrap();
        let pool = self.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, w, h);
        let (x, y, width, height, radius) = self.layout.shape;
        self.backdrop.set_material(self.layout.material.into());
        self.backdrop.set_shape(x, y, width, height, radius);
        self.backdrop.set_alpha(self.layout.alpha);
        self.surface.commit();
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
    wl_surface::WlSurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1
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
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            layer.ack_configure(serial);
            if !state.attached {
                state.attached = true;
                state.attach_and_commit(qh);
            }
        }
    }
}

struct TestClient {
    commands: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
    thread: Option<JoinHandle<()>>,
}

impl TestClient {
    /// A shell client with one layer surface carrying one backdrop (`CARD`) committed with the
    /// surface's first buffer.
    fn spawn(socket: PathBuf) -> TestClient {
        Self::spawn_with(socket, CARD)
    }

    fn spawn_with(socket: PathBuf, layout: Layout) -> TestClient {
        let (commands, inbox): (Sender<Cmd>, Receiver<Cmd>) = channel();
        let status = Arc::new(Mutex::new(Status::default()));
        let st = status.clone();
        let thread = std::thread::spawn(move || {
            let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            // version 6 for set_glass
            let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 1..=6, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                None,
                layout.layer,
                "backdrop-test".into(),
                &qh,
                (),
            );
            layer.set_size(layout.size.0 as u32, layout.size.1 as u32);
            layer.set_anchor(
                zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left,
            );
            layer.set_margin(layout.margin_top, 0, 0, layout.margin_left);
            layer.set_exclusive_zone(-1);
            let backdrop = effects.get_backdrop(&surface, &qh, ());
            surface.commit();
            let mut client = ClientState {
                shm,
                surface,
                backdrop,
                layout,
                attached: false,
            };
            loop {
                while let Ok(cmd) = inbox.try_recv() {
                    match cmd {
                        Cmd::SetMaterial(name) => client.backdrop.set_material(name.into()),
                        Cmd::SetShape {
                            x,
                            y,
                            width,
                            height,
                            radius,
                        } => client.backdrop.set_shape(x, y, width, height, radius),
                        Cmd::SetAlpha(a) => client.backdrop.set_alpha(a),
                        Cmd::SetGlass(style, container, spacing) => {
                            client.backdrop.set_glass(style, container, spacing)
                        }
                        Cmd::SetKeyboard(mode) => layer.set_keyboard_interactivity(mode),
                        Cmd::Commit => client.surface.commit(),
                    }
                }
                if queue.roundtrip(&mut client).is_err() {
                    let error = conn
                        .protocol_error()
                        .expect("the roundtrip failed with a protocol error");
                    assert_eq!(
                        error.object_interface,
                        cupertino_backdrop_v1::CupertinoBackdropV1::interface().name
                    );
                    st.lock().unwrap().backdrop_error = Some(error.code);
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

    fn send(&self, cmd: Cmd) {
        self.commands.send(cmd).unwrap();
    }

    fn backdrop_error(&self) -> Option<u32> {
        self.status.lock().unwrap().backdrop_error
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        // The thread may be blocked in a round trip only the (not running) compositor can answer:
        // it is detached and ends when the compositor drops its socket.
        self.thread.take();
    }
}

// ---- tests ---------------------------------------------------------------------------------

fn area(x: f64, y: f64, w: f64, h: f64) -> Rectangle<f64, smithay::utils::Logical> {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// The fields of a backdrop the protocol sets, without the cache identity.
fn summary(b: &[BackdropArea]) -> Vec<(String, Rectangle<f64, smithay::utils::Logical>, f64, f64)> {
    b.iter()
        .map(|b| (b.material.clone(), b.area, b.radius, b.alpha))
        .collect()
}

#[test]
fn backdrop_applies_with_the_commit_and_not_before() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the first backdrop commit", |s| {
        !layer_backdrops(&s.outputs[0].output).is_empty()
    });
    // surface origin is the margins (60, 50); the shape is relative to it
    let first = vec![("Menu".to_string(), area(70.0, 70.0, 300.0, 80.0), 16.0, 0.5)];
    assert_eq!(summary(&h.backdrops()), first);

    // new state without a commit: the committed one stays in effect
    client.send(Cmd::SetShape {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
        radius: 4.0,
    });
    client.send(Cmd::SetAlpha(1.0));
    for _ in 0..40 {
        h.step();
    }
    assert_eq!(summary(&h.backdrops()), first);

    // the commit applies it
    client.send(Cmd::Commit);
    h.wait_until("the second backdrop commit", |s| {
        layer_backdrops(&s.outputs[0].output)
            .iter()
            .any(|b| b.alpha == 1.0)
    });
    assert_eq!(
        summary(&h.backdrops()),
        vec![("Menu".to_string(), area(60.0, 50.0, 120.0, 40.0), 4.0, 1.0)]
    );
}

/// The surface that holds the keyboard focus of the seat, if any.
fn keyboard_focus(
    state: &Cupertino,
) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
    state.seat.get_keyboard().unwrap().current_focus()
}

#[test]
fn an_exclusive_layer_surface_takes_the_keyboard_and_gives_it_back() {
    use zwlr_layer_surface_v1::KeyboardInteractivity;
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the layer surface", |s| {
        !layer_backdrops(&s.outputs[0].output).is_empty()
    });
    assert_eq!(keyboard_focus(&h.state), None);

    client.send(Cmd::SetKeyboard(KeyboardInteractivity::Exclusive));
    client.send(Cmd::Commit);
    h.wait_until("the keyboard focus", |s| keyboard_focus(s).is_some());
    let layer_surface = h
        .state
        .exclusive_layer
        .clone()
        .expect("the compositor tracks the exclusive layer");
    assert_eq!(keyboard_focus(&h.state), Some(layer_surface));

    client.send(Cmd::SetKeyboard(KeyboardInteractivity::None));
    client.send(Cmd::Commit);
    h.wait_until("the keyboard to be released", |s| {
        keyboard_focus(s).is_none()
    });
    assert_eq!(h.state.exclusive_layer, None);
}

/// Protocol error codes of `cupertino_backdrop_v1.error`.
const UNKNOWN_MATERIAL: u32 = 0;
const INVALID_SHAPE: u32 = 1;
const INVALID_ALPHA: u32 = 2;
const INVALID_GLASS: u32 = 3;

fn assert_posts_error(cmd: Cmd, code: u32) {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the first backdrop commit", |s| {
        !layer_backdrops(&s.outputs[0].output).is_empty()
    });
    client.send(cmd);
    let end = Instant::now() + common::HANG_GUARD;
    while client.backdrop_error().is_none() && Instant::now() < end {
        h.step();
    }
    assert_eq!(client.backdrop_error(), Some(code));
}

#[test]
fn unknown_material_is_a_protocol_error() {
    assert_posts_error(Cmd::SetMaterial("NoSuchMaterial"), UNKNOWN_MATERIAL);
}

#[test]
fn negative_size_or_radius_is_a_protocol_error() {
    assert_posts_error(
        Cmd::SetShape {
            x: 0,
            y: 0,
            width: -1,
            height: 10,
            radius: 0.0,
        },
        INVALID_SHAPE,
    );
    assert_posts_error(
        Cmd::SetShape {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            radius: -2.0,
        },
        INVALID_SHAPE,
    );
}

#[test]
fn alpha_outside_zero_to_one_is_a_protocol_error() {
    assert_posts_error(Cmd::SetAlpha(1.5), INVALID_ALPHA);
}

#[test]
fn negative_glass_spacing_is_a_protocol_error() {
    assert_posts_error(
        Cmd::SetGlass(cupertino_backdrop_v1::GlassStyle::Regular, 1, -4.0),
        INVALID_GLASS,
    );
}

/// `set_glass` and `set_material` replace each other: the one sent last decides how the backdrop draws.
#[test]
fn the_last_of_set_glass_and_set_material_decides_the_look() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path());
    h.wait_until("the first backdrop commit", |s| {
        !layer_backdrops(&s.outputs[0].output).is_empty()
    });
    assert_eq!(h.backdrops()[0].glass, None);
    client.send(Cmd::SetGlass(
        cupertino_backdrop_v1::GlassStyle::Clear,
        2,
        12.0,
    ));
    client.send(Cmd::Commit);
    h.wait_until("the glass commit", |s| {
        layer_backdrops(&s.outputs[0].output)
            .iter()
            .any(|b| b.glass.is_some())
    });
    let glass = h.backdrops()[0].glass.unwrap();
    assert_eq!(
        (glass.style, glass.container, glass.spacing),
        (cupertino_theme::GlassStyle::Clear, 2, 12.0)
    );
    client.send(Cmd::SetMaterial("Menu"));
    client.send(Cmd::Commit);
    h.wait_until("the material commit", |s| {
        layer_backdrops(&s.outputs[0].output)
            .iter()
            .all(|b| b.glass.is_none())
    });
    assert_eq!(summary(&h.backdrops())[0].0, "Menu");
    // a glass material name is glass on its own again
    client.send(Cmd::SetMaterial("Glass"));
    client.send(Cmd::Commit);
    h.wait_until("the glass material commit", |s| {
        layer_backdrops(&s.outputs[0].output)
            .iter()
            .any(|b| b.glass.is_some())
    });
    assert_eq!(h.backdrops()[0].glass.unwrap().container, 0);
    assert_eq!(client.backdrop_error(), None);
}

/// The GLES material pass (shaders ported from `cupertino-glass`) against the CPU reference of the
/// same pipeline: a square backdrop strip over the procedural wallpaper must match within
/// quantisation.
#[test]
fn backdrop_material_pass_matches_the_cpu_reference() {
    use cupertino_glass::cpu::render_regions;
    use cupertino_glass::MaterialRegion;
    use cupertino_theme::{Appearance, MaterialOptions, Theme};
    use smithay::input::pointer::CursorImageStatus;

    const STRIP_HEIGHT: i32 = 24;
    let strip = Layout {
        size: (OUTPUT.0, STRIP_HEIGHT),
        margin_top: 0,
        margin_left: 0,
        material: "MenuBar",
        shape: (0, 0, OUTPUT.0, STRIP_HEIGHT, 0.0),
        alpha: 1.0,
        layer: zwlr_layer_shell_v1::Layer::Top,
        paint: None,
    };
    let theme = Theme::load().expect("specification files");
    for (material, dark) in [
        ("MenuBar", false),
        ("MenuBar", true),
        ("Sidebar", false),
        ("Menu", true),
    ] {
        let appearance = if dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        let shoot = |with_strip: bool| -> Option<image::RgbaImage> {
            let mut h = Harness::with_config(|c| c.appearance = appearance)?;
            h.state.cursor_status = CursorImageStatus::Hidden;
            let client = with_strip
                .then(|| TestClient::spawn_with(h.socket_path(), Layout { material, ..strip }));
            if client.is_some() {
                h.wait_until("the strip backdrop", |s| {
                    !layer_backdrops(&s.outputs[0].output).is_empty()
                });
            }
            for _ in 0..3 {
                h.step();
            }
            Some(h.screenshot(if with_strip { "gpu" } else { "wall" }))
        };
        let (Some(wall), Some(gpu)) = (shoot(false), shoot(true)) else {
            return;
        };
        let recipe = theme
            .recipe(appearance, material, &MaterialOptions::default())
            .unwrap();
        let region = MaterialRegion::new([0.0, 0.0, OUTPUT.0 as f32, STRIP_HEIGHT as f32], &recipe);
        let tint = cupertino_server::render::scene::wallpaper_tint(dark);
        let cpu = render_regions(&wall, &[region], 1.0, tint, false, false);
        let (mut sum, mut max, mut n) = (0.0f64, 0i32, 0usize);
        for y in 0..STRIP_HEIGHT as u32 {
            for x in 0..OUTPUT.0 as u32 {
                for c in 0..3 {
                    let d = (gpu.get_pixel(x, y)[c] as i32 - cpu.get_pixel(x, y)[c] as i32).abs();
                    sum += d as f64;
                    max = max.max(d);
                    n += 1;
                }
            }
        }
        let mean = sum / n as f64;
        eprintln!("{material} dark={dark}: mean abs diff {mean:.3}, max {max}");
        assert!(
            mean < 1.5 && max <= 16,
            "{material} dark={dark}: GLES vs CPU mean {mean:.3} max {max}"
        );
        // the strip must differ from the plain wallpaper (the pass really ran)
        let changed = (0..OUTPUT.0 as u32)
            .filter(|x| gpu.get_pixel(*x, 4) != wall.get_pixel(*x, 4))
            .count();
        assert!(
            changed > OUTPUT.0 as usize / 2,
            "{material}: strip unchanged"
        );
    }
}

/// The GLES glass pass against the CPU model of `cupertino-glass`: a rounded glass panel over the procedural wallpaper,
/// whose colours change slowly enough that every blur level of it is the wallpaper itself, must match it within the
/// error of the half-scale capture, in each appearance and with each accessibility recipe.
#[test]
fn glass_pass_matches_the_cpu_model() {
    use cupertino_defaults::accessibility::DisplayOptions;
    use cupertino_glass::glass::{Container, GlassShape};
    use cupertino_theme::{Appearance, GlassStyle};
    use smithay::input::pointer::CursorImageStatus;

    let panel = Layout {
        size: (400, 160),
        margin_top: 260,
        margin_left: 300,
        material: "Glass",
        shape: (0, 0, 400, 160, 24.0),
        alpha: 1.0,
        layer: zwlr_layer_shell_v1::Layer::Top,
        paint: None,
    };
    let options = |rt: bool, ic: bool| DisplayOptions {
        reduce_transparency: rt,
        increase_contrast: ic,
        ..DisplayOptions::default()
    };
    for (dark, display) in [
        (false, options(false, false)),
        (true, options(false, false)),
        (true, options(true, false)),
        (false, options(false, true)),
    ] {
        let appearance = if dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        let shoot = |with_panel: bool| -> Option<image::RgbaImage> {
            let mut h = Harness::with_config(|c| c.appearance = appearance)?;
            h.state.cursor_status = CursorImageStatus::Hidden;
            h.state.accessibility.settings.display = display;
            let client = with_panel.then(|| TestClient::spawn_with(h.socket_path(), panel));
            if client.is_some() {
                h.wait_until("the glass backdrop", |s| {
                    !layer_backdrops(&s.outputs[0].output).is_empty()
                });
            }
            for _ in 0..3 {
                h.step();
            }
            Some(h.screenshot(if with_panel { "glass" } else { "wall" }))
        };
        let (Some(wall), Some(gpu)) = (shoot(false), shoot(true)) else {
            return;
        };
        let at = |x: f32, y: f32| -> [f32; 3] {
            let x = (x.floor() as i64).clamp(0, OUTPUT.0 as i64 - 1) as u32;
            let y = (y.floor() as i64).clamp(0, OUTPUT.1 as i64 - 1) as u32;
            let p = wall.get_pixel(x, y);
            [0, 1, 2].map(|c| p[c] as f32 / 255.0)
        };
        let mode = cupertino_server::accessibility::glass_mode(dark, GlassStyle::Regular, display);
        let (x0, y0) = (panel.margin_left as f32, panel.margin_top as f32);
        let container = Container::new(
            vec![GlassShape::new([x0, y0, 400.0, 160.0], 24.0)],
            0.0,
            mode,
        );
        let (mut sum, mut max, mut n) = (0.0f64, 0i32, 0usize);
        let mut changed = 0;
        for y in panel.margin_top - 4..panel.margin_top + 164 {
            for x in panel.margin_left - 4..panel.margin_left + 404 {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let (c, cov) = container.shade(p, &|_, q| at(q[0], q[1]));
                let under = at(p[0], p[1]);
                let got = gpu.get_pixel(x as u32, y as u32);
                for i in 0..3 {
                    let want = ((under[i] * (1.0 - cov) + c[i] * cov) * 255.0).round() as i32;
                    let d = (got[i] as i32 - want).abs();
                    sum += d as f64;
                    max = max.max(d);
                    n += 1;
                }
                if got != wall.get_pixel(x as u32, y as u32) {
                    changed += 1;
                }
            }
        }
        let mean = sum / n as f64;
        eprintln!("glass dark={dark} {display:?}: mean abs diff {mean:.3}, max {max}");
        assert!(
            mean < 1.0 && max <= 8,
            "glass dark={dark} {display:?}: GLES vs CPU mean {mean:.3} max {max}"
        );
        assert!(changed > 400 * 160 / 2, "the glass pass ran");
        // the rim light lies along the light's axis: the top and the bottom edge lit, the sides not (measured on dark
        // glass at rest, where the light shows most)
        if !dark || display.reduce_transparency || display.increase_contrast {
            continue;
        }
        let (cx, cy) = (x0 as u32 + 200, y0 as u32 + 80);
        let lum = |x: u32, y: u32| {
            let p = gpu.get_pixel(x, y);
            p[0] as i32 + p[1] as i32 + p[2] as i32
        };
        let (top, bottom) = (y0 as u32, y0 as u32 + 159);
        let top_lift = lum(cx, top) - lum(cx, top + 3);
        let bottom_lift = lum(cx, bottom) - lum(cx, bottom - 3);
        let side_lift = lum(x0 as u32, cy) - lum(x0 as u32 + 3, cy);
        assert!(top_lift > 12, "the top rim is lit: {top_lift}");
        assert!(
            bottom_lift > top_lift / 2,
            "the bottom rim is lit too: {bottom_lift} of {top_lift}"
        );
        assert!(side_lift < top_lift / 3, "the sides are not: {side_lift}");
    }
}

/// Window-like content under the glass: dark grey with icon-sized blue squares and thin white bars, local to a
/// 600 × 320 surface; one square straddles the glass panel's left edge.
fn icons(x: i32, y: i32) -> [u8; 4] {
    let square = |x0: i32, y0: i32| (x0..x0 + 52).contains(&x) && (y0..y0 + 52).contains(&y);
    if square(76, 100) || square(210, 60) || square(330, 150) || square(470, 90) {
        [64, 140, 230, 255]
    } else if x % 120 == 0 || x % 120 == 1 || y == 200 {
        [240, 240, 240, 255]
    } else {
        [43, 43, 43, 255]
    }
}

/// `img` blurred with a Gaussian of standard deviation `sigma` pixels (three deviations each way, edges clamped),
/// on the encoded values, as the glass pass blurs its capture.
fn gaussian(img: &image::RgbaImage, sigma: f32) -> Vec<[f32; 3]> {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let r = (3.0 * sigma).ceil() as i32;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    let at = |x: i32, y: i32| {
        let p = img.get_pixel(x.clamp(0, w - 1) as u32, y.clamp(0, h - 1) as u32);
        [0, 1, 2].map(|c| p[c] as f32 / 255.0)
    };
    let mut tmp = vec![[0.0f32; 3]; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut a = [0.0; 3];
            for (i, kv) in k.iter().enumerate() {
                let p = at(x + i as i32 - r, y);
                for c in 0..3 {
                    a[c] += p[c] * kv / sum;
                }
            }
            tmp[(y * w + x) as usize] = a;
        }
    }
    let mut out = vec![[0.0f32; 3]; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut a = [0.0; 3];
            for (i, kv) in k.iter().enumerate() {
                let yy = (y + i as i32 - r).clamp(0, h - 1);
                let p = tmp[(yy * w + x) as usize];
                for c in 0..3 {
                    a[c] += p[c] * kv / sum;
                }
            }
            out[(y * w + x) as usize] = a;
        }
    }
    out
}

/// The GLES glass pass over sharp content against the CPU model: icon-sized squares and thin bars of a surface below
/// a dark glass panel, one square under its left edge, where the refraction bends what lies past the edge into the
/// rim. Each blur level and the blur fill of the model are Gaussians of the shot without the glass. The comparison
/// sees where the pass reads its backdrop: shifting the model's backdrop by 2 pixels breaks it.
#[test]
fn glass_pass_over_sharp_content_reads_the_backdrop_where_the_model_does() {
    use cupertino_glass::glass::{Container, GlassShape};
    use cupertino_theme::{Appearance, GlassMode};
    use smithay::backend::renderer::utils::with_renderer_surface_state;
    use smithay::desktop::layer_map_for_output;
    use smithay::input::pointer::CursorImageStatus;

    let content = Layout {
        size: (600, 320),
        margin_top: 180,
        margin_left: 200,
        material: "Menu",
        shape: (0, 0, 0, 0, 0.0),
        alpha: 1.0,
        layer: zwlr_layer_shell_v1::Layer::Bottom,
        paint: Some(icons),
    };
    let panel = Layout {
        size: (400, 160),
        margin_top: 260,
        margin_left: 300,
        material: "Glass",
        shape: (0, 0, 400, 160, 24.0),
        alpha: 1.0,
        layer: zwlr_layer_shell_v1::Layer::Top,
        paint: None,
    };
    let shoot = |with_panel: bool| -> Option<image::RgbaImage> {
        let mut h = Harness::with_config(|c| c.appearance = Appearance::Dark)?;
        h.state.cursor_status = CursorImageStatus::Hidden;
        let _content = TestClient::spawn_with(h.socket_path(), content);
        let _panel = with_panel.then(|| TestClient::spawn_with(h.socket_path(), panel));
        // a layer is mapped by its first commit, before its buffer: wait for every buffer, or a loaded machine shoots
        // the desktop before the content is drawn
        h.wait_until("the surfaces with their buffers", |s| {
            let map = layer_map_for_output(&s.outputs[0].output);
            let layers: Vec<_> = map.layers().collect();
            layers.len() == 1 + usize::from(with_panel)
                && layers.iter().all(|l| {
                    with_renderer_surface_state(l.wl_surface(), |st| st.buffer().is_some())
                        .unwrap_or(false)
                })
        });
        if with_panel {
            h.wait_until("the glass backdrop", |s| {
                !layer_backdrops(&s.outputs[0].output).is_empty()
            });
        }
        for _ in 0..6 {
            h.step();
        }
        Some(h.screenshot(if with_panel {
            "sharp-glass"
        } else {
            "sharp-wall"
        }))
    };
    let (Some(wall), Some(gpu)) = (shoot(false), shoot(true)) else {
        return;
    };
    assert_eq!(
        wall.get_pixel(300 + 10, 260 + 30)[2],
        230,
        "the content is under the panel"
    );
    let mode = GlassMode {
        dark: true,
        ..GlassMode::default()
    };
    let (x0, y0) = (panel.margin_left as f32, panel.margin_top as f32);
    let container = Container::new(
        vec![GlassShape::new([x0, y0, 400.0, 160.0], 24.0)],
        0.0,
        mode,
    );
    let p = &container.params;
    let mut sigmas: Vec<f32> = p.level_radii().to_vec();
    sigmas.push(p.blur.fill_radius);
    // only what the panel reads: its bounds and the reach of the widest level and the refraction
    let reach = p.reach().ceil() as i32;
    let (cx, cy) = (panel.margin_left - reach, panel.margin_top - reach);
    let (w, hgt) = (400 + 2 * reach, 160 + 2 * reach);
    let read =
        image::imageops::crop_imm(&wall, cx as u32, cy as u32, w as u32, hgt as u32).to_image();
    let blurred: Vec<Vec<[f32; 3]>> = sigmas.iter().map(|s| gaussian(&read, *s)).collect();
    let compare = |shift: f32| {
        let sample = |level: usize, q: [f32; 2]| -> [f32; 3] {
            // bilinear between pixel centres
            let (fx, fy) = (q[0] - 0.5 + shift - cx as f32, q[1] - 0.5 - cy as f32);
            let (ix, iy) = (fx.floor() as i32, fy.floor() as i32);
            let (tx, ty) = (fx - ix as f32, fy - iy as f32);
            let px = |x: i32, y: i32| {
                blurred[level][(y.clamp(0, hgt - 1) * w + x.clamp(0, w - 1)) as usize]
            };
            let (a, b, c, d) = (
                px(ix, iy),
                px(ix + 1, iy),
                px(ix, iy + 1),
                px(ix + 1, iy + 1),
            );
            std::array::from_fn(|i| {
                let top = a[i] + (b[i] - a[i]) * tx;
                let bottom = c[i] + (d[i] - c[i]) * tx;
                top + (bottom - top) * ty
            })
        };
        let (mut sum, mut max, mut n) = (0.0f64, 0i32, 0usize);
        // inside the panel, away from its antialiased outline
        for y in panel.margin_top + 2..panel.margin_top + 158 {
            for x in panel.margin_left + 2..panel.margin_left + 398 {
                let q = [x as f32 + 0.5, y as f32 + 0.5];
                let (c, cov) = container.shade(q, &sample);
                if cov < 1.0 {
                    continue;
                }
                let got = gpu.get_pixel(x as u32, y as u32);
                for i in 0..3 {
                    let d = (got[i] as i32 - (c[i] * 255.0).round() as i32).abs();
                    sum += d as f64;
                    max = max.max(d);
                    n += 1;
                }
            }
        }
        (sum / n as f64, max)
    };
    let (mean, max) = compare(0.0);
    let (shifted_mean, shifted_max) = compare(2.0);
    eprintln!(
        "glass over sharp content: mean {mean:.3} max {max}; model shifted 2 px: mean {shifted_mean:.3} max {shifted_max}"
    );
    assert!(
        mean < 1.0 && max <= 12,
        "GLES glass over sharp content vs CPU model: mean {mean:.3} max {max}"
    );
    assert!(
        shifted_max > 2 * max.max(6),
        "the comparison must see a 2 px offset: {shifted_max} vs {max}"
    );
}

/// Reduce Transparency and Increase Contrast turn a translucent menu-bar strip into the opaque fill
/// of the material's `-Opaque` variant (from the light and from the high-contrast catalog), and
/// turning them off brings the wallpaper back.
#[test]
fn reduce_transparency_and_increase_contrast_draw_the_material_opaque() {
    use cupertino_defaults::accessibility::DisplayOption;
    use cupertino_theme::{Appearance, MaterialOptions, Theme};
    use smithay::input::pointer::CursorImageStatus;

    const STRIP_HEIGHT: i32 = 24;
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    let strip = Layout {
        size: (OUTPUT.0, STRIP_HEIGHT),
        margin_top: 0,
        margin_left: 0,
        material: "MenuBar",
        shape: (0, 0, OUTPUT.0, STRIP_HEIGHT, 0.0),
        alpha: 1.0,
        layer: zwlr_layer_shell_v1::Layer::Top,
        paint: None,
    };
    let _client = TestClient::spawn_with(h.socket_path(), strip);
    h.wait_until("the strip backdrop", |s| {
        !layer_backdrops(&s.outputs[0].output).is_empty()
    });
    let row = |img: &image::RgbaImage| -> Vec<[u8; 3]> {
        (0..OUTPUT.0 as u32)
            .map(|x| {
                let p = img.get_pixel(x, STRIP_HEIGHT as u32 / 2);
                [p[0], p[1], p[2]]
            })
            .collect()
    };
    let theme = Theme::load().expect("specification files");
    let opaque_fill = |appearance: Appearance| -> [u8; 3] {
        let recipe = theme
            .recipe(
                appearance,
                "MenuBar",
                &MaterialOptions {
                    reduce_transparency: true,
                    ..MaterialOptions::default()
                },
            )
            .unwrap();
        assert!(
            recipe.opaque && recipe.backdrop.is_none(),
            "{}",
            recipe.name
        );
        let [r, g, b, a] = recipe.fills[0].rgba;
        assert_eq!(a, 1.0);
        [r, g, b].map(|c| (c * 255.0).round() as u8)
    };
    let uniform = |pixels: &[[u8; 3]], want: [u8; 3]| {
        pixels
            .iter()
            .all(|p| p.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 1))
    };

    let translucent = row(&h.screenshot("translucent"));
    let first = translucent[0];
    assert!(
        translucent
            .iter()
            .any(|p| p.iter().zip(first).any(|(a, b)| a.abs_diff(b) > 4)),
        "the translucent strip shows the wallpaper's gradient"
    );

    h.state
        .set_display_option(DisplayOption::ReduceTransparency, true);
    let reduced = row(&h.screenshot("reduce-transparency"));
    let want = opaque_fill(Appearance::Light);
    assert!(
        uniform(&reduced, want),
        "every pixel of the strip is the opaque fill {want:?}: {:?}",
        &reduced[..4]
    );

    h.state
        .set_display_option(DisplayOption::ReduceTransparency, false);
    h.state
        .set_display_option(DisplayOption::IncreaseContrast, true);
    let contrast = row(&h.screenshot("increase-contrast"));
    let want_contrast = opaque_fill(Appearance::LightHighContrast);
    assert_ne!(
        want_contrast, want,
        "the high-contrast catalog has its own opaque fill"
    );
    assert!(
        uniform(&contrast, want_contrast),
        "the strip is the high-contrast opaque fill {want_contrast:?}: {:?}",
        &contrast[..4]
    );

    h.state
        .set_display_option(DisplayOption::IncreaseContrast, false);
    assert_eq!(row(&h.screenshot("translucent-again")), translucent);
}

// ---- popup backdrops -------------------------------------------------------------------------

use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface, xdg_wm_base};

/// Popup of the layer surface: position relative to the layer surface, and size. Its backdrop
/// covers the whole popup.
const POPUP: (i32, i32, i32, i32) = (100, 50, 200, 100);

/// A shell client with a backdrop-less layer surface (`SURFACE` at the margins) and one xdg popup
/// on it whose surface carries a "Menu" backdrop.
struct PopupClient {
    shm: wl_shm::WlShm,
    layer_surface: wl_surface::WlSurface,
    popup_surface: wl_surface::WlSurface,
    backdrop: CupertinoBackdropV1,
}

impl PopupClient {
    fn attach(&self, surface: &wl_surface::WlSurface, (w, h): (i32, i32), qh: &QueueHandle<Self>) {
        let mut file = tempfile_like((w * h * 4) as usize);
        file.write_all(&vec![0u8; (w * h * 4) as usize]).unwrap();
        let pool = self.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
    }

    fn spawn(socket: PathBuf) -> JoinHandle<()> {
        std::thread::spawn(move || {
            let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<PopupClient>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 3..=6, ()).unwrap();
            let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 1..=1, ()).unwrap();
            let layer_surface = compositor.create_surface(&qh, ());
            let layer = layer_shell.get_layer_surface(
                &layer_surface,
                None,
                zwlr_layer_shell_v1::Layer::Top,
                "popup-test".into(),
                &qh,
                (),
            );
            layer.set_size(SURFACE.0 as u32, SURFACE.1 as u32);
            layer.set_anchor(
                zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left,
            );
            layer.set_margin(MARGIN_TOP, 0, 0, MARGIN_LEFT);
            layer.set_exclusive_zone(-1);
            layer_surface.commit();
            let popup_surface = compositor.create_surface(&qh, ());
            let backdrop = effects.get_backdrop(&popup_surface, &qh, ());
            let mut client = PopupClient {
                shm,
                layer_surface,
                popup_surface,
                backdrop,
            };
            // the layer surface is mapped before its popup is created
            queue.roundtrip(&mut client).unwrap();
            let (x, y, w, h) = POPUP;
            let positioner = wm_base.create_positioner(&qh, ());
            positioner.set_size(w, h);
            positioner.set_anchor_rect(x, y, 1, 1);
            positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
            positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
            let xdg = wm_base.get_xdg_surface(&client.popup_surface, &qh, ());
            let popup = xdg.get_popup(None, &positioner, &qh, ());
            layer.get_popup(&popup);
            client.popup_surface.commit();
            while queue.roundtrip(&mut client).is_ok() {
                std::thread::sleep(Duration::from_millis(5));
            }
        })
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PopupClient {
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
macro_rules! ignore_popup_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for PopupClient {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_popup_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1,
    xdg_positioner::XdgPositioner,
    xdg_popup::XdgPopup
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for PopupClient {
    fn event(
        _: &mut Self,
        wm_base: &xdg_wm_base::XdgWmBase,
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

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for PopupClient {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            layer.ack_configure(serial);
            state.attach(&state.layer_surface, SURFACE, qh);
            state.layer_surface.commit();
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for PopupClient {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            let (_, _, w, h) = POPUP;
            state.attach(&state.popup_surface, (w, h), qh);
            state.backdrop.set_material("Menu".into());
            state.backdrop.set_shape(0, 0, w, h, 0.0);
            state.popup_surface.commit();
        }
    }
}

/// A popup's backdrop is drawn behind the popup, at the popup's position, with its shadow around it and
/// nowhere else on its parent layer surface.
#[test]
fn popup_backdrop_draws_under_the_popup() {
    use smithay::input::pointer::CursorImageStatus;

    let shoot = |with_popup: bool| -> Option<image::RgbaImage> {
        let mut h = Harness::new()?;
        h.state.cursor_status = CursorImageStatus::Hidden;
        if with_popup {
            let _client = PopupClient::spawn(h.socket_path());
            for _ in 0..200 {
                h.step();
            }
        }
        Some(h.screenshot(if with_popup { "popup" } else { "wall" }))
    };
    let (Some(wall), Some(shot)) = (shoot(false), shoot(true)) else {
        return;
    };
    let (px, py, pw, ph) = POPUP;
    let (ox, oy) = (MARGIN_LEFT + px, MARGIN_TOP + py);
    let changed = |x0: i32, y0: i32, w: i32, h: i32| -> f64 {
        let mut n = 0;
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                n += usize::from(
                    wall.get_pixel(x as u32, y as u32) != shot.get_pixel(x as u32, y as u32),
                );
            }
        }
        n as f64 / (w * h) as f64
    };
    // inside the popup, inset past the blur's reach at the edges
    assert!(
        changed(ox + 10, oy + 10, pw - 20, ph - 20) > 0.9,
        "popup area shows no material"
    );
    // the popup casts its shadow (the panel row of the table: σ 8, offset 4) just below its edge ...
    assert!(
        changed(ox + 20, oy + ph + 1, pw - 40, 4) > 0.9,
        "no shadow below the popup"
    );
    // ... and the parent's area beyond the shadow's reach (σ 8 reaches 23 px) is untouched
    assert_eq!(
        changed(MARGIN_LEFT + 5, MARGIN_TOP + 5, px - 40, SURFACE.1 - 10),
        0.0,
        "material outside the popup"
    );
}

// ---- backdrop limits -----------------------------------------------------------------------

/// What a client does in the limit tests, in order.
#[derive(Clone, Copy)]
enum Step {
    /// A new surface.
    Surface,
    /// A backdrop on the surface with this index.
    Backdrop(usize),
    /// Destroy the backdrop with this index (in creation order).
    Destroy(usize),
}

struct Bare;

macro_rules! ignore_bare_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Bare {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_bare_events!(
    wl_compositor::WlCompositor,
    wl_surface::WlSurface,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1
);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Bare {
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

/// A client that runs `steps` and then waits one round trip: the code of the protocol error the server posted on the
/// effects object, or `None` when it stayed connected.
fn run_steps(socket: PathBuf, steps: Vec<Step>) -> JoinHandle<Option<u32>> {
    std::thread::spawn(move || {
        let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<Bare>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let mut surfaces = Vec::new();
        let mut backdrops = Vec::new();
        for step in steps {
            match step {
                Step::Surface => surfaces.push(compositor.create_surface(&qh, ())),
                Step::Backdrop(surface) => {
                    backdrops.push(effects.get_backdrop(&surfaces[surface], &qh, ()))
                }
                Step::Destroy(backdrop) => backdrops[backdrop].destroy(),
            }
        }
        match queue.roundtrip(&mut Bare) {
            Ok(_) => None,
            Err(_) => {
                let error = conn.protocol_error().expect("a protocol error");
                assert_eq!(
                    error.object_interface,
                    CupertinoAppEffectsV1::interface().name
                );
                Some(error.code)
            }
        }
    })
}

fn finish<T>(h: &mut Harness, client: JoinHandle<T>) -> T {
    while !client.is_finished() {
        h.step();
    }
    client.join().unwrap()
}

/// Protocol error code `too_many_backdrops` of `cupertino_app_effects_v1.error`.
const TOO_MANY_BACKDROPS: u32 = 2;

#[test]
fn a_surface_holds_64_backdrops_and_the_next_is_a_protocol_error() {
    let Some(mut h) = Harness::new() else { return };
    let mut steps = vec![Step::Surface];
    steps.extend((0..64).map(|_| Step::Backdrop(0)));
    let ok = run_steps(h.socket_path(), steps.clone());
    assert_eq!(finish(&mut h, ok), None);
    steps.push(Step::Backdrop(0));
    let over = run_steps(h.socket_path(), steps);
    assert_eq!(finish(&mut h, over), Some(TOO_MANY_BACKDROPS));
}

#[test]
fn a_client_holds_128_backdrops_over_its_surfaces_and_the_next_is_a_protocol_error() {
    let Some(mut h) = Harness::new() else { return };
    let mut steps = Vec::new();
    for surface in 0..2 {
        steps.push(Step::Surface);
        steps.extend((0..64).map(|_| Step::Backdrop(surface)));
    }
    let ok = run_steps(h.socket_path(), steps.clone());
    assert_eq!(finish(&mut h, ok), None);
    steps.push(Step::Surface);
    steps.push(Step::Backdrop(2));
    let over = run_steps(h.socket_path(), steps);
    assert_eq!(finish(&mut h, over), Some(TOO_MANY_BACKDROPS));
}

#[test]
fn a_destroyed_backdrop_frees_its_place() {
    let Some(mut h) = Harness::new() else { return };
    let mut steps = vec![Step::Surface];
    steps.extend((0..64).map(|_| Step::Backdrop(0)));
    steps.extend((0..64).map(Step::Destroy));
    steps.extend((0..64).map(|_| Step::Backdrop(0)));
    let client = run_steps(h.socket_path(), steps);
    assert_eq!(finish(&mut h, client), None);
}

#[test]
fn a_client_over_its_limit_does_not_take_the_places_of_another() {
    let Some(mut h) = Harness::new() else { return };
    let mut flood = vec![Step::Surface];
    flood.extend((0..65).map(|_| Step::Backdrop(0)));
    let flooder = run_steps(h.socket_path(), flood);
    assert_eq!(finish(&mut h, flooder), Some(TOO_MANY_BACKDROPS));
    let mut steps = vec![Step::Surface];
    steps.extend((0..64).map(|_| Step::Backdrop(0)));
    let other = run_steps(h.socket_path(), steps);
    assert_eq!(finish(&mut h, other), None);
}

// ---- disabled window controls --------------------------------------------------------------

use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::WindowControl;
use cupertino_server::app_effects_v1::{disabled_controls, DisabledControls};
use wayland_protocols::xdg::shell::client::xdg_toplevel;

ignore_bare_events!(
    xdg_wm_base::XdgWmBase,
    xdg_surface::XdgSurface,
    xdg_toplevel::XdgToplevel
);

/// Protocol error codes of `cupertino_app_effects_v1.error`.
const WINDOW: u32 = 1;
const INVALID_CONTROLS: u32 = 4;
/// How a client of the controls tests ended its round trip.
#[derive(Debug, PartialEq)]
enum Outcome {
    Connected,
    /// The code of the protocol error the server posted on the effects object.
    EffectsError(u32),
    /// The server ended the connection with an error the client does not keep, as `wl_display.invalid_method`.
    Ended,
}

/// A client on `cupertino_app_effects_v1` of `version` that sends `set_disabled_controls(disabled)` for a new surface,
/// an xdg toplevel when `toplevel`, commits the surface and waits one round trip: how the round trip ended, and the
/// connection, kept open so its window stays.
fn disable_controls(
    socket: PathBuf,
    version: u32,
    toplevel: bool,
    disabled: u32,
) -> JoinHandle<(Outcome, Connection)> {
    std::thread::spawn(move || {
        let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<Bare>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let effects: CupertinoAppEffectsV1 = globals.bind(&qh, version..=version, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let _role = toplevel.then(|| {
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            (xdg.get_toplevel(&qh, ()), xdg)
        });
        effects.set_disabled_controls(&surface, WindowControl::from_bits_retain(disabled));
        surface.commit();
        let outcome = match queue.roundtrip(&mut Bare) {
            Ok(_) => Outcome::Connected,
            Err(_) => match conn.protocol_error() {
                Some(error) => {
                    assert_eq!(
                        error.object_interface,
                        CupertinoAppEffectsV1::interface().name
                    );
                    Outcome::EffectsError(error.code)
                }
                None => Outcome::Ended,
            },
        };
        (outcome, conn)
    })
}

/// A client with a toplevel that sends `set_disabled_controls(minimize)` without a commit, then commits once for each
/// cue; it reports after the request and after each commit, each followed by a round trip.
fn commit_on_cue(socket: PathBuf) -> (Sender<()>, Receiver<()>, JoinHandle<Connection>) {
    let (cue, cues) = channel::<()>();
    let (done, dones) = channel();
    let client = std::thread::spawn(move || {
        let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<Bare>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
        let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 4..=4, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
        let _toplevel = xdg.get_toplevel(&qh, ());
        effects.set_disabled_controls(&surface, WindowControl::Minimize);
        queue.roundtrip(&mut Bare).unwrap();
        done.send(()).unwrap();
        while cues.recv().is_ok() {
            surface.commit();
            queue.roundtrip(&mut Bare).unwrap();
            done.send(()).unwrap();
        }
        conn
    });
    (cue, dones, client)
}

#[test]
fn disabled_controls_apply_with_the_next_commit_and_stay() {
    let Some(mut h) = Harness::new() else { return };
    let (cue, done, client) = commit_on_cue(h.socket_path());
    let reported = |h: &mut Harness| {
        while done.try_recv().is_err() {
            h.step();
        }
        disabled_controls(h.state.windows[0].toplevel.wl_surface())
    };
    let minimize = DisabledControls {
        minimize: true,
        zoom: false,
    };
    assert_eq!(
        reported(&mut h),
        DisabledControls::default(),
        "before the commit"
    );
    cue.send(()).unwrap();
    assert_eq!(reported(&mut h), minimize, "with the commit");
    cue.send(()).unwrap();
    assert_eq!(
        reported(&mut h),
        minimize,
        "a commit without a request keeps it"
    );
    drop(cue);
    finish(&mut h, client);
}

#[test]
fn disabling_controls_of_a_surface_that_is_not_a_toplevel_is_a_protocol_error() {
    let Some(mut h) = Harness::new() else { return };
    let client = disable_controls(h.socket_path(), 4, false, WindowControl::Zoom.bits());
    assert_eq!(finish(&mut h, client).0, Outcome::EffectsError(WINDOW));
}

#[test]
fn a_control_the_version_does_not_define_is_a_protocol_error() {
    let Some(mut h) = Harness::new() else { return };
    let client = disable_controls(h.socket_path(), 4, true, 4);
    assert_eq!(
        finish(&mut h, client).0,
        Outcome::EffectsError(INVALID_CONTROLS)
    );
}

#[test]
fn a_version_3_client_cannot_disable_controls() {
    let Some(mut h) = Harness::new() else { return };
    let client = disable_controls(h.socket_path(), 3, true, WindowControl::Minimize.bits());
    // wl_display.invalid_method: the request is newer than the object, and the server ends the connection
    assert_eq!(finish(&mut h, client).0, Outcome::Ended);
}
