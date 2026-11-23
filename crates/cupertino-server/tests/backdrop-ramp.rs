//! A backdrop's blur ramp (`cupertino_backdrop_v1.set_blur_ramp`, version 22) drawn by the headless compositor: a
//! toolbar band in a subsurface over its window's striped content blurs the stripes under its top and shows them
//! nearly sharp along its bottom, and blurs evenly again once the ramp is even.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use cupertino_assets::Assets;
use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_backdrop_v1::CupertinoBackdropV1;
use cupertino_server::state::{insert_client, ClientState};
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_toplevel, xdg_wm_base,
};
use smithay::reexports::wayland_server::Display;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle};

/// The output, in pixels at scale 1.
const OUTPUT: (i32, i32) = (480, 320);
/// The window's content: vertical stripes of this many pixels each, black and white in turn.
const STRIPE: i32 = 4;
/// The window's size in points.
const WINDOW: (i32, i32) = (240, 160);
/// The band's height: the subsurface over the top of the window that holds the band's backdrop.
const BAND: i32 = 80;
/// The material of the band, as Kit's toolbar row asks for it.
const BAND_MATERIAL: &str = "ToolbarBand";
/// The version of `cupertino_app_effects_v1` whose backdrops take `set_blur_ramp`.
const RAMP_VERSION: u32 = 22;

#[derive(Default)]
struct Client {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    effects: Option<CupertinoAppEffectsV1>,
    configured: bool,
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
                        version >= RAMP_VERSION,
                        "the server offers version {version}"
                    );
                    state.effects = Some(reg.bind(name, RAMP_VERSION, qh, ()));
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
wayland_client::delegate_noop!(Client: ignore CupertinoBackdropV1);

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

/// The headless compositor drawing one output, and one client of it.
struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    state: Cupertino,
    conn: Connection,
    queue: EventQueue<Client>,
    client: Client,
}

impl Rig {
    fn new() -> Rig {
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
        cupertino_server::backend::headless::init(&mut state, OUTPUT, 1.0)
            .expect("an offscreen renderer");
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
        };
        let _registry = rig.conn.display().get_registry(&rig.queue.handle(), ());
        rig.pump(6);
        rig
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
}

/// The spread of the green channel along row `y` of `image` from `x0` for `width` pixels: how much of the stripes
/// shows there, 255 where they are sharp and little where they are blurred.
fn spread(image: &image::RgbaImage, x0: u32, y: u32, width: u32) -> i32 {
    let values: Vec<i32> = (x0..x0 + width)
        .map(|x| i32::from(image.get_pixel(x, y)[1]))
        .collect();
    values.iter().max().unwrap() - values.iter().min().unwrap()
}

#[test]
fn a_ramped_band_blurs_the_content_under_its_top_and_shows_it_nearly_sharp_along_its_bottom() {
    let mut rig = Rig::new();
    let qh = rig.queue.handle();
    let effects = rig
        .client
        .effects
        .clone()
        .expect("the server offers cupertino_app_effects_v1");

    // the window: striped content
    let surface = rig
        .client
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&qh, ());
    let xdg = rig
        .client
        .wm_base
        .as_ref()
        .unwrap()
        .get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg.get_toplevel(&qh, ());
    toplevel.set_title("blur ramp".into());
    toplevel.set_app_id("org.cupertino.test.BlurRamp".into());
    surface.commit();
    for _ in 0..20 {
        rig.pump(1);
        if rig.client.configured {
            break;
        }
    }
    assert!(rig.client.configured, "the window is configured");
    let shm = rig.client.shm.clone().unwrap();
    let stripes = shm_buffer(&shm, &qh, WINDOW, |x| {
        if (x / STRIPE) % 2 == 0 {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    });

    // the band: a clear subsurface over the window's top, its backdrop the toolbar band's material, its blur
    // fading from all of it at the top to none at the bottom
    let band = rig
        .client
        .compositor
        .as_ref()
        .unwrap()
        .create_surface(&qh, ());
    let sub = rig
        .client
        .subcompositor
        .as_ref()
        .unwrap()
        .get_subsurface(&band, &surface, &qh, ());
    sub.set_position(0, 0);
    let clear = shm_buffer(&shm, &qh, (WINDOW.0, BAND), |_| [0, 0, 0, 0]);
    let backdrop = effects.get_backdrop(&band, &qh, ());
    backdrop.set_material(BAND_MATERIAL.into());
    backdrop.set_shape(0, 0, WINDOW.0, BAND, 0.0);
    backdrop.set_blur_ramp(1.0, 0.0);
    band.attach(Some(&clear), 0, 0);
    band.damage_buffer(0, 0, WINDOW.0, BAND);
    band.commit();
    surface.attach(Some(&stripes), 0, 0);
    surface.damage_buffer(0, 0, WINDOW.0, WINDOW.1);
    surface.commit();
    rig.pump(12);

    let id = rig
        .state
        .windows
        .iter()
        .find(|e| e.mapped)
        .map(|e| e.id)
        .expect("the window is mapped");
    let frame = rig.state.wm.window(id).unwrap().geometry();
    let (x, y) = (frame.x.max(0.0) as u32, frame.y.max(0.0) as u32);
    // well inside the window's rounded corners, and away from the band's edges and its blur's reach
    let (x0, width) = (x + 24, WINDOW.0 as u32 - 48);
    let row = |at: i32| y + at as u32;

    let image = rig.image();
    let below = spread(&image, x0, row(BAND + 20), width);
    let top = spread(&image, x0, row(4), width);
    let bottom = spread(&image, x0, row(BAND - 3), width);
    assert!(
        below > 200,
        "the stripes show sharp below the band: {below}"
    );
    assert!(
        bottom > 3 * top,
        "the band blurs its top ({top}) more than its bottom ({bottom})"
    );

    // an even ramp blurs the bottom as much as the top
    backdrop.set_blur_ramp(1.0, 1.0);
    band.commit();
    surface.commit();
    rig.pump(12);
    let image = rig.image();
    let even_bottom = spread(&image, x0, row(BAND - 3), width);
    assert!(
        even_bottom * 3 < bottom,
        "evenly the bottom is blurred too: {even_bottom}, ramped {bottom}"
    );
    drop((backdrop, sub, toplevel, xdg));
}
