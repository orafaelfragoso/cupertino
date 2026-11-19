//! The host program's side (`cupertino-shell` alone; `docs/architecture/shell-host.md#the-host-program's-side`).
//!
//! Manages the host Wayland connection (`cupertino_host_v1`, `cupertino_host_component_v1`),
//! per-component threads on an 8 MiB stack with `catch_unwind` isolation, the shared host bus,
//! and graceful or forced shutdown (`shut` calls `shutdown(SHUT_RDWR)` on sockets and removes
//! the component from the host bus).

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_kit::core::Environment;
use cupertino_protocols::client::host::cupertino_host_component_v1::{
    self, CupertinoHostComponentV1, Failure as WireFailure, StopReason as WireStopReason,
    UnavailableReason as WireUnavailable,
};
use cupertino_protocols::client::host::cupertino_host_v1::{
    self, CupertinoHostV1, Kind as WireKind,
};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection as WlConnection, Dispatch, EventQueue, QueueHandle, WEnum};

use crate::bus::{read_bridge_frame, write_bridge_frame, BusHub, Message, Sender};
use crate::context::{self, ComponentContext};
use crate::host::HELPER_JOIN_TIMEOUT;
use crate::{ComponentName, Entry, Error};

/// Protocol version of `cupertino_host_v1` this crate was built with.
pub const HOST_PROTOCOL_VERSION: u32 = 1;

/// Exit status a host uses when the server offers a mismatched `cupertino_host_v1` version.
pub const EXIT_VERSION: i32 = 76;

/// Stack size of each component thread (`shell-host.md#the-host-program's-side`).
pub const COMPONENT_STACK_BYTES: usize = 8 * 1024 * 1024;

/// How often the host checks component and render-thread liveness.
pub const LIVENESS_CHECK_INTERVAL: Duration = Duration::from_millis(500);
/// Busy threshold before reporting `Liveness::Busy`.
pub const BUSY_THRESHOLD: Duration = Duration::from_millis(500);
/// Grace period during startup or after restart before reporting unresponsive.
pub const STARTUP_GRACE: Duration = Duration::from_secs(10);
/// Threshold before reporting `Liveness::Unresponsive`.
pub const UNRESPONSIVE_THRESHOLD: Duration = Duration::from_secs(2);
/// Threshold before reporting `Liveness::Lost`.
pub const LOST_THRESHOLD: Duration = Duration::from_secs(10);

/// Kind of host process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Shared,
    Separate,
}

/// Liveness report sent by the host to the server for one component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Liveness {
    Busy,
    Unresponsive,
    Responsive,
    Lost,
}

/// Failure cause reported by the host when a component ends abnormally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Failure {
    Panic,
    Error,
    Exit,
}

/// Reason a host cannot start a listed component.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unavailable {
    Absent,
    Version,
}

/// A component's Wayland connection handed from the server through the host protocol.
pub struct Connection {
    stream: UnixStream,
}

impl Connection {
    pub(crate) fn from_fd(fd: OwnedFd) -> Self {
        Self {
            stream: UnixStream::from(fd),
        }
    }

    pub(crate) fn try_clone(&self) -> std::io::Result<UnixStream> {
        self.stream.try_clone()
    }

    pub(crate) fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Events produced by [`Host::wait`].
pub enum HostEvent {
    Kind(Kind),
    /// A complete component list, replacing the previous one.
    Components(Vec<String>),
    Connection {
        component: ComponentName,
        connection: Connection,
    },
    /// The server stopped restarting `component` after repeated failures.
    Stopped {
        component: ComponentName,
    },
    /// To the shared host: the server gave `component` up (in whichever host).
    ComponentStopped {
        component: ComponentName,
    },
    RestartComponent {
        component: ComponentName,
    },
    /// The server's quiet moment arrived: stop components and exit.
    Restart,
}

#[derive(Default)]
struct HostDispatchState {
    host_global: Option<CupertinoHostV1>,
    server_host_version: Option<u32>,
    pending_components: Vec<String>,
    events: Vec<HostEvent>,
    bus_hub: Option<Arc<BusHub>>,
    bridge_sockets: Vec<Arc<Mutex<UnixStream>>>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for HostDispatchState {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &WlConnection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            if interface == "cupertino_host_v1" {
                state.server_host_version = Some(version);
                if version == HOST_PROTOCOL_VERSION {
                    state.host_global =
                        Some(reg.bind::<CupertinoHostV1, _, _>(name, version, qh, ()));
                }
            }
        }
    }
}

impl Dispatch<CupertinoHostV1, ()> for HostDispatchState {
    fn event(
        state: &mut Self,
        _: &CupertinoHostV1,
        event: cupertino_host_v1::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_host_v1::Event::Kind {
                kind: WEnum::Value(k),
            } => {
                let kind = match k {
                    WireKind::Shared => Kind::Shared,
                    WireKind::Separate => Kind::Separate,
                    _ => return,
                };
                state.events.push(HostEvent::Kind(kind));
            }
            cupertino_host_v1::Event::Component { name } => {
                state.pending_components.push(name);
            }
            cupertino_host_v1::Event::Done => {
                let list = std::mem::take(&mut state.pending_components);
                state.events.push(HostEvent::Components(list));
            }
            cupertino_host_v1::Event::ComponentStopped { name } => {
                if let Some(component) = ComponentName::from_name(&name) {
                    state.events.push(HostEvent::ComponentStopped { component });
                }
            }
            cupertino_host_v1::Event::Restart => {
                state.events.push(HostEvent::Restart);
            }
            cupertino_host_v1::Event::Bus { fd } | cupertino_host_v1::Event::BusPeer { fd, .. } => {
                let stream = UnixStream::from(fd);
                if let Ok(reader_stream) = stream.try_clone() {
                    let hub = state.bus_hub.clone();
                    let _ = std::thread::Builder::new()
                        .name("host-bus-bridge".into())
                        .spawn(move || {
                            let mut reader = reader_stream;
                            while let Ok(env) = read_bridge_frame(&mut reader) {
                                if let Some(hub) = &hub {
                                    hub.deliver_bridged(env.from, env.message);
                                }
                            }
                        });
                }
                state.bridge_sockets.push(Arc::new(Mutex::new(stream)));
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHostComponentV1, String> for HostDispatchState {
    fn event(
        state: &mut Self,
        _: &CupertinoHostComponentV1,
        event: cupertino_host_component_v1::Event,
        name: &String,
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        let Some(component) = ComponentName::from_name(name) else {
            return;
        };
        match event {
            cupertino_host_component_v1::Event::Connection { fd } => {
                state.events.push(HostEvent::Connection {
                    component,
                    connection: Connection::from_fd(fd),
                });
            }
            cupertino_host_component_v1::Event::Stopped {
                reason: WEnum::Value(WireStopReason::GaveUp),
            } => {
                state.events.push(HostEvent::Stopped { component });
            }
            cupertino_host_component_v1::Event::Restart => {
                state.events.push(HostEvent::RestartComponent { component });
            }
            _ => {}
        }
    }
}

/// The host process's Wayland connection (`WAYLAND_SOCKET`, `host` role).
pub struct Host {
    conn: WlConnection,
    queue: EventQueue<HostDispatchState>,
    state: HostDispatchState,
    components: HashMap<String, CupertinoHostComponentV1>,
}

impl Host {
    /// Connect using `WAYLAND_SOCKET` handed by the server. Returns [`Error::Version`] when the
    /// server offers a different `cupertino_host_v1` version than [`HOST_PROTOCOL_VERSION`].
    pub fn connect() -> Result<Host, Error> {
        let raw_fd: i32 = std::env::var("WAYLAND_SOCKET")
            .map_err(|_| Error::Protocol("WAYLAND_SOCKET is not set".into()))?
            .parse()
            .map_err(|_| Error::Protocol("WAYLAND_SOCKET is not a valid fd".into()))?;
        std::env::remove_var("WAYLAND_SOCKET");
        // Safety: WAYLAND_SOCKET was exclusively handed to this host process by the server.
        let owned = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        Self::from_stream(UnixStream::from(owned))
    }

    fn from_stream(stream: UnixStream) -> Result<Host, Error> {
        let conn = WlConnection::from_socket(stream)
            .map_err(|e| Error::Protocol(format!("wayland host connect failed: {e}")))?;
        let mut queue = conn.new_event_queue::<HostDispatchState>();
        let qh = queue.handle();
        let mut state = HostDispatchState::default();
        let _reg = conn.display().get_registry(&qh, ());
        queue
            .roundtrip(&mut state)
            .map_err(|e| Error::Protocol(format!("host registry roundtrip failed: {e}")))?;
        match state.server_host_version {
            Some(v) if v == HOST_PROTOCOL_VERSION => {}
            Some(v) => {
                return Err(Error::Version {
                    expected: HOST_PROTOCOL_VERSION,
                    found: v,
                });
            }
            None => {
                return Err(Error::Protocol(
                    "server did not advertise cupertino_host_v1".into(),
                ));
            }
        }
        Ok(Self {
            conn,
            queue,
            state,
            components: HashMap::new(),
        })
    }

    /// Wait up to `timeout` for host protocol events.
    pub fn wait(&mut self, timeout: Duration) -> Result<Vec<HostEvent>, Error> {
        let _ = self.conn.flush();
        let _ = self.queue.dispatch_pending(&mut self.state);
        if !self.state.events.is_empty() {
            return Ok(std::mem::take(&mut self.state.events));
        }
        if let Some(guard) = self.conn.prepare_read() {
            let fd = guard.connection_fd().as_raw_fd();
            let mut pfd = libc_pollfd(fd);
            let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
            if poll_readable(&mut pfd, ms) {
                let _ = guard.read();
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| Error::Protocol(format!("host dispatch failed: {e}")))?;
        Ok(std::mem::take(&mut self.state.events))
    }

    fn ensure_component_obj(&mut self, name: &str) -> Option<CupertinoHostComponentV1> {
        if let Some(obj) = self.components.get(name) {
            return Some(obj.clone());
        }
        let host = self.state.host_global.as_ref()?;
        let qh = self.queue.handle();
        let obj = host.get_component(name.to_owned(), &qh, name.to_owned());
        self.components.insert(name.to_owned(), obj.clone());
        Some(obj)
    }

    /// Request a new per-component connection (`get_component` if needed + `connect`).
    pub fn connect_component(&mut self, name: &str) {
        if let Some(obj) = self.ensure_component_obj(name) {
            obj.connect();
            let _ = self.conn.flush();
        }
    }

    /// Tell the server that `name` is unavailable in this host.
    pub fn unavailable(&mut self, name: &str, reason: Unavailable) {
        if let Some(obj) = self.ensure_component_obj(name) {
            let wire = match reason {
                Unavailable::Absent => WireUnavailable::Absent,
                Unavailable::Version => WireUnavailable::Version,
            };
            obj.unavailable(wire);
            let _ = self.conn.flush();
        }
    }

    /// Destroy the component's host-protocol object.
    pub fn release(&mut self, name: &str) {
        if let Some(obj) = self.components.remove(name) {
            obj.destroy();
            let _ = self.conn.flush();
        }
    }

    /// Report component liveness (`busy`, `unresponsive`, `responsive`, `lost`).
    pub fn report(&mut self, component: ComponentName, state: Liveness) {
        if let Some(obj) = self.ensure_component_obj(component.as_str()) {
            match state {
                Liveness::Busy => obj.busy(),
                Liveness::Unresponsive => obj.unresponsive(),
                Liveness::Responsive => obj.responsive(),
                Liveness::Lost => obj.lost(),
            }
            let _ = self.conn.flush();
        }
    }

    /// Report that a component ended abnormally.
    pub fn failed(&mut self, component: ComponentName, cause: Failure) {
        if let Some(obj) = self.ensure_component_obj(component.as_str()) {
            let wire = match cause {
                Failure::Panic => WireFailure::Panic,
                Failure::Error => WireFailure::Error,
                Failure::Exit => WireFailure::Exit,
            };
            obj.failed(wire);
            let _ = self.conn.flush();
        }
    }

    /// Ask the server to restart this host at the next quiet moment (or immediately if separate).
    pub fn request_restart(&mut self) {
        if let Some(host) = &self.state.host_global {
            host.request_restart();
            let _ = self.conn.flush();
        }
    }

    pub(crate) fn attach_bus_hub(&mut self, hub: Arc<BusHub>) {
        self.state.bus_hub = Some(hub);
    }

    pub(crate) fn bridge_sockets(&self) -> Vec<Arc<Mutex<UnixStream>>> {
        self.state.bridge_sockets.clone()
    }
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

fn libc_pollfd(fd: i32) -> PollFd {
    PollFd {
        fd,
        events: 0x0001, // POLLIN
        revents: 0,
    }
}

fn poll_readable(pfd: &mut PollFd, timeout_ms: i32) -> bool {
    extern "C" {
        fn poll(fds: *mut PollFd, nfds: usize, timeout: i32) -> i32;
    }
    // Safety: `pfd` points to one valid `PollFd` for the duration of the syscall.
    let rc = unsafe { poll(pfd as *mut PollFd, 1, timeout_ms) };
    rc > 0 && (pfd.revents & 0x0001) != 0
}

/// Shared host runtime: environment, host bus, render-thread watch, and child-process reaper.
pub struct Runtime {
    kind: Kind,
    environment: Environment,
    bus: Arc<BusHub>,
    bridges: Arc<Mutex<Vec<Arc<Mutex<UnixStream>>>>>,
    render_busy_since: Arc<Mutex<Option<Instant>>>,
    children: Arc<Mutex<Vec<std::process::Child>>>,
}

impl Runtime {
    /// Construct a host runtime bound to `host`'s bus bridges and `environment`.
    pub fn for_host(
        host: &mut Host,
        kind: Kind,
        environment: Environment,
    ) -> Result<Runtime, Error> {
        let bus = Arc::new(BusHub::new());
        host.attach_bus_hub(Arc::clone(&bus));
        let bridges = Arc::new(Mutex::new(host.bridge_sockets()));
        Ok(Self {
            kind,
            environment,
            bus,
            bridges,
            render_busy_since: Arc::new(Mutex::new(None)),
            children: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Start `entry` on its own 8 MiB thread bound to `connection`.
    pub fn start(&self, entry: &'static Entry, connection: Connection) -> Result<Running, Error> {
        let component = entry.name();
        let sub_id = self.bus.register(component);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let busy_since = Arc::new(Mutex::new(None::<Instant>));
        let ended = Arc::new(Mutex::new(None::<Ended>));
        let helpers = Arc::new(Mutex::new(Vec::new()));
        let conn_for_shut = connection
            .try_clone()
            .map_err(|e| Error::Protocol(format!("clone component socket failed: {e}")))?;

        let t_stop = Arc::clone(&stop_flag);
        let t_busy = Arc::clone(&busy_since);
        let t_ended = Arc::clone(&ended);
        let t_bus = Arc::clone(&self.bus);
        let t_helpers = Arc::clone(&helpers);
        let t_children = Arc::clone(&self.children);
        let env = self.environment.clone();

        let builder = std::thread::Builder::new()
            .name(format!("shell-{}", component.as_str()))
            .stack_size(COMPONENT_STACK_BYTES);
        let handle = builder
            .spawn(move || {
                let mut ctx = ComponentContext {
                    component: Some(component),
                    environment: Some(env),
                    bus: Some(Arc::clone(&t_bus)),
                    bus_subscriber_id: Some(sub_id),
                    helpers: Arc::clone(&t_helpers),
                    children: t_children,
                    ..ComponentContext::default()
                };
                ctx.instance = 1;
                context::install(ctx);

                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (entry.runner)(&t_stop, &t_busy, connection)
                }));

                // Drop component context inside catch_unwind so teardown panics never abort the host.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    drop(context::take());
                }));

                t_bus.unregister(sub_id, component);

                let final_state = match outcome {
                    Ok(Ok(())) => Ended::Stopped,
                    Ok(Err(_)) => Ended::Failed(Failure::Error),
                    Err(_) => Ended::Failed(Failure::Panic),
                };
                if let Ok(mut slot) = t_ended.lock() {
                    *slot = Some(final_state);
                }
            })
            .map_err(|e| Error::Protocol(format!("spawn component thread failed: {e}")))?;

        Ok(Running {
            component,
            sub_id,
            bus: Arc::clone(&self.bus),
            stop_flag,
            busy_since,
            ended,
            helpers,
            socket: conn_for_shut,
            handle: Some(handle),
        })
    }

    /// Publish `message` on the host bus as `Sender::Host`. Separate hosts drop `Topic::Host`.
    pub fn publish(&self, message: Message) {
        let allow_host = self.kind == Kind::Shared;
        if message.topic() == crate::bus::Topic::Host && !allow_host {
            tracing::warn!("separate host may not publish Topic::Host; dropped");
            return;
        }
        self.bus.publish_from_host(allow_host, message.clone());
        if let Ok(bridges) = self.bridges.lock() {
            for sock in bridges.iter() {
                if let Ok(mut s) = sock.lock() {
                    let _ = write_bridge_frame(&mut *s, Sender::Host, &message);
                    let _ = s.flush();
                }
            }
        }
    }

    /// How long the shared render thread has been out of its wait, if currently busy.
    pub fn render_busy_for(&self) -> Option<Duration> {
        self.render_busy_since
            .lock()
            .ok()
            .and_then(|s| s.map(|t| t.elapsed()))
    }
}

/// Final termination status of a [`Running`] component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    Stopped,
    Failed(Failure),
}

/// Handle to one running component inside [`Runtime`].
pub struct Running {
    component: ComponentName,
    sub_id: u64,
    bus: Arc<BusHub>,
    stop_flag: Arc<AtomicBool>,
    busy_since: Arc<Mutex<Option<Instant>>>,
    ended: Arc<Mutex<Option<Ended>>>,
    helpers: Arc<Mutex<Vec<crate::host::HelperHandle>>>,
    socket: UnixStream,
    handle: Option<JoinHandle<()>>,
}

impl Running {
    /// How long the component's loop has been busy; `None` while waiting.
    pub fn busy_for(&self) -> Option<Duration> {
        self.busy_since
            .lock()
            .ok()
            .and_then(|s| s.map(|t| t.elapsed()))
    }

    /// Ask the component's loop and helper threads to stop cleanly.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Ok(helpers) = self.helpers.lock() {
            for h in helpers.iter() {
                h.stop.trigger();
            }
        }
    }

    /// Cut the component off immediately: shut down its Wayland socket, remove it from the host
    /// bus and drop its retained messages, and signal its helpers to stop.
    pub fn shut(&self) {
        self.stop();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        self.bus.unregister(self.sub_id, self.component);
    }

    /// Return how the component ended once its thread and helpers have finished.
    pub fn ended(&mut self) -> Option<Ended> {
        let state = self.ended.lock().ok().and_then(|s| *s)?;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // Join helper threads up to HELPER_JOIN_TIMEOUT (`shell-host.md#threads-and-child-processes`).
        let deadline = Instant::now() + HELPER_JOIN_TIMEOUT;
        if let Ok(mut helpers) = self.helpers.lock() {
            for h in helpers.iter_mut() {
                h.stop.trigger();
            }
            for h in helpers.iter_mut() {
                if let Some(jh) = h.handle.take() {
                    while !jh.is_finished() && Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    if jh.is_finished() {
                        let _ = jh.join();
                    }
                }
            }
        }
        Some(state)
    }
}
