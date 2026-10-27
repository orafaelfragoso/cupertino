//! Wayland clients the tests share: a client with one mapped toplevel and a session locker that covers the output with a
//! lock surface, each on a thread of its own that ends, and so disconnects the client, when the handle is dropped.
#![allow(dead_code)]

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// Memory order of an Argb8888 pixel is blue, green, red, alpha.
pub const TOPLEVEL_BLUE: [u8; 4] = [255, 0, 0, 255];
pub const LOCK_GREEN: [u8; 4] = [0, 255, 0, 255];

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
pub struct Client {
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
    pub fn toplevel(socket: PathBuf) -> Client {
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
    pub fn locker(socket: PathBuf) -> Client {
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

    pub fn locked(&self) -> bool {
        self.lock_status.lock().unwrap().locked
    }

    pub fn finished(&self) -> bool {
        self.lock_status.lock().unwrap().finished
    }

    /// Send `unlock_and_destroy`.
    pub fn unlock(&self) {
        self.unlock.store(true, Ordering::SeqCst);
    }

    /// Close the connection at once, as a crash would.
    pub fn kill(&self) {
        self.stream.shutdown(std::net::Shutdown::Both).unwrap();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}
