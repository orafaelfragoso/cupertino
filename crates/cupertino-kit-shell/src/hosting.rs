//! The host program's side (`cupertino-shell` alone; `docs/architecture/shell-host.md#the-host-program's-side`).
//!
//! Manages the host Wayland connection (`cupertino_host_v1`, `cupertino_host_component_v1`),
//! per-component threads on an 8 MiB stack with `catch_unwind` isolation, the shared host bus and its bridges
//! between hosts, and graceful or forced shutdown (`shut` calls `shutdown(SHUT_RDWR)` on the component's Wayland and
//! bus sockets and removes the component from the host bus).

use std::collections::HashMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
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
use rustix::event::{poll, EventfdFlags, PollFd, PollFlags};
use rustix::time::Timespec;
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection as WlConnection, Dispatch, EventQueue, QueueHandle, WEnum};

use crate::bus::{bridge, BusHub, Message};
use crate::context::{self, ComponentContext, Inbox, Sockets};
use crate::host::{Fault, Reaper, HELPER_JOIN_TIMEOUT};
use crate::link::ComponentLink;
use crate::runtime::{Hosting, Ran};
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
    /// The socket, kept to shut it down.
    stream: UnixStream,
    /// The connection the component's objects live on, once [`Runtime::start`] opened it.
    wayland: Option<WlConnection>,
}

impl Connection {
    pub(crate) fn from_fd(fd: OwnedFd) -> Self {
        Self {
            stream: UnixStream::from(fd),
            wayland: None,
        }
    }

    pub(crate) fn try_clone(&self) -> std::io::Result<UnixStream> {
        self.stream.try_clone()
    }

    /// The Wayland connection the component's `cupertino_component_v1` lives on, for the queues of its loop.
    #[allow(dead_code)] // the component loop's window system takes it over (`Connection::from_wayland`)
    pub(crate) fn wayland(&self) -> Option<&WlConnection> {
        self.wayland.as_ref()
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
    kind: Option<Kind>,
    pending_components: Vec<String>,
    events: Vec<HostEvent>,
    /// The host bus, there from the connection's start: the server sends the bridges' ends as soon as it binds.
    bus: Arc<BusHub>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for HostDispatchState {
    fn event(
        state: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        conn: &WlConnection,
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
                    let host = reg.bind::<CupertinoHostV1, _, _>(name, version, qh, ());
                    state.bus.bridges().set_reporter(host.clone(), conn.clone());
                    state.host_global = Some(host);
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
                state.kind = Some(kind);
                state.bus.bridges().set_kind(kind);
                state.events.push(HostEvent::Kind(kind));
            }
            cupertino_host_v1::Event::Component { name } => {
                state.pending_components.push(name);
            }
            cupertino_host_v1::Event::Done => {
                let list = std::mem::take(&mut state.pending_components);
                if state.kind == Some(Kind::Separate) {
                    // the bridge's hello names the one component a separate host runs
                    if let Some(own) = list.iter().find_map(|name| ComponentName::from_name(name)) {
                        bridge::set_own(&state.bus, own);
                    }
                }
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
            cupertino_host_v1::Event::Bus { fd } => {
                bridge::uplink(&state.bus, UnixStream::from(fd));
            }
            cupertino_host_v1::Event::BusPeer { component, fd } => {
                match ComponentName::from_name(&component) {
                    Some(component) => bridge::peer(&state.bus, component, UnixStream::from(fd)),
                    None => {
                        tracing::warn!(component, "bus_peer for a component of no table; closed")
                    }
                }
            }
            cupertino_host_v1::Event::BusPeerGone { component } => {
                if let Some(component) = ComponentName::from_name(&component) {
                    bridge::peer_gone(&state.bus, component);
                }
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
    /// Signalled as a component of a runtime made from this host ends, so [`Host::wait`] returns then.
    ends: Arc<Ends>,
}

/// An event descriptor a component's thread signals as it ends, which the host's wait reads.
struct Ends(OwnedFd);

impl Ends {
    fn new() -> Result<Ends, Error> {
        rustix::event::eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
            .map(Ends)
            .map_err(|e| Error::Protocol(format!("no event descriptor for the host's wait: {e}")))
    }

    /// A component ended: the host's wait returns.
    fn signal(&self) {
        let _ = rustix::io::write(&self.0, &1u64.to_ne_bytes());
    }

    /// Takes the signals given so far.
    fn clear(&self) {
        let mut count = [0u8; 8];
        let _ = rustix::io::read(&self.0, &mut count);
    }
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
        // the server cleared close-on-exec so the fd survived its exec; set again, no program the host starts
        // inherits the host-role connection
        rustix::io::fcntl_setfd(&owned, rustix::io::FdFlags::CLOEXEC).map_err(|e| {
            Error::Protocol(format!("WAYLAND_SOCKET cannot be closed on exec: {e}"))
        })?;
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
            ends: Arc::new(Ends::new()?),
        })
    }

    /// Wait up to `timeout` for host protocol events; it returns sooner, with what came meanwhile, as a component
    /// started on a runtime of this host ends, so the host reads [`Running::ended`] then. A connection the server
    /// closed, or that failed, is an error: the host has no server to run components for.
    pub fn wait(&mut self, timeout: Duration) -> Result<Vec<HostEvent>, Error> {
        let failed =
            |e: &dyn std::fmt::Display| Error::Protocol(format!("host connection failed: {e}"));
        match self.conn.flush() {
            Ok(()) => {}
            Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(failed(&e)),
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| failed(&e))?;
        if !self.state.events.is_empty() {
            return Ok(std::mem::take(&mut self.state.events));
        }
        if let Some(guard) = self.conn.prepare_read() {
            let fd = guard.connection_fd();
            let mut fds = [
                PollFd::new(&fd, PollFlags::IN),
                PollFd::new(&self.ends.0, PollFlags::IN),
            ];
            let timeout = Timespec::try_from(timeout).unwrap_or(Timespec {
                tv_sec: i64::MAX,
                tv_nsec: 0,
            });
            let polled = poll(&mut fds, Some(&timeout));
            if !fds[1].revents().is_empty() {
                self.ends.clear();
            }
            match polled {
                // readable, hung up or failed: the read says which
                Ok(_) if !fds[0].revents().is_empty() => match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(failed(&e)),
                },
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(e) => return Err(failed(&e)),
            }
        }
        self.queue
            .dispatch_pending(&mut self.state)
            .map_err(|e| failed(&e))?;
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

    /// The host bus, whose bridges the connection's events open from its start.
    pub(crate) fn bus(&self) -> Arc<BusHub> {
        Arc::clone(&self.state.bus)
    }
}

/// Shared host runtime: environment, host bus, render-thread watch, and child-process reaper.
pub struct Runtime {
    kind: Kind,
    environment: Environment,
    bus: Arc<BusHub>,
    /// How a component's helper that ignores its stop asks for the host's restart.
    restart: Option<Restart>,
    reaper: Option<Arc<Reaper>>,
    /// What a component's thread signals as it ends ([`Host::wait`]).
    ends: Arc<Ends>,
}

/// The host's `request_restart`, which a component's handle can send without the host.
#[derive(Clone)]
struct Restart {
    host: CupertinoHostV1,
    conn: WlConnection,
}

impl Restart {
    fn request(&self) {
        self.host.request_restart();
        let _ = self.conn.flush();
    }
}

impl Runtime {
    /// Construct a host runtime on `host`'s bus, whose bridges `host` opens as the server sends their ends.
    pub fn for_host(
        host: &mut Host,
        kind: Kind,
        environment: Environment,
    ) -> Result<Runtime, Error> {
        Ok(Self {
            kind,
            environment,
            bus: host.bus(),
            restart: host.state.host_global.clone().map(|host_global| Restart {
                host: host_global,
                conn: host.conn.clone(),
            }),
            reaper: Reaper::new(),
            ends: Arc::clone(&host.ends),
        })
    }

    /// Start `entry` on its own 8 MiB thread bound to `connection`, its loop Kit's UI loop on that connection.
    /// `Error::Version` when the connection offers another version of `cupertino_component_v1` than this crate was
    /// built with, or a global the component binds older than it needs or not at all: no thread starts.
    pub fn start(&self, entry: &'static Entry, connection: Connection) -> Result<Running, Error> {
        let component = entry.name();
        let socket = connection
            .try_clone()
            .map_err(|e| Error::Protocol(format!("clone component socket failed: {e}")))?;
        let (link, opened) = ComponentLink::open(socket)?;
        crate::runtime::check_globals(component, &opened.globals)?;
        let connection = Connection {
            wayland: Some(link.connection().clone()),
            ..connection
        };
        let conn_for_shut = connection
            .try_clone()
            .map_err(|e| Error::Protocol(format!("clone component socket failed: {e}")))?;
        let sub_id = self.bus.register(component);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let busy_since = Arc::new(Mutex::new(None::<Instant>));
        let ended = Arc::new(Mutex::new(None::<Ended>));
        let helpers = Arc::new(Mutex::new(Vec::new()));
        let inbox = Arc::new(Inbox::default());
        let fault = Arc::new(Fault::new(Arc::clone(&stop_flag), Arc::clone(&inbox)));
        let sockets = Sockets::default();

        let ctx = ComponentContext {
            component: Some(component),
            instance: opened.instance,
            holds: opened.holds,
            environment: Some(self.environment.clone()),
            bus: Some(Arc::clone(&self.bus)),
            bus_subscriber_id: Some(sub_id),
            helpers: Arc::clone(&helpers),
            fault: Some(Arc::clone(&fault)),
            reaper: self.reaper.clone(),
            link: Some(link),
            sockets: Arc::clone(&sockets),
            inbox: Arc::clone(&inbox),
            ..ComponentContext::default()
        };
        let hosting = Hosting {
            stop: Arc::clone(&stop_flag),
            busy: Arc::clone(&busy_since),
        };
        let t_ended = Arc::clone(&ended);
        let t_ends = Arc::clone(&self.ends);
        let t_bus = Arc::clone(&self.bus);
        let t_fault = Arc::clone(&fault);

        let builder = std::thread::Builder::new()
            .name(format!("shell-{}", component.as_str()))
            .stack_size(COMPONENT_STACK_BYTES);
        let handle = builder
            .spawn(move || {
                context::install(ctx);

                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (entry.runner)(&hosting, connection)
                }));

                // the component's state is dropped after the unwind, each part inside catch_unwind: a panic in its
                // teardown is caught and leaks what was left, and never aborts the host
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    drop(crate::runtime::take_state());
                }));
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    drop(context::take());
                }));

                t_bus.unregister(sub_id, component);

                let final_state = match outcome {
                    _ if t_fault.panicked() => Ended::Failed(Failure::Panic),
                    Ok(Ok(Ran::Stopped)) => Ended::Stopped,
                    // the host cut the component off: its loop failing on the closed socket is the host's doing
                    Ok(Err(_)) if t_fault.is_cut() => Ended::Stopped,
                    Ok(Ok(Ran::Exited)) => Ended::Failed(Failure::Exit),
                    Ok(Err(e)) => {
                        tracing::error!(component = component.as_str(), error = %e, "component failed");
                        Ended::Failed(Failure::Error)
                    }
                    Err(_) => Ended::Failed(Failure::Panic),
                };
                if let Ok(mut slot) = t_ended.lock() {
                    *slot = Some(final_state);
                }
                t_ends.signal();
            })
            .map_err(|e| {
                self.bus.unregister(sub_id, component);
                Error::Protocol(format!("spawn component thread failed: {e}"))
            })?;

        Ok(Running {
            component,
            sub_id,
            bus: Arc::clone(&self.bus),
            stop_flag,
            busy_since,
            inbox,
            restart: self.restart.clone(),
            ended,
            helpers,
            socket: conn_for_shut,
            sockets,
            fault,
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
        // the bus hands it to the bridges as well
        self.bus.publish_from_host(allow_host, message);
    }

    /// How long the process's render thread has been out of its wait, if currently busy: every component of the
    /// host draws through it, so one out of it for [`LOST_THRESHOLD`] is a hang of the whole host.
    pub fn render_busy_for(&self) -> Option<Duration> {
        cupertino_ui_render::render_thread::heartbeat().busy_for
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
    /// The component's inbox, woken so its loop sees a stop.
    inbox: Arc<Inbox>,
    restart: Option<Restart>,
    ended: Arc<Mutex<Option<Ended>>>,
    helpers: Arc<Mutex<Vec<crate::host::HelperHandle>>>,
    socket: UnixStream,
    /// The component's session-bus sockets (`bus::session`).
    sockets: Sockets,
    fault: Arc<Fault>,
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
        self.inbox.wake();
        if let Ok(helpers) = self.helpers.lock() {
            for h in helpers.iter() {
                h.stop.trigger();
            }
        }
    }

    /// Cut the component off immediately: shut down its Wayland socket and its session-bus sockets, remove it from
    /// the host bus and drop its retained messages, drop its helpers' results unread, and signal its helpers to stop.
    pub fn shut(&self) {
        self.fault.cut();
        self.stop();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
        for socket in self
            .sockets
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        self.bus.unregister(self.sub_id, self.component);
    }
    /// Return how the component ended once its thread and helpers have finished.
    pub fn ended(&mut self) -> Option<Ended> {
        let state = self.ended.lock().ok().and_then(|s| *s)?;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // Join helper threads up to HELPER_JOIN_TIMEOUT (`shell-host.md#threads-and-child-processes`): each helper's
        // end wakes the wait for it. One still running then is left behind, and the host asks for its restart, the
        // only way to be rid of a thread.
        let deadline = Instant::now() + HELPER_JOIN_TIMEOUT;
        let mut stuck = 0;
        {
            let mut helpers = self.helpers.lock().unwrap_or_else(PoisonError::into_inner);
            for h in helpers.iter() {
                h.stop.trigger();
            }
            for h in helpers.iter_mut() {
                let Some(jh) = h.handle.take() else {
                    continue;
                };
                if h.finished.wait_until(deadline) {
                    let _ = jh.join();
                } else {
                    stuck += 1;
                }
            }
        }
        if stuck > 0 {
            tracing::error!(
                component = self.component.as_str(),
                stuck,
                "helper threads ignored their stop; the host asks to restart"
            );
            if let Some(restart) = &self.restart {
                restart.request();
            }
        }
        Some(state)
    }
}
