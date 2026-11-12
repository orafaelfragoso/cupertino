//! A window that sets its own icon with `xdg_toplevel_icon_v1` (a theme name and a square `wl_shm` buffer) has
//! a copy of that icon on its window entry once it commits: a name that could be a path and a buffer over the size
//! cap are refused, the copy outlives the client's buffer, and the client may destroy the icon and then its
//! buffers before that commit. Against a headless compositor; tests skip (pass)
//! when no EGL device is available.

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::fs::FileExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::toplevel_icon::{WindowIcon, MAX_ICON_BUFFERS, MAX_ICON_PIXELS};
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
/// One ARGB8888 pixel, little-endian: opaque red.
const PIXEL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];

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

/// A `side` × `side` buffer of [`PIXEL`]s and the file behind it.
fn shm_buffer(
    shm: &wl_shm::WlShm,
    side: i32,
    qh: &QueueHandle<Window>,
) -> (wl_buffer::WlBuffer, std::fs::File) {
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    file.write_all(&PIXEL.repeat((side * side) as usize))
        .unwrap();
    let pool = shm.create_pool(file.as_fd(), side * side * 4, qh, ());
    let buffer = pool.create_buffer(0, side, side, side * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    (buffer, file)
}

/// What the client does with its icon objects after `set_icon`.
#[derive(Clone, Copy, PartialEq)]
enum Then {
    Keep,
    /// Clear the buffers' memory once the server has handled the first commit.
    Overwrite,
    /// Destroy the icon, then its buffers, before the first commit, in one batch of requests.
    Destroy,
}

/// A client whose window sets an icon with a theme name and one buffer per side of `sides` before its first
/// commit, then does `then`.
struct App {
    stop: Arc<AtomicBool>,
}

impl App {
    fn spawn(socket: PathBuf, name: &'static str, sides: &'static [i32], then: Then) -> App {
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
            let icon_buffers: Vec<_> = sides.iter().map(|s| shm_buffer(&shm, *s, &qh)).collect();
            icon.set_name(name.into());
            for (b, _) in &icon_buffers {
                icon.add_buffer(b, 1);
            }
            icons.set_icon(&toplevel, Some(&icon));
            if then == Then::Destroy {
                icon.destroy();
                for (b, _) in &icon_buffers {
                    b.destroy();
                }
            }
            surface.commit();
            let mut client = Window {
                buffer: shm_buffer(&shm, 200, &qh).0,
                surface,
            };
            if then == Then::Overwrite {
                queue.roundtrip(&mut client).unwrap();
                for (_, file) in &icon_buffers {
                    file.write_all_at(&vec![0; file.metadata().unwrap().len() as usize], 0)
                        .unwrap();
                }
            }
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

fn mapped_icon(h: &mut Harness) -> Arc<WindowIcon> {
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    h.state
        .windows
        .iter()
        .find(|w| w.mapped)
        .expect("a mapped window")
        .icon()
}

#[test]
fn a_window_keeps_a_copy_of_the_icon_it_sets_for_itself() {
    let Some(mut h) = Harness::new() else { return };
    let _app = App::spawn(h.socket_path(), ICON_NAME, &[16, ICON_SIZE], Then::Keep);
    let icon = mapped_icon(&mut h);
    assert_eq!(icon.name.as_deref(), Some(ICON_NAME));
    let image = icon.image.as_ref().expect("the largest buffer is copied");
    assert_eq!(image.size, ICON_SIZE as u32);
    assert_eq!(image.argb, PIXEL.repeat((ICON_SIZE * ICON_SIZE) as usize));
}

#[test]
fn a_window_that_destroys_its_icon_and_then_the_buffers_before_committing_maps_with_its_icon() {
    let Some(mut h) = Harness::new() else { return };
    let _app = App::spawn(h.socket_path(), ICON_NAME, &[ICON_SIZE], Then::Destroy);
    let icon = mapped_icon(&mut h);
    assert_eq!(icon.name.as_deref(), Some(ICON_NAME));
    assert_eq!(icon.image.as_ref().map(|i| i.size), Some(ICON_SIZE as u32));
}

#[test]
fn the_copy_does_not_follow_the_client_memory_after_the_commit() {
    let Some(mut h) = Harness::new() else { return };
    let _app = App::spawn(h.socket_path(), ICON_NAME, &[ICON_SIZE], Then::Overwrite);
    let icon = mapped_icon(&mut h);
    for _ in 0..20 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    }
    let window = h
        .state
        .windows
        .iter()
        .find(|w| w.mapped)
        .expect("the window stays");
    assert!(Arc::ptr_eq(&window.icon(), &icon));
    assert_eq!(
        icon.image.as_ref().map(|i| i.argb.len()),
        Some((ICON_SIZE * ICON_SIZE * 4) as usize)
    );
}

#[test]
fn a_name_that_could_be_a_path_is_refused() {
    let Some(mut h) = Harness::new() else { return };
    let _app = App::spawn(
        h.socket_path(),
        "../../etc/passwd",
        &[ICON_SIZE],
        Then::Keep,
    );
    let icon = mapped_icon(&mut h);
    assert_eq!(icon.name, None);
    assert!(icon.image.is_some(), "the image is kept");
}

/// One more buffer than the server looks at, of sides 1 to 33, the largest last.
const MANY: [i32; MAX_ICON_BUFFERS + 1] = {
    let mut sides = [0; MAX_ICON_BUFFERS + 1];
    let mut i = 0;
    while i < sides.len() {
        sides[i] = i as i32 + 1;
        i += 1;
    }
    sides
};

#[test]
fn only_the_first_buffers_are_looked_at_and_only_the_largest_of_them_is_copied() {
    let Some(mut h) = Harness::new() else { return };
    let _app = App::spawn(h.socket_path(), ICON_NAME, &MANY, Then::Keep);
    let icon = mapped_icon(&mut h);
    let image = icon.image.as_ref().expect("an icon image is kept");
    assert_eq!(image.size, MAX_ICON_BUFFERS as u32);
}

#[test]
fn a_buffer_over_the_size_cap_is_refused() {
    let Some(mut h) = Harness::new() else { return };
    let over = MAX_ICON_PIXELS + 1;
    let sides: &'static [i32] = Box::leak(Box::new([ICON_SIZE, over]));
    let _app = App::spawn(h.socket_path(), ICON_NAME, sides, Then::Keep);
    let icon = mapped_icon(&mut h);
    assert_eq!(icon.image.as_ref().map(|i| i.size), Some(ICON_SIZE as u32));

    let Some(mut h) = Harness::new() else { return };
    let only_over: &'static [i32] = Box::leak(Box::new([over]));
    let _app = App::spawn(h.socket_path(), ICON_NAME, only_over, Then::Keep);
    assert_eq!(mapped_icon(&mut h).image, None);
}
