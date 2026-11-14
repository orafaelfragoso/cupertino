//! `cupertino_layers_v1` end to end: the compositor runs in-process on an offscreen EGL device and a client thread makes
//! a layer shell surface the parent of layers, each request of the protocol sent as the client would. The tests read
//! the layers as the scene shows them, the events the client gets and the pixels drawn. Tests skip (pass) when no EGL
//! device is available.
//!
//! With `CUPERTINO_LAYER_CAPTURES=DIR`, `layers_v1::captures` writes each material effect drawn through layers, in the
//! light and the dark appearance at scale 2, to `DIR`.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_protocols::client::layers::cupertino_layer_v1::{self, CupertinoLayerV1};
use cupertino_protocols::client::layers::cupertino_layers_v1::CupertinoLayersV1;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::layers_v1::{present, presentation, LayerDraw, Presented};
use cupertino_server::Cupertino;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface as ServerSurface;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use cupertino_layer_v1::Property;

/// The output in device pixels; the captures' is as large in points at scale 2.
const OUTPUT: (i32, i32) = (800, 600);
/// The parent: a layer shell surface this size at these margins from the top-left corner.
const PARENT: (i32, i32) = (600, 400);
const MARGIN: (i32, i32) = (100, 50);
/// A popup of the parent: this size, its top-left corner at this point of the parent.
const POPUP: (i32, i32) = (200, 100);
const POPUP_AT: (i32, i32) = (100, 100);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new(scale: f64, dark: bool) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let cfg = Config {
            appearance: if dark {
                cupertino_theme::Appearance::Dark
            } else {
                cupertino_theme::Appearance::Light
            },
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
        let size = (
            (f64::from(OUTPUT.0) * scale) as i32,
            (f64::from(OUTPUT.1) * scale) as i32,
        );
        match headless::init(&mut state, size, scale) {
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
            Duration::from_millis(2),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&mut Harness) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(self) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn parent(&self) -> Option<ServerSurface> {
        let map = layer_map_for_output(&self.state.outputs[0].output);
        let surface = map.layers().next().map(|l| l.wl_surface().clone());
        surface
    }

    /// The parent's first popup.
    fn popup(&self) -> Option<ServerSurface> {
        let parent = self.parent()?;
        smithay::desktop::PopupManager::popups_for_surface(&parent)
            .next()
            .map(|(p, _)| p.wl_surface().clone())
    }

    /// The parent's layers as the scene shows them at the current time; it starts what waits for its first frame.
    fn layers(&self) -> Vec<LayerDraw> {
        self.parent().map_or_else(Vec::new, |p| {
            present(
                &p,
                self.state.now(),
                self.state.display_options().reduce_motion,
            )
        })
    }

    fn screenshot(&mut self, name: &str) -> image::RgbaImage {
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
}

// ---- test client ---------------------------------------------------------------------------

/// What the client heard from the server.
#[derive(Default, Debug)]
struct Heard {
    finished: Vec<u32>,
    replaced: Vec<(u32, f64, f64)>,
    settled: usize,
    /// Interface and code of the protocol error the server posted, if any.
    error: Option<(String, u32)>,
}

type Cmd = Box<dyn FnOnce(&mut Client, &QueueHandle<Client>) + Send>;

struct Client {
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    layers: CupertinoLayersV1,
    parent: wl_surface::WlSurface,
    /// Each layer's surface and object, in creation order.
    made: Vec<(wl_surface::WlSurface, CupertinoLayerV1)>,
    paint: fn(i32, i32) -> [u8; 4],
    heard: Arc<Mutex<Heard>>,
    configured: bool,
    wm: xdg_wm_base::XdgWmBase,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    /// The popup's surface and objects, once made ([`Client::popup`]).
    popup: Option<(
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
        xdg_popup::XdgPopup,
    )>,
}

/// A buffer of `w`×`h` painted by `paint` (straight RGBA by position).
fn buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<Client>,
    (w, h): (i32, i32),
    paint: &dyn Fn(i32, i32) -> [u8; 4],
) -> wl_buffer::WlBuffer {
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let [r, g, b, a] = paint(x, y);
            let pm = |c: u8| (u16::from(c) * u16::from(a) / 255) as u8;
            let i = ((y * w + x) * 4) as usize;
            pixels[i..i + 4].copy_from_slice(&[pm(b), pm(g), pm(r), a]);
        }
    }
    file.write_all(&pixels).unwrap();
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

impl Client {
    /// Make a layer of the parent whose content is `size` painted by `paint`; it shows from the parent's next commit.
    fn layer(
        &mut self,
        qh: &QueueHandle<Client>,
        size: (i32, i32),
        paint: &dyn Fn(i32, i32) -> [u8; 4],
    ) -> usize {
        let surface = self.compositor.create_surface(qh, ());
        let layer = self.layers.get_layer(&surface, &self.parent, qh, ());
        if size.0 > 0 {
            let b = buffer(&self.shm, qh, size, paint);
            surface.attach(Some(&b), 0, 0);
            surface.damage_buffer(0, 0, size.0, size.1);
        }
        surface.commit();
        self.made.push((surface, layer));
        self.made.len() - 1
    }

    fn l(&self, i: usize) -> &CupertinoLayerV1 {
        &self.made[i].1
    }

    /// Make a popup of the parent, [`POPUP`] large at [`POPUP_AT`] in it and painted red once configured.
    fn popup(&mut self, qh: &QueueHandle<Client>) {
        let surface = self.compositor.create_surface(qh, ());
        let xdg = self.wm.get_xdg_surface(&surface, qh, surface.clone());
        let positioner = self.wm.create_positioner(qh, ());
        positioner.set_size(POPUP.0, POPUP.1);
        positioner.set_anchor_rect(POPUP_AT.0, POPUP_AT.1, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        let popup = xdg.get_popup(None, &positioner, qh, ());
        self.layer_surface.get_popup(&popup);
        positioner.destroy();
        surface.commit();
        self.popup = Some((surface, xdg, popup));
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
    xdg_positioner::XdgPositioner,
    xdg_popup::XdgPopup,
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    CupertinoLayersV1
);

impl Dispatch<CupertinoLayerV1, ()> for Client {
    fn event(
        state: &mut Self,
        _: &CupertinoLayerV1,
        event: cupertino_layer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut heard = state.heard.lock().unwrap();
        match event {
            cupertino_layer_v1::Event::Finished { serial } => heard.finished.push(serial),
            cupertino_layer_v1::Event::Replaced {
                serial,
                value,
                velocity,
            } => heard.replaced.push((serial, value, velocity)),
            cupertino_layer_v1::Event::Settled => heard.settled += 1,
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

/// A popup's xdg surface, whose wl_surface draws its red frame on the first configure.
impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for Client {
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
            let b = buffer(&state.shm, qh, POPUP, &red);
            surface.attach(Some(&b), 0, 0);
            surface.damage_buffer(0, 0, POPUP.0, POPUP.1);
            surface.commit();
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Client {
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
            if !state.configured {
                state.configured = true;
                let b = buffer(&state.shm, qh, PARENT, &state.paint);
                state.parent.attach(Some(&b), 0, 0);
                state.parent.damage_buffer(0, 0, PARENT.0, PARENT.1);
                state.parent.commit();
            }
        }
    }
}

struct TestClient {
    commands: Sender<Cmd>,
    heard: Arc<Mutex<Heard>>,
}

impl TestClient {
    /// A client whose parent is a layer shell surface painted by `paint`.
    fn spawn(socket: PathBuf, paint: fn(i32, i32) -> [u8; 4]) -> TestClient {
        let (commands, inbox): (Sender<Cmd>, Receiver<Cmd>) = channel();
        let heard = Arc::new(Mutex::new(Heard::default()));
        let h = heard.clone();
        std::thread::spawn(move || {
            let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            let layers: CupertinoLayersV1 = globals.bind(&qh, 1..=3, ()).unwrap();
            let wm: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let parent = compositor.create_surface(&qh, ());
            let ls = shell.get_layer_surface(
                &parent,
                None,
                zwlr_layer_shell_v1::Layer::Top,
                "layers-test".into(),
                &qh,
                (),
            );
            ls.set_size(PARENT.0 as u32, PARENT.1 as u32);
            ls.set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
            ls.set_margin(MARGIN.1, 0, 0, MARGIN.0);
            ls.set_exclusive_zone(-1);
            parent.commit();
            let mut client = Client {
                compositor,
                shm,
                layers,
                parent,
                made: Vec::new(),
                paint,
                heard: h.clone(),
                configured: false,
                wm,
                layer_surface: ls,
                popup: None,
            };
            loop {
                while let Ok(cmd) = inbox.try_recv() {
                    cmd(&mut client, &qh);
                }
                if queue.roundtrip(&mut client).is_err() {
                    let error = conn.protocol_error().expect("a protocol error");
                    h.lock().unwrap().error = Some((error.object_interface, error.code));
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        TestClient { commands, heard }
    }

    fn send(&self, cmd: impl FnOnce(&mut Client, &QueueHandle<Client>) + Send + 'static) {
        self.commands.send(Box::new(cmd)).unwrap();
    }

    fn heard<R>(&self, f: impl FnOnce(&Heard) -> R) -> R {
        f(&self.heard.lock().unwrap())
    }
}

fn grey(_: i32, _: i32) -> [u8; 4] {
    [128, 128, 128, 255]
}

fn red(_: i32, _: i32) -> [u8; 4] {
    [255, 0, 0, 255]
}

/// A client with one red 100×50 layer, committed by the parent; the harness waits until it shows.
fn one_layer(h: &mut Harness) -> TestClient {
    let client = TestClient::spawn(h.socket_path(), grey);
    h.wait_until("the parent", |h| h.parent().is_some());
    client.send(|c, qh| {
        c.layer(qh, (100, 50), &red);
        c.parent.commit();
    });
    h.wait_until("the layer", |h| {
        h.layers()
            .first()
            .is_some_and(|l| l.content == (100.0, 50.0))
    });
    client
}

fn x_of(h: &Harness) -> f64 {
    h.layers()[0].x
}

/// The presentation of the parent's popup as the scene shows it now.
fn popup_presented(h: &Harness) -> Option<Presented> {
    h.popup()
        .and_then(|p| presentation(&p, h.state.now(), h.state.display_options().reduce_motion))
}

/// The parent's presentation as the scene shows it now.
fn presented(h: &Harness) -> Option<Presented> {
    h.parent()
        .and_then(|p| presentation(&p, h.state.now(), h.state.display_options().reduce_motion))
}

/// A client whose grey parent has a presentation, `made[0]`, committed with it; the harness waits until it shows.
fn presented_parent(h: &mut Harness) -> TestClient {
    let client = TestClient::spawn(h.socket_path(), grey);
    h.wait_until("the parent", |h| h.parent().is_some());
    client.send(|c, qh| {
        let parent = c.parent.clone();
        let p = c.layers.get_presentation(&parent, qh, ());
        c.made.push((parent, p));
        c.parent.commit();
    });
    h.wait_until("the presentation", |h| presented(h).is_some());
    client
}

// ---- tests ---------------------------------------------------------------------------------

#[test]
fn geometry_applies_with_the_parents_commit_in_fixed_point() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    let l = &h.layers()[0];
    assert_eq!(
        (l.x, l.y, l.width, l.height, l.opacity, l.scale),
        (0.0, 0.0, 100.0, 50.0, 1.0, 1.0)
    );
    // asked but not committed: what shows stays
    client.send(|c, _| {
        c.l(0).set_value(Property::X, 10.5);
        c.l(0).set_value(Property::Y, 20.25);
    });
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(x_of(&h), 0.0);
    // the layer's own commit does not apply it either
    client.send(|c, _| c.made[0].0.commit());
    for _ in 0..20 {
        h.step();
    }
    assert_eq!(x_of(&h), 0.0);
    client.send(|c, _| c.parent.commit());
    h.wait_until("the parent's commit", |h| x_of(h) == 10.5);
    assert_eq!(h.layers()[0].y, 20.25);
    // drawn on the output's device pixels: the red content starts half way into pixel 110 of the 1x output
    let shot = h.screenshot("fixed-point");
    let at = |x: u32, y: u32| shot.get_pixel(x, y).0;
    assert_eq!(at(MARGIN.0 as u32 + 60, MARGIN.1 as u32 + 40)[0], 255);
    assert_ne!(
        at(MARGIN.0 as u32 + 5, MARGIN.1 as u32 + 5),
        [255, 0, 0, 255]
    );
}

#[test]
fn an_animation_starts_with_its_first_frame_and_finishes_on_the_fixed_clock() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    h.state.fixed_time = Some(1000.0);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(7, Property::X, 100.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    // the first frame that shows the commit starts it at the fixed time
    h.wait_until("the animation", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, h.state.now()))
    });
    assert_eq!(x_of(&h), 0.0);
    h.state.fixed_time = Some(1000.5);
    assert!((x_of(&h) - 50.0).abs() < 1e-6);
    // the client hit-tests against the target
    let p = h.parent().unwrap();
    let serial = h.layers()[0].serial;
    assert_eq!(
        cupertino_server::layers_v1::target(&p, serial, 0),
        Some(100.0)
    );
    assert!(client.heard(|e| e.finished.is_empty()));
    h.state.fixed_time = Some(1001.0);
    h.wait_until("finished and settled", |_| {
        client.heard(|e| e.finished == [7] && e.settled == 1)
    });
    assert_eq!(x_of(&h), 100.0);
}

#[test]
fn a_retarget_is_continuous_and_reports_what_was_shown() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    h.state.fixed_time = Some(50.0);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(1, Property::X, 100.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the first animation", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, 50.0))
    });
    h.state.fixed_time = Some(50.25);
    client.send(|c, _| {
        c.l(0).animate_spring(2, Property::X, 0.0, 0.4, 0.9, 0.0);
        c.parent.commit();
    });
    h.wait_until("replaced", |_| client.heard(|e| !e.replaced.is_empty()));
    let (serial, value, velocity) = client.heard(|e| e.replaced[0]);
    assert_eq!(serial, 1);
    assert!((value - 25.0).abs() < 0.01, "{value}");
    assert!((velocity - 100.0).abs() < 0.5, "{velocity}");
    // the spring leaves from the value and the velocity shown
    let x0 = x_of(&h);
    h.state.fixed_time = Some(50.26);
    let x1 = x_of(&h);
    assert!((x0 - 25.0).abs() < 0.01 && x1 > x0, "{x0} {x1}");
    h.state.fixed_time = Some(60.0);
    h.wait_until("the spring", |_| client.heard(|e| e.finished == [2]));
    assert!(x_of(&h).abs() < 1e-9);
}

#[test]
fn the_time_scale_runs_animations_on_the_model_clock() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    // a thousand model seconds per second: a ten second animation ends within about ten milliseconds
    h.state.time_scale = 1000.0;
    let started = Instant::now();
    client.send(|c, _| {
        c.l(0)
            .animate_timed(3, Property::Opacity, 0.0, 10_000, 0.25, 0.1, 0.25, 1.0);
        c.parent.commit();
    });
    h.wait_until("finished", |_| client.heard(|e| e.finished == [3]));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(h.layers()[0].opacity, 0.0);
}

#[test]
fn reduce_motion_ends_spatial_animations_at_once_and_keeps_fades() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    h.state.accessibility.settings.display.reduce_motion = true;
    h.state.fixed_time = Some(10.0);
    client.send(|c, _| {
        c.l(0).animate_spring(4, Property::Y, 80.0, 0.5, 0.8, 0.0);
        c.l(0)
            .animate_timed(5, Property::Opacity, 0.0, 400, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the move", |h| h.layers()[0].y == 80.0);
    assert_eq!(
        h.layers()[0].opacity,
        1.0,
        "the fade starts from its first frame"
    );
    h.wait_until("the jump finished", |_| client.heard(|e| e.finished == [4]));
    h.state.fixed_time = Some(10.2);
    assert!((h.layers()[0].opacity - 0.5).abs() < 1e-6);
}

#[test]
fn a_set_value_replaces_and_cancel_freezes() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    h.state.fixed_time = Some(5.0);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(8, Property::Scale, 2.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("started", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, 5.0))
    });
    h.state.fixed_time = Some(5.5);
    client.send(|c, _| {
        c.l(0).cancel(Property::Scale);
        c.parent.commit();
    });
    h.wait_until("cancelled", |_| client.heard(|e| e.replaced.len() == 1));
    assert!((h.layers()[0].scale - 1.5).abs() < 1e-6);
    h.state.fixed_time = Some(9.0);
    assert!((h.layers()[0].scale - 1.5).abs() < 1e-6);
    h.wait_until("settled", |_| client.heard(|e| e.settled == 1));
}

#[test]
fn late_content_shows_the_placeholder_where_it_does_not_reach() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    client.send(|c, _| {
        c.l(0).set_value(Property::Width, 200.0);
        c.l(0).set_value(Property::Height, 100.0);
        c.l(0).set_placeholder(0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the larger bounds", |h| h.layers()[0].width == 200.0);
    let shot = h.screenshot("late-content");
    let at = |x: i32, y: i32| {
        shot.get_pixel((MARGIN.0 + x) as u32, (MARGIN.1 + y) as u32)
            .0
    };
    assert_eq!(at(50, 25), [255, 0, 0, 255], "the content");
    assert_eq!(at(150, 25), [0, 0, 255, 255], "right of it");
    assert_eq!(at(50, 75), [0, 0, 255, 255], "below it");
    assert_eq!(
        at(250, 25),
        [128, 128, 128, 255],
        "the parent past the bounds"
    );
    // the content arrives: it shows from its own commit, at the bounds the server shows
    client.send(|c, qh| {
        let b = buffer(&c.shm, qh, (200, 100), &|_, _| [0, 255, 0, 255]);
        c.made[0].0.attach(Some(&b), 0, 0);
        c.made[0].0.damage_buffer(0, 0, 200, 100);
        c.made[0].0.commit();
    });
    h.wait_until("the content", |h| h.layers()[0].content == (200.0, 100.0));
    let shot = h.screenshot("content-arrived");
    assert_eq!(
        shot.get_pixel((MARGIN.0 + 150) as u32, (MARGIN.1 + 75) as u32)
            .0,
        [0, 255, 0, 255]
    );
}

#[test]
fn layers_take_no_input_and_are_clipped_to_their_bounds_when_asked() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    client.send(|c, _| {
        c.l(0).set_value(Property::Width, 40.0);
        c.l(0).set_value(Property::CornerRadius, 0.0);
        c.l(0).set_masks_to_bounds(1);
        c.parent.commit();
    });
    h.wait_until("masked", |h| h.layers()[0].masks);
    let shot = h.screenshot("masked");
    let at = |x: i32, y: i32| {
        shot.get_pixel((MARGIN.0 + x) as u32, (MARGIN.1 + y) as u32)
            .0
    };
    assert_eq!(at(20, 25), [255, 0, 0, 255]);
    assert_eq!(at(70, 25), [128, 128, 128, 255], "clipped to the bounds");
    // the pointer over the layer is over the parent
    h.state.pointer_moved_to(
        ((MARGIN.0 + 20) as f64, (MARGIN.1 + 25) as f64).into(),
        1,
        None,
    );
    let parent = h.parent().unwrap();
    let focus = h.state.pointer.current_focus();
    assert!(
        focus.is_none_or(|f| {
            use smithay::wayland::seat::WaylandFocus;
            f.wl_surface().is_none_or(|s| *s == parent)
        }),
        "the layer's surface takes no input"
    );
}

#[test]
fn limits_and_bad_values_are_protocol_errors() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    // a timed animation over ten seconds
    let client = one_layer(&mut h);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(1, Property::X, 1.0, 10_001, 0.0, 0.0, 1.0, 1.0)
    });
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()),
        Some((
            "cupertino_layer_v1".into(),
            cupertino_layer_v1::Error::InvalidCurve as u32
        ))
    );
    // opacity above 1
    let client = one_layer(&mut h);
    client.send(|c, _| c.l(0).set_value(Property::Opacity, 1.5));
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidValue as u32
    );
    // a lens below 1
    let client = one_layer(&mut h);
    client.send(|c, _| c.l(0).set_glass(0, 0.5));
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidValue as u32
    );
    // a parent holds 64 layers
    let client = one_layer(&mut h);
    client.send(|c, qh| {
        for _ in 0..64 {
            c.layer(qh, (0, 0), &red);
        }
    });
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()),
        Some((
            "cupertino_layers_v1".into(),
            cupertino_protocols::client::layers::cupertino_layers_v1::Error::TooManyLayers as u32
        ))
    );
    // the surface is its own parent
    let client = one_layer(&mut h);
    client.send(|c, qh| {
        let parent = c.parent.clone();
        let _ = c.layers.get_layer(&parent, &parent, qh, ());
    });
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_protocols::client::layers::cupertino_layers_v1::Error::Role as u32
    );
}

#[test]
fn a_presentation_fades_moves_and_scales_all_its_surface_draws_on_the_server_clock() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = presented_parent(&mut h);
    assert_eq!(
        presented(&h),
        Some(Presented {
            x: 0.0,
            y: 0.0,
            opacity: 1.0,
            scale: 1.0
        }),
        "it starts at rest"
    );
    assert!(
        h.layers().is_empty(),
        "a presentation is not drawn as a layer"
    );
    let inside = (MARGIN.0 as u32 + 40, MARGIN.1 as u32 + 20);
    let shot = h.screenshot("presentation-rest");
    assert_eq!(shot.get_pixel(inside.0, inside.1).0, [128, 128, 128, 255]);
    // faded out with the commit: the surface no longer shows
    client.send(|c, _| {
        c.l(0).set_value(Property::Opacity, 0.0);
        c.parent.commit();
    });
    h.wait_until("the fade", |h| {
        presented(h).is_some_and(|p| p.opacity == 0.0)
    });
    let shot = h.screenshot("presentation-faded");
    assert_ne!(shot.get_pixel(inside.0, inside.1).0, [128, 128, 128, 255]);
    // a fade in along the server's clock, finished and settled
    h.state.fixed_time = Some(200.0);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(9, Property::Opacity, 1.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the animation", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, 200.0))
    });
    h.state.fixed_time = Some(200.5);
    assert!((presented(&h).unwrap().opacity - 0.5).abs() < 1e-6);
    h.state.fixed_time = Some(201.0);
    h.wait_until("finished and settled", |_| {
        client.heard(|e| e.finished == [9] && e.settled == 1)
    });
    // moved and scaled about its centre: the top-left corner leaves, the centre stays
    client.send(|c, _| {
        c.l(0).set_value(Property::X, 30.0);
        c.l(0).set_value(Property::Scale, 0.5);
        c.parent.commit();
    });
    h.wait_until("the move", |h| {
        presented(h).is_some_and(|p| p.x == 30.0 && p.scale == 0.5)
    });
    let shot = h.screenshot("presentation-scaled");
    assert_ne!(shot.get_pixel(inside.0, inside.1).0, [128, 128, 128, 255]);
    let centre = (
        (MARGIN.0 + PARENT.0 / 2 + 30) as u32,
        (MARGIN.1 + PARENT.1 / 2) as u32,
    );
    assert_eq!(shot.get_pixel(centre.0, centre.1).0, [128, 128, 128, 255]);
    let right_edge = ((MARGIN.0 + PARENT.0 - 10) as u32, centre.1);
    assert_ne!(
        shot.get_pixel(right_edge.0, right_edge.1).0,
        [128, 128, 128, 255]
    );
    // destroyed: back at rest with the next commit
    client.send(|c, _| {
        c.l(0).destroy();
        c.parent.commit();
    });
    h.wait_until("at rest", |h| presented(h).is_none());
}

#[test]
fn a_presentation_is_a_layer_shell_surface_s_own_and_has_four_properties() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    // width is not a property of a presentation
    let client = presented_parent(&mut h);
    client.send(|c, _| c.l(0).set_value(Property::Width, 10.0));
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()),
        Some((
            "cupertino_layer_v1".into(),
            cupertino_layer_v1::Error::InvalidProperty as u32
        ))
    );
    // nor is glass
    let client = presented_parent(&mut h);
    client.send(|c, _| c.l(0).set_glass(0, 1.0));
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidProperty as u32
    );
    // a second presentation of the same surface
    let client = presented_parent(&mut h);
    client.send(|c, qh| {
        let parent = c.parent.clone();
        let _ = c.layers.get_presentation(&parent, qh, ());
    });
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()),
        Some((
            "cupertino_layers_v1".into(),
            cupertino_protocols::client::layers::cupertino_layers_v1::Error::Role as u32
        ))
    );
    // a surface without the layer shell's role
    let client = presented_parent(&mut h);
    client.send(|c, qh| {
        let plain = c.compositor.create_surface(qh, ());
        let _ = c.layers.get_presentation(&plain, qh, ());
    });
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_protocols::client::layers::cupertino_layers_v1::Error::Role as u32
    );
}

/// The colour at `(x, y)` of the parent, in `shot`.
fn at(shot: &image::RgbaImage, x: i32, y: i32) -> [u8; 4] {
    shot.get_pixel((MARGIN.0 + x) as u32, (MARGIN.1 + y) as u32)
        .0
}

fn differs(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(y) as u32)
        .sum::<u32>()
        > 12
}

#[test]
fn a_glass_group_joins_the_glass_of_its_layers_into_one_shape_as_they_move() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = TestClient::spawn(h.socket_path(), scene);
    h.wait_until("the parent", |h| h.parent().is_some());
    // a field and a button beside it, 8 points apart, each with glass and no content
    client.send(|c, qh| {
        for (x, w) in [(100.0, 200.0), (308.0, 56.0)] {
            let i = c.layer(qh, (0, 0), &red);
            let l = c.l(i);
            l.set_value(Property::X, x);
            l.set_value(Property::Y, 100.0);
            l.set_value(Property::Width, w);
            l.set_value(Property::Height, 56.0);
            l.set_value(Property::CornerRadius, 28.0);
            l.set_glass(0, 1.0);
        }
        c.parent.commit();
    });
    h.wait_until("the layers", |h| {
        h.layers().len() == 2 && h.layers()[1].width == 56.0
    });
    let gap = (304, 128);
    let apart = h.screenshot("glass-apart");
    let behind = scene(gap.0, gap.1);
    assert_eq!(
        at(&apart, gap.0, gap.1),
        behind,
        "the gap shows the parent while the glass is apart"
    );
    // in one group the shapes join across the gap
    client.send(|c, _| {
        c.l(0).set_glass_group(7, 16.0);
        c.l(1).set_glass_group(7, 16.0);
        c.parent.commit();
    });
    h.wait_until("the group", |h| h.layers().iter().all(|l| l.group == 7));
    assert!(h.layers().iter().all(|l| l.spacing == 16.0));
    let joined = h.screenshot("glass-joined");
    assert!(
        differs(at(&joined, gap.0, gap.1), behind),
        "the joined glass fills the gap: {:?}",
        at(&joined, gap.0, gap.1)
    );
    // the button moves out on the server's clock and its glass parts from the field's as the shapes do
    h.state.fixed_time = Some(300.0);
    client.send(|c, _| {
        c.l(1)
            .animate_timed(3, Property::X, 400.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the move", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, 300.0))
    });
    h.state.fixed_time = Some(301.0);
    h.wait_until("finished", |_| client.heard(|e| e.finished == [3]));
    let parted = h.screenshot("glass-parted");
    assert_eq!(
        at(&parted, 350, 128),
        scene(350, 128),
        "far apart the group's shapes no longer join"
    );
    assert!(
        differs(at(&parted, 428, 128), scene(428, 128)),
        "the button's glass moved with it"
    );
    // a spacing past the most the server blends is refused
    client.send(|c, _| c.l(0).set_glass_group(1, 65.0));
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidValue as u32
    );
}

#[test]
fn a_popup_s_presentation_scales_and_fades_it_on_the_server_clock() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = TestClient::spawn(h.socket_path(), grey);
    h.wait_until("the parent", |h| h.parent().is_some());
    client.send(|c, qh| c.popup(qh));
    let red = [255, 0, 0, 255];
    let near_corner = (POPUP_AT.0 + 10, POPUP_AT.1 + 10);
    let centre = (POPUP_AT.0 + POPUP.0 / 2, POPUP_AT.1 + POPUP.1 / 2);
    let mut shot = h.screenshot("popup");
    let end = Instant::now() + common::HANG_GUARD;
    while at(&shot, near_corner.0, near_corner.1) != red && Instant::now() < end {
        shot = h.screenshot("popup");
    }
    assert_eq!(
        at(&shot, near_corner.0, near_corner.1),
        red,
        "the popup shows"
    );
    // its presentation at half its size about its centre, then faded out along the server's clock
    client.send(|c, qh| {
        let surface = c.popup.as_ref().unwrap().0.clone();
        let p = c.layers.get_presentation(&surface, qh, ());
        p.set_value(Property::Scale, 0.5);
        surface.commit();
        c.made.push((surface, p));
    });
    h.wait_until("the scale", |h| {
        popup_presented(h).is_some_and(|p| p.scale == 0.5)
    });
    let half = h.screenshot("popup-half");
    assert_eq!(at(&half, centre.0, centre.1), red, "the centre stays");
    assert_eq!(
        at(&half, near_corner.0, near_corner.1),
        [128, 128, 128, 255],
        "the corner drew in towards it"
    );
    h.state.fixed_time = Some(400.0);
    client.send(|c, _| {
        let (surface, p) = c.made.last().unwrap();
        p.animate_timed(5, Property::Opacity, 0.0, 200, 0.33, 1.0, 0.67, 1.0);
        surface.commit();
    });
    h.wait_until("the fade", |h| {
        popup_presented(h).is_some_and(|p| p.opacity < 1.0)
            || h.popup()
                .is_some_and(|s| !cupertino_server::layers_v1::settled(&s, 400.0))
    });
    h.state.fixed_time = Some(400.2);
    h.wait_until("finished and settled", |_| {
        client.heard(|e| e.finished == [5] && e.settled == 1)
    });
    let gone = h.screenshot("popup-gone");
    assert_eq!(
        at(&gone, centre.0, centre.1),
        [128, 128, 128, 255],
        "faded out"
    );
    assert!(client.heard(|e| e.error.is_none()));
}

#[test]
fn too_many_animations_is_a_protocol_error() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    client.send(|c, _| {
        for i in 0..=cupertino_server::layers_v1::MAX_ANIMATIONS as u32 {
            c.l(0)
                .animate_timed(i, Property::X, 1.0, 1000, 0.0, 0.0, 1.0, 1.0);
        }
    });
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::TooManyAnimations as u32
    );
}

// ---- the material effects, by eye ----------------------------------------------------------

/// A colourful scene behind the effects: stripes of colour with a dark and a light band.
fn scene(x: i32, y: i32) -> [u8; 4] {
    let band = (x / 24) % 6;
    let c = match band {
        0 => [230, 60, 50],
        1 => [250, 190, 40],
        2 => [60, 180, 90],
        3 => [40, 120, 230],
        4 => [150, 70, 200],
        _ => [245, 245, 245],
    };
    if y > 300 {
        return [20, 20, 30, 255];
    }
    [c[0], c[1], c[2], 255]
}

/// Glyph-like coverage: white bars of a word on transparency.
fn word(x: i32, y: i32) -> [u8; 4] {
    let stroke = (x % 14) < 9 && (6..34).contains(&y) && (y % 10 < 7 || x % 14 < 3);
    if stroke {
        [255, 255, 255, 255]
    } else {
        [0, 0, 0, 0]
    }
}

#[test]
fn captures() {
    let Some(dir) = std::env::var_os("CUPERTINO_LAYER_CAPTURES").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    for dark in [false, true] {
        let Some(mut h) = Harness::new(2.0, dark) else {
            return;
        };
        let client = TestClient::spawn(h.socket_path(), scene);
        h.wait_until("the parent", |h| h.parent().is_some());
        client.send(|c, qh| {
            // vibrant foreground: a regular glass pill with a vibrant word over it, and a secondary one
            let pill = c.layer(qh, (0, 0), &red);
            for (prop, v) in [
                (Property::X, 20.0),
                (Property::Y, 20.0),
                (Property::Width, 260.0),
                (Property::Height, 60.0),
                (Property::CornerRadius, 30.0),
            ] {
                c.l(pill).set_value(prop, v);
            }
            c.l(pill).set_glass(0, 1.0);
            let text = c.layer(qh, (126, 40), &word);
            c.l(text).set_value(Property::X, 40.0);
            c.l(text).set_value(Property::Y, 30.0);
            c.l(text).set_blend(cupertino_layer_v1::Blend::Vibrant);
            let text2 = c.layer(qh, (84, 40), &word);
            c.l(text2).set_value(Property::X, 180.0);
            c.l(text2).set_value(Property::Y, 30.0);
            c.l(text2)
                .set_blend(cupertino_layer_v1::Blend::VibrantSecondary);
            // a lifted thumb: a raised clear lens over a track, grown by a quarter
            let thumb = c.layer(qh, (0, 0), &red);
            for (prop, v) in [
                (Property::X, 340.0),
                (Property::Y, 30.0),
                (Property::Width, 50.0),
                (Property::Height, 31.0),
                (Property::CornerRadius, 15.5),
                (Property::Scale, 1.25),
            ] {
                c.l(thumb).set_value(prop, v);
            }
            c.l(thumb).set_glass(1, 1.5);
            c.l(thumb)
                .set_elevation(cupertino_layer_v1::Elevation::Raised);
            // a pointer-lit glass card
            let lit = c.layer(qh, (0, 0), &red);
            for (prop, v) in [
                (Property::X, 440.0),
                (Property::Y, 20.0),
                (Property::Width, 140.0),
                (Property::Height, 90.0),
                (Property::CornerRadius, 22.0),
            ] {
                c.l(lit).set_value(prop, v);
            }
            c.l(lit).set_glass(0, 1.0);
            c.l(lit)
                .set_highlight(cupertino_layer_v1::Highlight::Pointer);
            // variable blur: a bar whose blur fades out towards its bottom, beside one that blurs evenly
            let ramp = c.layer(qh, (0, 0), &red);
            for (prop, v) in [
                (Property::X, 20.0),
                (Property::Y, 150.0),
                (Property::Width, 270.0),
                (Property::Height, 120.0),
                (Property::CornerRadius, 16.0),
            ] {
                c.l(ramp).set_value(prop, v);
            }
            c.l(ramp).set_glass(14, 1.0);
            c.l(ramp).set_blur_ramp(1.0, 0.0);
            let even = c.layer(qh, (0, 0), &red);
            for (prop, v) in [
                (Property::X, 310.0),
                (Property::Y, 150.0),
                (Property::Width, 270.0),
                (Property::Height, 120.0),
                (Property::CornerRadius, 16.0),
            ] {
                c.l(even).set_value(prop, v);
            }
            c.l(even).set_glass(14, 1.0);
            // vibrant over the dark band: lifted towards white
            let low = c.layer(qh, (126, 40), &word);
            c.l(low).set_value(Property::X, 20.0);
            c.l(low).set_value(Property::Y, 330.0);
            c.l(low).set_blend(cupertino_layer_v1::Blend::Vibrant);
            c.parent.commit();
        });
        h.wait_until("the layers", |h| h.layers().len() == 8);
        // the pointer up and to the left of the lit card
        h.state.pointer_moved_to(
            ((MARGIN.0 + 440) as f64, (MARGIN.1 + 10) as f64).into(),
            1,
            None,
        );
        for _ in 0..10 {
            h.step();
        }
        let shot = h.screenshot("effects");
        let name = if dark { "dark" } else { "light" };
        shot.save(dir.join(format!("effects-{name}.png"))).unwrap();
        // each effect cropped (device pixels at scale 2)
        let crop = |x: i32, y: i32, w: i32, h: i32| {
            image::imageops::crop_imm(
                &shot,
                ((MARGIN.0 + x) * 2) as u32,
                ((MARGIN.1 + y) * 2) as u32,
                (w * 2) as u32,
                (h * 2) as u32,
            )
            .to_image()
        };
        crop(0, 0, 320, 110)
            .save(dir.join(format!("vibrant-{name}.png")))
            .unwrap();
        crop(320, 0, 100, 110)
            .save(dir.join(format!("lifted-lens-{name}.png")))
            .unwrap();
        crop(420, 0, 180, 130)
            .save(dir.join(format!("pointer-lit-{name}.png")))
            .unwrap();
        crop(0, 140, 600, 140)
            .save(dir.join(format!("variable-blur-{name}.png")))
            .unwrap();
        crop(0, 310, 200, 70)
            .save(dir.join(format!("vibrant-on-dark-band-{name}.png")))
            .unwrap();
        drop(client);
    }
}
