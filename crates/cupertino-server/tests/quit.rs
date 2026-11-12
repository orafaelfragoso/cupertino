//! The quit sequence before a log out (`quit.rs`) against a headless compositor: the most recently used application is
//! asked first, an application that closes its toplevel is reported quit, one that ignores the request stays, and with no
//! toplevel left there is nothing to quit. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
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
        common::each_connection_a_process(&mut state);
        match headless::init(&mut state, (800, 600), 1.0) {
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
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(self.state.socket_name.as_ref().unwrap())
    }

    fn mapped(&self) -> usize {
        self.state.windows.iter().filter(|e| e.mapped).count()
    }
}

struct ClientState {
    shm: wl_shm::WlShm,
    /// The server sent `close`.
    asked: Arc<AtomicBool>,
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
    wl_surface::WlSurface
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientState {
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

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if matches!(event, xdg_toplevel::Event::Close) {
            state.asked.store(true, Ordering::SeqCst);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, wl_surface::WlSurface> for ClientState {
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
            let (w, h) = (200, 150);
            let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC)
                .unwrap();
            let mut file = std::fs::File::from(fd);
            file.write_all(&vec![255u8; (w * h * 4) as usize]).unwrap();
            let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
            let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
            pool.destroy();
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, w, h);
            surface.commit();
        }
    }
}

/// A client with one toplevel of `app_id`; with `obeys` it destroys the toplevel when asked to close. Dropping it
/// disconnects it.
struct App {
    stop: Arc<AtomicBool>,
    asked: Arc<AtomicBool>,
}

impl App {
    fn start(socket: PathBuf, app_id: &'static str, obeys: bool) -> App {
        let (stop, asked) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let (stop2, asked2) = (stop.clone(), asked.clone());
        std::thread::spawn(move || {
            let stream = std::os::unix::net::UnixStream::connect(&socket)
                .expect("connecting to the compositor");
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let mut client = ClientState {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                asked: asked2,
            };
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            surface.commit();
            let mut window = Some((toplevel, xdg, surface));
            while !stop2.load(Ordering::SeqCst) {
                if obeys && client.asked.load(Ordering::SeqCst) {
                    if let Some((toplevel, xdg, surface)) = window.take() {
                        toplevel.destroy();
                        xdg.destroy();
                        surface.destroy();
                    }
                }
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            drop(wm_base);
        });
        App { stop, asked }
    }

    fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[test]
fn applications_are_asked_to_quit_one_at_a_time_most_recent_first() {
    let Some(mut h) = Harness::new() else { return };
    let socket = h.socket_path();
    let stubborn = App::start(socket.clone(), "org.example.stubborn", false);
    h.wait_until("the first toplevel", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 1
    });
    let polite = App::start(socket, "org.example.polite", true);
    h.wait_until("the second toplevel", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });

    assert_eq!(
        h.state.quit_next_application(),
        ("org.example.polite".to_string(), 1),
        "the most recently used application first"
    );
    h.wait_until("the polite application to quit", |s| s.quitting().is_none());
    assert!(
        polite.asked() && !stubborn.asked(),
        "only the application being quit is asked"
    );
    assert_eq!(h.mapped(), 1);

    assert_eq!(
        h.state.quit_next_application(),
        ("org.example.stubborn".to_string(), 1)
    );
    h.wait_until("the close request to arrive", |_| stubborn.asked());
    for _ in 0..20 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    }
    assert_eq!(
        h.state.quitting(),
        Some("org.example.stubborn"),
        "an application that ignores the request is still being quit"
    );
    assert_eq!(
        h.state.force_quit_application("org.example.unknown"),
        0,
        "an unknown application has no process to kill"
    );

    drop(stubborn);
    h.wait_until("the stubborn client to go away", |s| s.quitting().is_none());
    assert_eq!(
        h.state.quit_next_application(),
        (String::new(), 0),
        "nothing is left to quit"
    );
}
