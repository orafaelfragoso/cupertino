//! Wayland clients the tests share: a client with one mapped toplevel and a session locker that covers the output with a
//! lock surface, each on a thread of its own that ends, and so disconnects the client, when the handle is dropped.
#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat, wl_shm,
    wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1, zwp_keyboard_shortcuts_inhibitor_v1,
};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

/// Memory order of an Argb8888 pixel is blue, green, red, alpha.
pub mod desktop;
pub mod overlays;
pub mod shell;
pub mod spawn;

pub const TOPLEVEL_BLUE: [u8; 4] = [255, 0, 0, 255];
pub const LOCK_GREEN: [u8; 4] = [0, 255, 0, 255];

/// Drain pings at the current model time before jumping to an absolute timestamp. Tests of
/// deliberately unresponsive clients must move `fixed_time` directly instead.
pub fn set_time(state: &mut Cupertino, event_loop: &mut EventLoop<'static, Cupertino>, time: f64) {
    set_time_with(state, time, |state| {
        cupertino_server::backend::headless::step(state, event_loop, Duration::from_millis(5))
            .expect("dispatching clients before advancing model time");
    });
}

/// The same clock barrier for clients whose queues the test must dispatch itself.
pub fn set_time_with(state: &mut Cupertino, time: f64, mut dispatch: impl FnMut(&mut Cupertino)) {
    state.fixed_time = Some(state.now());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        dispatch(state);
        if !state.hung.awaiting_pong() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for pongs before advancing model time"
        );
    }
    state.fixed_time = Some(time);
}

/// A headless test may skip without EGL locally, but CI requires a working renderer.
pub fn egl_unavailable(error: impl std::fmt::Display) {
    assert!(
        std::env::var("CUPERTINO_EGL_REQUIRED").as_deref() != Ok("1"),
        "no EGL device and CUPERTINO_EGL_REQUIRED=1: {error}"
    );
    eprintln!("SKIPPED: no EGL device ({error}); set CUPERTINO_EGL_REQUIRED=1 to fail instead");
}

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
    /// Button presses the client's pointer received.
    presses: u32,
    /// The `active` (true) and `inactive` (false) events of the client's keyboard shortcuts inhibitor, in order.
    inhibitor: Vec<bool>,
    /// The keys (evdev codes) the client's keyboard received, with whether they were pressed.
    keys: Vec<(u32, bool)>,
}

struct ClientState {
    shm: wl_shm::WlShm,
    lock_status: Arc<Mutex<LockStatus>>,
    toplevel_size: (i32, i32),
    /// The client does not answer pings (it is hung); a ping that arrives meanwhile waits in `held_ping`.
    silent: Arc<AtomicBool>,
    held_ping: Option<(xdg_wm_base::XdgWmBase, u32)>,
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
    xdg_activation_v1::XdgActivationV1,
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientState {
    fn event(
        state: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            if state.silent.load(Ordering::SeqCst) {
                state.held_ping = Some((base.clone(), serial));
            } else {
                base.pong(serial);
            }
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

impl Dispatch<wl_seat::WlSeat, ()> for ClientState {
    fn event(
        _: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(qh, ());
            }
            if caps.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_pointer::Event::Button {
            state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
            ..
        } = event
        {
            state.lock_status.lock().unwrap().presses += 1;
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: wayland_client::WEnum::Value(key_state),
            ..
        } = event
        {
            state
                .lock_status
                .lock()
                .unwrap()
                .keys
                .push((key, key_state == wl_keyboard::KeyState::Pressed));
        }
    }
}

impl Dispatch<zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1, ()>
    for ClientState
{
    fn event(
        state: &mut Self,
        _: &zwp_keyboard_shortcuts_inhibitor_v1::ZwpKeyboardShortcutsInhibitorV1,
        event: zwp_keyboard_shortcuts_inhibitor_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut status = state.lock_status.lock().unwrap();
        match event {
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Active => status.inhibitor.push(true),
            zwp_keyboard_shortcuts_inhibitor_v1::Event::Inactive => status.inhibitor.push(false),
            _ => {}
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
    silent: Arc<AtomicBool>,
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
        Self::run_over(
            UnixStream::connect(&socket).expect("connecting to the compositor"),
            setup,
        )
    }

    fn run_over(
        stream: UnixStream,
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
        let silent = Arc::new(AtomicBool::new(false));
        let (stop2, unlock2, status2, silent2) = (
            stop.clone(),
            unlock.clone(),
            lock_status.clone(),
            silent.clone(),
        );
        let thread_stream = stream.try_clone().unwrap();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(thread_stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let mut client = ClientState {
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                lock_status: status2,
                toplevel_size: (400, 300),
                silent: silent2.clone(),
                held_ping: None,
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
                // a client that gets going again answers the ping it was too busy for
                if !silent2.load(Ordering::SeqCst) {
                    if let Some((base, serial)) = client.held_ping.take() {
                        base.pong(serial);
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Client {
            stream,
            stop,
            unlock,
            lock_status,
            silent,
        }
    }

    /// Stop answering pings, as a hung program does, or start again (the ping that came meanwhile is answered then).
    pub fn mute(&self, on: bool) {
        self.silent.store(on, Ordering::SeqCst);
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

    /// A client with one mapped toplevel that asks to inhibit the compositor's keyboard shortcuts for it, and records
    /// the `active` / `inactive` events and the keys its keyboard receives.
    pub fn inhibiting(socket: PathBuf, app_id: &'static str) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let manager: zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            let inhibitor = manager.inhibit_shortcuts(&surface, &seat, &qh, ());
            surface.commit();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((wm_base, xdg, toplevel, surface, seat, manager, inhibitor)),
                lock: None,
            }
        })
    }

    /// The `active` (true) and `inactive` (false) events of the shortcuts inhibitor so far.
    pub fn inhibitor_events(&self) -> Vec<bool> {
        self.lock_status.lock().unwrap().inhibitor.clone()
    }

    /// The keys (evdev codes) the keyboard received so far, with whether each was pressed.
    pub fn keys(&self) -> Vec<(u32, bool)> {
        self.lock_status.lock().unwrap().keys.clone()
    }

    /// A client that connects at once and maps one toplevel of `app_id` when `gate` is set, after presenting `token` for its
    /// surface (xdg-activation, before the first commit) when it has one.
    pub fn toplevel_after(
        socket: PathBuf,
        app_id: &'static str,
        gate: Arc<AtomicBool>,
        token: Arc<Mutex<Option<String>>>,
    ) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let activation: xdg_activation_v1::XdgActivationV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            while !gate.load(Ordering::SeqCst) {
                queue.roundtrip(client).unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            if let Some(token) = token.lock().unwrap().take() {
                activation.activate(token, &surface);
            }
            surface.commit();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((wm_base, xdg, toplevel, surface, activation)),
                lock: None,
            }
        })
    }

    /// A client that locks the session and covers the output with a lock surface.
    pub fn locker(stream: UnixStream) -> Client {
        Self::run_over(stream, |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=5, ()).unwrap();
            let lock = manager.lock(&qh, ());
            let surface = compositor.create_surface(&qh, ());
            let lock_surface = lock.get_lock_surface(&surface, &output, &qh, surface.clone());
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((manager, output, seat, surface, lock_surface)),
                lock: Some(lock),
            }
        })
    }

    pub fn locked(&self) -> bool {
        self.lock_status.lock().unwrap().locked
    }

    /// Button presses the client's pointer got (the lock surface has the pointer while the session is locked).
    pub fn presses(&self) -> u32 {
        self.lock_status.lock().unwrap().presses
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

/// The clients of a test are threads of the test process: make each connection stand for a process of its own, as the
/// first-window rule of the focus policy sees them.
/// A connection the server made for the lock screen: the session lock is offered to it alone.
pub fn session_agent(state: &Cupertino) -> UnixStream {
    cupertino_server::shell_connection::shell_process_connection(
        &state.display_handle,
        &[cupertino_server::authorization::ShellRole::SessionLock],
    )
    .unwrap()
}

pub fn each_connection_a_process(state: &mut Cupertino) {
    state.process_of = Box::new(|_, client| {
        let mut hasher = DefaultHasher::new();
        client.id().hash(&mut hasher);
        Some((0, hasher.finish()))
    });
}
