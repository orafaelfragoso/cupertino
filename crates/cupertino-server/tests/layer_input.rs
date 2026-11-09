//! Pointer hit testing over layer surfaces: the compositor runs in-process on an offscreen EGL device and one client maps
//! two overlay surfaces that overlap. Tests skip (pass) when no EGL device is available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::desktop::{layer_map_for_output, WindowSurfaceType};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

const OUTPUT: (i32, i32) = (1000, 700);
/// A card at the top right that takes the pointer everywhere.
const CARD: (u32, u32) = (300, 200);
/// A full-height strip at the right edge, mapped after the card so it stacks above it, with an empty input region.
const STRIP_WIDTH: u32 = 400;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
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

    /// The namespace of the layer surface the pointer would focus at `(x, y)`.
    fn namespace_under(&self, x: f64, y: f64) -> Option<String> {
        let (surface, _) = self.state.surface_under((x, y).into())?;
        let map = layer_map_for_output(&self.state.outputs[0].output);
        map.layer_for_surface(&surface, WindowSurfaceType::TOPLEVEL)
            .map(|l| l.namespace().to_string())
    }
}

struct ClientState {
    shm: wl_shm::WlShm,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClientState {
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
        impl Dispatch<$t, ()> for ClientState {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_region::WlRegion,
    wl_output::WlOutput,
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);

/// The surface is the user data, so the configure can attach a buffer of the configured size.
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, wl_surface::WlSurface> for ClientState {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        surface: &wl_surface::WlSurface,
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
            let (w, h) = (width as i32, height as i32);
            let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC)
                .unwrap();
            let mut file = std::fs::File::from(fd);
            file.write_all(&vec![128u8; (w * h * 4) as usize]).unwrap();
            let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
            let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
            pool.destroy();
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, w, h);
            surface.commit();
        }
    }
}
impl Dispatch<wl_surface::WlSurface, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Map the card, then the strip above it, on the overlay layer, and keep the connection served.
fn spawn_client(socket: PathBuf) {
    std::thread::spawn(move || {
        let conn = Connection::from_socket(
            UnixStream::connect(&socket).expect("connecting to the compositor"),
        )
        .unwrap();
        let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
            globals.bind(&qh, 1..=4, ()).unwrap();
        let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
        let mut client = ClientState {
            shm: globals.bind(&qh, 1..=1, ()).unwrap(),
        };
        let overlay = |namespace: &str, size: (u32, u32), anchor: zwlr_layer_surface_v1::Anchor| {
            let surface = compositor.create_surface(&qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                Some(&output),
                zwlr_layer_shell_v1::Layer::Overlay,
                namespace.into(),
                &qh,
                surface.clone(),
            );
            layer.set_size(size.0, size.1);
            layer.set_anchor(anchor);
            layer.set_exclusive_zone(-1);
            (surface, layer)
        };
        let (card, _card_layer) = overlay(
            "card",
            CARD,
            zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Right,
        );
        card.commit();
        queue.roundtrip(&mut client).unwrap();
        let (strip, _strip_layer) = overlay(
            "strip",
            (STRIP_WIDTH, 0),
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Bottom
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        let empty = compositor.create_region(&qh, ());
        strip.set_input_region(Some(&empty));
        strip.commit();
        while queue.roundtrip(&mut client).is_ok() {
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

#[test]
fn a_layer_surface_without_input_at_the_point_lets_the_one_below_take_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    spawn_client(h.socket_path());
    h.wait_until("both overlay surfaces to show a buffer", |s| {
        let map = layer_map_for_output(&s.outputs[0].output);
        map.layers().count() == 2 && map.layers().all(|l| !l.bbox_with_popups().is_empty())
    });
    let inside_card = (f64::from(OUTPUT.0) - 100.0, 100.0);
    assert_eq!(
        h.namespace_under(inside_card.0, inside_card.1).as_deref(),
        Some("card"),
        "the strip above has no input region there"
    );
    let strip_only = (f64::from(OUTPUT.0) - 100.0, 500.0);
    assert_eq!(
        h.namespace_under(strip_only.0, strip_only.1),
        None,
        "only the strip covers this point, and it takes no input"
    );
}
