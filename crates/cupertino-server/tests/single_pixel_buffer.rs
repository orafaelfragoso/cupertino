//! Surfaces drawn with one `wp_single_pixel_buffer_v1` buffer stretched by a viewport, against a headless compositor:
//! a window shows its colour across the window, the way a browser fills a solid surface; a layer surface over the whole
//! output, the way the shell shows a catcher of presses or a scrim, shows its colour (or nothing, transparent), takes
//! the pointer where its input region says, and follows the output as it turns and changes scale. Tests skip (pass)
//! when no EGL device is available.

use crate::common;

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_output, wl_region, wl_registry, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1;
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const OUTPUT: (i32, i32) = (1000, 700);
const SIZE: (i32, i32) = (400, 300);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
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

    fn screenshot(&mut self) -> image::RgbaImage {
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-single-pixel.png",
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

struct Window {
    surface: wl_surface::WlSurface,
    buffer: wl_buffer::WlBuffer,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Window {
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
        impl Dispatch<$t, ()> for Window {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    xdg_toplevel::XdgToplevel,
    wp_viewporter::WpViewporter,
    wp_viewport::WpViewport,
    WpSinglePixelBufferManagerV1
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Window {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for Window {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg.ack_configure(serial);
            state.surface.attach(Some(&state.buffer), 0, 0);
            state.surface.damage_buffer(0, 0, 1, 1);
            state.surface.commit();
        }
    }
}

/// A client whose only window is one opaque red pixel stretched to `SIZE`.
struct App {
    stop: Arc<AtomicBool>,
}

impl App {
    fn spawn(socket: PathBuf) -> App {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(
                UnixStream::connect(socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let (globals, mut queue) = registry_queue_init::<Window>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let viewporter: wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
            let pixels: WpSinglePixelBufferManagerV1 = globals
                .bind(&qh, 1..=1, ())
                .expect("wp_single_pixel_buffer_manager_v1 is advertised");
            let surface = compositor.create_surface(&qh, ());
            let viewport = viewporter.get_viewport(&surface, &qh, ());
            viewport.set_destination(SIZE.0, SIZE.1);
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.solid".into());
            toplevel.set_title("Solid".into());
            surface.commit();
            let mut client = Window {
                buffer: pixels.create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &qh, ()),
                surface,
            };
            while !flag.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        App { stop }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_single_pixel_buffer_fills_its_viewport_with_its_colour() {
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    let _app = App::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let id = h
        .state
        .windows
        .iter()
        .find(|w| w.mapped)
        .expect("a mapped window")
        .id;
    let frame = h.state.wm.window(id).unwrap().geometry();
    assert_eq!(
        (frame.w, frame.h),
        (f64::from(SIZE.0), f64::from(SIZE.1)),
        "the window takes the viewport's size, not the buffer's"
    );
    for _ in 0..100 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    }
    let shot = h.screenshot();
    for (x, y) in [(0.5, 0.5), (0.1, 0.9), (0.9, 0.1)] {
        let px = shot
            .get_pixel(
                (frame.x + frame.w * x) as u32,
                (frame.y + frame.h * y) as u32,
            )
            .0;
        assert_eq!(px[..3], [255, 0, 0], "red at ({x}, {y}) of the window");
    }
}

/// What a layer client of one stretched pixel heard from the compositor.
#[derive(Default)]
struct Heard {
    /// `wl_surface.enter` arrived.
    entered: AtomicBool,
    /// Frame callbacks answered.
    frames: AtomicU32,
    /// The latest `wl_surface.preferred_buffer_scale`.
    preferred_scale: AtomicI32,
    /// The size of the latest configure, in points.
    configured: Mutex<(u32, u32)>,
}

/// A layer surface over the whole output whose only buffer is one pixel of `color` (premultiplied, over the whole
/// `u32` range), stretched by its viewport to each size the compositor configures, as a shell client shows a catcher
/// of presses (transparent) or a scrim (translucent).
struct Layered {
    surface: wl_surface::WlSurface,
    viewport: wp_viewport::WpViewport,
    buffer: wl_buffer::WlBuffer,
    heard: Arc<Heard>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Layered {
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
macro_rules! layered_ignores {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Layered {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
layered_ignores!(
    wl_compositor::WlCompositor,
    wl_output::WlOutput,
    wl_buffer::WlBuffer,
    wl_region::WlRegion,
    wp_viewporter::WpViewporter,
    wp_viewport::WpViewport,
    WpSinglePixelBufferManagerV1,
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);

impl Dispatch<wl_surface::WlSurface, ()> for Layered {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { .. } => state.heard.entered.store(true, Ordering::SeqCst),
            wl_surface::Event::PreferredBufferScale { factor } => {
                state.heard.preferred_scale.store(factor, Ordering::SeqCst)
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for Layered {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.heard.frames.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Layered {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            layer.ack_configure(serial);
            *state.heard.configured.lock().unwrap() = (width, height);
            state.viewport.set_destination(width as i32, height as i32);
            state.surface.attach(Some(&state.buffer), 0, 0);
            state.surface.damage_buffer(0, 0, 1, 1);
            state.surface.frame(qh, ());
            state.surface.commit();
        }
    }
}

/// Map a [`Layered`] surface named `namespace` on `layer`, taking input only in `input` when it is some, and keep its
/// connection served until the compositor goes away.
fn spawn_layered(
    socket: PathBuf,
    namespace: &'static str,
    layer: zwlr_layer_shell_v1::Layer,
    color: [u32; 4],
    input: Option<[i32; 4]>,
) -> Arc<Heard> {
    let heard = Arc::new(Heard::default());
    let shared = Arc::clone(&heard);
    std::thread::spawn(move || {
        let conn = Connection::from_socket(
            UnixStream::connect(socket).expect("connecting to the compositor"),
        )
        .unwrap();
        let (globals, mut queue) = registry_queue_init::<Layered>(&conn).unwrap();
        let qh = queue.handle();
        // version 6 tells the surface its preferred buffer scale
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 6..=6, ()).unwrap();
        let viewporter: wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
        let pixels: WpSinglePixelBufferManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
        let shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).unwrap();
        // bound, so the surface can be told it entered the output
        let _output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
        let surface = compositor.create_surface(&qh, ());
        let viewport = viewporter.get_viewport(&surface, &qh, ());
        let layer_surface =
            shell.get_layer_surface(&surface, None, layer, namespace.into(), &qh, ());
        layer_surface.set_size(0, 0);
        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Right
                | zwlr_layer_surface_v1::Anchor::Bottom
                | zwlr_layer_surface_v1::Anchor::Left,
        );
        layer_surface.set_exclusive_zone(-1);
        if let Some([x, y, w, h]) = input {
            let region = compositor.create_region(&qh, ());
            region.add(x, y, w, h);
            surface.set_input_region(Some(&region));
        }
        surface.commit();
        let [r, g, b, a] = color;
        let mut client = Layered {
            buffer: pixels.create_u32_rgba_buffer(r, g, b, a, &qh, ()),
            surface,
            viewport,
            heard: shared,
        };
        // blocks until the compositor sends something; ends when it goes away
        while queue.blocking_dispatch(&mut client).is_ok() {}
        drop(layer_surface);
    });
    heard
}

impl Harness {
    /// The namespace of the layer surface the pointer would focus at `(x, y)`.
    fn namespace_under(&self, x: f64, y: f64) -> Option<String> {
        let (surface, _) = self.state.surface_under((x, y).into())?;
        let map = layer_map_for_output(&self.state.outputs[0].output);
        map.layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
            .map(|l| l.namespace().to_string())
    }

    /// The size, in points, of the layer surface named `namespace` as mapped, and the output's.
    fn layer_and_output(&self, namespace: &str) -> Option<((i32, i32), (i32, i32))> {
        let map = layer_map_for_output(&self.state.outputs[0].output);
        let output = map.non_exclusive_zone().size;
        let found = map
            .layers()
            .find(|l| l.namespace() == namespace)
            .map(|l| l.bbox().size)
            .filter(|size| size.w > 0)
            .map(|size| ((size.w, size.h), (output.w, output.h)));
        found
    }

    /// Turn the output to `transform` at `scale`, as the display settings do.
    fn set_output(&mut self, scale: f64, transform: Transform) {
        let head = &self.state.displays.heads[0];
        let (id, mut wanted) = (head.id, head.state);
        wanted.scale = scale;
        wanted.transform = transform;
        self.state
            .configure_heads(&[(id, wanted)], false)
            .expect("the display takes the scale and the turn");
        self.state.heads_changed();
    }
}

/// The pixels at the corners and the middle of a 1000 x 700 screenshot.
fn samples(shot: &image::RgbaImage) -> Vec<[u8; 4]> {
    [(2, 2), (500, 350), (997, 2), (2, 697), (997, 697)]
        .into_iter()
        .map(|(x, y)| shot.get_pixel(x, y).0)
        .collect()
}

/// A transparent layer surface over the whole output whose buffer is one stretched pixel, as a panel's catcher of
/// presses around it: it shows nothing, takes the pointer everywhere on the output, is told the output it is on and
/// gets its frame callbacks; turned and set to a fractional scale, the output gives it a new size the pixel is
/// stretched to again, and the buffer scale it prefers.
#[test]
fn a_transparent_pixel_over_the_output_shows_nothing_and_takes_the_pointer_everywhere() {
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    for _ in 0..20 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    }
    let before = samples(&h.screenshot());
    let heard = spawn_layered(
        h.socket_path(),
        "catcher",
        zwlr_layer_shell_v1::Layer::Top,
        [0; 4],
        None,
    );
    h.wait_until("the catcher over the output", |s| {
        let map = layer_map_for_output(&s.outputs[0].output);
        let mapped = map
            .layers()
            .any(|l| l.namespace() == "catcher" && l.bbox().size.w > 0);
        mapped
    });
    let watch = Arc::clone(&heard);
    h.wait_until("the catcher's output and frame callback", move |_| {
        watch.entered.load(Ordering::SeqCst) && watch.frames.load(Ordering::SeqCst) > 0
    });
    assert_eq!(
        h.layer_and_output("catcher"),
        Some(((OUTPUT.0, OUTPUT.1), (OUTPUT.0, OUTPUT.1))),
        "the viewport's size, not the pixel's"
    );
    assert_eq!(samples(&h.screenshot()), before, "nothing shows");
    for (x, y) in [
        (1.0, 1.0),
        (500.0, 350.0),
        (998.5, 1.0),
        (1.0, 698.5),
        (998.5, 698.5),
    ] {
        assert_eq!(
            h.namespace_under(x, y).as_deref(),
            Some("catcher"),
            "({x}, {y}) is the catcher's"
        );
    }

    // the output turns a quarter at 1.5 pixels a point: the catcher is configured to the new extent and covers it
    h.set_output(1.5, Transform::_90);
    let watch = Arc::clone(&heard);
    h.wait_until("the catcher over the turned output", move |s| {
        let map = layer_map_for_output(&s.outputs[0].output);
        let zone = map.non_exclusive_zone().size;
        zone.h > zone.w
            && map
                .layers()
                .any(|l| l.namespace() == "catcher" && l.bbox().size == zone)
            && watch.preferred_scale.load(Ordering::SeqCst) == 2
    });
    let (layer, output) = h.layer_and_output("catcher").unwrap();
    assert_eq!(layer, output);
    assert_eq!(
        *heard.configured.lock().unwrap(),
        (output.0 as u32, output.1 as u32)
    );
    let (w, h_) = (f64::from(output.0), f64::from(output.1));
    for (x, y) in [(0.5, 0.5), (w / 2.0, h_ / 2.0), (w - 0.5, h_ - 0.5)] {
        assert_eq!(
            h.namespace_under(x, y).as_deref(),
            Some("catcher"),
            "({x}, {y}) of the turned output"
        );
    }
}

/// A translucent black pixel stretched over the output on the overlay layer, as a scrim, darkens everything under it
/// by its alpha; with an empty input region the pointer goes through it.
#[test]
fn a_translucent_pixel_over_the_output_dims_it_and_without_input_lets_the_pointer_through() {
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    for _ in 0..20 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    }
    let before = samples(&h.screenshot());
    let half = u32::MAX / 2;
    let heard = spawn_layered(
        h.socket_path(),
        "scrim",
        zwlr_layer_shell_v1::Layer::Overlay,
        [0, 0, 0, half],
        Some([0, 0, 0, 0]),
    );
    let watch = Arc::clone(&heard);
    h.wait_until("the scrim over the output", move |s| {
        let map = layer_map_for_output(&s.outputs[0].output);
        watch.frames.load(Ordering::SeqCst) > 0
            && map
                .layers()
                .any(|l| l.namespace() == "scrim" && l.bbox().size.w == OUTPUT.0)
    });
    let after = samples(&h.screenshot());
    for (b, a) in before.iter().zip(&after) {
        for c in 0..3 {
            let dimmed = i32::from(b[c]) / 2;
            assert!(
                (i32::from(a[c]) - dimmed).abs() <= 2,
                "{a:?} is {b:?} at half its brightness"
            );
        }
    }
    assert_eq!(h.namespace_under(500.0, 350.0), None, "no input region");
}
