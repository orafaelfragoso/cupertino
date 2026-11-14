//! A display at scale 1.5 drawn at 2 and shrunk to its pixels (`fractional_scale_mode` `supersample`, the default),
//! against a headless compositor and a client that draws at the scale it is asked for, with a toolbar ending on a
//! point that falls half way across a pixel at 1.5: the client is asked for 2 and its buffer of twice its points
//! shows at its size in points; the display shows the frame drawn at 2 shrunk with the area filter, at the display's
//! pixels, with no row of the wallpaper under the toolbar's edge and the pointer drawn at the display's own scale.
//! Under `direct` the client is asked for 1.5 and a row it declares opaque but leaves clear shows black, never the
//! wallpaper. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_defaults::compositor::{FractionalScaleMode, FractionalScaleModes};
use cupertino_raster_math::resample::shrink;
use cupertino_render_api::{Completion as _, RenderStatus, Renderer as _};
use cupertino_scene::SceneLayer;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::render::gles::{Gles, GlesResources, GlesTarget};
use cupertino_server::render::screenshot::render_to_image;
use cupertino_server::Cupertino;
use image::RgbaImage;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::backend::renderer::{Color32F, Offscreen};
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::fractional_scale::with_fractional_scale;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// The display: 1200 x 900 pixels at 1.5 are 800 x 600 points, drawn at 2 into 1600 x 1200.
const PANEL: (i32, i32) = (1200, 900);
const SCALE: f64 = 1.5;
/// The window, in points.
const SIZE: (i32, i32) = (400, 300);
/// Where the client's toolbar ends, in points: half way across a pixel at 1.5, on a whole pixel at 2.
const TOOLBAR: f64 = 87.0;
const TOOLBAR_RGB: [u8; 3] = [0xe0, 0xe0, 0xe0];
const CONTENT_RGB: [u8; 3] = [0x30, 0x40, 0x50];
/// The desktop's solid colour, which nothing in the window may show.
const WALLPAPER: [u8; 3] = [0, 255, 0];

fn harness(mode: FractionalScaleMode) -> Option<(Cupertino, EventLoop<'static, Cupertino>)> {
    let event_loop: EventLoop<'static, Cupertino> =
        EventLoop::try_new().expect("creating the event loop");
    let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
    let config = Config {
        fractional_scale_mode: FractionalScaleModes {
            others: Some(mode),
            displays: Default::default(),
        },
        wallpaper_color: Some(WALLPAPER),
        ..Config::default()
    };
    let mut state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        config,
        true,
        &cupertino_assets::Assets::builtin(),
    )
    .expect("creating the compositor");
    match headless::init(&mut state, PANEL, SCALE) {
        Ok(()) => Some((state, event_loop)),
        Err(e) => {
            common::egl_unavailable(format!("{e:#}"));
            None
        }
    }
}

fn wait_until(
    state: &mut Cupertino,
    event_loop: &mut EventLoop<'static, Cupertino>,
    what: &str,
    mut cond: impl FnMut(&Cupertino) -> bool,
) {
    let end = Instant::now() + common::HANG_GUARD;
    while Instant::now() < end {
        headless::step(state, event_loop, Duration::from_millis(5)).unwrap();
        if cond(state) {
            return;
        }
    }
    panic!("timed out waiting for {what}");
}

fn socket_path(state: &Cupertino) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    PathBuf::from(dir).join(state.socket_name.as_ref().unwrap())
}

/// What the client heard and drew.
#[derive(Default)]
struct Heard {
    /// The latest preferred scale, in 120ths.
    preferred: AtomicU32,
    /// The latest `wl_surface.preferred_buffer_scale`.
    buffer_scale: AtomicI32,
    /// The scale (120ths) of the last buffer drawn.
    drawn: AtomicU32,
}

/// A window that draws at the preferred scale it is told, through a viewport: a toolbar to [`TOOLBAR`] points, then
/// content, all declared opaque; with `seam`, the device row the toolbar's end falls in is left clear, as a client
/// that lays its parts out in rounded pixels can leave it.
struct Pane {
    surface: wl_surface::WlSurface,
    viewport: wp_viewport::WpViewport,
    shm: wl_shm::WlShm,
    region: wl_region::WlRegion,
    configured: bool,
    seam: bool,
    heard: Arc<Heard>,
}

impl Pane {
    fn draw(&mut self, qh: &QueueHandle<Pane>) {
        let preferred = self.heard.preferred.load(Ordering::SeqCst);
        if !self.configured || preferred == 0 {
            return;
        }
        let s = f64::from(preferred) / 120.0;
        let (w, h) = (
            (f64::from(SIZE.0) * s).round() as i32,
            (f64::from(SIZE.1) * s).round() as i32,
        );
        let edge = TOOLBAR * s;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let y = f64::from(y);
            let rgba = if self.seam && edge.fract() != 0.0 && y == edge.floor() {
                [0, 0, 0, 0]
            } else if y < edge.floor() || (edge.fract() == 0.0 && y < edge) {
                [TOOLBAR_RGB[0], TOOLBAR_RGB[1], TOOLBAR_RGB[2], 255]
            } else {
                [CONTENT_RGB[0], CONTENT_RGB[1], CONTENT_RGB[2], 255]
            };
            for _ in 0..w {
                // argb8888, little-endian
                pixels.extend_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
            }
        }
        let fd =
            rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
        let mut file = std::fs::File::from(fd);
        file.set_len(pixels.len() as u64).unwrap();
        file.write_all(&pixels).unwrap();
        let pool = self
            .shm
            .create_pool(file.as_fd(), pixels.len() as i32, qh, ());
        let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
        pool.destroy();
        self.viewport.set_destination(SIZE.0, SIZE.1);
        self.surface.set_opaque_region(Some(&self.region));
        self.surface.attach(Some(&buffer), 0, 0);
        self.surface.damage_buffer(0, 0, w, h);
        self.surface.commit();
        self.heard.drawn.store(preferred, Ordering::SeqCst);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Pane {
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
        impl Dispatch<$t, ()> for Pane {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_buffer::WlBuffer,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_region::WlRegion,
    xdg_toplevel::XdgToplevel,
    wp_viewporter::WpViewporter,
    wp_viewport::WpViewport,
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1
);

impl Dispatch<wl_surface::WlSurface, ()> for Pane {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.heard.buffer_scale.store(factor, Ordering::SeqCst);
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for Pane {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            let before = state.heard.preferred.swap(scale, Ordering::SeqCst);
            if before != scale {
                state.draw(qh);
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Pane {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for Pane {
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
            let first = !state.configured;
            state.configured = true;
            if first {
                state.draw(qh);
            } else {
                state.surface.commit();
            }
        }
    }
}

/// The client's thread, stopped when dropped.
struct App {
    stop: Arc<AtomicBool>,
    heard: Arc<Heard>,
}

impl App {
    fn spawn(socket: PathBuf, seam: bool) -> App {
        let stop = Arc::new(AtomicBool::new(false));
        let heard = Arc::new(Heard::default());
        let (flag, heard2) = (stop.clone(), heard.clone());
        std::thread::spawn(move || {
            let conn = Connection::from_socket(
                UnixStream::connect(socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let (globals, mut queue) = registry_queue_init::<Pane>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 6..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let viewporter: wp_viewporter::WpViewporter = globals.bind(&qh, 1..=1, ()).unwrap();
            let fractional: wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let viewport = viewporter.get_viewport(&surface, &qh, ());
            let _scale = fractional.get_fractional_scale(&surface, &qh, ());
            let region = compositor.create_region(&qh, ());
            region.add(0, 0, SIZE.0, SIZE.1);
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.pane".into());
            toplevel.set_title("Pane".into());
            surface.commit();
            let mut client = Pane {
                surface,
                viewport,
                shm,
                region,
                configured: false,
                seam,
                heard: heard2,
            };
            while !flag.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        App { stop, heard }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// A compositor in `mode` with the client's window drawn at the scale it was asked for, settled.
fn settled(
    mode: FractionalScaleMode,
    seam: bool,
) -> Option<(Cupertino, EventLoop<'static, Cupertino>, App)> {
    let (mut state, mut event_loop) = harness(mode)?;
    state.cursor_status = CursorImageStatus::Hidden;
    for o in &mut state.outputs {
        o.cache.supersample.dither = false;
    }
    let app = App::spawn(socket_path(&state), seam);
    let heard = app.heard.clone();
    wait_until(
        &mut state,
        &mut event_loop,
        "the window to map at the scale it is asked for",
        move |s| {
            let drawn = heard.drawn.load(Ordering::SeqCst);
            drawn != 0
                && drawn == heard.preferred.load(Ordering::SeqCst)
                && s.windows.iter().any(|w| w.mapped)
        },
    );
    for _ in 0..60 {
        headless::step(&mut state, &mut event_loop, Duration::from_millis(5)).unwrap();
    }
    Some((state, event_loop, app))
}

/// Put the window at `(x, y)` points and let it settle there.
fn place(state: &mut Cupertino, event_loop: &mut EventLoop<'static, Cupertino>, x: f64, y: f64) {
    let id = state.windows.iter().find(|w| w.mapped).unwrap().id;
    let output = state.outputs[0].id;
    state.command(cupertino_wm::Command::MoveWindowToOutput {
        window: id,
        output,
        frame: cupertino_wm::Rect::new(x, y, f64::from(SIZE.0), f64::from(SIZE.1)),
    });
    let t = state.now() + 5.0;
    common::set_time(state, event_loop, t);
    state.tick();
    for _ in 0..20 {
        headless::step(state, event_loop, Duration::from_millis(5)).unwrap();
    }
    let frame = state.wm.window(id).unwrap().geometry();
    assert_eq!(
        (frame.x, frame.y),
        (x, y),
        "the window lies where it was put"
    );
}

/// The window's frame in output pixels at the display's scale: x, y, width, height.
fn window_px(state: &Cupertino) -> (u32, u32, u32, u32) {
    let id = state.windows.iter().find(|w| w.mapped).unwrap().id;
    let f = state.wm.window(id).unwrap().geometry();
    let o = state.outputs[0].rect;
    (
        ((f.x - o.x) * SCALE).round() as u32,
        ((f.y - o.y) * SCALE).round() as u32,
        (f.w * SCALE).round() as u32,
        (f.h * SCALE).round() as u32,
    )
}

/// The display's image, as a screenshot hands it out.
fn shot(state: &mut Cupertino, cursor: bool) -> RgbaImage {
    let mut backend = state.backend.take().unwrap();
    let image = state
        .draw_output_image(backend.renderer(), 0, cursor, false)
        .expect("an image of the output");
    state.backend = Some(backend);
    image
}

fn is_wallpaper(p: [u8; 4]) -> bool {
    p[1] > p[0].saturating_add(60) && p[1] > p[2].saturating_add(60)
}

#[test]
fn a_client_at_1_5_draws_at_2_and_the_display_shows_that_frame_shrunk_with_the_area_filter() {
    let Some((mut state, mut event_loop, app)) = settled(FractionalScaleMode::Supersample, false)
    else {
        return;
    };
    // on a whole number of points, the toolbar's end half way across a pixel at 1.5
    place(&mut state, &mut event_loop, 100.0, 120.0);
    // asked for 2, exactly and as a whole number; the output keeps its scale and points
    assert_eq!(app.heard.preferred.load(Ordering::SeqCst), 240);
    assert_eq!(app.heard.buffer_scale.load(Ordering::SeqCst), 2);
    assert_eq!(state.outputs[0].scale, SCALE);
    assert_eq!(state.outputs[0].render_scale, 2.0);
    assert_eq!(
        (state.outputs[0].rect.w, state.outputs[0].rect.h),
        (800.0, 600.0)
    );
    let entry = state.windows.iter().find(|w| w.mapped).unwrap();
    let surface = entry.surface().clone();
    let preferred = with_states(&surface, |s| {
        with_fractional_scale(s, |fs| fs.preferred_scale())
    });
    assert_eq!(preferred, Some(2.0));
    // its buffer is twice its points, and the window is its points
    let buffer = with_renderer_surface_state(&surface, |s| s.buffer_size()).flatten();
    assert_eq!(buffer.map(|b| (b.w, b.h)), Some((800, 600)));
    let id = entry.id;
    let frame = state.wm.window(id).unwrap().geometry();
    assert_eq!((frame.w, frame.h), (f64::from(SIZE.0), f64::from(SIZE.1)));

    // the scene is composed at 2, the pointer and what lies above it at the display's own scale
    let mut backend = state.backend.take().unwrap();
    let composed = state
        .compose_output(backend.renderer(), 0, true)
        .expect("a scene");
    assert_eq!(composed.frame.output.scale, 2.0);
    assert_eq!(
        (
            composed.frame.output.physical_size.width,
            composed.frame.output.physical_size.height
        ),
        (1600, 1200)
    );
    let top = composed
        .resources
        .top
        .as_ref()
        .expect("the layers drawn over the shrink");
    assert_eq!(top.frame.output.scale, SCALE);
    assert!(composed
        .frame
        .items()
        .iter()
        .all(|(layer, _)| *layer < SceneLayer::Cursor));
    // the frame at 2, drawn straight, and the image the display shows: the first shrunk with the area filter
    let elements = state.scene_elements(backend.renderer(), 0, false).unwrap();
    let at_two = render_to_image(
        backend.renderer(),
        &elements,
        (1600, 1200).into(),
        2.0,
        cupertino_server::frame::SCREENSHOT_CLEAR,
    )
    .expect("the frame at 2");
    state.backend = Some(backend);
    let shown = shot(&mut state, false);
    assert_eq!(
        (shown.width(), shown.height()),
        (PANEL.0 as u32, PANEL.1 as u32),
        "a screenshot is the display's pixels"
    );
    let texels: Vec<[f32; 4]> = at_two
        .pixels()
        .map(|p| p.0.map(|c| f32::from(c) / 255.0))
        .collect();
    let want = shrink(&texels, (1600, 1200), (PANEL.0 as u32, PANEL.1 as u32));
    let mut worst = (0u8, (0, 0));
    for (x, y, p) in shown.enumerate_pixels() {
        let w = want[(y * PANEL.0 as u32 + x) as usize];
        for (shown, want) in p.0.iter().zip(w).take(3) {
            let d = shown.abs_diff((want.clamp(0.0, 1.0) * 255.0).round() as u8);
            if d > worst.0 {
                worst = (d, (x, y));
            }
        }
    }
    assert!(
        worst.0 <= 2,
        "{} steps from the filter's at {:?}",
        worst.0,
        worst.1
    );

    // the toolbar's end, half way across a pixel at 1.5: that row is part toolbar and part content, never wallpaper
    let (x, y, w, _) = window_px(&state);
    let edge = y + (TOOLBAR * SCALE).floor() as u32;
    for row in edge - 1..=edge + 1 {
        for col in x + 10..x + w - 10 {
            let p = shown.get_pixel(col, row).0;
            assert!(
                !is_wallpaper(p),
                "the wallpaper shows at ({col}, {row}): {p:?}"
            );
        }
    }
    let mid = shown.get_pixel(x + w / 2, edge).0;
    for c in 0..3 {
        let (lo, hi) = (
            TOOLBAR_RGB[c].min(CONTENT_RGB[c]),
            TOOLBAR_RGB[c].max(CONTENT_RGB[c]),
        );
        assert!(
            mid[c] > lo + 20 && mid[c] < hi - 20,
            "the row is part toolbar, part content: {mid:?}"
        );
    }
    drop(app);
}

#[test]
fn an_unchanged_desktop_draws_no_frame_and_the_pointer_is_drawn_at_the_display_s_own_scale() {
    let Some((mut state, _event_loop, app)) = settled(FractionalScaleMode::Supersample, false)
    else {
        return;
    };
    // frames drawn into a kept target, age 1 after the first: the second, with nothing changed, draws nothing
    let mut backend = state.backend.take().unwrap();
    let renderer = backend.renderer();
    let mut target: GlesTexture = renderer
        .create_buffer(Fourcc::Abgr8888, PANEL.into())
        .expect("a target");
    let mut statuses = Vec::new();
    for age in [0, 1, 1] {
        let composed = state.compose_output(renderer, 0, false).expect("a scene");
        let (res, cache, tracker) = state.gles_parts(0).unwrap();
        let mut resources = GlesResources::new(res, cache, composed.resources);
        let done = Gles
            .render(
                &composed.frame,
                &mut resources,
                GlesTarget::Texture {
                    renderer,
                    texture: &mut target,
                    tracker,
                    age,
                    clear: Color32F::BLACK,
                },
            )
            .expect("render");
        statuses.push(done.status());
    }
    assert_eq!(
        statuses,
        [
            RenderStatus::Rendered,
            RenderStatus::Empty,
            RenderStatus::Empty
        ]
    );
    state.backend = Some(backend);

    // the pointer over the wallpaper: the same pixels drawn at 2 and shrunk as drawn straight at 1.5
    state.cursor_status = CursorImageStatus::default_named();
    let at = smithay::utils::Point::from((700.0, 500.0));
    state.pointer.set_location(at);
    let supersampled = shot(&mut state, true);
    let (mut direct, _direct_loop) = harness(FractionalScaleMode::Direct).unwrap();
    direct.cursor_status = CursorImageStatus::default_named();
    direct.pointer.set_location(at);
    let straight = shot(&mut direct, true);
    let (cx, cy) = ((700.0 * SCALE) as u32, (500.0 * SCALE) as u32);
    let mut differs = 0;
    let mut drawn = 0;
    for y in cy.saturating_sub(10)..cy + 50 {
        for x in cx.saturating_sub(10)..cx + 50 {
            let (a, b) = (supersampled.get_pixel(x, y).0, straight.get_pixel(x, y).0);
            drawn += usize::from(!is_wallpaper(a));
            differs += usize::from(a.iter().zip(b).any(|(p, q)| p.abs_diff(q) > 2));
        }
    }
    assert!(drawn > 50, "the pointer is drawn: {drawn} pixels");
    assert_eq!(
        differs, 0,
        "the pointer is drawn as at the display's own scale"
    );
    drop(app);
}

#[test]
fn under_direct_the_client_draws_at_1_5_and_a_row_it_declares_opaque_but_leaves_clear_shows_black()
{
    let Some((mut state, mut event_loop, app)) = settled(FractionalScaleMode::Direct, true) else {
        return;
    };
    assert_eq!(app.heard.preferred.load(Ordering::SeqCst), 180);
    assert_eq!(state.outputs[0].render_scale, SCALE);
    let mut backend = state.backend.take().unwrap();
    let composed = state
        .compose_output(backend.renderer(), 0, true)
        .expect("a scene");
    assert!(composed.resources.top.is_none(), "drawn straight at 1.5");
    assert_eq!(composed.frame.output.scale, SCALE);
    state.backend = Some(backend);
    // the window's top on a whole pixel and half way across one: its content lands on the nearest whole pixel
    for top in [120.0, 121.0] {
        place(&mut state, &mut event_loop, 100.0, top);
        let shown = shot(&mut state, false);
        let (x, _, w, _) = window_px(&state);
        let row = (top * SCALE + 0.5).floor() as u32 + (TOOLBAR * SCALE).floor() as u32;
        for col in x + 10..x + w - 10 {
            let p = shown.get_pixel(col, row).0;
            assert!(
                !is_wallpaper(p),
                "at {top}: the wallpaper shows at ({col}, {row}): {p:?}"
            );
            assert!(
                p[..3].iter().all(|c| *c < 8),
                "at {top}: black at ({col}, {row}): {p:?}"
            );
        }
        // the rows round it are the client's
        assert_eq!(shown.get_pixel(x + w / 2, row - 1).0[..3], TOOLBAR_RGB);
        assert_eq!(shown.get_pixel(x + w / 2, row + 1).0[..3], CONTENT_RGB);
    }
}
