//! An ordinary application of the tests: one toplevel of magenta content, on a thread of its own that answers the
//! server until the connection ends.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;

use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// The size of the window's content, in pixels at scale 1.
pub const WINDOW_SIZE: (i32, i32) = (100, 80);

/// One pixel of the window in `wl_shm` ARGB8888 memory order (blue, green, red, alpha): an opaque magenta, which
/// neither the wallpaper nor the test components show.
const MAGENTA: [u8; 4] = [255, 0, 255, 255];

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm_base: Option<xdg_wm_base::XdgWmBase>,
    configured: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => state.compositor = Some(registry.bind(name, 4, qh, ())),
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm_base = Some(registry.bind(name, 1, qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for App {
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

wayland_client::delegate_noop!(App: ignore xdg_toplevel::XdgToplevel);
wayland_client::delegate_noop!(App: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(App: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(App: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(App: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(App: ignore wl_buffer::WlBuffer);

/// Show a window of `app_id` on `stream`, from a thread that answers the server until the connection ends.
pub fn spawn(stream: UnixStream, app_id: &'static str) {
    std::thread::spawn(move || {
        let conn = Connection::from_socket(stream).expect("the client connects");
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut app = App::default();
        let _registry = conn.display().get_registry(&qh, ());
        if queue.roundtrip(&mut app).is_err() {
            return;
        }
        let surface = app.compositor.as_ref().unwrap().create_surface(&qh, ());
        let xdg = app
            .wm_base
            .as_ref()
            .unwrap()
            .get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg.get_toplevel(&qh, ());
        toplevel.set_title("magenta".into());
        toplevel.set_app_id(app_id.into());
        surface.commit();
        while !app.configured {
            if queue.blocking_dispatch(&mut app).is_err() {
                return;
            }
        }
        let (width, height) = WINDOW_SIZE;
        let len = width * height * 4;
        let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC)
            .expect("a memfd");
        let mut file = std::fs::File::from(fd);
        let bytes: Vec<u8> = MAGENTA.iter().copied().cycle().take(len as usize).collect();
        file.write_all(&bytes).unwrap();
        let pool = app
            .shm
            .as_ref()
            .unwrap()
            .create_pool(file.as_fd(), len, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width,
            height,
            width * 4,
            wl_shm::Format::Argb8888,
            &qh,
            (),
        );
        pool.destroy();
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
        while queue.blocking_dispatch(&mut app).is_ok() {}
    });
}
