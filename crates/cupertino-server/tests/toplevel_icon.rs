//! A window that sets its own icon with `xdg_toplevel_icon_v1` (a theme name and a square `wl_shm` buffer) has
//! that icon on its window entry once it commits, against a headless compositor. Tests skip (pass) when no EGL
//! device is available.

mod common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols::xdg::toplevel_icon::v1::client::{
    xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1, xdg_toplevel_icon_v1::XdgToplevelIconV1,
};

const OUTPUT: (i32, i32) = (1000, 700);
const ICON_NAME: &str = "org.example.painter";
const ICON_SIZE: i32 = 32;

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
        let end = Instant::now() + Duration::from_secs(10);
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
}

struct Window {
    surface: wl_surface::WlSurface,
    buffer: wl_buffer::WlBuffer,
    _icon: XdgToplevelIconV1,
    _icon_buffer: wl_buffer::WlBuffer,
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
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
    xdg_toplevel::XdgToplevel,
    XdgToplevelIconManagerV1,
    XdgToplevelIconV1
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
            state.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            state.surface.commit();
        }
    }
}

fn shm_buffer(shm: &wl_shm::WlShm, side: i32, qh: &QueueHandle<Window>) -> wl_buffer::WlBuffer {
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&vec![0xff; (side * side * 4) as usize])
        .unwrap();
    let pool = shm.create_pool(file.as_fd(), side * side * 4, qh, ());
    let buffer = pool.create_buffer(0, side, side, side * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

/// A client whose window sets an icon with a theme name and one `ICON_SIZE` buffer before its first commit.
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
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let icons: XdgToplevelIconManagerV1 = globals
                .bind(&qh, 1..=1, ())
                .expect("xdg_toplevel_icon_manager_v1 is advertised");
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.painter".into());
            toplevel.set_title("Painter".into());
            let icon = icons.create_icon(&qh, ());
            let icon_buffer = shm_buffer(&shm, ICON_SIZE, &qh);
            icon.set_name(ICON_NAME.into());
            icon.add_buffer(&icon_buffer, 1);
            icons.set_icon(&toplevel, Some(&icon));
            surface.commit();
            let mut client = Window {
                buffer: shm_buffer(&shm, 200, &qh),
                surface,
                _icon: icon,
                _icon_buffer: icon_buffer,
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
fn a_window_keeps_the_icon_it_sets_for_itself() {
    let Some(mut h) = Harness::new() else { return };
    let _app = App::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let window = h
        .state
        .windows
        .iter()
        .find(|w| w.mapped)
        .expect("a mapped window");
    assert_eq!(window.icon_name().as_deref(), Some(ICON_NAME));
    let buffers = window.icon_buffers();
    assert_eq!(buffers.len(), 1, "the one icon buffer");
    let (buffer, scale) = &buffers[0];
    let size =
        smithay::wayland::shm::with_buffer_contents(buffer, |_, _, data| (data.width, data.height))
            .expect("an shm buffer");
    assert_eq!((size, *scale), ((ICON_SIZE, ICON_SIZE), 1));
}
