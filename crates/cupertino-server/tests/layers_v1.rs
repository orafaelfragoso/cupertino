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
            let layers: CupertinoLayersV1 = globals.bind(&qh, 1..=5, ()).unwrap();
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

/// The channel of `shot` at the parent's point `(x, y)` plus `(dx, dy)` device pixels, at scale 2.
fn at2(shot: &image::RgbaImage, (x, y): (f64, f64), (dx, dy): (i32, i32)) -> [u8; 4] {
    let px = ((f64::from(MARGIN.0) + x) * 2.0) as i32 + dx;
    let py = ((f64::from(MARGIN.1) + y) * 2.0) as i32 + dy;
    shot.get_pixel(px as u32, py as u32).0
}

fn black(c: [u8; 4]) -> bool {
    c[..3].iter().all(|&v| v <= 3)
}

#[test]
fn a_glass_edge_runs_outside_the_group_s_joined_outline_at_its_width_whatever_the_scale() {
    let Some(mut h) = Harness::new(2.0, false) else {
        return;
    };
    let client = TestClient::spawn(h.socket_path(), grey);
    h.wait_until("the parent", |h| h.parent().is_some());
    // a capsule of glass with an edge a point wide, black
    client.send(|c, qh| {
        let i = c.layer(qh, (0, 0), &red);
        let l = c.l(i);
        l.set_value(Property::X, 100.0);
        l.set_value(Property::Y, 100.0);
        l.set_value(Property::Width, 200.0);
        l.set_value(Property::Height, 56.0);
        l.set_value(Property::CornerRadius, 28.0);
        l.set_glass(0, 1.0);
        l.set_glass_edge(1.0, 0.0, 0.0, 0.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the edge", |h| {
        h.layers().first().is_some_and(|l| l.edge.width == 1.0)
    });
    assert_eq!(h.layers()[0].edge.color, [0.0, 0.0, 0.0, 1.0]);
    let shot = h.screenshot("edge");
    // along the top at the middle: the two device pixels above the outline are the line, the glass below it and the
    // parent above it are not
    let top = (200.0, 100.0);
    for dy in [-2, -1] {
        assert!(
            black(at2(&shot, top, (0, dy))),
            "{dy}: {:?}",
            at2(&shot, top, (0, dy))
        );
    }
    for dy in [-4, -3, 0, 1] {
        assert!(
            !black(at2(&shot, top, (0, dy))),
            "{dy}: {:?}",
            at2(&shot, top, (0, dy))
        );
    }
    // scaled by 1.5 about its centre the outline moves out to 86 points and the line keeps its two pixels
    client.send(|c, _| {
        c.l(0).set_value(Property::Scale, 1.5);
        c.parent.commit();
    });
    h.wait_until("the scale", |h| h.layers()[0].scale == 1.5);
    let scaled = h.screenshot("edge-scaled");
    let top = (200.0, 86.0);
    for dy in [-2, -1] {
        assert!(
            black(at2(&scaled, top, (0, dy))),
            "scaled {dy}: {:?}",
            at2(&scaled, top, (0, dy))
        );
    }
    for dy in [-4, -3, 0, 1] {
        assert!(
            !black(at2(&scaled, top, (0, dy))),
            "scaled {dy}: {:?}",
            at2(&scaled, top, (0, dy))
        );
    }
    // a button budding out of the field's right end, in one glass group: the line follows each shape's outline, left
    // out where each shape's own outline lies inside the other, and the group draws the edge of its first layer
    client.send(|c, qh| {
        c.l(0).set_value(Property::Scale, 1.0);
        let i = c.layer(qh, (0, 0), &red);
        let l = c.l(i);
        l.set_value(Property::X, 280.0);
        l.set_value(Property::Y, 100.0);
        l.set_value(Property::Width, 56.0);
        l.set_value(Property::Height, 56.0);
        l.set_value(Property::CornerRadius, 28.0);
        l.set_glass(0, 1.0);
        c.l(0).set_glass_group(7, 8.0);
        c.l(1).set_glass_group(7, 8.0);
        c.parent.commit();
    });
    h.wait_until("the group", |h| {
        h.layers().len() == 2
            && h.layers().iter().all(|l| l.group == 7)
            && h.layers()[0].scale == 1.0
    });
    let joined = h.screenshot("edge-joined");
    let middle = 128.0;
    for (what, x) in [
        ("the field's end inside the button", 300.0),
        ("the button's side inside the field", 280.0),
    ] {
        for dx in [-2, -1, 0, 1] {
            let c = at2(&joined, (x, middle), (dx, 0));
            assert!(!black(c), "{what} {dx}: {c:?}");
        }
    }
    for dx in [0, 1] {
        let c = at2(&joined, (336.0, middle), (dx, 0));
        assert!(black(c), "the button's outer side {dx}: {c:?}");
    }
    // an edge wider than the server draws is refused
    client.send(|c, _| c.l(0).set_glass_edge(4.5, 0.0, 0.0, 0.0, 1.0));
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidValue as u32
    );
}

/// The share of a unit Gaussian below `z` standard deviations (its integral), from the error function's rational
/// approximation, good to well under a thousandth.
fn gaussian_share(z: f64) -> f64 {
    let x = z.abs() / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-x * x).exp();
    0.5 * (1.0 + erf.copysign(z))
}

#[test]
fn a_layer_s_content_blur_is_in_points_and_scales_with_the_layer_at_a_fractional_scale() {
    let Some(mut h) = Harness::new(1.5, false) else {
        return;
    };
    let client = one_layer(&mut h);
    client.send(|c, _| {
        c.l(0).set_value(Property::X, 200.0);
        c.l(0).set_value(Property::Y, 100.0);
        c.l(0).set_value(Property::ContentBlur, 4.0);
        c.parent.commit();
    });
    h.wait_until("the blur", |h| h.layers()[0].content_blur == 4.0);
    // the right edge lies at the parent's 300 points: 600 device pixels from the output's left edge at 1.5
    let edge = f64::from(MARGIN.0 + 300) * 1.5;
    let row = ((f64::from(MARGIN.1) + 125.0) * 1.5) as u32;
    let profile = |shot: &image::RgbaImage, sigma_px: f64| {
        for x in (edge as u32 - 16)..(edge as u32 + 16) {
            let share = gaussian_share((edge - (f64::from(x) + 0.5)) / sigma_px);
            let red = 255.0 * share + 128.0 * (1.0 - share);
            let shown = shot.get_pixel(x, row).0;
            assert!(
                (f64::from(shown[0]) - red).abs() <= 3.0,
                "{x}: {shown:?} against {red:.1} for a deviation of {sigma_px} pixels"
            );
        }
    };
    // four points are six device pixels
    profile(&h.screenshot("blur-fractional"), 6.0);
    // scaled to half about its centre, the layer's blur is half as wide on the output, as its content is, and centred
    // on the edge the scaled content is drawn at (which the output's pixel grid rounds)
    client.send(|c, _| {
        c.l(0).set_value(Property::Scale, 0.5);
        c.l(0).set_value(Property::ContentBlur, 0.0);
        c.parent.commit();
    });
    h.wait_until("the scale", |h| h.layers()[0].scale == 0.5);
    let sharp = h.screenshot("blur-scaled-sharp");
    // the content's centre stays at the parent's (250, 125); its right edge comes in to about 275 points
    let near = (f64::from(MARGIN.0 + 275) * 1.5) as u32;
    let edge = (near - 4..near + 4)
        .find(|&x| sharp.get_pixel(x, row).0[0] < 192)
        .expect("the scaled content's edge");
    assert!(
        edge.abs_diff(near) <= 1,
        "the scaled edge at {edge}, not by {near}"
    );
    client.send(|c, _| {
        c.l(0).set_value(Property::ContentBlur, 4.0);
        c.parent.commit();
    });
    h.wait_until("the blur again", |h| h.layers()[0].content_blur == 4.0);
    let shot = h.screenshot("blur-scaled");
    let edge = f64::from(edge);
    for x in (edge as u32 - 10)..(edge as u32 + 10) {
        let share = gaussian_share((edge - (f64::from(x) + 0.5)) / 3.0);
        let red = 255.0 * share + 128.0 * (1.0 - share);
        let shown = shot.get_pixel(x, row).0;
        assert!(
            (f64::from(shown[0]) - red).abs() <= 3.0,
            "{x}: {shown:?} against {red:.1} scaled"
        );
    }
}

#[test]
fn a_layer_s_content_blurs_past_its_edges_by_the_gaussian_on_the_server_clock_and_the_parent_stays_sharp(
) {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    client.send(|c, _| {
        c.l(0).set_value(Property::X, 200.0);
        c.l(0).set_value(Property::Y, 100.0);
        c.parent.commit();
    });
    h.wait_until("the layer placed", |h| h.layers()[0].x == 200.0);
    // the red content covers the parent's (200, 100) to (300, 150)
    let (inside, outside, middle) = ((298, 125), (303, 125), (250, 125));
    let sharp = h.screenshot("blur-sharp");
    assert_eq!(at(&sharp, inside.0, inside.1), [255, 0, 0, 255]);
    assert_eq!(at(&sharp, outside.0, outside.1), grey(0, 0));
    // blurred by four points the edge softens both ways: the parent beside it reddens, the content by it pales
    client.send(|c, _| {
        c.l(0).set_value(Property::ContentBlur, 4.0);
        c.parent.commit();
    });
    h.wait_until("the blur", |h| h.layers()[0].content_blur == 4.0);
    let soft = h.screenshot("blur-soft");
    // across the right edge the colour follows the Gaussian's integral, the red's share of each pixel
    for x in 288..314 {
        let share = gaussian_share((300.0 - (f64::from(x) + 0.5)) / 4.0);
        let red = 255.0 * share + 128.0 * (1.0 - share);
        let shown = at(&soft, x, 125);
        assert!(
            (f64::from(shown[0]) - red).abs() <= 3.0
                && (f64::from(shown[1]) - 128.0 * (1.0 - share)).abs() <= 3.0,
            "{x}: {shown:?} against {red:.1}"
        );
    }
    let (i, o, m) = (
        at(&soft, inside.0, inside.1),
        at(&soft, outside.0, outside.1),
        at(&soft, middle.0, middle.1),
    );
    assert!(
        o[0] > 150 && o[1] < 110,
        "the blur reaches past the edge: {o:?}"
    );
    assert!(i[0] < 255 && i[1] > 10, "the edge's inside softens: {i:?}");
    assert!(
        m[0] >= 252 && m[1] <= 3,
        "well inside the content stays as it is: {m:?}"
    );
    assert_eq!(at(&soft, 120, 125), grey(0, 0), "the parent is not blurred");
    // a blur animates on the server's clock like any property
    h.state.fixed_time = Some(40.0);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(9, Property::ContentBlur, 12.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the blur's animation", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, 40.0))
    });
    h.state.fixed_time = Some(40.5);
    assert!((h.layers()[0].content_blur - 8.0).abs() < 1e-6);
    let wider = h.screenshot("blur-wider");
    assert!(
        at(&wider, 310, 125)[0] > at(&soft, 310, 125)[0],
        "a wider blur reaches further"
    );
    h.state.fixed_time = Some(41.0);
    h.wait_until("finished", |_| client.heard(|e| e.finished == [9]));
    // under reduce motion a blur goes to its target at once
    h.state.accessibility.settings.display.reduce_motion = true;
    client.send(|c, _| {
        c.l(0)
            .animate_timed(10, Property::ContentBlur, 0.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("sharp again", |h| h.layers()[0].content_blur == 0.0);
    // past the widest blur the server takes is refused
    client.send(|c, _| c.l(0).set_value(Property::ContentBlur, 64.5));
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
fn desktop_control_reads_the_clients_layouts_once_their_layers_show_their_targets() {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = presented_parent(&mut h);
    assert!(h.state.layers.settled(h.state.now()), "at rest");
    h.state.fixed_time = Some(500.0);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(4, Property::Opacity, 0.0, 300, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("the fade", |h| !h.state.layers.settled(500.0));
    h.state.fixed_time = Some(500.2);
    assert!(
        !h.state.layers.settled(500.2),
        "mid-way what is shown is not the layout's"
    );
    h.state.fixed_time = Some(500.3);
    h.wait_until("finished", |_| client.heard(|e| e.finished == [4]));
    assert!(h.state.layers.settled(500.3), "shown at its target again");
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

/// The toolkit's window system against the server (`cupertino-ui-wayland`'s `surface-layers`), at scale 1.5: a window
/// hands over a surface layer with its frame, which the server shows as a layer of the window's surface at the rect
/// the window gave it, drawn from the layer's own buffer at the window's scale; the fade the window then asks for runs
/// on the server's clock to its end while the window commits no new buffer, and the content blur it sends as samples
/// goes up and back down with it.
#[test]
fn a_toolkit_window_s_surface_layer_shows_at_its_rect_and_fades_on_the_server_clock_without_a_window_buffer(
) {
    use smithay::backend::renderer::utils::with_renderer_surface_state;
    use std::process::Stdio;
    let Some(mut h) = Harness::new(1.5, false) else {
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let report = home.path().join("report");
    let binary = common::fixtures::artifact("cupertino-ui-wayland", "example", "surface-layers");
    /// The window's process, ended with the test.
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _window = Kill(
        common::spawn::cleared(binary, home.path())
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("WAYLAND_DISPLAY", h.socket_path())
            .arg("--report")
            .arg(&report)
            .stdout(Stdio::null())
            .spawn()
            .expect("the window"),
    );
    let toplevel = |h: &Harness| {
        h.state
            .windows
            .iter()
            .find(|w| w.mapped)
            .map(|w| w.toplevel.wl_surface().clone())
    };
    let drawn = |h: &Harness| {
        toplevel(h).map_or_else(Vec::new, |s| {
            present(&s, h.state.now(), h.state.display_options().reduce_motion)
        })
    };
    h.wait_until("the window's layer and its frame", |h| {
        drawn(h).first().is_some_and(|l| l.content != (0.0, 0.0))
    });
    let layer = drawn(&h)[0].clone();
    assert_eq!(layer.content, (120.0, 60.0), "its frame, in points");
    let pixels = with_renderer_surface_state(&layer.surface, |s| s.buffer_size())
        .flatten()
        .expect("the layer's buffer");
    assert_eq!((pixels.w, pixels.h), (180, 90), "at the window's scale");
    assert_eq!(
        (layer.x, layer.y, layer.width, layer.height, layer.opacity),
        (40.0, 30.0, 120.0, 60.0, 1.0),
        "at the rect the window gave it"
    );
    let surface = toplevel(&h).unwrap();
    let commit = |s: &ServerSurface| with_renderer_surface_state(s, |state| state.current_commit());
    h.wait_until("the fade asked for", |_| {
        std::fs::read_to_string(&report).is_ok_and(|r| r.contains("faded 1"))
    });
    let r = std::fs::read_to_string(&report).unwrap();
    assert!(r.contains("surface_layers 1"), "{r}");
    assert!(r.contains("layer_frames 1"), "{r}");
    let first = commit(&surface);
    let (mut between, mut new_buffers, mut last) = (0, 0, first);
    let mut blurred = 0.0_f64;
    h.wait_until("the layer faded out", |h| {
        let Some((shown, blur)) = drawn(h).first().map(|l| (l.opacity, l.content_blur)) else {
            return false;
        };
        blurred = blurred.max(blur);
        if shown > 0.0 && shown < 1.0 {
            between += 1;
        }
        let now = commit(&surface);
        if now != last {
            new_buffers += 1;
            last = now;
        }
        shown == 0.0
    });
    eprintln!(
        "trace: fade: {between} server steps part way, {new_buffers} new buffers of the window, blur up to {blurred:.2}"
    );
    assert!(between > 0, "the fade ran on the server");
    assert_eq!(
        new_buffers, 0,
        "the window drew no frame while its layer faded"
    );
    assert!(
        blurred > 2.0 && blurred <= 8.0,
        "the blur went up through its samples: {blurred}"
    );
    let end = drawn(&h)[0].content_blur;
    assert!(end < 0.5, "and back down towards its last: {end}");
}

/// The same window on the GPU (`--renderer`, `CUPERTINO_RENDERER=gpu`): the layer's frames are drawn by the toolkit's
/// GPU renderer on the presenter's device and shown from dmabufs of the layer's own surface, at 1, 1.5 and 2: its
/// buffer is drawn at the window's scale and shown at the rect the window gave it, in points, as from shared memory.
/// Skips (or with `CUPERTINO_GPU_REQUIRED=1` fails) without a render node.
#[test]
fn a_toolkit_window_s_surface_layer_drawn_on_the_gpu_shows_from_dmabufs_of_its_own() {
    for scale in [1.0, 1.5, 2.0] {
        if !gpu_layer_at(scale) {
            return;
        }
    }
}

/// [`a_toolkit_window_s_surface_layer_drawn_on_the_gpu_shows_from_dmabufs_of_its_own`] at `scale`; false where it
/// skips.
fn gpu_layer_at(scale: f64) -> bool {
    use smithay::backend::renderer::utils::{
        with_renderer_surface_state, RendererSurfaceStateUserData,
    };
    use smithay::wayland::compositor::with_states;
    use smithay::wayland::dmabuf::get_dmabuf;
    use std::process::Stdio;
    let required = std::env::var("CUPERTINO_GPU_REQUIRED").as_deref() == Ok("1");
    let Some(mut h) = Harness::new(scale, false) else {
        assert!(!required, "CUPERTINO_GPU_REQUIRED=1: no EGL device");
        return false;
    };
    let renderer = h.state.backend.as_mut().unwrap().renderer();
    if cupertino_server::backend::render_node(renderer).is_none() {
        assert!(
            !required,
            "CUPERTINO_GPU_REQUIRED=1: the device has no render node"
        );
        eprintln!(
            "SKIPPED: the device has no render node; set CUPERTINO_GPU_REQUIRED=1 to fail instead"
        );
        return false;
    }
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    let report = home.path().join("report");
    let binary = common::fixtures::artifact("cupertino-ui-wayland", "example", "surface-layers");
    /// The window's process, ended with the test.
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut command = common::spawn::cleared(binary, home.path());
    command
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("WAYLAND_DISPLAY", h.socket_path())
        .env("CUPERTINO_RENDERER", "gpu")
        .args(["--renderer", "--report"])
        .arg(&report)
        .stdout(Stdio::null());
    for var in [
        "LIBGL_ALWAYS_SOFTWARE",
        "__EGL_VENDOR_LIBRARY_FILENAMES",
        "GALLIUM_DRIVER",
    ] {
        if let Some(value) = std::env::var_os(var) {
            command.env(var, value);
        }
    }
    let _window = Kill(command.spawn().expect("the window"));
    let drawn = |h: &Harness| {
        h.state
            .windows
            .iter()
            .find(|w| w.mapped)
            .map_or_else(Vec::new, |w| {
                present(w.toplevel.wl_surface(), h.state.now(), false)
            })
    };
    let on_dmabuf = |surface: &ServerSurface| {
        with_states(surface, |states| {
            let buffer = states
                .data_map
                .get::<RendererSurfaceStateUserData>()
                .and_then(|s| s.lock().unwrap().buffer().cloned());
            buffer.is_some_and(|b| get_dmabuf(&b).is_ok())
        })
    };
    let layer_on_dmabuf = |h: &Harness| drawn(h).first().is_some_and(|l| on_dmabuf(&l.surface));
    // the layer's first frame from a dmabuf, where it shows before its fade
    h.wait_until("the layer's first frame from a dmabuf", |h| {
        layer_on_dmabuf(h)
    });
    let layer = drawn(&h)[0].clone();
    // the buffers are made in size steps and cropped to the frame
    let source = with_renderer_surface_state(&layer.surface, |s| s.view())
        .flatten()
        .expect("the layer's buffer")
        .src
        .size;
    assert_eq!(
        (source.w, source.h),
        (120.0 * scale, 60.0 * scale),
        "at {scale}: drawn at the window's scale"
    );
    assert_eq!(
        layer.content,
        (120.0, 60.0),
        "at {scale}: its frame shown at the layer's size in points"
    );
    assert_eq!(
        (layer.x, layer.y, layer.width, layer.height),
        (40.0, 30.0, 120.0, 60.0),
        "at {scale}: at the rect the window gave it"
    );
    let mut from_dmabuf = 0;
    h.wait_until("the GPU frames handed over", |h| {
        if layer_on_dmabuf(h) {
            from_dmabuf += 1;
        }
        std::fs::read_to_string(&report).is_ok_and(|r| r.contains("faded 1"))
    });
    // the last frame handed over is shown once the server has taken it
    h.wait_until("the layer's frame from a dmabuf", |h| layer_on_dmabuf(h));
    assert_eq!(
        drawn(&h)[0].content,
        (120.0, 60.0),
        "at {scale}: every frame at the layer's size"
    );
    let r = std::fs::read_to_string(&report).unwrap();
    eprintln!("trace: at {scale}, {from_dmabuf} server steps showed the layer from a dmabuf; {r}");
    assert!(r.contains("surface_layers 1"), "{r}");
    assert!(
        r.contains(&format!("layer_gpu_frames {}", 8)),
        "every GPU frame was handed over: {r}"
    );
    true
}

/// Samples as the protocol carries them: 32-bit floats in the host's byte order.
fn sample_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_ne_bytes()).collect()
}

#[test]
fn a_property_moves_through_samples_in_straight_lines_on_the_server_clock_from_its_first_to_its_last(
) {
    let Some(mut h) = Harness::new(1.0, false) else {
        return;
    };
    let client = one_layer(&mut h);
    client.send(|c, _| {
        c.l(0).set_value(Property::X, 500.0);
        c.parent.commit();
    });
    h.wait_until("the layer placed", |h| x_of(h) == 500.0);
    // three steps over 0.3 s, which no single curve draws: out, back and further out
    h.state.fixed_time = Some(50.0);
    client.send(|c, _| {
        c.l(0).animate_samples(
            21,
            Property::X,
            300,
            sample_bytes(&[200.0, 300.0, 250.0, 400.0]),
        );
        c.parent.commit();
    });
    h.wait_until("the samples' animation", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, 50.0))
    });
    assert_eq!(x_of(&h), 200.0, "it starts at its first sample");
    for (t, x) in [(50.05, 250.0), (50.15, 275.0), (50.25, 325.0)] {
        h.state.fixed_time = Some(t);
        assert!((x_of(&h) - x).abs() < 1e-3, "{t}: {} against {x}", x_of(&h));
    }
    h.state.fixed_time = Some(50.4);
    h.wait_until("finished", |_| client.heard(|e| e.finished == [21]));
    assert_eq!(x_of(&h), 400.0, "it rests at its last sample");
    // a curve after it, replacing it part way, starts from what the samples show
    h.state.fixed_time = Some(60.0);
    client.send(|c, _| {
        c.l(0)
            .animate_samples(22, Property::X, 300, sample_bytes(&[400.0, 100.0]));
        c.parent.commit();
    });
    h.wait_until("the second", |h| {
        h.parent()
            .is_some_and(|p| !cupertino_server::layers_v1::settled(&p, 60.0))
    });
    h.state.fixed_time = Some(60.15);
    client.send(|c, _| {
        c.l(0)
            .animate_timed(23, Property::X, 0.0, 1000, 0.0, 0.0, 1.0, 1.0);
        c.parent.commit();
    });
    h.wait_until("replaced", |_| !client.heard(|e| e.replaced.is_empty()));
    let (serial, value, velocity) = client.heard(|e| e.replaced[0]);
    assert_eq!(serial, 22);
    assert!((value - 250.0).abs() < 0.01, "{value}");
    assert!(
        (velocity + 1000.0).abs() < 0.5,
        "the samples' slope: {velocity}"
    );
    // under reduce motion a spatial property goes to its last sample at once
    h.state.accessibility.settings.display.reduce_motion = true;
    client.send(|c, _| {
        c.l(0)
            .animate_samples(24, Property::X, 300, sample_bytes(&[10.0, 90.0, 30.0]));
        c.parent.commit();
    });
    h.wait_until("at its last sample", |h| x_of(h) == 30.0);
    // as many samples as one request carries go through
    let most = sample_bytes(&(0..1000).map(|i| i as f32).collect::<Vec<_>>());
    client.send(move |c, _| {
        c.l(0).animate_samples(27, Property::X, 300, most.clone());
        c.parent.commit();
    });
    h.wait_until("at the last of the most samples", |h| x_of(h) == 999.0);
    // one sample is not a motion, and an opacity past 1 is out of its range
    client.send(|c, _| {
        c.l(0)
            .animate_samples(25, Property::Opacity, 300, sample_bytes(&[0.5]))
    });
    h.wait_until("the error", |_| client.heard(|e| e.error.is_some()));
    assert_eq!(
        client.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidCurve as u32
    );
    let other = one_layer(&mut h);
    other.send(|c, _| {
        c.l(0)
            .animate_samples(26, Property::Opacity, 300, sample_bytes(&[0.5, 1.5]))
    });
    h.wait_until("the error", |_| other.heard(|e| e.error.is_some()));
    assert_eq!(
        other.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidValue as u32
    );
    // nor is one more than the most
    let past = one_layer(&mut h);
    let more = sample_bytes(&(0..1001).map(|i| i as f32).collect::<Vec<_>>());
    past.send(move |c, _| c.l(0).animate_samples(28, Property::X, 300, more.clone()));
    h.wait_until("the error", |_| past.heard(|e| e.error.is_some()));
    assert_eq!(
        past.heard(|e| e.error.clone()).unwrap().1,
        cupertino_layer_v1::Error::InvalidCurve as u32
    );
}
