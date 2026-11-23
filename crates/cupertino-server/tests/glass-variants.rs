//! Glass named by its system variant (`cupertino_backdrop_v1.set_glass_variant` and `set_glass_tint`, version 23)
//! drawn by the headless compositor: a tint lays its colour over the glass; a variant follows its window's activation
//! and takes the inactive recipe in a window that is not key, where glass a version 22 client asked for by its style
//! draws as before; and interactive glass grows under a press along the compositor's clock, lets go with a small
//! undershoot, and does not grow under Reduce Motion.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use cupertino_assets::Assets;
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::{
    self as cupertino_backdrop_v1, CupertinoBackdropV1, GlassModifier, GlassStyle, GlassVariant, SizeClass, Tone,
};
use cupertino_server::glass_press::PRESS_SCALE;
use cupertino_server::state::{insert_client, ClientState};
use cupertino_server::{Config, Cupertino};
use cupertino_wm::WindowId;
use smithay::backend::input::ButtonState;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::Display;
use smithay::utils::Point;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface,
    wl_surface,
};
use wayland_client::backend::ObjectId;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};

/// The version of `cupertino_app_effects_v1` whose backdrops take `set_glass_variant` and `set_glass_tint`.
const VARIANT_VERSION: u32 = 23;
/// The version before it: a client of it names glass only by its style.
const LEGACY_VERSION: u32 = 22;
/// The primary button's evdev code.
const BTN_LEFT: u32 = 0x110;
/// The width of the pressed capsule in points, as the samples of the press were measured on.
const CAPSULE: (i32, i32) = (160, 60);
/// How far, in points, the capsule's width may be from a sample of the press.
const SAMPLE_TOLERANCE: f64 = 0.25;
/// The compositor's clock when a rig starts, in seconds: the tests move it themselves.
const START: f64 = 100.0;
/// How far the clock moves on for a window's opening, an activation change or a press to finish, in seconds.
const SETTLE: f64 = 2.0;
/// A mid grey window content, memory order blue, green, red, alpha.
const GREY: [u8; 4] = [128, 128, 128, 255];

#[derive(Default)]
struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    effects: Option<CupertinoAppEffectsV1>,
    legacy_effects: Option<CupertinoAppEffectsV1>,
    configured: bool,
    /// The tones the server sent each backdrop, oldest first.
    tones: Vec<(ObjectId, WEnum<Tone>)>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Client {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(reg.bind(name, 4, qh, ())),
                "wl_subcompositor" => state.subcompositor = Some(reg.bind(name, 1, qh, ())),
                "wl_shm" => state.shm = Some(reg.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(reg.bind(name, 1, qh, ())),
                "cupertino_app_effects_v1" => {
                    assert!(
                        version >= VARIANT_VERSION,
                        "the server offers version {version}"
                    );
                    state.effects = Some(reg.bind(name, VARIANT_VERSION, qh, ()));
                    state.legacy_effects = Some(reg.bind(name, LEGACY_VERSION, qh, ()));
                }
                _ => {}
            }
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

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.configured = true;
        }
    }
}

wayland_client::delegate_noop!(Client: ignore xdg_toplevel::XdgToplevel);
wayland_client::delegate_noop!(Client: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(Client: ignore wl_subcompositor::WlSubcompositor);
wayland_client::delegate_noop!(Client: ignore wl_subsurface::WlSubsurface);
wayland_client::delegate_noop!(Client: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(Client: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(Client: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(Client: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(Client: ignore CupertinoAppEffectsV1);
impl Dispatch<CupertinoBackdropV1, ()> for Client {
    fn event(
        state: &mut Self,
        backdrop: &CupertinoBackdropV1,
        event: cupertino_backdrop_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cupertino_backdrop_v1::Event::Tone { tone } = event {
            state.tones.push((backdrop.id(), tone));
        }
    }
}

/// A `width` × `height` ARGB8888 buffer whose pixel at `x` (memory order blue, green, red, alpha) is `pixel(x)`.
fn shm_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<Client>,
    (width, height): (i32, i32),
    pixel: impl Fn(i32) -> [u8; 4],
) -> wl_buffer::WlBuffer {
    let len = width * height * 4;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    let row: Vec<u8> = (0..width).flat_map(&pixel).collect();
    for _ in 0..height {
        file.write_all(&row).unwrap();
    }
    let pool = shm.create_pool(file.as_fd(), len, qh, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    buffer
}

/// A mapped toplevel and what keeps it alive.
struct Window {
    surface: wl_surface::WlSurface,
    id: WindowId,
    _xdg: (xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel),
}

/// A clear subsurface of a window and the glass backdrop over all of it.
struct Pane {
    surface: wl_surface::WlSurface,
    backdrop: CupertinoBackdropV1,
    _sub: wl_subsurface::WlSubsurface,
}

/// The headless compositor drawing one output, and one client of it.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    conn: Connection,
    queue: EventQueue<Client>,
    client: Client,
    scale: f64,
}

impl Rig {
    /// An output of `size` logical points at `scale`.
    fn new(size: (i32, i32), scale: f64) -> Rig {
        // the software rasteriser, as the program tests run the headless server
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            false,
            &Assets::builtin(),
        )
        .unwrap();
        let pixels = (
            (f64::from(size.0) * scale) as i32,
            (f64::from(size.1) * scale) as i32,
        );
        cupertino_server::backend::headless::init(&mut state, pixels, scale)
            .expect("an offscreen renderer");
        state.fixed_time = Some(START);
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&state.display_handle, ours, ClientState::default()).unwrap();
        let conn = Connection::from_socket(theirs).unwrap();
        let queue = conn.new_event_queue();
        let mut rig = Rig {
            event_loop,
            state,
            conn,
            queue,
            client: Client::default(),
            scale,
        };
        let _registry = rig.conn.display().get_registry(&rig.queue.handle(), ());
        rig.pump(6);
        rig
    }

    /// The compositor's clock.
    fn now(&self) -> f64 {
        self.state.fixed_time.unwrap()
    }

    /// Let what was sent take effect and every animation it started end: turns of the loops, the clock moved on by
    /// [`SETTLE`], and turns again.
    fn settle(&mut self) {
        self.pump(6);
        self.state.fixed_time = Some(self.now() + SETTLE);
        self.pump(6);
    }

    /// `turns` turns of the compositor's loop and of the client's, each drawing the output.
    fn pump(&mut self, turns: usize) {
        for _ in 0..turns {
            let _ = self.conn.flush();
            let _ = self
                .event_loop
                .dispatch(Some(Duration::from_millis(2)), &mut self.state);
            self.state.tick();
            cupertino_server::backend::headless::render(&mut self.state);
            let _ = self.state.display_handle.flush_clients();
            if let Some(guard) = self.conn.prepare_read() {
                let _ = guard.read();
            }
            let _ = self.queue.dispatch_pending(&mut self.client);
        }
    }

    /// The output as it shows now, without the pointer.
    fn image(&mut self) -> image::RgbaImage {
        let mut backend = self.state.backend.take().unwrap();
        let image = self
            .state
            .draw_output_image(backend.renderer(), 0, false, false)
            .unwrap();
        self.state.backend = Some(backend);
        image
    }

    /// A toplevel titled `title`, configured, its content not yet attached.
    fn window(
        &mut self,
        title: &str,
    ) -> (
        wl_surface::WlSurface,
        xdg_surface::XdgSurface,
        xdg_toplevel::XdgToplevel,
    ) {
        let qh = self.queue.handle();
        let surface = self
            .client
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let xdg = self
            .client
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title(title.into());
        toplevel.set_app_id(format!("org.cupertino.test.{title}"));
        self.client.configured = false;
        surface.commit();
        for _ in 0..20 {
            self.pump(1);
            if self.client.configured {
                break;
            }
        }
        assert!(self.client.configured, "{title} is configured");
        (surface, xdg, toplevel)
    }

    /// A toplevel of `size` points filled with `pixel(x)`, once it is mapped.
    fn map(&mut self, title: &str, size: (i32, i32), pixel: impl Fn(i32) -> [u8; 4]) -> Window {
        let before: Vec<WindowId> = self.mapped();
        let (surface, xdg, toplevel) = self.window(title);
        let buffer = shm_buffer(
            self.client.shm.as_ref().unwrap(),
            &self.queue.handle(),
            size,
            pixel,
        );
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        surface.commit();
        self.settle();
        let id = self
            .mapped()
            .into_iter()
            .find(|id| !before.contains(id))
            .unwrap_or_else(|| panic!("{title} is mapped"));
        Window {
            surface,
            id,
            _xdg: (xdg, toplevel),
        }
    }

    fn mapped(&self) -> Vec<WindowId> {
        self.state
            .windows
            .iter()
            .filter(|e| e.mapped)
            .map(|e| e.id)
            .collect()
    }

    /// A clear subsurface of `parent` at `at` of `size` points with a glass backdrop of `effects` over all of it,
    /// its corners of `radius`; it shows on the parent's next commit.
    fn pane(
        &mut self,
        parent: &wl_surface::WlSurface,
        effects: &CupertinoAppEffectsV1,
        at: (i32, i32),
        size: (i32, i32),
        radius: f64,
    ) -> Pane {
        let qh = self.queue.handle();
        let surface = self
            .client
            .compositor
            .as_ref()
            .unwrap()
            .create_surface(&qh, ());
        let sub =
            self.client
                .subcompositor
                .as_ref()
                .unwrap()
                .get_subsurface(&surface, parent, &qh, ());
        sub.set_position(at.0, at.1);
        let clear = shm_buffer(self.client.shm.as_ref().unwrap(), &qh, size, |_| {
            [0, 0, 0, 0]
        });
        let backdrop = effects.get_backdrop(&surface, &qh, ());
        backdrop.set_shape(0, 0, size.0, size.1, radius);
        surface.attach(Some(&clear), 0, 0);
        surface.damage_buffer(0, 0, size.0, size.1);
        Pane {
            surface,
            backdrop,
            _sub: sub,
        }
    }

    /// The top left of window `id` in output pixels.
    fn origin(&self, id: WindowId) -> (f64, f64) {
        let frame = self.state.wm.window(id).unwrap().geometry();
        (frame.x * self.scale, frame.y * self.scale)
    }

    /// The pixels of `image` in the rectangle at `at` of `size` points from window `id`'s top left, inset by `inset`
    /// points on every side.
    fn pixels<'a>(
        &self,
        image: &'a image::RgbaImage,
        id: WindowId,
        at: (i32, i32),
        size: (i32, i32),
        inset: i32,
    ) -> impl Iterator<Item = [u8; 4]> + 'a {
        let (ox, oy) = self.origin(id);
        let s = self.scale;
        let x0 = (ox + f64::from(at.0 + inset) * s) as u32;
        let y0 = (oy + f64::from(at.1 + inset) * s) as u32;
        let x1 = (ox + f64::from(at.0 + size.0 - inset) * s) as u32;
        let y1 = (oy + f64::from(at.1 + size.1 - inset) * s) as u32;
        (y0..y1).flat_map(move |y| (x0..x1).map(move |x| image.get_pixel(x, y).0))
    }
}

/// The mean red, green and blue of `pixels`.
fn mean(pixels: impl Iterator<Item = [u8; 4]>) -> [f64; 3] {
    let (mut sum, mut n) = ([0.0; 3], 0.0);
    for p in pixels {
        for c in 0..3 {
            sum[c] += f64::from(p[c]);
        }
        n += 1.0;
    }
    sum.map(|s| s / n)
}

/// The mean difference per channel of two equally long runs of pixels.
fn difference(a: Vec<[u8; 4]>, b: Vec<[u8; 4]>) -> f64 {
    assert_eq!(a.len(), b.len());
    let total: f64 = a
        .iter()
        .zip(&b)
        .map(|(p, q)| (0..3).map(|c| f64::from(p[c].abs_diff(q[c]))).sum::<f64>())
        .sum();
    total / (3.0 * a.len() as f64)
}

#[test]
fn a_tinted_backdrop_lays_its_colour_over_its_glass() {
    let mut rig = Rig::new((480, 320), 1.0);
    let effects = rig
        .client
        .effects
        .clone()
        .expect("the server offers app effects");
    let window = rig.map("Tint", (240, 160), |_| GREY);
    let (plain_at, tinted_at, size) = ((20, 50), (130, 50), (90, 60));
    let plain = rig.pane(&window.surface, &effects, plain_at, size, 12.0);
    let tinted = rig.pane(&window.surface, &effects, tinted_at, size, 12.0);
    for pane in [&plain, &tinted] {
        pane.backdrop.set_glass_variant(
            GlassVariant::Regular,
            GlassModifier::empty(),
            SizeClass::Dynamic,
            0,
            0.0,
        );
    }
    tinted.backdrop.set_glass_tint(1.0, 0.0, 0.0, 1.0);
    plain.surface.commit();
    tinted.surface.commit();
    window.surface.commit();
    rig.settle();

    let image = rig.image();
    let [r, g, _] = mean(rig.pixels(&image, window.id, plain_at, size, 14));
    assert!(
        (r - g).abs() < 8.0,
        "untinted glass over grey stays grey: {r} {g}"
    );
    let [r, g, b] = mean(rig.pixels(&image, window.id, tinted_at, size, 14));
    assert!(
        r > g + 60.0 && r > b + 60.0,
        "the tinted glass shows red: {r} {g} {b}"
    );

    // alpha 0 takes the tint off again
    tinted.backdrop.set_glass_tint(1.0, 0.0, 0.0, 0.0);
    tinted.surface.commit();
    window.surface.commit();
    rig.settle();
    let image = rig.image();
    let [r, g, _] = mean(rig.pixels(&image, window.id, tinted_at, size, 14));
    assert!(
        (r - g).abs() < 8.0,
        "without its tint the glass is grey again: {r} {g}"
    );
}

#[test]
fn a_variant_takes_the_inactive_recipe_in_a_window_that_is_not_key_and_a_style_of_an_older_client_does_not(
) {
    let mut rig = Rig::new((960, 640), 1.0);
    let effects = rig
        .client
        .effects
        .clone()
        .expect("the server offers app effects");
    let legacy = rig.client.legacy_effects.clone().unwrap();
    // stripes of 4 pixels; both panes start on a stripe's edge so each covers the same content
    let window = rig.map("Inactive", (400, 240), |x| {
        if (x / 4) % 2 == 0 {
            [40, 40, 40, 255]
        } else {
            [220, 220, 220, 255]
        }
    });
    let (styled_at, named_at, size) = ((24, 150), (232, 150), (144, 64));
    let styled = rig.pane(&window.surface, &legacy, styled_at, size, 16.0);
    let named = rig.pane(&window.surface, &effects, named_at, size, 16.0);
    styled.backdrop.set_glass(GlassStyle::Regular, 0, 0.0);
    named.backdrop.set_glass_variant(
        GlassVariant::Regular,
        GlassModifier::empty(),
        SizeClass::Dynamic,
        0,
        0.0,
    );
    styled.surface.commit();
    named.surface.commit();
    window.surface.commit();
    rig.settle();
    assert_eq!(rig.state.wm.key_window(), Some(window.id));

    let image = rig.image();
    let styled_key: Vec<_> = rig.pixels(&image, window.id, styled_at, size, 2).collect();
    let named_key: Vec<_> = rig.pixels(&image, window.id, named_at, size, 2).collect();
    let alike = difference(styled_key.clone(), named_key.clone());
    assert!(
        alike < 1.0,
        "in the key window the variant draws as the style does: {alike}"
    );

    // another window takes the key, as a click on it gives it
    let other = rig.map("Front", (48, 32), |_| GREY);
    rig.state.focus_window_by_input(other.id);
    rig.settle();
    assert_eq!(rig.state.wm.key_window(), Some(other.id));
    let front = rig.state.wm.window(other.id).unwrap().geometry();
    let back = rig.state.wm.window(window.id).unwrap().geometry();
    let panes_top = back.y + f64::from(styled_at.1) - 24.0;
    assert!(
        front.y + front.h < panes_top
            || front.y > back.y + back.h + 24.0
            || front.x > back.x + back.w + 24.0
            || front.x + front.w < back.x - 24.0,
        "the front window {front:?} lies off the panes of {back:?}"
    );

    let image = rig.image();
    let styled_inactive: Vec<_> = rig.pixels(&image, window.id, styled_at, size, 2).collect();
    let named_inactive: Vec<_> = rig.pixels(&image, window.id, named_at, size, 2).collect();
    let styled_change = difference(styled_key, styled_inactive);
    let named_change = difference(named_key, named_inactive);
    assert!(
        styled_change < 0.5,
        "the older client's glass draws as before: {styled_change}"
    );
    assert!(
        named_change > 1.0 && named_change > 4.0 * styled_change,
        "the variant takes the inactive recipe: {named_change}, the style {styled_change}"
    );
}

/// The width in output pixels of the run of red pixels along row `y` of `image` between `x0` and `x1`.
fn red_width(image: &image::RgbaImage, y: u32, x0: u32, x1: u32) -> u32 {
    let redness = |x: u32| {
        let p = image.get_pixel(x, y).0;
        i32::from(p[0]) - i32::from(p[1])
    };
    let peak = (x0..x1).map(redness).max().unwrap();
    assert!(peak > 60, "the capsule shows red: {peak}");
    (x0..x1).filter(|x| redness(*x) * 2 > peak).count() as u32
}

/// A red tinted interactive capsule on a scale 2 output, at rest at `t0`, and how to measure its width.
struct Capsule {
    rig: Rig,
    window: Window,
    _pane: Pane,
    at: (i32, i32),
}

impl Capsule {
    fn new() -> Capsule {
        Capsule::with(GlassModifier::Interactive, 1.0)
    }

    /// The capsule of regular glass with `modifiers`, its glass drawn `scale` times its size.
    fn with(modifiers: GlassModifier, scale: f64) -> Capsule {
        let mut rig = Rig::new((480, 320), 2.0);
        let effects = rig
            .client
            .effects
            .clone()
            .expect("the server offers app effects");
        let window = rig.map("Press", (240, 160), |_| GREY);
        let at = (40, 50);
        let pane = rig.pane(
            &window.surface,
            &effects,
            at,
            CAPSULE,
            f64::from(CAPSULE.1) / 2.0,
        );
        pane.backdrop.set_glass_variant(
            GlassVariant::Regular,
            modifiers,
            SizeClass::Dynamic,
            0,
            0.0,
        );
        pane.backdrop.set_glass_tint(1.0, 0.0, 0.0, 1.0);
        if scale != 1.0 {
            pane.backdrop.set_glass_scale(scale);
        }
        pane.surface.commit();
        window.surface.commit();
        rig.settle();
        Capsule {
            rig,
            window,
            _pane: pane,
            at,
        }
    }

    /// The capsule's centre in global logical points.
    fn centre(&self) -> Point<f64, smithay::utils::Logical> {
        let frame = self.rig.state.wm.window(self.window.id).unwrap().geometry();
        Point::from((
            frame.x + f64::from(self.at.0 + CAPSULE.0 / 2),
            frame.y + f64::from(self.at.1 + CAPSULE.1 / 2),
        ))
    }

    /// The width of the capsule in output pixels as it shows at `t`.
    fn width_at(&mut self, t: f64) -> u32 {
        self.rig.state.fixed_time = Some(t);
        self.rig.pump(1);
        let image = self.rig.image();
        let centre = self.centre();
        let s = self.rig.scale;
        let reach = f64::from(CAPSULE.0) / 2.0 + 20.0;
        red_width(
            &image,
            (centre.y * s) as u32,
            ((centre.x - reach) * s) as u32,
            ((centre.x + reach) * s) as u32,
        )
    }

    fn press(&mut self, t: f64) {
        self.rig.state.fixed_time = Some(t);
        self.rig.state.pointer_moved_to(self.centre(), 0, None);
        self.rig
            .state
            .pointer_button(BTN_LEFT, ButtonState::Pressed, 1);
    }

    fn release(&mut self, t: f64) {
        self.rig.state.fixed_time = Some(t);
        self.rig
            .state
            .pointer_button(BTN_LEFT, ButtonState::Released, 2);
    }

    /// The capsule's width in points as the press scales it at `t`.
    fn points_at(&self, t: f64) -> f64 {
        f64::from(CAPSULE.0) * self.rig.state.glass_press.scale(t)
    }
}

#[test]
fn interactive_glass_grows_under_a_press_along_the_samples_and_lets_go_with_an_undershoot() {
    let mut capsule = Capsule::new();
    let t0 = capsule.rig.now();
    let rest = capsule.width_at(t0);
    assert!(
        rest.abs_diff(CAPSULE.0 as u32 * 2) <= 4,
        "at rest the capsule is {} pixels wide: {rest}",
        CAPSULE.0 * 2
    );
    capsule.press(t0);
    for (after, width) in [(0.1, 161.5), (0.2, 162.4), (0.3, 162.25)] {
        let points = capsule.points_at(t0 + after);
        assert!(
            (points - width).abs() <= SAMPLE_TOLERANCE,
            "{after} s into the press the capsule is {points} points wide, the sample {width}"
        );
    }
    let settled = capsule.points_at(t0 + 1.0);
    assert!((settled - f64::from(CAPSULE.0) * PRESS_SCALE).abs() < 0.01);

    // the output shows it: 2.24 points on a scale 2 output, about 4.5 pixels
    let grown = capsule.width_at(t0 + 0.2);
    assert!(
        (rest + 3..=rest + 7).contains(&grown),
        "the pressed capsule shows {grown} pixels wide, at rest {rest}"
    );

    // the release undershoots to about 159.8 points and is at rest by 500 ms
    let t1 = t0 + 1.0;
    capsule.release(t1);
    let least = (1..50)
        .map(|i| capsule.points_at(t1 + f64::from(i) * 0.01))
        .fold(f64::MAX, f64::min);
    assert!(
        (least - 159.8).abs() <= SAMPLE_TOLERANCE,
        "the release undershoots to {least} points"
    );
    let still = capsule.points_at(t1 + 0.5);
    assert!(
        (still - f64::from(CAPSULE.0)).abs() < 0.05,
        "at rest by 500 ms: {still}"
    );
    let after = capsule.width_at(t1 + 0.6);
    assert!(after.abs_diff(rest) <= 1, "back to {rest} pixels: {after}");
}

#[test]
fn under_reduce_motion_interactive_glass_does_not_grow_under_a_press() {
    let mut capsule = Capsule::new();
    let t0 = capsule.rig.now();
    capsule
        .rig
        .state
        .accessibility
        .settings
        .display
        .reduce_motion = true;
    let rest = capsule.width_at(t0);
    capsule.press(t0);
    for after in [0.1, 0.2, 0.3, 1.0] {
        let width = capsule.width_at(t0 + after);
        assert!(
            width.abs_diff(rest) <= 1,
            "{after} s into the press the capsule is {width} pixels wide, at rest {rest}"
        );
    }
    capsule.release(t0 + 1.0);
}

#[test]
fn a_press_beside_interactive_glass_after_one_on_it_asks_for_no_frames() {
    let mut capsule = Capsule::new();
    let t0 = capsule.rig.now();
    capsule.press(t0);
    capsule.width_at(t0 + 0.1);
    assert!(
        capsule.rig.state.glass_press.wants_tick(t0 + 0.1),
        "the glass under the press grows"
    );
    capsule.release(t0 + 0.2);
    let t1 = t0 + SETTLE;
    capsule.width_at(t1);
    assert!(!capsule.rig.state.glass_press.wants_tick(t1));

    // a click on the window beside the capsule, on no glass
    let frame = capsule
        .rig
        .state
        .wm
        .window(capsule.window.id)
        .unwrap()
        .geometry();
    let beside = Point::from((frame.x + 220.0, frame.y + 140.0));
    capsule.rig.state.pointer_moved_to(beside, 0, None);
    capsule
        .rig
        .state
        .pointer_button(BTN_LEFT, ButtonState::Pressed, 3);
    for after in [0.0, 0.05, 0.1] {
        capsule.width_at(t1 + after);
        assert!(
            !capsule.rig.state.glass_press.wants_tick(t1 + after),
            "{after} s into a press on no glass the press asks for frames"
        );
    }
    capsule.rig.state.fixed_time = Some(t1 + 0.2);
    capsule
        .rig
        .state
        .pointer_button(BTN_LEFT, ButtonState::Released, 4);
    assert!(!capsule.rig.state.animating());
}

/// The toolbar of [`a_toolbars_container_draws_alike_whichever_order_its_chosen_segment_was_made_in`]: its size in
/// points, and the left, top, width and height of each item in it.
const TOOLBAR: (i32, i32) = (360, 72);
const TOOLBAR_ITEMS: [(i32, i32, i32, i32); 3] = [(12, 16, 96, 40), (132, 16, 96, 40), (252, 16, 96, 40)];
/// The container the toolbar's items join.
const TOOLBAR_CONTAINER: u32 = 7;

/// The output showing a window of light and dark stripes under a toolbar of glass in one container: a toolbar item
/// made first, then the others in `order` (indices into [`TOOLBAR_ITEMS`]), the chosen segment (`chosen`) of the tab
/// variant and the rest toolbar items; and the toolbar's pixels.
fn toolbar_pixels(order: [usize; 2], chosen: usize) -> Vec<[u8; 4]> {
    let mut rig = Rig::new((480, 320), 1.0);
    let effects = rig.client.effects.clone().unwrap();
    let window = rig.map("Toolbar", (400, 200), |x| {
        if (x / 6) % 2 == 0 {
            [60, 60, 60, 255]
        } else {
            [235, 235, 235, 255]
        }
    });
    let at = (20, 40);
    let bar = rig.pane(&window.surface, &effects, at, TOOLBAR, 0.0);
    bar.backdrop.destroy();
    let qh = rig.queue.handle();
    let backdrops: Vec<CupertinoBackdropV1> = [0]
        .into_iter()
        .chain(order)
        .map(|i| {
            let (x, y, w, h) = TOOLBAR_ITEMS[i];
            let backdrop = effects.get_backdrop(&bar.surface, &qh, ());
            backdrop.set_shape(x, y, w, h, f64::from(h) / 2.0);
            let variant = if i == chosen {
                GlassVariant::Tab
            } else {
                GlassVariant::ToolbarItem
            };
            backdrop.set_glass_variant(
                variant,
                GlassModifier::empty(),
                SizeClass::Dynamic,
                TOOLBAR_CONTAINER,
                0.0,
            );
            backdrop
        })
        .collect();
    bar.surface.commit();
    window.surface.commit();
    rig.settle();
    let image = rig.image();
    let pixels = rig.pixels(&image, window.id, at, TOOLBAR, 0).collect();
    drop(backdrops);
    pixels
}

#[test]
fn a_toolbars_container_draws_alike_whichever_order_its_chosen_segment_was_made_in() {
    // the chosen segment made last, or before the last toolbar item
    let last = toolbar_pixels([1, 2], 2);
    let before = toolbar_pixels([2, 1], 2);
    let change = difference(last, before);
    assert!(
        change < 0.05,
        "the toolbar draws {change} apart in the two orders"
    );
}

#[test]
fn small_glass_follows_what_lies_behind_it_when_adaptive_or_of_an_older_clients_style_and_not_otherwise() {
    let mut rig = Rig::new((480, 320), 1.0);
    let effects = rig.client.effects.clone().unwrap();
    let legacy = rig.client.legacy_effects.clone().unwrap();
    let window = rig.map("Tones", (400, 200), |_| [250, 250, 250, 255]);
    // small enough for their text to follow what lies behind them
    let size = (100, 40);
    let fixed = rig.pane(&window.surface, &effects, (20, 60), size, 20.0);
    let adaptive = rig.pane(&window.surface, &effects, (150, 60), size, 20.0);
    let styled = rig.pane(&window.surface, &legacy, (280, 60), size, 20.0);
    for (pane, modifiers) in [
        (&fixed, GlassModifier::empty()),
        (&adaptive, GlassModifier::Adaptive),
    ] {
        pane.backdrop.set_glass_variant(
            GlassVariant::Regular,
            modifiers,
            SizeClass::Dynamic,
            0,
            0.0,
        );
    }
    styled.backdrop.set_glass(GlassStyle::Regular, 0, 0.0);
    for pane in [&fixed, &adaptive, &styled] {
        pane.surface.commit();
    }
    window.surface.commit();
    rig.settle();
    rig.settle();

    let tones = |pane: &Pane| {
        rig.client
            .tones
            .iter()
            .filter(|(id, _)| *id == pane.backdrop.id())
            .count()
    };
    assert!(tones(&adaptive) > 0, "adaptive glass follows: {:?}", rig.client.tones);
    assert!(tones(&styled) > 0, "the older client's style follows as before");
    assert_eq!(tones(&fixed), 0, "glass not asked to adapt keeps the appearance");
}

/// How many times its size the scaled capsule's glass is drawn.
const SCALED: f64 = 1.1;

#[test]
fn a_scaled_glass_backdrop_draws_larger_about_its_centre() {
    let mut plain = Capsule::with(GlassModifier::empty(), 1.0);
    let mut scaled = Capsule::with(GlassModifier::empty(), SCALED);
    let t = plain.rig.now();
    let rest = plain.width_at(t);
    let grown = scaled.width_at(scaled.rig.now());
    let expected = f64::from(rest) * SCALED;
    assert!(
        (f64::from(grown) - expected).abs() <= 4.0,
        "the scaled capsule shows {grown} pixels wide, {expected} expected from {rest}"
    );
    // about its centre: as much red each side of the centre, and past the subsurface it lies on
    let image = scaled.rig.image();
    let centre = scaled.centre();
    let s = scaled.rig.scale;
    let y = (centre.y * s) as u32;
    let mid = (centre.x * s) as u32;
    let reach = ((f64::from(CAPSULE.0) / 2.0 + 30.0) * s) as u32;
    let left = red_width(&image, y, mid - reach, mid);
    let right = red_width(&image, y, mid, mid + reach);
    assert!(
        left.abs_diff(right) <= 2,
        "{left} pixels left of the centre, {right} right"
    );
    assert!(
        f64::from(left) > f64::from(CAPSULE.0) / 2.0 * s + 4.0,
        "the glass reaches past its subsurface: {left}"
    );
}

#[test]
fn a_menu_named_by_its_variant_keeps_the_popover_outline_of_the_menu_style() {
    use cupertino_server::glass_look::{of_wire, GlassLook};
    use cupertino_theme::GlassStyle as Style;
    let menu = |bits: GlassModifier| {
        of_wire(
            GlassVariant::Menu as u32,
            bits.bits(),
            SizeClass::Dynamic as u32,
        )
        .expect("the menu variant")
    };
    let without_shadow = menu(GlassModifier::ExcludeShadow);
    assert_eq!(
        without_shadow.style,
        Some(Style::Menu),
        "draws as the menu style does"
    );
    assert!(without_shadow.popover_outline());
    assert!(menu(GlassModifier::empty()).popover_outline());
    assert!(GlassLook::of_style(Style::Menu).popover_outline());
    assert!(!GlassLook::of_style(Style::Regular).popover_outline());
}

#[test]
fn document_modal_glass_window_glass_casts_the_sheets_shadow_and_other_glass_keeps_its_own() {
    use cupertino_server::glass_look::of_wire;
    use cupertino_theme::glass::GlassShadow;
    use cupertino_theme::GlassParams;
    let look = |variant: GlassVariant, bits: GlassModifier| {
        of_wire(variant as u32, bits.bits(), SizeClass::Dynamic as u32).expect("known bits")
    };
    let sheet = look(GlassVariant::GlassWindow, GlassModifier::DocumentModal);
    assert!(sheet.document_modal);
    // the drop shadow's black in each appearance, while the window is active and while it is not
    for (dark, active, black) in [
        (false, true, 0.6),
        (true, true, 0.7),
        (false, false, 0.45),
        (true, false, 0.55),
    ] {
        let base = cupertino_theme::GlassMode {
            dark,
            ..cupertino_theme::GlassMode::default()
        };
        let mode = sheet.mode(base, active);
        let mut params = GlassParams::new(mode, 200.0);
        let alert = params.shadow;
        sheet.lay_sheet_shadow(&mut params, mode);
        let expected: GlassShadow = alert.sheet(active, dark);
        assert_eq!(params.shadow, expected, "dark {dark}, active {active}");
        assert!(
            (params.shadow.fill[3] - black).abs() < 1e-6,
            "dark {dark}, active {active}: {:?}",
            params.shadow.fill
        );
        assert!(
            params.shadow.fill[3] < alert.fill[3],
            "fainter than the alert's"
        );
    }
    // an alert, a sheet without its own shadow and another variant keep what their recipe casts
    for other in [
        look(GlassVariant::GlassWindow, GlassModifier::empty()),
        look(
            GlassVariant::GlassWindow,
            GlassModifier::DocumentModal | GlassModifier::ExcludeShadow,
        ),
        look(GlassVariant::Menu, GlassModifier::DocumentModal),
    ] {
        let mode = other.mode(cupertino_theme::GlassMode::default(), true);
        let mut params = GlassParams::new(mode, 200.0);
        let before = params.shadow;
        other.lay_sheet_shadow(&mut params, mode);
        assert_eq!(params.shadow, before, "{other:?}");
    }
}
