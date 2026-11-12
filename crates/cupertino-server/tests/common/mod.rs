//! Wayland clients the tests share: a client with one mapped toplevel and a session locker that covers the output with a
//! lock surface, each on a thread of its own that ends, and so disconnects the client, when the handle is dropped.

pub mod fixtures {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../ci/test-fixtures.rs"
    ));
}

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_protocols::client::app_effects::cupertino_app_effects_v1::CupertinoAppEffectsV1;
use cupertino_protocols::client::app_effects::cupertino_appearance_v1::{
    self, CupertinoAppearanceV1,
};
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region,
    wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_subcompositor, wl_subsurface, wl_surface,
};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
};
use wayland_protocols::wp::keyboard_shortcuts_inhibit::zv1::client::{
    zwp_keyboard_shortcuts_inhibit_manager_v1, zwp_keyboard_shortcuts_inhibitor_v1,
};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_confined_pointer_v1, zwp_locked_pointer_v1, zwp_pointer_constraints_v1,
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1, zwp_relative_pointer_v1,
};
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1;
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// Memory order of an Argb8888 pixel is blue, green, red, alpha.
pub mod absolute;
pub mod desktop;
pub mod events;
pub mod overlays;
pub mod shell;
pub mod spawn;

/// How long a test waits for what it waits on before it calls it a hang. The waits are for events (a frame
/// presented, a state the server reached), never for time to pass, so the limit only ends a test that would otherwise
/// hang, and it is generous enough that a loaded machine never reaches it.
pub const HANG_GUARD: Duration = Duration::from_secs(60);

/// The roundtrips a client thread has finished, as a barrier for the test that serves it: [`Rounds::barrier`] holds
/// once the server has handled the client's answer to every event sent before the barrier began, so a test asserts
/// what did not happen only after it. The server runs on the test's thread, so what it sent before the barrier is on
/// the client's socket then: it arrives within the second roundtrip after that, what the client sends in answer goes
/// out before the sync of the third, and the server handles it before answering that sync.
#[derive(Clone, Default)]
pub struct Rounds(Arc<(AtomicUsize, AtomicBool)>);

impl Rounds {
    /// The client thread finished a roundtrip.
    pub fn tick(&self) {
        self.0 .0.fetch_add(1, Ordering::SeqCst);
    }

    /// Marks the client thread ended when dropped, however it ends, so a barrier does not wait on it.
    pub fn ended_on_drop(&self) -> RoundsEnd {
        RoundsEnd(self.clone())
    }

    /// The condition that the client has finished three roundtrips more than it has now, or ended.
    pub fn barrier(&self) -> impl Fn() -> bool + 'static {
        let target = self.0 .0.load(Ordering::SeqCst) + 3;
        let rounds = self.clone();
        move || rounds.0 .1.load(Ordering::SeqCst) || rounds.0 .0.load(Ordering::SeqCst) >= target
    }
}

/// See [`Rounds::ended_on_drop`].
pub struct RoundsEnd(Rounds);

impl Drop for RoundsEnd {
    fn drop(&mut self) {
        self.0 .0 .1.store(true, Ordering::SeqCst);
    }
}

/// A barrier for a client the test does not run, a real application: the server pings it (`xdg_wm_base.ping`) and
/// waits for the pong, three times in a row. A client answers a ping once it has dispatched every event sent before
/// it, and a ping sent after a pong reaches it only once it is back waiting on its connection, so what it did with
/// the events it had before the barrier, its requests and the frames it drew, is on the server by the last pong. A
/// ping already out (the server's own, for hung applications) is waited out first. Ends early if the client goes.
pub fn ping_barrier(
    state: &mut Cupertino,
    client: &smithay::wayland::shell::xdg::ShellClient,
    mut step: impl FnMut(&mut Cupertino),
) {
    use smithay::wayland::shell::PingError;
    let deadline = Instant::now() + HANG_GUARD;
    let ping = || client.send_ping(smithay::utils::SERIAL_COUNTER.next_serial());
    // a ping of the barrier's own goes out once nothing is pending
    loop {
        match ping() {
            Ok(()) => break,
            Err(PingError::PingAlreadyPending(_)) => {}
            Err(PingError::DeadSurface) => return,
        }
        assert!(Instant::now() < deadline, "timed out waiting for a pong");
        step(state);
    }
    // each pong lets the next ping out; the one sent after the third pong is left for the client to answer
    let mut pongs = 0;
    while pongs < 3 {
        assert!(Instant::now() < deadline, "timed out waiting for a pong");
        step(state);
        match ping() {
            Ok(()) => pongs += 1,
            Err(PingError::PingAlreadyPending(_)) => {}
            Err(PingError::DeadSurface) => return,
        }
    }
}

pub const TOPLEVEL_BLUE: [u8; 4] = [255, 0, 0, 255];
pub const LOCK_GREEN: [u8; 4] = [0, 255, 0, 255];
/// The colour [`Client::recolouring`] changes to, in the same byte order as [`TOPLEVEL_BLUE`].
pub const TOPLEVEL_RED: [u8; 4] = [0, 0, 255, 255];

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
    let deadline = Instant::now() + HANG_GUARD;
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

/// Let the clients of this test program use data control, as the person's `data-control.conf` would: the program's file
/// is pinned as it is now.
pub fn allow_data_control(state: &mut Cupertino) {
    let exe = std::env::current_exe().expect("the test program");
    let entry = cupertino_defaults::data_control::Entry::for_program(&exe).expect("an entry");
    state.set_data_control_allowed(vec![entry]);
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
    /// The `locked` / `confined` (true) and `unlocked` / `unconfined` (false) events of the pointer constraint.
    constraint: Vec<bool>,
    /// The surface-local positions of the pointer's motion events.
    motions: Vec<(f64, f64)>,
    /// The unaccelerated deltas of the relative pointer's motion events.
    relative: Vec<(f64, f64)>,
    /// The session lock's `locked` and the appearance observer's `session_lock` events, in order.
    lock_events: Vec<String>,
    /// The bits of the appearance observer's `display_options` events, in order.
    display_options: Vec<u32>,
    /// Whether each configure of the client's toplevel had the `activated` state, in order.
    activations: Vec<bool>,
}

struct ClientState {
    /// The seat's pointer, once the seat announced one.
    pointer: Option<wl_pointer::WlPointer>,
    shm: wl_shm::WlShm,
    lock_status: Arc<Mutex<LockStatus>>,
    toplevel_size: (i32, i32),
    /// The client does not answer pings (it is hung); a ping that arrives meanwhile waits in `held_ping`.
    silent: Arc<AtomicBool>,
    held_ping: Option<(xdg_wm_base::XdgWmBase, u32)>,
    /// The roundtrips the thread finished, setup loops included.
    rounds: Rounds,
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
    xdg_activation_v1::XdgActivationV1,
    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
    zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1,
    zwp_pointer_constraints_v1::ZwpPointerConstraintsV1,
    zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    wl_region::WlRegion,
    wl_subcompositor::WlSubcompositor,
    wl_subsurface::WlSubsurface,
    zwlr_layer_shell_v1::ZwlrLayerShellV1,
    CupertinoAppEffectsV1
);

impl Dispatch<CupertinoAppearanceV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &CupertinoAppearanceV1,
        event: cupertino_appearance_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut status = state.lock_status.lock().unwrap();
        match event {
            cupertino_appearance_v1::Event::SessionLock { locked } => {
                status.lock_events.push(format!("session_lock {locked}"));
            }
            cupertino_appearance_v1::Event::DisplayOptions { options } => {
                status.display_options.push(options.into());
            }
            _ => {}
        }
    }
}

/// `xdg_toplevel.configure`: whether it is activated is recorded.
impl Dispatch<xdg_toplevel::XdgToplevel, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_toplevel::Event::Configure { states, .. } = event {
            let activated = states
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_ne_bytes(*c))
                .any(|s| s == xdg_toplevel::State::Activated as u32);
            state
                .lock_status
                .lock()
                .unwrap()
                .activations
                .push(activated);
        }
    }
}

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
            ext_session_lock_v1::Event::Locked => {
                status.locked = true;
                status.lock_events.push("locked".into());
            }
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
        state: &mut Self,
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
                state.pointer = Some(seat.get_pointer(qh, ()));
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
        match event {
            wl_pointer::Event::Button {
                state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => state.lock_status.lock().unwrap().presses += 1,
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state
                .lock_status
                .lock()
                .unwrap()
                .motions
                .push((surface_x, surface_y)),
            _ => {}
        }
    }
}

impl Dispatch<zwp_relative_pointer_v1::ZwpRelativePointerV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_relative_pointer_v1::ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_relative_pointer_v1::Event::RelativeMotion {
            dx_unaccel,
            dy_unaccel,
            ..
        } = event
        {
            state
                .lock_status
                .lock()
                .unwrap()
                .relative
                .push((dx_unaccel, dy_unaccel));
        }
    }
}

impl Dispatch<zwp_locked_pointer_v1::ZwpLockedPointerV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_locked_pointer_v1::ZwpLockedPointerV1,
        event: zwp_locked_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut status = state.lock_status.lock().unwrap();
        match event {
            zwp_locked_pointer_v1::Event::Locked => status.constraint.push(true),
            zwp_locked_pointer_v1::Event::Unlocked => status.constraint.push(false),
            _ => {}
        }
    }
}

impl Dispatch<zwp_confined_pointer_v1::ZwpConfinedPointerV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &zwp_confined_pointer_v1::ZwpConfinedPointerV1,
        event: zwp_confined_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut status = state.lock_status.lock().unwrap();
        match event {
            zwp_confined_pointer_v1::Event::Confined => status.constraint.push(true),
            zwp_confined_pointer_v1::Event::Unconfined => status.constraint.push(false),
            _ => {}
        }
    }
}

/// The surface is the user data, so the configure can attach a buffer.
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
            let size = (width as i32, height as i32);
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

/// The reply to the `wl_display.sync` of [`roundtrip_while`].
impl Dispatch<wl_callback::WlCallback, Arc<AtomicBool>> for ClientState {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        done: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            done.store(true, Ordering::SeqCst);
        }
    }
}

/// A roundtrip (`wl_display.sync`, then every event up to its reply) that calls `waiting` again and again while the
/// reply is out. The server answers only when the test dispatches it, so a client thread blocked in a plain roundtrip
/// could not do what the test asks of it while the test holds the server still.
fn roundtrip_while(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<ClientState>,
    client: &mut ClientState,
    mut waiting: impl FnMut(),
) -> Result<(), ()> {
    let done = Arc::new(AtomicBool::new(false));
    conn.display().sync(&queue.handle(), done.clone());
    loop {
        waiting();
        queue.flush().map_err(drop)?;
        queue.dispatch_pending(client).map_err(drop)?;
        if done.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut readable = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialised pollfd that lives through the call.
        if unsafe { libc::poll(&mut readable, 1, 5) } > 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(()),
            }
        }
    }
}

/// The pointer constraint a client asks for: a lock, or a confine to a surface-local rectangle.
#[derive(Clone, Copy)]
pub enum Constraint {
    Lock,
    Confine { x: i32, y: i32, w: i32, h: i32 },
}

/// Keeps a client's protocol objects alive for the life of its thread; the lock object is kept apart because
/// the thread uses it to unlock, and so is what `Client::unlock` does besides.
struct Setup {
    objects: Box<dyn std::any::Any>,
    lock: Option<ext_session_lock_v1::ExtSessionLockV1>,
    release: Option<Box<dyn FnOnce()>>,
}

/// A client thread; dropping the handle makes the thread end and so disconnects the client.
pub struct Client {
    /// The client's end of the connection, to cut it without the client's cooperation.
    stream: UnixStream,
    stop: Arc<AtomicBool>,
    unlock: Arc<AtomicBool>,
    lock_status: Arc<Mutex<LockStatus>>,
    silent: Arc<AtomicBool>,
    rounds: Rounds,
    /// Set once the unlock [`Client::unlock`] asked for is on the connection.
    unlock_sent: Arc<AtomicBool>,
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
        let rounds = Rounds::default();
        let unlock_sent = Arc::new(AtomicBool::new(false));
        let (stop2, unlock2, status2, silent2, rounds2, sent2) = (
            stop.clone(),
            unlock.clone(),
            lock_status.clone(),
            silent.clone(),
            rounds.clone(),
            unlock_sent.clone(),
        );
        let thread_stream = stream.try_clone().unwrap();
        std::thread::spawn(move || {
            let _ended = rounds2.ended_on_drop();
            let conn = Connection::from_socket(thread_stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let mut client = ClientState {
                pointer: None,
                shm: globals.bind(&qh, 1..=1, ()).unwrap(),
                lock_status: status2,
                toplevel_size: (400, 300),
                silent: silent2.clone(),
                held_ping: None,
                rounds: rounds2.clone(),
            };
            let Setup {
                objects: _objects,
                mut lock,
                mut release,
            } = setup(&mut queue, &mut client, &globals);
            // What `Client::unlock` asks for. The thread looks for it while a roundtrip waits for the server too: a test
            // that asks and then holds the server still gets its request written all the same.
            let mut unlock_when_asked = || {
                if unlock2.swap(false, Ordering::SeqCst) {
                    if let Some(lock) = lock.take() {
                        lock.unlock_and_destroy();
                        if conn.flush().is_ok() {
                            sent2.store(true, Ordering::SeqCst);
                        }
                    }
                    if let Some(release) = release.take() {
                        release();
                    }
                }
            };
            while !stop2.load(Ordering::SeqCst) {
                if roundtrip_while(&conn, &mut queue, &mut client, &mut unlock_when_asked).is_err()
                {
                    break;
                }
                client.rounds.tick();
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
            rounds,
            unlock_sent,
        }
    }

    /// The client's [`Rounds::barrier`]: true once it has every event sent before, and the server handled its answers.
    pub fn barrier(&self) -> impl Fn() -> bool + 'static {
        self.rounds.barrier()
    }

    /// The client's roundtrips, for a harness that keeps a barrier over all its clients.
    pub fn rounds(&self) -> Rounds {
        self.rounds.clone()
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
                release: None,
            }
        })
    }

    /// A client with one mapped blue toplevel that, when [`Client::unlock`] asks, attaches a new red buffer of the same
    /// size, with the whole buffer damaged when `damaged`, else with no damage at all.
    pub fn recolouring(socket: PathBuf, damaged: bool) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.recolouring".to_string());
            surface.commit();
            queue.roundtrip(client).unwrap();
            let (shm, size, target) = (client.shm.clone(), client.toplevel_size, surface.clone());
            let release: Box<dyn FnOnce()> = Box::new(move || {
                target.attach(Some(&solid_buffer(&shm, size, TOPLEVEL_RED, &qh)), 0, 0);
                if damaged {
                    target.damage_buffer(0, 0, size.0, size.1);
                }
                target.commit();
            });
            Setup {
                objects: Box::new((wm_base, xdg, toplevel, surface)),
                lock: None,
                release: Some(release),
            }
        })
    }

    /// A client with one mapped toplevel that asks for decorations in `mode` (`xdg-decoration`) before its first commit:
    /// server-side ones the compositor draws, or client-side ones it says it draws itself.
    pub fn decorated(socket: PathBuf, mode: zxdg_toplevel_decoration_v1::Mode) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let manager: zxdg_decoration_manager_v1::ZxdgDecorationManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id("org.example.decorated".to_string());
            let decoration = manager.get_toplevel_decoration(&toplevel, &qh, ());
            decoration.set_mode(mode);
            surface.commit();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((wm_base, manager, decoration, xdg, toplevel, surface)),
                lock: None,
                release: None,
            }
        })
    }

    /// A client with one mapped toplevel of `app_id` on the seat: it records the keys its keyboard receives and the
    /// motions and button presses of its pointer.
    pub fn listening(socket: PathBuf, app_id: &'static str) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            surface.commit();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((wm_base, xdg, toplevel, surface, seat)),
                lock: None,
                release: None,
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
                release: None,
            }
        })
    }

    /// A client with two mapped toplevels of `app_id`, each asking to inhibit the compositor's keyboard shortcuts.
    pub fn inhibiting_twice(socket: PathBuf, app_id: &'static str) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let manager: zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let windows: Vec<_> = (0..2)
                .map(|_| {
                    let surface = compositor.create_surface(&qh, ());
                    let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
                    let toplevel = xdg.get_toplevel(&qh, ());
                    toplevel.set_app_id(app_id.to_string());
                    let inhibitor = manager.inhibit_shortcuts(&surface, &seat, &qh, ());
                    surface.commit();
                    (surface, xdg, toplevel, inhibitor)
                })
                .collect();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((wm_base, seat, manager, windows)),
                lock: None,
                release: None,
            }
        })
    }

    /// A client whose only surface is a layer surface (overlay, exclusive keyboard) that asks to inhibit the
    /// compositor's keyboard shortcuts for it.
    pub fn inhibiting_layer(socket: PathBuf) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let manager: zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                None,
                zwlr_layer_shell_v1::Layer::Overlay,
                "inhibiting".to_string(),
                &qh,
                surface.clone(),
            );
            layer.set_size(120, 80);
            layer.set_keyboard_interactivity(
                zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive,
            );
            let inhibitor = manager.inhibit_shortcuts(&surface, &seat, &qh, ());
            surface.commit();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((layer_shell, layer, surface, seat, manager, inhibitor)),
                lock: None,
                release: None,
            }
        })
    }

    /// A client with one mapped toplevel that asks for a persistent pointer `constraint` on it, and also to inhibit the
    /// shortcuts when `inhibit`. It records the constraint's events, the pointer's motion and its relative motion.
    pub fn constraining(
        socket: PathBuf,
        app_id: &'static str,
        constraint: Constraint,
        inhibit: bool,
    ) -> Client {
        Self::constraining_with(socket, app_id, constraint, inhibit, false)
    }

    /// As [`Client::constraining`] with a pointer lock, its toplevel wholly covered by a subsurface, as a game or a
    /// video drawn in one is: the pointer is over the subsurface, never over the surface with the lock.
    pub fn constraining_under_subsurface(socket: PathBuf, app_id: &'static str) -> Client {
        Self::constraining_with(socket, app_id, Constraint::Lock, false, true)
    }

    fn constraining_with(
        socket: PathBuf,
        app_id: &'static str,
        constraint: Constraint,
        inhibit: bool,
        covered: bool,
    ) -> Client {
        Self::run(socket, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let constraints: zwp_pointer_constraints_v1::ZwpPointerConstraintsV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let relative_manager: zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            queue.roundtrip(client).unwrap();
            let pointer = client.pointer.clone().expect("the seat has a pointer");
            let relative = relative_manager.get_relative_pointer(&pointer, &qh, ());
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, surface.clone());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            let inhibitor = inhibit.then(|| {
                let manager: zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1 =
                    globals.bind(&qh, 1..=1, ()).unwrap();
                (manager.inhibit_shortcuts(&surface, &seat, &qh, ()), manager)
            });
            let cover = covered.then(|| {
                let subcompositor: wl_subcompositor::WlSubcompositor =
                    globals.bind(&qh, 1..=1, ()).unwrap();
                let child = compositor.create_surface(&qh, ());
                let subsurface = subcompositor.get_subsurface(&child, &surface, &qh, ());
                let size = client.toplevel_size;
                child.attach(
                    Some(&solid_buffer(&client.shm, size, TOPLEVEL_BLUE, &qh)),
                    0,
                    0,
                );
                child.damage_buffer(0, 0, size.0, size.1);
                child.commit();
                (subcompositor, subsurface, child)
            });
            let lifetime = zwp_pointer_constraints_v1::Lifetime::Persistent;
            let (held, release): (Box<dyn std::any::Any>, Box<dyn FnOnce()>) = match constraint {
                Constraint::Lock => {
                    let locked =
                        constraints.lock_pointer(&surface, &pointer, None, lifetime, &qh, ());
                    let destroy = locked.clone();
                    (Box::new(locked), Box::new(move || destroy.destroy()))
                }
                Constraint::Confine { x, y, w, h } => {
                    let region = compositor.create_region(&qh, ());
                    region.add(x, y, w, h);
                    let confined = constraints.confine_pointer(
                        &surface,
                        &pointer,
                        Some(&region),
                        lifetime,
                        &qh,
                        (),
                    );
                    let destroy = confined.clone();
                    (
                        Box::new((confined, region)),
                        Box::new(move || destroy.destroy()),
                    )
                }
            };
            surface.commit();
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new((
                    wm_base,
                    xdg,
                    toplevel,
                    surface,
                    seat,
                    constraints,
                    relative_manager,
                    relative,
                    inhibitor,
                    held,
                    cover,
                )),
                lock: None,
                release: Some(release),
            }
        })
    }

    /// The `locked` / `confined` (true) and `unlocked` / `unconfined` (false) events of the pointer constraint so far.
    pub fn constraint_events(&self) -> Vec<bool> {
        self.lock_status.lock().unwrap().constraint.clone()
    }

    /// The surface-local positions of the pointer's motion events so far.
    pub fn motions(&self) -> Vec<(f64, f64)> {
        self.lock_status.lock().unwrap().motions.clone()
    }

    /// The unaccelerated deltas of the relative motion events so far.
    pub fn relative_motions(&self) -> Vec<(f64, f64)> {
        self.lock_status.lock().unwrap().relative.clone()
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
                client.rounds.tick();
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
                release: None,
            }
        })
    }

    /// A client that locks the session and covers the output with a lock surface.
    pub fn locker(stream: UnixStream) -> Client {
        Self::locking(stream, false)
    }

    /// A [`Self::locker`] that observes the session's appearance at version 5 before it locks, so the order of what it
    /// is told shows in [`Self::lock_events`].
    pub fn observing_locker(stream: UnixStream) -> Client {
        Self::locking(stream, true)
    }

    /// A lock client that asks for the lock and never presents anything, as one that hung right after it asked: it
    /// makes no lock surface, so the lock stays pending until the compositor stops waiting for one.
    pub fn hung_locker(stream: UnixStream) -> Client {
        Self::run_over(stream, |queue, client, globals| {
            let qh = queue.handle();
            let manager: ext_session_lock_manager_v1::ExtSessionLockManagerV1 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let lock = manager.lock(&qh, ());
            queue.roundtrip(client).unwrap();
            Setup {
                objects: Box::new(manager),
                lock: Some(lock),
                release: None,
            }
        })
    }

    /// Whether each configure of the client's toplevel had the `activated` state, in order.
    pub fn activations(&self) -> Vec<bool> {
        self.lock_status.lock().unwrap().activations.clone()
    }

    fn locking(stream: UnixStream, observe: bool) -> Client {
        Self::run_over(stream, move |queue, client, globals| {
            let qh = queue.handle();
            let observer = observe.then(|| {
                let effects: CupertinoAppEffectsV1 = globals.bind(&qh, 5..=5, ()).unwrap();
                let observer = effects.get_appearance(&qh, ());
                (effects, observer)
            });
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
                objects: Box::new((manager, output, seat, surface, lock_surface, observer)),
                lock: Some(lock),
                release: None,
            }
        })
    }

    /// The authentication prompt over `stream` (a connection with the Prompt role, [`prompt_agent`]): an overlay layer
    /// surface of 120 x 80 in the top left corner of the output, drawn [`TOPLEVEL_BLUE`]. [`Client::unlock`] unmaps it.
    pub fn prompt(stream: UnixStream) -> Client {
        Self::overlay(
            stream,
            "prompt",
            (120, 80),
            zwlr_layer_surface_v1::KeyboardInteractivity::None,
        )
    }

    /// An overlay layer surface of `size` in the top left corner of the output over `stream`, named `namespace`, which
    /// takes the keyboard as `keyboard` says.
    pub fn overlay(
        stream: UnixStream,
        namespace: &'static str,
        size: (u32, u32),
        keyboard: zwlr_layer_surface_v1::KeyboardInteractivity,
    ) -> Client {
        let anchor = zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Left;
        Self::layer(
            stream,
            zwlr_layer_shell_v1::Layer::Overlay,
            namespace,
            size,
            anchor,
            keyboard,
        )
    }

    /// The outside-click catcher of a shell panel: a top layer surface over the whole output that ignores the exclusive
    /// zones of others, over `stream`.
    pub fn catcher(stream: UnixStream) -> Client {
        Self::layer(
            stream,
            zwlr_layer_shell_v1::Layer::Top,
            "catcher",
            (0, 0),
            zwlr_layer_surface_v1::Anchor::all(),
            zwlr_layer_surface_v1::KeyboardInteractivity::None,
        )
    }

    /// A layer surface of `size` on `level` over `stream`, named `namespace`, anchored to `anchor`, which takes the
    /// keyboard as `keyboard` says.
    pub fn layer(
        stream: UnixStream,
        level: zwlr_layer_shell_v1::Layer,
        namespace: &'static str,
        size: (u32, u32),
        anchor: zwlr_layer_surface_v1::Anchor,
        keyboard: zwlr_layer_surface_v1::KeyboardInteractivity,
    ) -> Client {
        Self::run_over(stream, move |queue, client, globals| {
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1 =
                globals.bind(&qh, 1..=4, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let layer = layer_shell.get_layer_surface(
                &surface,
                None,
                level,
                namespace.to_string(),
                &qh,
                surface.clone(),
            );
            layer.set_size(size.0, size.1);
            layer.set_anchor(anchor);
            if anchor == zwlr_layer_surface_v1::Anchor::all() {
                layer.set_exclusive_zone(-1);
            }
            layer.set_keyboard_interactivity(keyboard);
            surface.commit();
            queue.roundtrip(client).unwrap();
            let unmapped = surface.clone();
            Setup {
                objects: Box::new((layer_shell, layer, surface)),
                lock: None,
                release: Some(Box::new(move || {
                    unmapped.attach(None, 0, 0);
                    unmapped.commit();
                })),
            }
        })
    }

    /// A client that observes the session's appearance through `cupertino_app_effects_v1` bound at `version`.
    pub fn appearance_observer(socket: PathBuf, version: u32) -> Client {
        Self::run(socket, move |queue, _, globals| {
            let qh = queue.handle();
            let effects: CupertinoAppEffectsV1 = globals.bind(&qh, version..=version, ()).unwrap();
            let observer = effects.get_appearance(&qh, ());
            Setup {
                objects: Box::new((effects, observer)),
                lock: None,
                release: None,
            }
        })
    }

    /// The session lock's `locked` and the appearance observer's `session_lock` events the client got, in order.
    pub fn lock_events(&self) -> Vec<String> {
        self.lock_status.lock().unwrap().lock_events.clone()
    }

    pub fn locked(&self) -> bool {
        self.lock_status.lock().unwrap().locked
    }

    /// The bits of the appearance observer's `display_options` events the client got, in order.
    pub fn display_options(&self) -> Vec<u32> {
        self.lock_status.lock().unwrap().display_options.clone()
    }

    /// Button presses the client's pointer got (the lock surface has the pointer while the session is locked).
    pub fn presses(&self) -> u32 {
        self.lock_status.lock().unwrap().presses
    }

    pub fn finished(&self) -> bool {
        self.lock_status.lock().unwrap().finished
    }

    /// Send `unlock_and_destroy`; a constraining client destroys its pointer constraint, a prompt unmaps its surface.
    pub fn unlock(&self) {
        self.unlock.store(true, Ordering::SeqCst);
    }

    /// The unlock [`Client::unlock`] asked for has been written to the connection, whether or not the server read it.
    pub fn unlock_sent(&self) -> bool {
        self.unlock_sent.load(Ordering::SeqCst)
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

/// A connection the server made for the authentication prompt.
pub fn prompt_agent(state: &Cupertino) -> UnixStream {
    cupertino_server::shell_connection::shell_process_connection(
        &state.display_handle,
        &[cupertino_server::authorization::ShellRole::Prompt],
    )
    .unwrap()
}

/// The process a shell connection was made for, standing in for a shell process the server supervises. It is killed
/// and reaped when dropped, so a failing test leaves none behind.
pub struct Holder(std::process::Child);

impl Holder {
    /// Kill and reap the process, as the supervisor does when it sees the exit: the connection's roles end with it.
    pub fn end(self) {}
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A connection the server made for a process of its own, holding `roles` until that process exits ([`Holder::end`]).
pub fn held_shell_connection(
    state: &Cupertino,
    roles: &[cupertino_server::authorization::ShellRole],
) -> (UnixStream, Holder) {
    use cupertino_server::shell_connection::{
        bind_holder, connect_bound_client, shell_process_state,
    };
    let holder = Holder(
        std::process::Command::new("sleep")
            .arg("60")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (stream, client) =
        connect_bound_client(&state.display_handle, shell_process_state(roles)).unwrap();
    bind_holder(&client, holder.0.id()).unwrap();
    (stream, holder)
}

pub fn each_connection_a_process(state: &mut Cupertino) {
    state.process_of = Box::new(|_, client| {
        let mut hasher = DefaultHasher::new();
        client.id().hash(&mut hasher);
        Some((0, hasher.finish()))
    });
}
