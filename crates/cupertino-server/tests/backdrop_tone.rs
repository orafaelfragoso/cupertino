//! The tone of a glass backdrop end to end: the compositor runs in-process on an offscreen EGL device, a client puts a
//! small and a large glass backdrop over a wall it paints, and the test watches the `tone` events the client receives
//! as the wall turns from black to white. Over walls of saturated colours, labels the client draws on the glass in the
//! colours the tones pick are measured against the glass face around them in a capture of the output. The tests wait on
//! events, not on time: a change the client makes counts once the server presented it, measured the faces it read back
//! and the client received what the server sent since ([`settle`]). Tests skip (pass) when no EGL device is available.

use crate::common;

use std::cell::Cell;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, SendError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::HANG_GUARD;

use cupertino_glass::glass::face_color;
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1, Tone,
};
use cupertino_server::app_effects_v1::layer_backdrops;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_theme::glass::{
    contrast_ratio, foreground_rgb, relative_luminance, FaceTone, GlassMode, GlassParams,
    GlassStyle, FACE_TONE_HYSTERESIS, FACE_TONE_THRESHOLD, VIBRANT_PLACEHOLDER, VIBRANT_PRIMARY,
};
use cupertino_theme::Appearance;
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const OUTPUT: (i32, i32) = (800, 500);
/// The surface over the wall: a small capsule control and a large panel, both glass.
const PANEL: (i32, i32) = (320, 140);
const SMALL: (i32, i32, i32, i32) = (10, 10, 44, 28);
const LARGE: (i32, i32, i32, i32) = (120, 10, 180, 120);
/// Where the panel lies on the output.
const PANEL_AT: (i32, i32) = (100, 100);
/// Labels drawn on the glass, panel-local (x, y, width, height): one on the small control, a primary label and a
/// placeholder on the large panel; and a patch of bare face beside each, which the labels are measured against.
const SMALL_LABEL: (i32, i32, i32, i32) = (28, 18, 12, 12);
const SMALL_FACE: (i32, i32, i32, i32) = (18, 21, 6, 6);
const PRIMARY_LABEL: (i32, i32, i32, i32) = (150, 40, 40, 16);
const PLACEHOLDER_LABEL: (i32, i32, i32, i32) = (150, 80, 40, 16);
const LARGE_FACE: (i32, i32, i32, i32) = (220, 50, 40, 40);

/// Straight RGBA of the labels the client draws: on the small control, the primary label and the placeholder.
type Labels = [[f32; 4]; 3];

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new(appearance: Appearance) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let cfg = Config {
            appearance,
            ..Config::default()
        };
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
        let end = Instant::now() + HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Whether the glass still has readbacks of faces in flight.
    fn tone_reads_in_flight(&self) -> bool {
        self.state
            .render
            .as_ref()
            .and_then(|r| r.material.as_ref())
            .is_some_and(|m| m.borrow().tone_reads_in_flight())
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn screenshot(&mut self) -> image::RgbaImage {
        static SHOTS: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-{}-tone.png",
            std::process::id(),
            SHOTS.fetch_add(1, Ordering::Relaxed)
        ));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }
}

/// Which backdrop an event names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Which {
    Small,
    Large,
}

type Heard = Arc<Mutex<Vec<(Which, Tone)>>>;

struct Client {
    shm: wl_shm::WlShm,
    wall: wl_surface::WlSurface,
    panel: wl_surface::WlSurface,
    small: CupertinoBackdropV1,
    large: CupertinoBackdropV1,
    heard: Heard,
    progress: Arc<Progress>,
}

/// What a client has done of what the test asked: commits the server presented (their frame callbacks came) and
/// roundtrips finished after a [`Ask::Sync`].
#[derive(Default)]
struct Progress {
    shown: AtomicUsize,
    synced: AtomicUsize,
}

fn shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<Client>,
    size: (i32, i32),
    rgb: Option<[u8; 3]>,
) -> wl_buffer::WlBuffer {
    let px = match rgb {
        // ARGB8888 is B, G, R, A in memory
        Some([r, g, b]) => [b, g, r, 255],
        None => [0; 4],
    };
    pixels_buffer(shm, qh, size, &px.repeat((size.0 * size.1) as usize))
}

/// A transparent panel with `labels` drawn as solid patches where the labels go.
fn labels_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<Client>,
    labels: &Labels,
) -> wl_buffer::WlBuffer {
    let (w, h) = PANEL;
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for (rgba, (x0, y0, lw, lh)) in
        labels
            .iter()
            .zip([SMALL_LABEL, PRIMARY_LABEL, PLACEHOLDER_LABEL])
    {
        // premultiplied ARGB8888: B, G, R, A in memory
        let a = rgba[3];
        let byte = |v: f32| (v * 255.0).round() as u8;
        let px = [
            byte(rgba[2] * a),
            byte(rgba[1] * a),
            byte(rgba[0] * a),
            byte(a),
        ];
        for y in y0..y0 + lh {
            for x in x0..x0 + lw {
                let i = ((y * w + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&px);
            }
        }
    }
    pixels_buffer(shm, qh, PANEL, &pixels)
}

fn pixels_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<Client>,
    size: (i32, i32),
    pixels: &[u8],
) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.set_len((w * h * 4) as u64).unwrap();
    file.write_all(pixels).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

impl Client {
    fn paint_wall(&self, qh: &QueueHandle<Client>, rgb: [u8; 3]) {
        let buffer = shm_buffer(&self.shm, qh, OUTPUT, Some(rgb));
        self.wall.attach(Some(&buffer), 0, 0);
        self.wall.damage_buffer(0, 0, OUTPUT.0, OUTPUT.1);
        self.wall.commit();
    }

    /// A commit the test asked for, which counts as shown once the server presented it.
    fn commit_asked(&self, surface: &wl_surface::WlSurface, qh: &QueueHandle<Client>) {
        surface.frame(qh, ());
        surface.commit();
    }

    fn show_panel(&self, qh: &QueueHandle<Client>) {
        let buffer = shm_buffer(&self.shm, qh, PANEL, None);
        self.panel.attach(Some(&buffer), 0, 0);
        self.panel.damage_buffer(0, 0, PANEL.0, PANEL.1);
        for (b, (x, y, w, h)) in [(&self.small, SMALL), (&self.large, LARGE)] {
            b.set_glass(cupertino_backdrop_v1::GlassStyle::Regular, 0, 0.0);
            b.set_shape(x, y, w, h, f64::from(h.min(w)) / 2.0);
        }
        self.panel.commit();
    }

    /// The wall turns `rgb`, as the test asked.
    fn repaint_wall(&self, qh: &QueueHandle<Client>, rgb: [u8; 3]) {
        let buffer = shm_buffer(&self.shm, qh, OUTPUT, Some(rgb));
        self.wall.attach(Some(&buffer), 0, 0);
        self.wall.damage_buffer(0, 0, OUTPUT.0, OUTPUT.1);
        self.commit_asked(&self.wall, qh);
    }

    /// The small control's backdrop becomes the material `name`, which is not glass.
    fn plain_small(&self, qh: &QueueHandle<Client>, name: &str) {
        self.small.set_material(name.to_string());
        self.commit_asked(&self.panel, qh);
    }

    fn draw_labels(&self, qh: &QueueHandle<Client>, labels: &Labels) {
        let buffer = labels_buffer(&self.shm, qh, labels);
        self.panel.attach(Some(&buffer), 0, 0);
        self.panel.damage_buffer(0, 0, PANEL.0, PANEL.1);
        self.commit_asked(&self.panel, qh);
    }
}

/// What the test asks of the client: a wall of one colour, labels on the glass, or a material other than glass under
/// the small control, each one commit; or a roundtrip, after which the client has received every event the server
/// sent before it.
enum Ask {
    Wall([u8; 3]),
    Labels(Labels),
    Plain(&'static str),
    Sync,
}

impl From<[u8; 3]> for Ask {
    fn from(rgb: [u8; 3]) -> Ask {
        Ask::Wall(rgb)
    }
}

/// The test's end of a client: it sends the asks and counts the commits and roundtrips it asked for, to compare with
/// the client's [`Progress`].
struct Asker {
    asks: Sender<Ask>,
    commits: Cell<usize>,
    syncs: Cell<usize>,
    progress: Arc<Progress>,
}

impl Asker {
    fn send(&self, ask: impl Into<Ask>) -> Result<(), SendError<Ask>> {
        let ask = ask.into();
        let counter = match ask {
            Ask::Sync => &self.syncs,
            _ => &self.commits,
        };
        counter.set(counter.get() + 1);
        self.asks.send(ask)
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
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
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
            state.progress.shown.fetch_add(1, Ordering::SeqCst);
        }
    }
}

ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    CupertinoAppEffectsV1
);

impl Dispatch<CupertinoBackdropV1, Which> for Client {
    fn event(
        state: &mut Self,
        _: &CupertinoBackdropV1,
        event: cupertino_backdrop_v1::Event,
        which: &Which,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_backdrop_v1::Event::Tone {
            tone: WEnum::Value(tone),
        } = event
        {
            state.heard.lock().unwrap().push((*which, tone));
        }
    }
}

/// The layer surface's user data: whether it is the wall.
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, bool> for Client {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        wall: &bool,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            layer.ack_configure(serial);
            if *wall {
                state.paint_wall(qh, [0, 0, 0]);
            } else {
                state.show_panel(qh);
            }
        }
    }
}

/// A client on `cupertino_app_effects_v1` of `version`: a black wall on the background layer and the glass panel over
/// it. Each colour sent repaints the wall; the tones it hears are pushed to `heard`.
fn spawn(socket: PathBuf, version: u32, heard: Heard) -> Asker {
    spawn_asking(socket, version, heard)
}

/// [`spawn`], which also draws labels on the glass and changes the small control's material when asked.
fn spawn_asking(socket: PathBuf, version: u32, heard: Heard) -> Asker {
    spawn_late(socket, version, heard, Duration::ZERO)
}

/// [`spawn_asking`] for a client that does each thing asked `late`, as a client on a loaded machine may.
fn spawn_late(socket: PathBuf, version: u32, heard: Heard, late: Duration) -> Asker {
    let (tx, rx): (Sender<Ask>, Receiver<Ask>) = channel();
    let progress = Arc::new(Progress::default());
    let asker = Asker {
        asks: tx,
        commits: Cell::new(0),
        syncs: Cell::new(0),
        progress: progress.clone(),
    };
    std::thread::spawn(move || {
        let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
        let conn = Connection::from_socket(stream).unwrap();
        let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
        let layers: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).unwrap();
        let effects: CupertinoAppEffectsV1 = globals.bind(&qh, version..=version, ()).unwrap();
        let wall = compositor.create_surface(&qh, ());
        let wall_layer = layers.get_layer_surface(
            &wall,
            None,
            zwlr_layer_shell_v1::Layer::Background,
            "wall".into(),
            &qh,
            true,
        );
        wall_layer.set_size(OUTPUT.0 as u32, OUTPUT.1 as u32);
        wall_layer.set_exclusive_zone(-1);
        wall.commit();
        let panel = compositor.create_surface(&qh, ());
        let panel_layer = layers.get_layer_surface(
            &panel,
            None,
            zwlr_layer_shell_v1::Layer::Top,
            "panel".into(),
            &qh,
            false,
        );
        panel_layer.set_size(PANEL.0 as u32, PANEL.1 as u32);
        panel_layer
            .set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
        panel_layer.set_margin(100, 0, 0, 100);
        panel_layer.set_exclusive_zone(-1);
        let small = effects.get_backdrop(&panel, &qh, Which::Small);
        let large = effects.get_backdrop(&panel, &qh, Which::Large);
        panel.commit();
        let mut client = Client {
            shm,
            wall,
            panel,
            small,
            large,
            heard,
            progress,
        };
        loop {
            let mut syncs = 0;
            while let Ok(ask) = rx.try_recv() {
                std::thread::sleep(late);
                match ask {
                    Ask::Wall(rgb) => client.repaint_wall(&qh, rgb),
                    Ask::Labels(labels) => client.draw_labels(&qh, &labels),
                    Ask::Plain(name) => client.plain_small(&qh, name),
                    Ask::Sync => syncs += 1,
                }
            }
            if queue.roundtrip(&mut client).is_err() {
                panic!("the client failed: {:?}", conn.protocol_error());
            }
            client.progress.synced.fetch_add(syncs, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    asker
}

fn heard_of(heard: &Heard, which: Which) -> Vec<Tone> {
    heard
        .lock()
        .unwrap()
        .iter()
        .filter(|(w, _)| *w == which)
        .map(|(_, t)| *t)
        .collect()
}

/// Wait until what the test asked of `client` had its effect, so that an event the server would send for it has
/// arrived: every commit asked was presented (its frame callback came), every face the glass read back by then was
/// measured, and a roundtrip after that finished, so the client received the tones those faces led to.
fn settle(h: &mut Harness, client: &Asker) {
    let (commits, progress) = (client.commits.get(), client.progress.clone());
    h.wait_until("the asked commits presented", move |_| {
        progress.shown.load(Ordering::SeqCst) >= commits
    });
    let end = Instant::now() + HANG_GUARD;
    while h.tone_reads_in_flight() {
        assert!(
            Instant::now() < end,
            "timed out waiting for the faces read back"
        );
        h.step();
    }
    client.send(Ask::Sync).unwrap();
    let (syncs, progress) = (client.syncs.get(), client.progress.clone());
    h.wait_until("the client's roundtrip", move |_| {
        progress.synced.load(Ordering::SeqCst) >= syncs
    });
}

#[test]
fn a_small_dark_glass_control_is_told_its_face_turned_light_and_a_large_panel_is_not() {
    let Some(mut h) = Harness::new(Appearance::Dark) else {
        return;
    };
    let heard = Heard::default();
    let wall = spawn(h.socket_path(), 7, heard.clone());
    h.wait_until("the glass backdrops", |s| {
        layer_backdrops(&s.outputs[0].output).len() == 2
    });
    let small = |h: &Heard| heard_of(h, Which::Small);
    let seen = heard.clone();
    h.wait_until("the small control's dark face", move |_| {
        small(&seen) == [Tone::Dark]
    });
    wall.send([255, 255, 255]).unwrap();
    let seen = heard.clone();
    h.wait_until("the small control's light face", move |_| {
        small(&seen) == [Tone::Dark, Tone::Light]
    });
    // nothing changes: no more events, however many frames go by
    settle(&mut h, &wall);
    assert_eq!(small(&heard), [Tone::Dark, Tone::Light]);
    // the large panel keeps the appearance and is told nothing
    assert!(heard_of(&heard, Which::Large).is_empty());
    // back to black flips it back
    wall.send([0, 0, 0]).unwrap();
    let seen = heard.clone();
    h.wait_until("the small control's dark face again", move |_| {
        small(&seen) == [Tone::Dark, Tone::Light, Tone::Dark]
    });
}

#[test]
fn a_small_control_that_stops_being_glass_goes_back_to_the_appearance_once() {
    let Some(mut h) = Harness::new(Appearance::Dark) else {
        return;
    };
    let heard = Heard::default();
    let client = spawn_asking(h.socket_path(), 7, heard.clone());
    let small = |h: &Heard| heard_of(h, Which::Small);
    let seen = heard.clone();
    h.wait_until("the small control's dark face", move |_| {
        small(&seen) == [Tone::Dark]
    });
    client.send(Ask::Wall([255, 255, 255])).unwrap();
    let seen = heard.clone();
    h.wait_until("the small control's light face", move |_| {
        small(&seen) == [Tone::Dark, Tone::Light]
    });
    // a material that is not glass: its text keeps the appearance's colours, and the client is told so once
    client.send(Ask::Plain("Sidebar")).unwrap();
    let seen = heard.clone();
    h.wait_until("the small control back to the appearance", move |_| {
        small(&seen) == [Tone::Dark, Tone::Light, Tone::Appearance]
    });
    client.send(Ask::Wall([0, 0, 0])).unwrap();
    settle(&mut h, &client);
    assert_eq!(small(&heard), [Tone::Dark, Tone::Light, Tone::Appearance]);
}

#[test]
fn light_glass_keeps_a_light_face_over_black_and_white() {
    let Some(mut h) = Harness::new(Appearance::Light) else {
        return;
    };
    let heard = Heard::default();
    let wall = spawn(h.socket_path(), 7, heard.clone());
    let seen = heard.clone();
    // the milky face of light glass stays light even over black, so its text stays dark
    h.wait_until("the small control's light face", move |_| {
        heard_of(&seen, Which::Small) == [Tone::Light]
    });
    wall.send([255, 255, 255]).unwrap();
    settle(&mut h, &wall);
    wall.send([0, 0, 0]).unwrap();
    settle(&mut h, &wall);
    assert_eq!(heard_of(&heard, Which::Small), [Tone::Light]);
    assert!(heard_of(&heard, Which::Large).is_empty());
}

/// A change counts once the server presented it and the client received its tone, however late the client makes it:
/// right after settling, the tone the client heard is the new one.
#[test]
fn settling_waits_for_a_client_that_answers_late() {
    let Some(mut h) = Harness::new(Appearance::Dark) else {
        return;
    };
    let heard = Heard::default();
    let wall = spawn_late(
        h.socket_path(),
        7,
        heard.clone(),
        Duration::from_millis(500),
    );
    let seen = heard.clone();
    h.wait_until("the small control's dark face", move |_| {
        heard_of(&seen, Which::Small) == [Tone::Dark]
    });
    wall.send([255, 255, 255]).unwrap();
    settle(&mut h, &wall);
    assert_eq!(heard_of(&heard, Which::Small), [Tone::Dark, Tone::Light]);
}

#[test]
fn a_version_6_client_hears_no_tone() {
    let Some(mut h) = Harness::new(Appearance::Dark) else {
        return;
    };
    let heard = Heard::default();
    let wall = spawn(h.socket_path(), 6, heard.clone());
    h.wait_until("the glass backdrops", |s| {
        layer_backdrops(&s.outputs[0].output).len() == 2
    });
    settle(&mut h, &wall);
    wall.send([255, 255, 255]).unwrap();
    settle(&mut h, &wall);
    assert!(heard.lock().unwrap().is_empty());
}

/// Saturated colours and the extremes, as a busy wallpaper puts them under a panel.
const BUSY: [[u8; 3]; 11] = [
    [235, 30, 30],
    [250, 140, 20],
    [245, 230, 30],
    [40, 230, 40],
    [30, 220, 240],
    [30, 60, 240],
    [150, 40, 240],
    [240, 40, 200],
    [255, 255, 255],
    [120, 120, 120],
    [0, 0, 0],
];

/// The contrast a label must reach: 4.5:1, which the face allows unless the rule for text on glass gives less by
/// design. A large panel keeps its appearance's foreground whatever lies behind it, so over a bright face its labels
/// read no better than that foreground does.
const LEGIBLE: f32 = 4.5;
/// Where a small control's face sits next to the threshold, within the hysteresis, the tone may be either; the label
/// then reads no worse than at the edge of that band.
fn band_contrast() -> f32 {
    let edge = FACE_TONE_THRESHOLD + FACE_TONE_HYSTERESIS;
    (1.0 + 0.05) / (edge + 0.05)
}

/// The mean colour of an output-local rectangle, as encoded sRGB from 0 to 1.
fn mean(img: &image::RgbaImage, (x0, y0, w, h): (i32, i32, i32, i32)) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            let p = img.get_pixel(x as u32, y as u32);
            for c in 0..3 {
                sum[c] += f32::from(p[c]) / 255.0;
            }
        }
    }
    sum.map(|s| s / (w * h) as f32)
}

/// A panel-local rectangle on the output, shrunk by `inset` on every side.
fn on_output((x, y, w, h): (i32, i32, i32, i32), inset: i32) -> (i32, i32, i32, i32) {
    (
        PANEL_AT.0 + x + inset,
        PANEL_AT.1 + y + inset,
        w - 2 * inset,
        h - 2 * inset,
    )
}

/// `rgb` at `alpha` over `face`, as the compositor blends the window over the glass.
fn over(rgb: [f32; 3], alpha: f32, face: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|c| rgb[c] * alpha + face[c] * (1.0 - alpha))
}

fn measured_tone(heard: &Heard) -> Option<FaceTone> {
    match heard_of(heard, Which::Small).last() {
        Some(Tone::Dark) => Some(FaceTone::Dark),
        Some(Tone::Light) => Some(FaceTone::Light),
        _ => None,
    }
}

#[test]
fn labels_on_glass_over_a_busy_wallpaper_reach_the_contrast_the_rule_allows() {
    use cupertino_defaults::accessibility::DisplayOptions;
    for (appearance, increase_contrast) in [
        (Appearance::Light, false),
        (Appearance::Dark, false),
        (Appearance::Light, true),
        (Appearance::Dark, true),
    ] {
        let Some(mut h) = Harness::new(appearance) else {
            return;
        };
        h.state.cursor_status = CursorImageStatus::Hidden;
        h.state.accessibility.settings.display = DisplayOptions {
            increase_contrast,
            ..DisplayOptions::default()
        };
        let dark = appearance == Appearance::Dark;
        let heard = Heard::default();
        let client = spawn_asking(h.socket_path(), 7, heard.clone());
        h.wait_until("the glass backdrops", |s| {
            layer_backdrops(&s.outputs[0].output).len() == 2
        });
        for wall in BUSY {
            client.send(Ask::Wall(wall)).unwrap();
            settle(&mut h, &client);
            let tone = measured_tone(&heard);
            let large = foreground_rgb(dark, None);
            let small = foreground_rgb(dark, tone);
            let labels = [
                [small[0], small[1], small[2], VIBRANT_PRIMARY],
                [large[0], large[1], large[2], VIBRANT_PRIMARY],
                [large[0], large[1], large[2], VIBRANT_PLACEHOLDER],
            ];
            client.send(Ask::Labels(labels)).unwrap();
            settle(&mut h, &client);
            let shot = h.screenshot();
            let what =
                format!("{appearance:?} increase contrast {increase_contrast}, wall {wall:?}");

            let face = mean(&shot, on_output(SMALL_FACE, 0));
            let label = mean(&shot, on_output(SMALL_LABEL, 2));
            let got = contrast_ratio(label, face);
            let near = (relative_luminance(face) - FACE_TONE_THRESHOLD).abs()
                <= 2.0 * FACE_TONE_HYSTERESIS;
            let need = if near { band_contrast() } else { LEGIBLE };
            eprintln!(
                "{what}: small {tone:?} face {:.3} label {got:.2} (needs {need:.2})",
                relative_luminance(face)
            );
            assert!(
                got >= need - 0.1,
                "{what}: the small control's label reads at {got:.2}:1"
            );

            let face = mean(&shot, on_output(LARGE_FACE, 0));
            if dark {
                // dark glass holds its face under the brightness cap, which is what keeps white labels readable over
                // a bright wall: the face is never brighter than the recipe's face over white
                let mode = GlassMode {
                    dark,
                    style: GlassStyle::Regular,
                    increase_contrast,
                    ..GlassMode::default()
                };
                let params = GlassParams::new(mode, LARGE.2.min(LARGE.3) as f32);
                let brightest =
                    relative_luminance(face_color([1.0; 3], &params, params.face.max_luma));
                assert!(
                    relative_luminance(face) <= brightest + 0.02,
                    "{what}: the panel's face {:.3} is brighter than the cap allows ({brightest:.3})",
                    relative_luminance(face)
                );
            }
            for (name, rect, level) in [
                ("primary label", PRIMARY_LABEL, VIBRANT_PRIMARY),
                ("placeholder", PLACEHOLDER_LABEL, VIBRANT_PLACEHOLDER),
            ] {
                let got = contrast_ratio(mean(&shot, on_output(rect, 2)), face);
                let rule = contrast_ratio(over(large, level, face), face);
                let need = LEGIBLE.min(rule);
                eprintln!(
                    "{what}: large {name} face {:.3} {got:.2} (needs {need:.2})",
                    relative_luminance(face)
                );
                assert!(
                    got >= need - 0.1,
                    "{what}: the panel's {name} reads at {got:.2}:1"
                );
            }
        }
    }
}
