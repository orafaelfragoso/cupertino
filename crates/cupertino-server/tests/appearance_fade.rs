//! A sidebar window through an appearance change, against an in-process compositor on an offscreen EGL device. The
//! window is drawn as a sidebar window is: its surface transparent but for a fine translucent line where the sidebar
//! meets the content, over two backdrops of the window (the sidebar's material on the left, the window background's
//! on the right). When the appearance changes, the window as it showed fades out over the window drawn again. Every
//! frame of that fade shows the sidebar, the line and the content where they were, each in a colour between its old
//! and its new one, starting from the window exactly as it showed. The tests step a fixed clock and skip (pass) when no
//! EGL device is available.

use crate::common;

use std::collections::BTreeSet;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::HANG_GUARD;
use cupertino_effects::TransformKind;
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::{
    CupertinoAppEffectsV1, TitlebarStyle,
};
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::CupertinoBackdropV1;
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_theme::Appearance;
use cupertino_wm::{Command, Rect, WindowId};
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (640, 440);
/// The window's size in points, which the server leaves to the client.
const SIZE: (i32, i32) = (480, 320);
/// Where the sidebar ends and the content begins, in points from the window's left edge.
const SPLIT: i32 = 200;
/// The line where the sidebar meets the content: black at this alpha, one device pixel wide, on the sidebar's side.
const LINE_ALPHA: u8 = 26;
/// Where the window is put, in output points.
const FRAME: (f64, f64) = (80.0, 60.0);
/// Frames sampled across the fade, its start and its end included.
const STEPS: u32 = 5;
/// How far a channel may stray from the colours the fade runs between (filtering and the blur's rounding).
const TOLERANCE: u8 = 4;

// ---- the compositor --------------------------------------------------------------------------------------------------

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new(scale: f64) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let config = Config {
            appearance: Appearance::Light,
            ..Config::default()
        };
        let mut state = Cupertino::new(
            Display::new().expect("creating the Wayland display"),
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        let display = HeadlessDisplay {
            name: "headless-1".into(),
            size: (
                (f64::from(OUTPUT.0) * scale).round() as i32,
                (f64::from(OUTPUT.1) * scale).round() as i32,
            ),
            physical_mm: (0, 0),
        };
        if let Err(e) = headless::init_displays(&mut state, &[display], Some(scale)) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        state.cursor_status = CursorImageStatus::Hidden;
        Some(Harness { state, event_loop })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Move the fixed clock to `time` and draw what it shows.
    fn at_time(&mut self, time: f64) {
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        self.step();
    }

    /// Let every animation finish.
    fn settle(&mut self) {
        let time = self.state.now() + 5.0;
        self.at_time(time);
    }

    fn socket(&self) -> PathBuf {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(self.state.socket_name.as_ref().unwrap())
    }

    /// Wait for the window `shown` counts the commits of, put it at [`FRAME`] and let it settle there.
    fn open(&mut self, shown: &Arc<AtomicUsize>) -> WindowId {
        let known: BTreeSet<WindowId> = self.state.windows.iter().map(|w| w.id).collect();
        let shown = shown.clone();
        self.wait_until("the window", move |s| {
            shown.load(Ordering::SeqCst) > 0
                && s.windows.iter().any(|w| w.mapped && !known.contains(&w.id))
        });
        let id = self
            .state
            .windows
            .iter()
            .rev()
            .find(|w| w.mapped)
            .unwrap()
            .id;
        let output = self.state.outputs[0].id;
        self.state.command(Command::MoveWindowToOutput {
            window: id,
            output,
            frame: Rect::new(FRAME.0, FRAME.1, f64::from(SIZE.0), f64::from(SIZE.1)),
        });
        self.settle();
        id
    }

    fn screenshot(&mut self) -> image::RgbaImage {
        static SHOTS: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-{}-appearance-fade.png",
            std::process::id(),
            SHOTS.fetch_add(1, Ordering::Relaxed)
        ));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let wanted = path.clone();
        self.wait_until("the screenshot", move |_| wanted.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }
}

// ---- the client ------------------------------------------------------------------------------------------------------

struct Client {
    shm: wl_shm::WlShm,
    root: wl_surface::WlSurface,
    root_xdg: xdg_surface::XdgSurface,
    effects: CupertinoAppEffectsV1,
    backdrops: Vec<CupertinoBackdropV1>,
    scale: i32,
    shown: Arc<AtomicUsize>,
}

impl Client {
    /// Draw the window: transparent but for the line, one device pixel wide just left of [`SPLIT`], over the
    /// sidebar's backdrop left of it and the window background's right of it.
    fn draw(&mut self, qh: &QueueHandle<Client>) {
        let s = self.scale;
        let (w, h) = (SIZE.0 * s, SIZE.1 * s);
        let mut bytes = vec![0u8; (w * h * 4) as usize];
        let line = SPLIT * s - 1;
        for y in 0..h {
            // ARGB8888, premultiplied, is B, G, R, A in memory: black at the line's alpha
            let i = ((y * w + line) * 4) as usize;
            bytes[i + 3] = LINE_ALPHA;
        }
        let fd =
            rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let mut file = std::fs::File::from(fd);
        file.write_all(&bytes).unwrap();
        let pool = self.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        if self.backdrops.is_empty() {
            for (material, x, width) in [
                ("Sidebar", 0, SPLIT),
                ("WindowBackground", SPLIT, SIZE.0 - SPLIT),
            ] {
                let backdrop = self.effects.get_backdrop(&self.root, qh, ());
                backdrop.set_material(material.into());
                backdrop.set_shape(x, 0, width, SIZE.1, 0.0);
                self.backdrops.push(backdrop);
            }
        }
        self.root.set_buffer_scale(s);
        self.root.attach(Some(&buffer), 0, 0);
        self.root.damage_buffer(0, 0, w, h);
        self.root.frame(qh, ());
        self.root.commit();
    }
}

/// A client with one sidebar window drawn at `scale`; returns the count of its commits the server presented.
fn spawn(socket: PathBuf, scale: i32) -> Arc<AtomicUsize> {
    let shown = Arc::new(AtomicUsize::new(0));
    let counted = shown.clone();
    std::thread::spawn(move || run(socket, scale, counted));
    shown
}

fn run(socket: PathBuf, scale: i32, shown: Arc<AtomicUsize>) {
    let conn = Connection::from_socket(UnixStream::connect(socket).expect("connecting")).unwrap();
    let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 6..=6, ()).unwrap();
    let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
    let decorations: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 6..=6, ()).unwrap();
    let root = compositor.create_surface(&qh, ());
    let root_xdg = wm_base.get_xdg_surface(&root, &qh, ());
    let toplevel = root_xdg.get_toplevel(&qh, ());
    toplevel.set_app_id("org.example.sidebar".into());
    toplevel.set_title("Sidebar".into());
    let decoration = decorations.get_toplevel_decoration(&toplevel, &qh, ());
    decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    effects.set_titlebar_style(&root, TitlebarStyle::Sidebar);
    root.commit();
    let mut client = Client {
        shm: globals.bind(&qh, 1..=1, ()).unwrap(),
        root,
        root_xdg,
        effects,
        backdrops: Vec::new(),
        scale,
        shown,
    };
    let _keep = (toplevel, decoration);
    while queue.blocking_dispatch(&mut client).is_ok() {}
    if let Some(e) = conn.protocol_error() {
        panic!("the client failed: {e:?}");
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
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
        impl Dispatch<$t, ()> for Client {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_surface::WlSurface,
    wl_output::WlOutput,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1
);

impl Dispatch<wl_callback::WlCallback, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.shown.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
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
            if *xdg == state.root_xdg {
                state.draw(qh);
            }
        }
    }
}

// ---- the tests -------------------------------------------------------------------------------------------------------

/// The sidebar just left of the line, the line, and the content just right of it, on the window's middle row, in a
/// screenshot of an output at `scale`.
fn divider(shot: &image::RgbaImage, scale: i32) -> [[u8; 3]; 3] {
    let line = ((FRAME.0 + f64::from(SPLIT)) * f64::from(scale)) as u32 - 1;
    let y = ((FRAME.1 + f64::from(SIZE.1) / 2.0) * f64::from(scale)) as u32;
    [line - 2, line, line + 2].map(|x| {
        let p = shot.get_pixel(x, y).0;
        [p[0], p[1], p[2]]
    })
}

/// Whether every channel of `c` lies between those of `a` and `b`, give or take [`TOLERANCE`].
fn between(c: [u8; 3], a: [u8; 3], b: [u8; 3]) -> bool {
    (0..3).all(|i| {
        let (lo, hi) = (a[i].min(b[i]), a[i].max(b[i]));
        c[i] >= lo.saturating_sub(TOLERANCE) && c[i] <= hi.saturating_add(TOLERANCE)
    })
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= TOLERANCE)
}

/// Light to dark at `scale`: the first frame of the fade is the window as it showed, and in every frame up to the
/// last the sidebar, the line and the content keep their places, each between its light and its dark colour.
fn the_divider_fades_in_place(scale: i32) {
    let Some(mut h) = Harness::new(f64::from(scale)) else {
        return;
    };
    let shown = spawn(h.socket(), scale);
    h.open(&shown);
    let light = divider(&h.screenshot(), scale);
    assert!(
        !near(light[0], light[2]),
        "the sidebar and the content differ in light: {light:?}"
    );
    let (start, fade) = (h.state.now(), TransformKind::AppearanceFade.duration());
    h.state.set_appearance(Appearance::Dark);
    let mut frames = Vec::new();
    for i in 0..=STEPS {
        h.at_time(start + fade * f64::from(i) / f64::from(STEPS));
        frames.push(divider(&h.screenshot(), scale));
    }
    h.settle();
    let dark = divider(&h.screenshot(), scale);
    assert!(
        (0..3).all(|j| near(frames[0][j], light[j])),
        "at {scale}x the fade starts from the window as it showed: {:?}, not {light:?}",
        frames[0]
    );
    for (i, frame) in frames.iter().enumerate() {
        for (j, part) in ["the sidebar", "the line", "the content"]
            .iter()
            .enumerate()
        {
            assert!(
                between(frame[j], light[j], dark[j]),
                "at {scale}x, frame {i} of the fade: {part} is {:?}, not between {:?} and {:?}",
                frame[j],
                light[j],
                dark[j]
            );
        }
    }
}

#[test]
fn a_sidebar_window_fades_to_dark_with_its_divider_in_place_at_1x() {
    the_divider_fades_in_place(1);
}

#[test]
fn a_sidebar_window_fades_to_dark_with_its_divider_in_place_at_2x() {
    the_divider_fades_in_place(2);
}
