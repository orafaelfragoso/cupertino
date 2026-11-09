//! A window's toolbar drawn in a subsurface, with its glass on the subsurface (`cupertino_app_effects_v1` version 11),
//! against an in-process compositor on an offscreen EGL device. The window's own surface paints a band of colour under
//! the toolbar row, as content scrolled under a toolbar does; the subsurface lies over the row, transparent but for a
//! glyph on its glass capsule. The glass draws above the window's content and reads it, and the glyph draws above the
//! glass. The tests wait on events (frames the server presented, the client's roundtrips), not on time, and skip (pass)
//! when no EGL device is available.

mod common;

use std::collections::BTreeSet;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::HANG_GUARD;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::{
    CupertinoAppEffectsV1, TitlebarStyle,
};
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1, Tone,
};
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::backend::Backend;
use cupertino_server::config::Config;
use cupertino_server::state::PointerMode;
use cupertino_server::Cupertino;
use cupertino_theme::Appearance;
use cupertino_wm::{Command, Point as MPoint, Rect, Requester, WindowId};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::Offscreen;
use smithay::backend::renderer::{gles::GlesTexture, Bind};
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Buffer, Physical, Rectangle, Size, Transform};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_region, wl_registry, wl_shm, wl_shm_pool,
    wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (900, 600);
/// The window's size, which the server leaves to the client.
const SIZE: (i32, i32) = (480, 300);
/// The toolbar row the subsurface covers, from the window's top edge.
const BAND: i32 = 52;
/// The glass capsule on the subsurface, subsurface-local (x, y, width, height), clear of the window's buttons.
const CAPSULE: (i32, i32, i32, i32) = (180, 8, 120, 36);
/// The glyph the subsurface draws on the capsule, subsurface-local.
const GLYPH: (i32, i32, i32, i32) = (192, 20, 12, 12);
/// The rows of the window's content a change far below the row repaints.
const STRIP: (i32, i32) = (200, 240);
const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 200, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const GRAY: [u8; 3] = [128, 128, 128];

// ---- the compositor --------------------------------------------------------------------------------------------------

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

/// A display of the harness: its name, its size in points and its scale.
type Screen = (&'static str, (i32, i32), f64);

impl Harness {
    fn new(appearance: Appearance, screens: &[Screen]) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let config = Config {
            appearance,
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
        let display = |(name, size, scale): Screen| HeadlessDisplay {
            name: name.into(),
            size: (
                (f64::from(size.0) * scale).round() as i32,
                (f64::from(size.1) * scale).round() as i32,
            ),
            physical_mm: (0, 0),
        };
        if let Err(e) =
            headless::init_displays(&mut state, &[display(screens[0])], Some(screens[0].2))
        {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        for &screen in &screens[1..] {
            headless::add_display(&mut state, &display(screen), Some(screen.2)).unwrap();
        }
        state.cursor_status = CursorImageStatus::Hidden;
        Some(Harness { state, event_loop })
    }

    fn one(appearance: Appearance, scale: f64) -> Option<Harness> {
        Self::new(appearance, &[("headless-1", OUTPUT, scale)])
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

    /// Let every animation finish.
    fn settle(&mut self) {
        let time = self.state.now() + 5.0;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        self.step();
    }

    fn socket(&self) -> PathBuf {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(self.state.socket_name.as_ref().unwrap())
    }

    /// Open `app`'s window and wait until it shows its toolbar subsurface.
    fn open(&mut self, app: &App) -> WindowId {
        let known: BTreeSet<WindowId> = self.state.windows.iter().map(|w| w.id).collect();
        let progress = app.progress.clone();
        self.wait_until("the window and its toolbar", move |s| {
            progress.shown.load(Ordering::SeqCst) > 0
                && s.windows.iter().any(|w| w.mapped && !known.contains(&w.id))
        });
        self.settle();
        let id = self
            .state
            .windows
            .iter()
            .rev()
            .find(|w| w.mapped)
            .unwrap()
            .id;
        id
    }

    /// Put `window` with its content at `frame` (points) on output `idx`.
    fn put(&mut self, window: WindowId, idx: usize, frame: Rect) {
        let output = self.state.outputs[idx].id;
        self.state.command(Command::MoveWindowToOutput {
            window,
            output,
            frame,
        });
        self.settle();
    }

    /// Drag `window` from `from` to `to` (output points) and let go.
    fn drag(&mut self, window: WindowId, from: (f64, f64), to: (f64, f64)) {
        let moved = |h: &mut Harness, x: f64, y: f64| {
            let time = (h.state.now() * 1000.0) as u32;
            h.state.pointer_moved_to((x, y).into(), time, None);
        };
        moved(self, from.0, from.1);
        self.state.begin_model_interaction(Command::BeginMove {
            window,
            pointer: MPoint::new(from.0, from.1),
        });
        for i in 1..=10 {
            let t = f64::from(i) / 10.0;
            moved(
                self,
                from.0 + (to.0 - from.0) * t,
                from.1 + (to.1 - from.1) * t,
            );
            self.step();
        }
        self.state.command(Command::PointerRelease);
        self.state.pointer_mode = PointerMode::Normal;
        self.settle();
    }

    fn frame(&self, window: WindowId) -> Rect {
        self.state.wm.window(window).unwrap().geometry()
    }

    /// Wait until every commit asked of `app` was presented, then until the frames after it are drawn.
    fn presented(&mut self, app: &App) {
        let (asked, progress) = (app.asked.get(), app.progress.clone());
        self.wait_until("the asked commits presented", move |_| {
            progress.answered.load(Ordering::SeqCst) >= asked
        });
        self.settle();
    }

    /// One image per output, the first output's first.
    fn screenshots(&mut self) -> Vec<image::RgbaImage> {
        static SHOTS: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-{}-subsurface.png",
            std::process::id(),
            SHOTS.fetch_add(1, Ordering::Relaxed)
        ));
        self.state.screenshot_each_output = true;
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let names: Vec<PathBuf> = self
            .state
            .outputs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                if i == 0 {
                    path.clone()
                } else {
                    let stem = path.file_stem().unwrap().to_string_lossy();
                    path.with_file_name(format!("{stem}-{}.png", o.output.name()))
                }
            })
            .collect();
        let wanted = names.clone();
        self.wait_until("the screenshots", move |_| {
            wanted.iter().all(|p| p.exists())
        });
        names
            .into_iter()
            .map(|p| {
                let img = image::open(&p).unwrap().to_rgba8();
                let _ = std::fs::remove_file(p);
                img
            })
            .collect()
    }

    fn screenshot(&mut self) -> image::RgbaImage {
        self.screenshots().remove(0)
    }
}

/// The colour at window point (x, y) of `window` (`frame`) in a screenshot of an output at `scale` whose top left
/// corner is at `origin` (points).
fn at(
    shot: &image::RgbaImage,
    frame: Rect,
    origin: (f64, f64),
    scale: f64,
    (x, y): (f64, f64),
) -> [u8; 3] {
    let px = ((frame.x - origin.0 + x) * scale).floor() as u32;
    let py = ((frame.y - origin.1 + y) * scale).floor() as u32;
    let p = shot.get_pixel(px, py).0;
    [p[0], p[1], p[2]]
}

fn close(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= tolerance)
}

/// The middle of the capsule, away from the glyph, window-local.
fn capsule_face() -> (f64, f64) {
    let (x, y, w, h) = CAPSULE;
    (f64::from(x + w) - 24.0, f64::from(y + h / 2))
}

fn glyph_middle() -> (f64, f64) {
    let (x, y, w, h) = GLYPH;
    (f64::from(x + w / 2), f64::from(y + h / 2))
}

/// A point of the toolbar row away from the capsule and the window's buttons.
fn bare_row() -> (f64, f64) {
    (f64::from(CAPSULE.0 + CAPSULE.2) + 60.0, f64::from(BAND / 2))
}

// ---- the client ------------------------------------------------------------------------------------------------------

/// What the test asks of the client, one commit of the window's surface each: the band under the toolbar row in
/// another colour, damaged alone, or the strip far below it repainted, damaged alone.
enum Ask {
    Band([u8; 3]),
    Strip([u8; 3]),
}

/// What the client did and heard.
#[derive(Default)]
struct Progress {
    /// Commits of the window's surface the server presented.
    shown: AtomicUsize,
    /// Of them, the commits the test asked for.
    answered: AtomicUsize,
    /// The outputs the subsurface is on (`wl_surface.enter` less `leave`), by the output's protocol id.
    sub_on: Mutex<BTreeSet<u32>>,
    /// The subsurface's last preferred buffer scale.
    sub_scale: AtomicUsize,
    /// The tones the capsule's backdrop was sent.
    tones: Mutex<Vec<Tone>>,
}

struct App {
    asks: Sender<Ask>,
    asked: std::cell::Cell<usize>,
    progress: Arc<Progress>,
}

impl App {
    /// A client with one unified window whose toolbar is a subsurface carrying a glass capsule, made through
    /// `cupertino_app_effects_v1` at `version`. The window's content is white with `band` under the toolbar row.
    fn spawn(socket: PathBuf, version: u32, band: [u8; 3]) -> App {
        let (asks, rx) = channel();
        let progress = Arc::new(Progress::default());
        let shared = progress.clone();
        std::thread::spawn(move || run(socket, version, band, rx, shared));
        App {
            asks,
            asked: std::cell::Cell::new(0),
            progress,
        }
    }

    fn ask(&self, ask: Ask) {
        self.asked.set(self.asked.get() + 1);
        self.asks.send(ask).unwrap();
    }
}

struct Client {
    shm: wl_shm::WlShm,
    root: wl_surface::WlSurface,
    root_xdg: xdg_surface::XdgSurface,
    sub: wl_surface::WlSurface,
    capsule: Option<CupertinoBackdropV1>,
    effects: CupertinoAppEffectsV1,
    size: (i32, i32),
    band: [u8; 3],
    strip: [u8; 3],
    progress: Arc<Progress>,
}

fn buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<Client>,
    (w, h): (i32, i32),
    pixel: impl Fn(i32, i32) -> Option<[u8; 3]>,
) -> wl_buffer::WlBuffer {
    let mut bytes = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            if let Some([r, g, b]) = pixel(x, y) {
                // ARGB8888 is B, G, R, A in memory
                let i = ((y * w + x) * 4) as usize;
                bytes[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
    }
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&bytes).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

impl Client {
    /// Draw the window's surface whole, damaging `damage` (surface rows `from..to`, all of them with `None`), and
    /// commit it with a frame callback the test counts.
    fn draw_root(&self, qh: &QueueHandle<Client>, damage: Option<(i32, i32)>, asked: bool) {
        let (band, strip) = (self.band, self.strip);
        let buffer = buffer(&self.shm, qh, self.size, |_, y| {
            Some(if y < BAND {
                band
            } else if (STRIP.0..STRIP.1).contains(&y) {
                strip
            } else {
                [255, 255, 255]
            })
        });
        self.root.attach(Some(&buffer), 0, 0);
        match damage {
            Some((from, to)) => self.root.damage_buffer(0, from, self.size.0, to - from),
            None => self.root.damage_buffer(0, 0, self.size.0, self.size.1),
        }
        self.root.frame(qh, asked);
        self.root.commit();
    }

    /// Draw the toolbar subsurface: transparent but for the glyph, across the window's width. It is synchronized, so
    /// it shows with the window's next commit.
    fn draw_sub(&mut self, qh: &QueueHandle<Client>) {
        let (gx, gy, gw, gh) = GLYPH;
        let buffer = buffer(&self.shm, qh, (self.size.0, BAND), |x, y| {
            ((gx..gx + gw).contains(&x) && (gy..gy + gh).contains(&y)).then_some(BLUE)
        });
        self.sub.attach(Some(&buffer), 0, 0);
        self.sub.damage_buffer(0, 0, self.size.0, BAND);
        if self.capsule.is_none() {
            let capsule = self.effects.get_backdrop(&self.sub, qh, ());
            let (x, y, w, h) = CAPSULE;
            capsule.set_glass(cupertino_backdrop_v1::GlassStyle::Regular, 1, 0.0);
            capsule.set_shape(x, y, w, h, f64::from(h) / 2.0);
            self.capsule = Some(capsule);
        }
        self.sub.commit();
    }
}

fn run(socket: PathBuf, version: u32, band: [u8; 3], asks: Receiver<Ask>, progress: Arc<Progress>) {
    let conn = Connection::from_socket(UnixStream::connect(socket).expect("connecting")).unwrap();
    let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
    let qh = queue.handle();
    let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 6..=6, ()).unwrap();
    let subcompositor: wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ()).unwrap();
    let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
    let decorations: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
        globals.bind(&qh, 1..=1, ()).unwrap();
    let effects: CupertinoAppEffectsV1 = globals.bind(&qh, version..=version, ()).unwrap();
    // the outputs, so the server tells the surfaces which ones they are on
    let outputs: Vec<wl_output::WlOutput> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|g| g.interface == "wl_output")
            .map(|g| {
                globals.registry().bind::<wl_output::WlOutput, _, _>(
                    g.name,
                    g.version.min(4),
                    &qh,
                    (),
                )
            })
            .collect()
    });
    let root = compositor.create_surface(&qh, "root");
    let root_xdg = wm_base.get_xdg_surface(&root, &qh, ());
    let toplevel = root_xdg.get_toplevel(&qh, ());
    toplevel.set_app_id("org.example.toolbar".into());
    toplevel.set_title("Toolbar".into());
    let decoration = decorations.get_toplevel_decoration(&toplevel, &qh, ());
    decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
    effects.set_titlebar_style(&root, TitlebarStyle::Unified);
    // the toolbar: over the window's top edge, synchronized with it, taking no input
    let sub = compositor.create_surface(&qh, "sub");
    let subsurface = subcompositor.get_subsurface(&sub, &root, &qh, ());
    subsurface.set_position(0, 0);
    subsurface.set_sync();
    let empty = compositor.create_region(&qh, ());
    sub.set_input_region(Some(&empty));
    empty.destroy();
    root.commit();
    let mut client = Client {
        shm: globals.bind(&qh, 1..=1, ()).unwrap(),
        root,
        root_xdg,
        sub,
        capsule: None,
        effects,
        size: SIZE,
        band,
        strip: [255, 255, 255],
        progress,
    };
    let _keep = (toplevel, decoration, subsurface, outputs);
    loop {
        if queue.roundtrip(&mut client).is_err() {
            // the compositor going away ends the test; a protocol error is the client's fault
            if let Some(e) = conn.protocol_error() {
                panic!("the client failed: {e:?}");
            }
            return;
        }
        match asks.recv_timeout(Duration::from_millis(5)) {
            Ok(Ask::Band(rgb)) => {
                client.band = rgb;
                client.draw_root(&qh, Some((0, BAND)), true);
            }
            Ok(Ask::Strip(rgb)) => {
                client.strip = rgb;
                client.draw_root(&qh, Some(STRIP), true);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
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
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    wl_region::WlRegion,
    wl_output::WlOutput,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    CupertinoAppEffectsV1
);

/// The subsurface's outputs and scale.
impl Dispatch<wl_surface::WlSurface, &'static str> for Client {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        tag: &&'static str,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if *tag != "sub" {
            return;
        }
        let p = &state.progress;
        match event {
            wl_surface::Event::Enter { output } => {
                p.sub_on.lock().unwrap().insert(output.id().protocol_id());
            }
            wl_surface::Event::Leave { output } => {
                p.sub_on.lock().unwrap().remove(&output.id().protocol_id());
            }
            wl_surface::Event::PreferredBufferScale { factor } => {
                p.sub_scale.store(factor as usize, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

/// A presented commit of the window's surface; the user data says whether the test asked for it.
impl Dispatch<wl_callback::WlCallback, bool> for Client {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        asked: &bool,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.progress.shown.fetch_add(1, Ordering::SeqCst);
            if *asked {
                state.progress.answered.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
}

impl Dispatch<CupertinoBackdropV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &CupertinoBackdropV1,
        event: cupertino_backdrop_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_backdrop_v1::Event::Tone {
            tone: WEnum::Value(tone),
        } = event
        {
            state.progress.tones.lock().unwrap().push(tone);
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
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            state.size = if width > 0 && height > 0 {
                (width, height)
            } else {
                SIZE
            };
        }
    }
}

/// Each configure draws the toolbar, then the window, whose commit shows both.
impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        if *xdg != state.root_xdg {
            return;
        }
        xdg.ack_configure(serial);
        state.draw_sub(qh);
        state.draw_root(qh, None, false);
    }
}

// ---- the tests -------------------------------------------------------------------------------------------------------

/// The window `app` opened, put at a known place on the first output.
fn placed(h: &mut Harness, app: &App) -> (WindowId, Rect) {
    let id = h.open(app);
    h.put(
        id,
        0,
        Rect::new(100.0, 120.0, f64::from(SIZE.0), f64::from(SIZE.1)),
    );
    h.presented(app);
    (id, h.frame(id))
}

#[test]
fn content_under_the_toolbar_shows_through_its_glass_and_the_glyph_lies_on_it() {
    for (appearance, scale) in [
        (Appearance::Dark, 1.0),
        (Appearance::Light, 1.0),
        (Appearance::Dark, 1.5),
        (Appearance::Light, 2.0),
    ] {
        let Some(mut h) = Harness::one(appearance, scale) else {
            return;
        };
        let app = App::spawn(h.socket(), 11, RED);
        let (_, frame) = placed(&mut h, &app);
        let look = |h: &mut Harness| {
            let shot = h.screenshot();
            let px = |p| at(&shot, frame, (0.0, 0.0), scale, p);
            (px(capsule_face()), px(glyph_middle()), px(bare_row()))
        };
        let (face, glyph, bare) = look(&mut h);
        let what = format!("{appearance:?} at {scale}x");
        assert!(
            close(bare, RED, 2),
            "{what}: the row shows the content where the toolbar is transparent: {bare:?}"
        );
        assert!(
            close(glyph, BLUE, 2),
            "{what}: the toolbar's glyph lies on its glass: {glyph:?}"
        );
        assert!(
            !close(face, RED, 8),
            "{what}: the glass lies over the content and does not stay under it: {face:?}"
        );
        assert!(
            i32::from(face[0]) > i32::from(face[1]) + 20
                && i32::from(face[0]) > i32::from(face[2]) + 20,
            "{what}: the red content shows through the glass: {face:?}"
        );
        // a new band reaches the glass with the commit that paints it
        app.ask(Ask::Band(GREEN));
        h.presented(&app);
        let (face, glyph, bare) = look(&mut h);
        assert!(close(bare, GREEN, 2), "{what}: {bare:?}");
        assert!(close(glyph, BLUE, 2), "{what}: {glyph:?}");
        assert!(
            i32::from(face[1]) > i32::from(face[0]) + 20
                && i32::from(face[1]) > i32::from(face[2]) + 20,
            "{what}: the glass reads the content of the latest commit: {face:?}"
        );
    }
}

#[test]
fn a_subsurface_backdrop_made_below_version_11_draws_nothing() {
    let Some(mut h) = Harness::one(Appearance::Dark, 1.0) else {
        return;
    };
    let app = App::spawn(h.socket(), 10, RED);
    let (_, frame) = placed(&mut h, &app);
    let shot = h.screenshot();
    let face = at(&shot, frame, (0.0, 0.0), 1.0, capsule_face());
    assert!(
        close(face, RED, 2),
        "an older client's subsurface backdrop stays undrawn, as before: {face:?}"
    );
    let glyph = at(&shot, frame, (0.0, 0.0), 1.0, glyph_middle());
    assert!(close(glyph, BLUE, 2), "{glyph:?}");
}

/// How far apart the capsule's face is over a red and over a green band, with the display options `options`; `None`
/// without an EGL device.
fn face_spread(options: DisplayOptions) -> Option<([u8; 3], [u8; 3], u32)> {
    let mut h = Harness::one(Appearance::Dark, 1.0)?;
    h.state.accessibility.settings.display = options;
    h.state.request_redraw();
    let app = App::spawn(h.socket(), 11, RED);
    let (_, frame) = placed(&mut h, &app);
    let face = |h: &mut Harness| at(&h.screenshot(), frame, (0.0, 0.0), 1.0, capsule_face());
    let over_red = face(&mut h);
    app.ask(Ask::Band(GREEN));
    h.presented(&app);
    let over_green = face(&mut h);
    let spread = over_red
        .iter()
        .zip(over_green)
        .map(|(a, b)| u32::from(a.abs_diff(b)))
        .sum();
    Some((over_red, over_green, spread))
}

#[test]
fn the_toolbar_glass_follows_reduce_transparency_and_increase_contrast() {
    let Some((red, green, plain)) = face_spread(DisplayOptions::default()) else {
        return;
    };
    assert!(
        !close(red, RED, 8) && !close(green, GREEN, 8),
        "{red:?} {green:?}"
    );
    let (red, green, reduced) = face_spread(DisplayOptions {
        reduce_transparency: true,
        ..DisplayOptions::default()
    })
    .unwrap();
    assert!(
        !close(red, RED, 8) && !close(green, GREEN, 8),
        "{red:?} {green:?}"
    );
    assert!(
        reduced * 2 < plain,
        "with Reduce Transparency the toolbar's glass is the denser recipe: the content under it moves its face \
         {reduced} against {plain} without it"
    );
    let (red, green, contrast) = face_spread(DisplayOptions {
        increase_contrast: true,
        ..DisplayOptions::default()
    })
    .unwrap();
    assert!(
        !close(red, RED, 8) && !close(green, GREEN, 8),
        "{red:?} {green:?}"
    );
    assert!(
        contrast > 8,
        "with Increase Contrast the glass still shows the content under it: {red:?} over red, {green:?} over green"
    );
}

#[test]
fn an_inactive_window_and_a_full_screen_one_keep_their_toolbar_glass_over_the_content() {
    let Some(mut h) = Harness::one(Appearance::Dark, 1.0) else {
        return;
    };
    let app = App::spawn(h.socket(), 11, RED);
    let (id, frame) = placed(&mut h, &app);
    let active = at(&h.screenshot(), frame, (0.0, 0.0), 1.0, capsule_face());
    // another window takes the focus, away from the first
    let other = App::spawn(h.socket(), 11, GREEN);
    let second = h.open(&other);
    h.put(second, 0, Rect::new(620.0, 300.0, 240.0, 200.0));
    h.presented(&other);
    assert_ne!(
        h.state.wm.key_window(),
        Some(id),
        "the first window is inactive"
    );
    let shot = h.screenshot();
    let inactive = at(&shot, frame, (0.0, 0.0), 1.0, capsule_face());
    assert!(
        !close(inactive, RED, 8) && inactive[0] > inactive[1],
        "the inactive window's glass lies over its content and shows it: {inactive:?} (active {active:?})"
    );
    assert!(close(
        at(&shot, frame, (0.0, 0.0), 1.0, glyph_middle()),
        BLUE,
        2
    ));
    // full screen: the window fills the output and its toolbar's glass is still over its content
    h.state.command(Command::Fullscreen {
        window: id,
        output: None,
        requested_by: Requester::User,
    });
    h.settle();
    let (before, progress) = (
        app.progress.shown.load(Ordering::SeqCst),
        app.progress.clone(),
    );
    h.wait_until("the full-screen frame", move |s| {
        progress.shown.load(Ordering::SeqCst) > before
            && s.wm
                .window(id)
                .is_some_and(|w| w.geometry().w as i32 == OUTPUT.0)
    });
    h.settle();
    let full = h.frame(id);
    let shot = h.screenshot();
    let face = at(&shot, full, (0.0, 0.0), 1.0, capsule_face());
    assert!(
        !close(face, RED, 8) && face[0] > face[1] + 20,
        "a full-screen window's toolbar glass lies over its content: {face:?}"
    );
    assert!(close(
        at(&shot, full, (0.0, 0.0), 1.0, glyph_middle()),
        BLUE,
        2
    ));
}

#[test]
fn a_window_across_two_displays_tells_its_toolbar_of_both_and_draws_its_glass_on_the_one_it_shows_on(
) {
    const LEFT: (i32, i32) = (800, 600);
    let screens = [("headless-1", LEFT, 1.0), ("headless-2", (800, 600), 2.0)];
    let Some(mut h) = Harness::new(Appearance::Dark, &screens) else {
        return;
    };
    let app = App::spawn(h.socket(), 11, RED);
    let id = h.open(&app);
    // dragged across the seam: the capsule's face on the left display, the bare row past it on the right one
    h.put(
        id,
        0,
        Rect::new(300.0, 100.0, f64::from(SIZE.0), f64::from(SIZE.1)),
    );
    let x = f64::from(LEFT.0) - f64::from(CAPSULE.0 + CAPSULE.2) - 20.0;
    h.drag(id, (310.0, 110.0), (x + 10.0, 110.0));
    let frame = h.frame(id);
    assert!(frame.x < 800.0 && frame.x + frame.w > 800.0, "{frame:?}");
    let progress = app.progress.clone();
    h.wait_until("the toolbar on both displays", move |_| {
        progress.sub_on.lock().unwrap().len() == 2
    });
    let shots = h.screenshots();
    let face = at(&shots[0], frame, (0.0, 0.0), 1.0, capsule_face());
    assert!(
        !close(face, RED, 8) && face[0] > face[1] + 20,
        "the glass on the left display: {face:?}"
    );
    // a window shows on the display it is mostly on; its toolbar is still told it is on both
    let right_origin = (f64::from(LEFT.0), 0.0);
    // the window moves on: the capsule is on the right display at 2x
    h.put(
        id,
        1,
        Rect::new(900.0, 100.0, f64::from(SIZE.0), f64::from(SIZE.1)),
    );
    let progress = app.progress.clone();
    h.wait_until("the toolbar on the right display at its scale", move |_| {
        progress.sub_on.lock().unwrap().len() == 1 && progress.sub_scale.load(Ordering::SeqCst) == 2
    });
    h.settle();
    let frame = h.frame(id);
    let shots = h.screenshots();
    let face = at(&shots[1], frame, right_origin, 2.0, capsule_face());
    let glyph = at(&shots[1], frame, right_origin, 2.0, glyph_middle());
    assert!(
        !close(face, RED, 8) && face[0] > face[1] + 20,
        "the glass on the right display at 2x: {face:?}"
    );
    assert!(close(glyph, BLUE, 2), "{glyph:?}");
}

#[test]
fn a_small_toolbar_capsule_is_told_the_tone_of_the_content_under_it() {
    let Some(mut h) = Harness::one(Appearance::Dark, 1.0) else {
        return;
    };
    let app = App::spawn(h.socket(), 11, [0, 0, 0]);
    let _ = placed(&mut h, &app);
    let progress = app.progress.clone();
    h.wait_until("a dark face over black content", move |_| {
        progress.tones.lock().unwrap().last() == Some(&Tone::Dark)
    });
    app.ask(Ask::Band([255, 255, 255]));
    let progress = app.progress.clone();
    h.wait_until("a light face over white content", move |_| {
        progress.tones.lock().unwrap().last() == Some(&Tone::Light)
    });
}

/// An output's buffer and damage tracker, kept from frame to frame.
struct Tracked {
    texture: GlesTexture,
    tracker: OutputDamageTracker,
    drawn: bool,
}

/// Draw output 0 into `t` over its previous frame and return the damage, in output pixels.
fn damage(h: &mut Harness, t: &mut Tracked) -> Vec<Rectangle<i32, Physical>> {
    let mut backend = h.state.backend.take().unwrap();
    #[allow(irrefutable_let_patterns)]
    let Backend::Headless(ref mut hb) = backend
    else {
        panic!("headless backend required")
    };
    let elements = h.state.build_output_elements(&mut hb.renderer, 0);
    let age = usize::from(t.drawn);
    t.drawn = true;
    let rects = {
        let mut target = hb.renderer.bind(&mut t.texture).unwrap();
        let result = t
            .tracker
            .render_output(
                &mut hb.renderer,
                &mut target,
                age,
                &elements,
                headless::CLEAR,
            )
            .unwrap();
        result.damage.cloned().unwrap_or_default()
    };
    drop(elements);
    h.state.backend = Some(backend);
    rects
}

#[test]
fn content_changing_under_the_window_repaints_no_more_than_its_damage_and_the_toolbar_row() {
    for scale in [1.0, 1.5] {
        let Some(mut h) = Harness::one(Appearance::Dark, scale) else {
            return;
        };
        let app = App::spawn(h.socket(), 11, RED);
        let (_, frame) = placed(&mut h, &app);
        let size = h.state.outputs[0].output.current_mode().unwrap().size;
        let mut t = {
            let mut backend = h.state.backend.take().unwrap();
            #[allow(irrefutable_let_patterns)]
            let Backend::Headless(ref mut hb) = backend
            else {
                panic!("headless backend required")
            };
            let buf: Size<i32, Buffer> = (size.w, size.h).into();
            let texture = hb.renderer.create_buffer(Fourcc::Abgr8888, buf).unwrap();
            h.state.backend = Some(backend);
            Tracked {
                texture,
                tracker: OutputDamageTracker::new((size.w, size.h), scale, Transform::Normal),
                drawn: false,
            }
        };
        damage(&mut h, &mut t);
        assert!(
            damage(&mut h, &mut t).is_empty(),
            "{scale}x: nothing changed"
        );
        let px = |v: f64| (v * scale).floor() as i32;
        let pxe = |v: f64| (v * scale).ceil() as i32;
        // the toolbar row across the window, in output pixels
        let row = Rectangle::<i32, Physical>::new(
            (px(frame.x), px(frame.y)).into(),
            (
                pxe(frame.x + frame.w) - px(frame.x),
                pxe(frame.y + f64::from(BAND)) - px(frame.y),
            )
                .into(),
        );
        // the content repaints a strip far below the row: the glass reads the window again, inside the row only
        app.ask(Ask::Strip(GRAY));
        let (asked, progress) = (app.asked.get(), app.progress.clone());
        h.wait_until("the strip presented", move |_| {
            progress.answered.load(Ordering::SeqCst) >= asked
        });
        let strip = Rectangle::<i32, Physical>::new(
            (px(frame.x), px(frame.y + f64::from(STRIP.0))).into(),
            (
                pxe(frame.x + frame.w) - px(frame.x),
                pxe(frame.y + f64::from(STRIP.1)) - px(frame.y + f64::from(STRIP.0)),
            )
                .into(),
        );
        let rects = damage(&mut h, &mut t);
        assert!(!rects.is_empty(), "{scale}x: the strip repaints");
        let grown = |r: Rectangle<i32, Physical>| {
            Rectangle::new(
                (r.loc.x - 1, r.loc.y - 1).into(),
                (r.size.w + 2, r.size.h + 2).into(),
            )
        };
        for r in &rects {
            assert!(
                grown(strip).contains_rect(*r) || grown(row).contains_rect(*r),
                "{scale}x: damage {r:?} lies in the strip {strip:?} or the toolbar row {row:?}; all: {rects:?}"
            );
        }
    }
}

#[test]
fn the_toolbar_takes_no_input_and_presses_reach_the_window_under_it() {
    let Some(mut h) = Harness::one(Appearance::Dark, 1.0) else {
        return;
    };
    let app = App::spawn(h.socket(), 11, RED);
    let (_, frame) = placed(&mut h, &app);
    let (x, y) = capsule_face();
    let under = h
        .state
        .surface_under((frame.x + x, frame.y + y).into())
        .map(|(s, _)| s);
    let window = h
        .state
        .windows
        .iter()
        .find(|w| w.mapped)
        .unwrap()
        .surface()
        .clone();
    assert_eq!(
        under.as_ref(),
        Some(&window),
        "a point of the toolbar with an empty input region finds the window's own surface"
    );
}
