//! A popup of an application window with a material backdrop is drawn over that material (a
//! context menu's backdrop), against a headless compositor. The client opens its window and popup
//! the way the shell processes do; a window's own material is drawn while it minimizes. Tests skip (pass) when no
//! EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::{
    CupertinoAppEffectsV1, TitlebarStyle,
};
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::{
    CupertinoBackdropV1, GlassStyle,
};
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::events::BTN_LEFT;
use cupertino_server::Cupertino;
use cupertino_theme::recipe::WINDOW_BACKGROUND_MATERIAL;
use cupertino_wm::{Command, Rect, WindowId};
use smithay::backend::input::ButtonState;
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};

const OUTPUT: (i32, i32) = (1000, 700);
const SIZE: (u32, u32) = (500, 300);
/// The popup in the window surface's coordinates.
const POPUP: (f64, f64, f64, f64) = (100.0, 100.0, 160.0, 80.0);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        Self::with_config(Config::default())
    }

    fn with_config(config: Config) -> Option<Harness> {
        Self::with_scale(config, 1.0)
    }

    /// A display of `OUTPUT` points at `scale` device pixels a point.
    fn with_scale(config: Config, scale: f64) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        let pixels = (
            (f64::from(OUTPUT.0) * scale).round() as i32,
            (f64::from(OUTPUT.1) * scale).round() as i32,
        );
        match headless::init(&mut state, pixels, scale) {
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

    fn step(&mut self, n: usize) {
        for _ in 0..n {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
        }
    }

    fn screenshot(&mut self) -> image::RgbaImage {
        let path = std::env::temp_dir().join(format!(
            "cupertino-test-{}-window-popup.png",
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
    shm: wl_shm::WlShm,
    window: wl_surface::WlSurface,
    window_xdg: xdg_surface::XdgSurface,
    /// The size of the latest toplevel configure, `SIZE` while the server leaves the size to the client.
    size: (i32, i32),
    /// The byte value of every channel of the window's buffer.
    fill: u8,
    popup: Option<(wl_surface::WlSurface, xdg_surface::XdgSurface)>,
    press_serial: Option<u32>,
}

impl Window {
    /// Attach a buffer of one byte value (white or transparent) and commit it.
    fn draw(
        &self,
        surface: &wl_surface::WlSurface,
        (w, h): (i32, i32),
        value: u8,
        qh: &QueueHandle<Self>,
    ) {
        let fd =
            rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let mut file = std::fs::File::from(fd);
        file.write_all(&vec![value; (w * h * 4) as usize]).unwrap();
        let pool = self.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
    }
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
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    xdg_positioner::XdgPositioner,
    xdg_popup::XdgPopup,
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    CupertinoAppEffectsV1,
    CupertinoBackdropV1
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Window {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            if width > 0 && height > 0 {
                state.size = (width, height);
            }
        }
    }
}

/// The window is drawn in its fill and the popup transparent on every configure.
impl Dispatch<xdg_surface::XdgSurface, ()> for Window {
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
        xdg.ack_configure(serial);
        if *xdg == state.window_xdg {
            state.draw(&state.window, state.size, state.fill, qh);
        } else if let Some((surface, _)) = state.popup.as_ref().filter(|(_, p)| p == xdg) {
            let (_, _, w, h) = POPUP;
            state.draw(surface, (w as i32, h as i32), 0, qh);
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Window {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Window {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            serial,
            button: BTN_LEFT,
            state: WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        {
            state.press_serial.get_or_insert(serial);
        }
    }
}

/// A client with one unified window. A white one opens a popup with a `Menu` material backdrop once it has had a
/// button press (the popup needs an input serial); a `Sidebar` one (`sidebar` is its backdrop's corner radius) is
/// transparent over a backdrop of that material covering it, as the Files sidebar is.
struct App {
    stop: Arc<AtomicBool>,
}

impl App {
    fn spawn(socket: PathBuf, sidebar: Option<f64>) -> App {
        Self::spawn_with(socket, sidebar, false, None)
    }

    /// The same app whose sidebar backdrop is sidebar glass rather than the material when `glass`. With `split` (a width
    /// in points) the sidebar covers only that much of the window's left and the content's window background backdrop
    /// begins where it ends, as in a sidebar window.
    fn spawn_with(socket: PathBuf, sidebar: Option<f64>, glass: bool, split: Option<i32>) -> App {
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
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=5, ()).unwrap();
            let decorations: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let effects: CupertinoAppEffectsV1 = globals
                .bind(&qh, if glass { 18..=18 } else { 1..=1 }, ())
                .unwrap();
            let window = compositor.create_surface(&qh, ());
            let window_xdg = wm_base.get_xdg_surface(&window, &qh, ());
            let toplevel = window_xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.popup".into());
            toplevel.set_title("Window".into());
            toplevel.set_min_size(100, 100);
            let decoration = decorations.get_toplevel_decoration(&toplevel, &qh, ());
            decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ServerSide);
            effects.set_titlebar_style(&window, TitlebarStyle::Unified);
            let mut backdrop = sidebar.map(|radius| {
                let b: CupertinoBackdropV1 = effects.get_backdrop(&window, &qh, ());
                if glass {
                    b.set_glass(GlassStyle::Sidebar, 0, 0.0);
                } else {
                    b.set_material("Sidebar".into());
                }
                b.set_shape(0, 0, split.unwrap_or(SIZE.0 as i32), SIZE.1 as i32, radius);
                b
            });
            let content = split.map(|w| {
                let b: CupertinoBackdropV1 = effects.get_backdrop(&window, &qh, ());
                b.set_material(WINDOW_BACKGROUND_MATERIAL.into());
                b.set_shape(w, 0, SIZE.0 as i32 - w, SIZE.1 as i32, 0.0);
                b
            });
            let sidebar = sidebar.is_some();
            window.commit();
            let mut client = Window {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                window,
                window_xdg: window_xdg.clone(),
                size: (SIZE.0 as i32, SIZE.1 as i32),
                fill: if sidebar { 0 } else { 255 },
                popup: None,
                press_serial: None,
            };
            while !flag.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                if let (Some(serial), None, false) = (client.press_serial, &client.popup, sidebar) {
                    let (x, y, w, h) = POPUP;
                    let (x, y, w, h) = (x as i32, y as i32, w as i32, h as i32);
                    let positioner = wm_base.create_positioner(&qh, ());
                    positioner.set_size(w, h);
                    positioner.set_anchor_rect(x, y, w, h);
                    positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
                    positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
                    let surface = compositor.create_surface(&qh, ());
                    let popup_xdg = wm_base.get_xdg_surface(&surface, &qh, ());
                    let popup = popup_xdg.get_popup(Some(&window_xdg), &positioner, &qh, ());
                    positioner.destroy();
                    popup.grab(&seat, serial);
                    let b: CupertinoBackdropV1 = effects.get_backdrop(&surface, &qh, ());
                    b.set_material("Menu".into());
                    b.set_shape(0, 0, w, h, 10.0);
                    surface.commit();
                    client.popup = Some((surface, popup_xdg));
                    backdrop = Some(b);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            for b in backdrop.into_iter().chain(content) {
                b.destroy();
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
fn a_window_popup_draws_its_material_over_the_window_and_only_there() {
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    let _app = App::spawn(h.socket_path(), None);
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let t = h.state.now() + 5.0;
    let time = t;
    common::set_time(&mut h.state, &mut h.event_loop, time);
    h.state.tick();
    let id = h
        .state
        .windows
        .iter()
        .find(|w| w.mapped)
        .expect("a mapped window")
        .id;
    let frame = h.state.wm.window(id).unwrap().geometry();
    // a press inside the window gives the client the serial its popup needs
    h.state
        .pointer_moved_to((frame.x + 400.0, frame.y + 200.0).into(), 0, None);
    h.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
    h.state.pointer_button(BTN_LEFT, ButtonState::Released, 0);
    for _ in 0..200 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    }
    let shot = h.screenshot();
    let (px, py, pw, ph) = POPUP;
    let at = |x: f64, y: f64| shot.get_pixel((frame.x + x) as u32, (frame.y + y) as u32).0;
    let inside = at(px + pw / 2.0, py + ph / 2.0);
    // beyond the reach of the popup's shadow (σ 17 reaches 48 points)
    let outside = at(px - 60.0, py + ph / 2.0);
    assert_eq!(
        outside[..3],
        [255, 255, 255],
        "the window is white outside the popup"
    );
    assert!(
        inside[..3].iter().all(|c| *c < 250),
        "the popup's material darkens the white window under it: {inside:?}"
    );
}

#[test]
fn a_minimizing_window_keeps_its_material_where_it_is_otherwise_transparent() {
    let mut config = Config::default();
    config.effects.minimize = Some(cupertino_effects::MinimizeKind::Scale);
    // the translucent fill that stands in for a material without the material pass
    let fill: [f64; 3] = if config.is_dark() {
        [0.12, 0.12, 0.14]
    } else {
        [0.96, 0.96, 0.98]
    };
    let Some(mut h) = Harness::with_config(config) else {
        return;
    };
    h.state.cursor_status = CursorImageStatus::Hidden;
    let _app = App::spawn(h.socket_path(), Some(10.0));
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let t0 = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, t0);
    h.state.tick();
    h.step(20);
    let id = h.state.wm.key_window().expect("a key window");
    let frame = h.state.wm.window(id).unwrap().geometry();
    let centre = (
        (frame.x + frame.w / 2.0) as u32,
        (frame.y + frame.h / 2.0) as u32,
    );
    let at_rest = h.screenshot().get_pixel(centre.0, centre.1).0;
    h.state.command(Command::Minimize {
        window: id,
        target: Some(Rect::new(
            frame.x + frame.w / 2.0 - 24.0,
            f64::from(OUTPUT.1) - 60.0,
            48.0,
            48.0,
        )),
    });
    common::set_time(&mut h.state, &mut h.event_loop, t0 + 0.02);
    h.state.tick();
    h.step(3);
    let minimizing = h.screenshot().get_pixel(centre.0, centre.1).0;
    common::set_time(&mut h.state, &mut h.event_loop, t0 + 3.0);
    h.state.tick();
    h.step(5);
    assert!(h.state.wm.window(id).unwrap().state().is_minimized());
    let behind = h.screenshot().get_pixel(centre.0, centre.1).0;
    let differs =
        |a: [u8; 4], b: [u8; 4]| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 24);
    assert!(
        differs(at_rest, behind),
        "at rest the material covers the desktop: {at_rest:?} over {behind:?}"
    );
    let stand_in: Vec<u8> = fill
        .iter()
        .zip(&behind[..3])
        .map(|(f, b)| (f * 255.0 * 0.78 + f64::from(*b) * 0.22).round() as u8)
        .collect();
    assert!(
        minimizing[..3]
            .iter()
            .zip(&stand_in)
            .all(|(x, y)| x.abs_diff(*y) <= 8),
        "while minimizing the window draws its material's fill over the desktop: {minimizing:?}, expected {stand_in:?} over {behind:?}"
    );
}

#[test]
fn a_square_backdrop_over_the_whole_window_follows_the_window_s_rounded_corners() {
    square_backdrop_follows_the_rounded_corners(false);
}

#[test]
fn a_square_glass_backdrop_over_the_whole_window_follows_the_window_s_rounded_corners() {
    square_backdrop_follows_the_rounded_corners(true);
}

/// A square sidebar backdrop (`glass` or the material) over the whole window is cut to the window's rounded corners
/// and drawn up to its edges past them.
fn square_backdrop_follows_the_rounded_corners(glass: bool) {
    for scale in [1.0, 1.5] {
        let Some(mut h) = Harness::with_scale(Config::default(), scale) else {
            return;
        };
        h.state.cursor_status = CursorImageStatus::Hidden;
        let _app = App::spawn_with(h.socket_path(), Some(0.0), glass, None);
        h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
        let t = h.state.now() + 5.0;
        common::set_time(&mut h.state, &mut h.event_loop, t);
        h.state.tick();
        h.step(20);
        let id = h.state.wm.key_window().expect("a key window");
        let frame = h.state.wm.window(id).unwrap().geometry();
        let shot = h.screenshot();
        // in device pixels: the window's edges and its corner radius
        let at = |x: f64, y: f64| shot.get_pixel(x.round() as u32, y.round() as u32).0;
        let (left, right, bottom) = (
            frame.x * scale,
            (frame.x + frame.w) * scale - 1.0,
            (frame.y + frame.h) * scale - 1.0,
        );
        let radius = Config::default().window.corner_radius * scale;
        let differs = |a: [u8; 4], b: [u8; 4]| {
            a[..3]
                .iter()
                .zip(&b[..3])
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap()
        };
        // (direction into the window along x, the corner's x)
        for (inward, x) in [(1.0, left), (-1.0, right)] {
            let beside = at(x - inward, bottom + 1.0);
            let inside = at(x + inward * 30.0 * scale, bottom - 2.0);
            assert!(
                differs(inside, beside) > 24,
                "at {scale}x the material covers the window: {inside:?} over {beside:?}"
            );
            // the corner pixel and one a tenth of the radius in on the diagonal lie past the rounded shape and the fine
            // outline just outside it
            for d in [0.0, (radius * 0.1).floor()] {
                let corner = at(x + inward * d, bottom - d);
                assert!(
                    differs(corner, beside) <= 12,
                    "at {scale}x past the rounded corner the desktop shows, as just outside the window: \
                     {corner:?} {d} px in, beside {beside:?}"
                );
            }
            // a radius in along the bottom edge the material is drawn again
            let edge = at(x + inward * (radius + 2.0), bottom - 1.0);
            assert!(
                differs(edge, beside) > 24,
                "at {scale}x the material reaches the bottom edge past the corner: {edge:?} over {beside:?}"
            );
        }
    }
}

#[test]
fn a_window_s_sidebar_glass_meets_the_content_s_backdrop_with_no_line_between_them() {
    // a sidebar 161 points wide ends half way across a device pixel at 1.5, and on whole pixels at 1 and 2
    for (scale, width) in [(1.0, 161), (1.5, 160), (1.5, 161), (2.0, 161)] {
        let Some(mut h) = Harness::with_scale(Config::default(), scale) else {
            return;
        };
        h.state.cursor_status = CursorImageStatus::Hidden;
        let _app = App::spawn_with(h.socket_path(), Some(0.0), true, Some(width));
        h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
        let t = h.state.now() + 5.0;
        common::set_time(&mut h.state, &mut h.event_loop, t);
        h.state.tick();
        h.step(20);
        let id = h.state.wm.key_window().expect("a key window");
        let frame = h.state.wm.window(id).unwrap().geometry();
        let shot = h.screenshot();
        let edge = ((frame.x + f64::from(width)) * scale).round() as u32;
        let luma = |x: u32, y: u32| {
            let p = shot.get_pixel(x, y).0;
            0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
        };
        // down the middle of the window, the pixels across the boundary: the glass's last two and the content's first
        // two lie between the glass a little way in and the content a little way in
        for y in [0.3, 0.5, 0.7] {
            let y = ((frame.y + frame.h * y) * scale) as u32;
            let row: Vec<f64> = (edge - 3..edge + 3).map(|x| luma(x, y)).collect();
            let (glass, content) = (row[0], row[5]);
            let (lo, hi) = (glass.min(content), glass.max(content));
            eprintln!("{scale}x {width}: {row:.1?}");
            for (i, l) in row[1..5].iter().enumerate() {
                assert!(
                    *l >= lo - 6.0 && *l <= hi + 6.0,
                    "at {scale}x with a sidebar {width} points wide the pixel {} from the boundary is {l:.1}, \
                     outside the glass's {glass:.1} and the content's {content:.1}: {row:.1?}",
                    i as i32 - 2
                );
            }
        }
    }
}

/// The white window and the sidebar window over it, opened in that order, the sidebar window at `frame` (with its
/// content's window background backdrop past `split` points); and the sidebar window's id.
fn sidebar_over_a_window(h: &mut Harness, frame: Rect, split: Option<i32>) -> (WindowId, [App; 2]) {
    let white = App::spawn(h.socket_path(), None);
    h.wait_until("the white window to map", |s| {
        s.windows.iter().any(|w| w.mapped)
    });
    let sidebar = App::spawn_with(h.socket_path(), Some(10.0), false, split);
    h.wait_until("the sidebar window to map", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
    });
    let ids: Vec<WindowId> = h
        .state
        .windows
        .iter()
        .filter(|w| w.mapped)
        .map(|w| w.id)
        .collect();
    let output = h.state.outputs[0].id;
    for (id, frame) in [
        (ids[0], Rect::new(60.0, 60.0, 500.0, 300.0)),
        (ids[1], frame),
    ] {
        h.state.command(Command::MoveWindowToOutput {
            window: id,
            output,
            frame,
        });
    }
    let t = h.state.now() + 5.0;
    common::set_time(&mut h.state, &mut h.event_loop, t);
    h.state.tick();
    h.step(20);
    (ids[1], [white, sidebar])
}

fn reblurs(h: &Harness) -> u64 {
    let render = h.state.render.as_ref().expect("a renderer");
    render
        .material
        .as_ref()
        .map_or(0, |m| m.borrow().stats.reblurs)
}

#[test]
fn a_sidebar_dragged_over_a_window_is_blurred_again_in_part_and_looks_as_if_blurred_whole() {
    // the sidebar alone, and beside the content's window background
    for split in [None, Some(200)] {
        dragged_backdrops_match_whole_ones(split);
    }
}

fn dragged_backdrops_match_whole_ones(split: Option<i32>) {
    let end = Rect::new(348.0, 232.0, 500.0, 300.0);
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    let (id, _apps) = sidebar_over_a_window(&mut h, Rect::new(300.0, 200.0, 500.0, 300.0), split);
    let output = h.state.outputs[0].id;
    let before = reblurs(&h);
    // dragged a few points a frame, its backdrop reaching over the white window's edge and corner
    for i in 1..=8 {
        let frame = Rect::new(
            300.0 + 6.0 * f64::from(i),
            200.0 + 4.0 * f64::from(i),
            500.0,
            300.0,
        );
        h.state.command(Command::MoveWindowToOutput {
            window: id,
            output,
            frame,
        });
        h.step(3);
    }
    assert_eq!(h.state.wm.window(id).unwrap().geometry(), end);
    assert!(
        reblurs(&h) - before >= 8 * (1 + u64::from(split.is_some())),
        "each frame of the drag makes the backdrops again only where they changed or newly reach"
    );
    let dragged = h.screenshot();
    drop(_apps);
    drop(h);
    // the same windows put where the drag ended in one jump from where they open
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    let (_, _apps) = sidebar_over_a_window(&mut h, end, split);
    let whole = h.screenshot();
    let worst = dragged
        .pixels()
        .zip(whole.pixels())
        .map(|(a, b)| {
            a.0.iter()
                .zip(&b.0)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap()
        })
        .max()
        .unwrap();
    assert!(
        worst <= 1,
        "the dragged sidebar ({split:?}) differs from one blurred whole by {worst}"
    );
}
