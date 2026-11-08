//! The tone of a glass backdrop end to end: the compositor runs in-process on an offscreen EGL device, a client puts a
//! small and a large glass backdrop over a wall it paints, and the test watches the `tone` events the client receives
//! as the wall turns from black to white. Tests skip (pass) when no EGL device is available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::{
    self, CupertinoBackdropV1, Tone,
};
use cupertino_server::app_effects_v1::layer_backdrops;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_theme::Appearance;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const OUTPUT: (i32, i32) = (800, 500);
/// The surface over the wall: a small capsule control and a large panel, both glass.
const PANEL: (i32, i32) = (320, 140);
const SMALL: (i32, i32, i32, i32) = (10, 10, 44, 28);
const LARGE: (i32, i32, i32, i32) = (120, 10, 180, 120);

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
}

fn shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<Client>,
    size: (i32, i32),
    rgb: Option<[u8; 3]>,
) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.set_len((w * h * 4) as u64).unwrap();
    let px = match rgb {
        // ARGB8888 is B, G, R, A in memory
        Some([r, g, b]) => [b, g, r, 255],
        None => [0; 4],
    };
    file.write_all(&px.repeat((w * h) as usize)).unwrap();
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
fn spawn(socket: PathBuf, version: u32, heard: Heard) -> Sender<[u8; 3]> {
    let (tx, rx): (Sender<[u8; 3]>, Receiver<[u8; 3]>) = channel();
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
        };
        loop {
            while let Ok(rgb) = rx.try_recv() {
                client.paint_wall(&qh, rgb);
            }
            if queue.roundtrip(&mut client).is_err() {
                panic!("the client failed: {:?}", conn.protocol_error());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    tx
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

/// Let frames go by, so that an event the server would send arrives.
fn settle(h: &mut Harness) {
    for _ in 0..40 {
        h.step();
    }
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
    settle(&mut h);
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
    settle(&mut h);
    wall.send([0, 0, 0]).unwrap();
    settle(&mut h);
    assert_eq!(heard_of(&heard, Which::Small), [Tone::Light]);
    assert!(heard_of(&heard, Which::Large).is_empty());
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
    for _ in 0..20 {
        h.step();
    }
    wall.send([255, 255, 255]).unwrap();
    for _ in 0..60 {
        h.step();
    }
    assert!(heard.lock().unwrap().is_empty());
}
