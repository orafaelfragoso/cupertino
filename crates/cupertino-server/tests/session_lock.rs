//! Session lock against a headless compositor: a toplevel client and a lock client connect through the socket.
//! While the lock holds, only the lock surface may take the pointer, the keyboard and the pixels. Tests skip
//! (pass) when no EGL device is available.

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);
/// Memory order of an Argb8888 pixel is blue, green, red, alpha.
const TOPLEVEL_BLUE: [u8; 4] = [255, 0, 0, 255];
const LOCK_GREEN: [u8; 4] = [0, 255, 0, 255];

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .ok()?;
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
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

    fn screenshot(&mut self, name: &str) -> image::RgbaImage {
        let path =
            std::env::temp_dir().join(format!("cupertino-test-{}-{name}.png", std::process::id()));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }

    /// A point inside the only toplevel.
    fn toplevel_center(&self) -> (f64, f64) {
        let id = self.state.wm.key_window().expect("a focused toplevel");
        let g = self.state.wm.window(id).unwrap().geometry();
        (g.x + g.w / 2.0, g.y + g.h / 2.0)
    }

    fn surface_under(
        &self,
        p: (f64, f64),
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.surface_under(p.into()).map(|(s, _)| s)
    }

    fn keyboard_focus(
        &self,
    ) -> Option<smithay::reexports::wayland_server::protocol::wl_surface::WlSurface> {
        self.state.seat.get_keyboard().unwrap().current_focus()
    }

    fn center_pixel(&mut self, name: &str) -> [u8; 4] {
        let p = self.toplevel_center();
        self.screenshot(name).get_pixel(p.0 as u32, p.1 as u32).0
    }
}

// ---- clients -------------------------------------------------------------------------------

fn solid_buffer(
    shm: &wl_shm::WlShm,
    size: (i32, i32),
    color: [u8; 4],
    qh: &QueueHandle<ClientState>,
) -> wl_buffer::WlBuffer {
    let (w, h) = size;
    let fd = rustix::fs::memfd_create("cupertino-test", rustix::fs::MemfdFlags::CLOEXEC).unwrap();
    let mut file = std::fs::File::from(fd);
    let row: Vec<u8> = color.iter().copied().cycle().take(w as usize * 4).collect();
    for _ in 0..h {
        file.write_all(&row).unwrap();
    }
    let pool = shm.create_pool(file.as_fd(), w * h * 4, qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
    pool.destroy();
    buffer
}

#[derive(Default)]
struct LockStatus {
    locked: bool,
    finished: bool,
}

struct ClientState {
    shm: wl_shm::WlShm,
    lock_status: Arc<Mutex<LockStatus>>,
    toplevel_size: (i32, i32),
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
    wl_surface::WlSurface,
    wl_output::WlOutput,
    xdg_toplevel::XdgToplevel,
    ext_session_lock_manager_v1::ExtSessionLockManagerV1
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

/// The surface is the user data, so the configure can attach a buffer.
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
            let size = state.toplevel_size;
            surface.attach(
                Some(&solid_buffer(&state.shm, size, TOPLEVEL_BLUE, qh)),
                0,
                0,
            );
            surface.damage_buffer(0, 0, size.0, size.1);
            surface.commit();
        }
    }
}

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut status = state.lock_status.lock().unwrap();
        match event {
            ext_session_lock_v1::Event::Locked => status.locked = true,
            ext_session_lock_v1::Event::Finished => status.finished = true,
            _ => {}
        }
    }
}

/// The surface is the user data, so the configure can attach a buffer of the configured size.
impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, wl_surface::WlSurface>
    for ClientState
{
    fn event(
        state: &mut Self,
        lock_surface: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            lock_surface.ack_configure(serial);
            let size = (width as i32, height as i32);
            surface.attach(Some(&solid_buffer(&state.shm, size, LOCK_GREEN, qh)), 0, 0);
            surface.damage_buffer(0, 0, size.0, size.1);
            surface.commit();
        }
    }
}

/// Keeps a client's protocol objects alive for the life of its thread; the lock object is kept apart because
/// the thread uses it to unlock.
struct Setup {
    objects: Box<dyn std::any::Any>,
    lock: Option<ext_session_lock_v1::ExtSessionLockV1>,
}

/// A client thread; dropping the handle makes the thread end and so disconnects the client.
struct Client {
    /// The client's end of the connection, to cut it without the client's cooperation.
    stream: UnixStream,
    stop: Arc<AtomicBool>,
    unlock: Arc<AtomicBool>,
    lock_status: Arc<Mutex<LockStatus>>,
}

impl Client {
    fn run(
        socket: PathBuf,
        setup: impl FnOnce(
                &mut wayland_client::EventQueue<ClientState>,
                &mut ClientState,
                &wayland_client::globals::GlobalList,
            ) -> Setup
            + Send
            + 'static,
    ) -> Client {
        let (stop, unlock) = (
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        );
        let lock_status = Arc::new(Mutex::new(LockStatus::default()));
        let (stop2, unlock2, status2) = (stop.clone(), unlock.clone(), lock_status.clone());
        let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
        let thread_stream = stream.try_clone().unwrap();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(thread_stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let mut client = ClientState {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                lock_status: status2,
                toplevel_size: (400, 300),
            };
            let Setup {
                objects: _objects,
                mut lock,
            } = setup(&mut queue, &mut client, &globals);
            while !stop2.load(Ordering::SeqCst) {
                if unlock2.swap(false, Ordering::SeqCst) {
                    if let Some(lock) = lock.take() {
                        lock.unlock_and_destroy();
                    }
                }
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Client {
            stream,
            stop,
            unlock,
            lock_status,
        }
    }

    /// A client with one mapped toplevel.
    fn toplevel(socket: PathBuf) -> Client {
        Self::run(socket, |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.locked-out".to_string());
            surface.commit();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((wm_base, xdg, toplevel, surface)),
                lock: None,
            }
        })
    }

    /// A client that locks the session and covers the output with a lock surface.
    fn locker(socket: PathBuf) -> Client {
        Self::run(socket, |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
            let lock = manager.lock(&qh, ());
            let surface = compositor.create_surface(&qh, ());
            let lock_surface = lock.get_lock_surface(&surface, &output, &qh, surface.clone());
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((manager, output, surface, lock_surface)),
                lock: Some(lock),
            }
        })
    }

    fn locked(&self) -> bool {
        self.lock_status.lock().unwrap().locked
    }

    fn finished(&self) -> bool {
        self.lock_status.lock().unwrap().finished
    }

    /// Send `unlock_and_destroy`.
    fn unlock(&self) {
        self.unlock.store(true, Ordering::SeqCst);
    }

    /// Close the connection at once, as a crash would.
    fn kill(&self) {
        self.stream.shutdown(std::net::Shutdown::Both).unwrap();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn mapped_toplevel(h: &mut Harness) -> (Client, (f64, f64)) {
    let client = Client::toplevel(h.socket_path());
    h.wait_until("the toplevel to be mapped", |s| {
        s.wm.windows().count() == 1
            && s.wm
                .key_window()
                .is_some_and(|w| s.entry(w).is_some_and(|e| e.mapped))
    });
    // the opening animation settles
    h.state.fixed_time = Some(h.state.now() + 5.0);
    h.state.tick();
    let center = h.toplevel_center();
    let under = h
        .surface_under(center)
        .expect("the toplevel is under its center");
    assert!(
        h.state.entry_for_surface(&under).is_some(),
        "a toplevel surface is hit before the lock"
    );
    (client, center)
}

#[test]
fn a_lock_covers_the_toplevel_and_gives_the_pointer_and_keyboard_to_the_lock_surface() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    assert_eq!(
        h.center_pixel("before")[..3],
        [0, 0, 255][..],
        "the toplevel shows before the lock"
    );

    let locker = Client::locker(h.socket_path());
    h.wait_until("the session to be locked", |_| locker.locked());

    let under = h
        .surface_under(center)
        .expect("the lock surface is under the pointer");
    assert!(
        h.state.is_lock_surface(&under),
        "the surface under the former toplevel is the lock surface"
    );
    assert!(
        h.keyboard_focus()
            .is_some_and(|s| h.state.is_lock_surface(&s)),
        "the keyboard is on the lock surface"
    );
    assert_eq!(
        h.center_pixel("locked")[..3],
        [0, 255, 0][..],
        "only the lock surface is drawn"
    );
}

#[test]
fn a_lock_client_that_disconnects_leaves_the_session_locked_and_black() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let locker = Client::locker(h.socket_path());
    h.wait_until("the session to be locked", |_| locker.locked());
    drop(locker);

    h.wait_until("the lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    assert!(
        h.state.session_lock.is_active(),
        "the session is still locked"
    );
    assert!(
        h.keyboard_focus().is_none(),
        "no keyboard focus while the lock has no client"
    );
    assert_eq!(
        h.center_pixel("orphaned")[..3],
        [0, 0, 0][..],
        "the output is black"
    );

    let successor = Client::locker(h.socket_path());
    h.wait_until("a new client to take the lock", |_| successor.locked());
    let under = h
        .surface_under(center)
        .expect("the new lock surface is under the pointer");
    assert!(h.state.is_lock_surface(&under));
}

#[test]
fn a_second_lock_request_is_refused_while_the_session_is_locked() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, _) = mapped_toplevel(&mut h);
    let holder = Client::locker(h.socket_path());
    h.wait_until("the session to be locked", |_| holder.locked());
    let intruder = Client::locker(h.socket_path());
    h.wait_until("the second request to be refused", |_| intruder.finished());
    assert!(!intruder.locked());
}

#[test]
fn an_unlock_from_a_refused_lock_does_not_open_a_session_whose_locker_crashed() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let holder = Client::locker(h.socket_path());
    h.wait_until("the session to be locked", |_| holder.locked());
    let intruder = Client::locker(h.socket_path());
    h.wait_until("the second request to be refused", |_| intruder.finished());

    // both reach the compositor in the same dispatch: the intruder's unlock request and the holder's crash
    intruder.unlock();
    std::thread::sleep(Duration::from_millis(100));
    holder.kill();
    h.wait_until("the holder's lock surface to go away", |s| {
        s.surface_under(center.into()).is_none()
    });
    assert!(
        h.state.session_lock.is_active(),
        "the session is still locked"
    );
    assert_eq!(
        h.center_pixel("after the intruder's unlock")[..3],
        [0, 0, 0][..],
        "the output is black"
    );
}

#[test]
fn unlocking_gives_the_toplevel_back_the_pointer_the_keyboard_and_the_screen() {
    let Some(mut h) = Harness::new() else { return };
    let (_toplevel, center) = mapped_toplevel(&mut h);
    let toplevel_surface = h.surface_under(center).unwrap();
    let locker = Client::locker(h.socket_path());
    h.wait_until("the session to be locked", |_| locker.locked());

    locker.unlock();
    h.wait_until("the session to be unlocked", |s| {
        !s.session_lock.is_active()
    });
    assert_eq!(
        h.surface_under(center),
        Some(toplevel_surface.clone()),
        "the toplevel is under the pointer again"
    );
    assert_eq!(
        h.keyboard_focus(),
        Some(toplevel_surface),
        "the toplevel has the keyboard again"
    );
    assert_eq!(
        h.center_pixel("unlocked")[..3],
        [0, 0, 255][..],
        "the toplevel is drawn again"
    );
}
