//! Clients that map a popup over a window and a layer-shell surface, to compose what ordinary toplevels do not.
#![allow(dead_code)]

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// Popup rectangle in the window surface's coordinates.
pub const POPUP: (i32, i32, i32, i32) = (60, 40, 120, 70);
const WINDOW: (i32, i32) = (400, 260);

/// Stops the client's thread when dropped.
pub struct Handle(Arc<AtomicBool>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct State {
    shm: wl_shm::WlShm,
    popup_surface: Option<wl_surface::WlSurface>,
    popup_xdg: Option<xdg_surface::XdgSurface>,
}

impl State {
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

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
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
        impl Dispatch<$t, ()> for State {
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
    wl_output::WlOutput,
    xdg_toplevel::XdgToplevel,
    xdg_positioner::XdgPositioner,
    xdg_popup::XdgPopup,
    zwlr_layer_shell_v1::ZwlrLayerShellV1
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for State {
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

/// The surface is the user data; the window and its popup are drawn on every configure.
impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for State {
    fn event(
        state: &mut Self,
        xdg: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let xdg_surface::Event::Configure { serial } = event else {
            return;
        };
        xdg.ack_configure(serial);
        if state.popup_xdg.as_ref() == Some(xdg) {
            state.draw(surface, (POPUP.2, POPUP.3), 160, qh);
        } else {
            state.draw(surface, WINDOW, 255, qh);
        }
    }
}

/// The surface is the user data, so the configure can attach a buffer of the configured size.
impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, wl_surface::WlSurface> for State {
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
            state.draw(surface, (width as i32, height as i32), 128, qh);
        }
    }
}

fn connect(
    socket: PathBuf,
) -> (
    Connection,
    wayland_client::globals::GlobalList,
    wayland_client::EventQueue<State>,
) {
    let conn =
        Connection::from_socket(UnixStream::connect(socket).expect("connecting to the compositor"))
            .unwrap();
    let (globals, queue) = registry_queue_init::<State>(&conn).unwrap();
    (conn, globals, queue)
}

/// A window with a popup over it, mapped without a grab.
pub fn popup_window(socket: PathBuf) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        let (_conn, globals, mut queue) = connect(socket);
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
        let window = compositor.create_surface(&qh, ());
        let window_xdg = wm_base.get_xdg_surface(&window, &qh, window.clone());
        let toplevel = window_xdg.get_toplevel(&qh, ());
        toplevel.set_app_id("org.example.popup".into());
        toplevel.set_title("Window".into());
        window.commit();
        let mut client = State {
            shm: globals.bind(&qh, 1..=1, ()).unwrap(),
            popup_surface: None,
            popup_xdg: None,
        };
        let mut opened = false;
        while !flag.load(Ordering::SeqCst) {
            if queue.roundtrip(&mut client).is_err() {
                break;
            }
            if !opened {
                opened = true;
                let (x, y, w, h) = POPUP;
                let positioner = wm_base.create_positioner(&qh, ());
                positioner.set_size(w, h);
                positioner.set_anchor_rect(x, y, w, h);
                positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
                positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
                let surface = compositor.create_surface(&qh, ());
                let popup_xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
                let _popup = popup_xdg.get_popup(Some(&window_xdg), &positioner, &qh, ());
                positioner.destroy();
                surface.commit();
                client.popup_surface = Some(surface);
                client.popup_xdg = Some(popup_xdg);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    Handle(stop)
}

/// A `size` surface on the overlay layer anchored to the top left, with `margin` points from the corner.
pub fn layer_surface(socket: PathBuf, size: (u32, u32), margin: i32) -> Handle {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        let (_conn, globals, mut queue) = connect(socket);
        let qh = queue.handle();
        let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
        let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
            globals.bind(&qh, 1..=4, ()).unwrap();
        let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
        let mut client = State {
            shm: globals.bind(&qh, 1..=1, ()).unwrap(),
            popup_surface: None,
            popup_xdg: None,
        };
        let surface = compositor.create_surface(&qh, ());
        let layer = layer_shell.get_layer_surface(
            &surface,
            Some(&output),
            zwlr_layer_shell_v1::Layer::Overlay,
            "parity".into(),
            &qh,
            surface.clone(),
        );
        layer.set_size(size.0, size.1);
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left);
        layer.set_margin(margin, 0, 0, margin);
        layer.set_exclusive_zone(-1);
        surface.commit();
        while !flag.load(Ordering::SeqCst) && queue.roundtrip(&mut client).is_ok() {
            std::thread::sleep(Duration::from_millis(10));
        }
    });
    Handle(stop)
}
