//! A stand-in for the server's side of the host protocol (`cupertino_host_v1`, `cupertino_host_component_v1`,
//! `cupertino_component_v1`), served from a thread of its own over socket pairs, so the hosting runtime's public
//! interface runs as a host runs it: `Host::connect` on `WAYLAND_SOCKET`, a component's connection on request, the
//! bridges' ends, and a bus-name announcement answered with a nonce. Its display globals ([`display`]) let a
//! component's Kit loop run on its connection.

#![allow(dead_code)]

pub mod display;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_kit_shell::hosting::{Connection, Host, HostEvent, Kind};
use cupertino_kit_shell::ComponentName;
use cupertino_protocols::server::host::cupertino_component_v1::{
    self as component_v1, BusNameResult, CupertinoComponentV1,
};
use cupertino_protocols::server::host::cupertino_host_component_v1::{
    self as host_component_v1, CupertinoHostComponentV1,
};
use cupertino_protocols::server::host::cupertino_host_v1::{
    self as host_v1, CupertinoHostV1, Kind as WireKind,
};
use serde_json::{json, Value};
use wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;
use wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
use wayland_server::{Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New};

/// The instance number the stand-in gives every component connection, as a server does on its third start.
pub const INSTANCE: u32 = 3;
/// The key of the surface the stand-in holds for every component from its instance before.
pub const HELD: &str = "bar";

/// The component whose bus-name announcement the stand-in refuses without a nonce, as the server fails closed when
/// it has no random nonce to give.
pub const NO_NONCE: &str = "notifications";

/// How long a test waits for anything the host or the stand-in does.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// What the stand-in saw of the host and its components.
#[derive(Clone, Debug, PartialEq)]
pub enum Seen {
    BusDropped(String),
    Failed(String, u32),
    Announced {
        component: String,
        name: String,
        nonce: String,
    },
    /// A component asked the server to hold its surface under `key` (`persist`).
    Persisted {
        component: String,
        key: String,
    },
    /// A component's persistent windows of its first view are shown (`ready`).
    Ready(String),
    /// A surface of a component committed a buffer; `namespace` is its layer surface's, empty for another surface.
    Drawn {
        component: String,
        namespace: String,
    },
    /// The host asked the server to restart it.
    RestartRequested,
    /// A component asked the session to bring an application forward.
    ActivateApp {
        component: String,
        serial: u32,
        desktop_id: String,
    },
}

/// What the test asks the stand-in to do.
enum Ask {
    BusPeer(String, OwnedFd),
    Bus(OwnedFd),
    BusPeerGone(String),
    CloseHost,
    /// A connection of a client of its own, as the server gives a program it starts.
    Client(Sender<UnixStream>),
    /// `AnnounceComponent(nonce)` from `sender`: answers whether it is accepted, and sends `bus_name`.
    Judge {
        sender: String,
        nonce: String,
        accept: fn(&str) -> bool,
        reply: Sender<bool>,
    },
}

/// Judges `AnnounceComponent(nonce)` calls as the server does.
#[derive(Clone)]
pub struct Judge(Sender<Ask>);

impl Judge {
    /// Accepted when `sender` is the name announced with `nonce` and `accept` takes the announcing component; the
    /// component hears the answer (`bus_name`) on its Wayland connection.
    pub fn judge(&self, sender: &str, nonce: &str, accept: fn(&str) -> bool) -> bool {
        let (reply, answer) = mpsc::channel();
        let asked = self.0.send(Ask::Judge {
            sender: sender.into(),
            nonce: nonce.into(),
            accept,
            reply,
        });
        asked.is_ok() && answer.recv_timeout(PATIENCE).unwrap_or(false)
    }
}

/// The stand-in server, running until it is dropped.
pub struct Server {
    asks: Sender<Ask>,
    seen: Receiver<Seen>,
    quit: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

struct Pending {
    component: String,
    name: String,
    resource: CupertinoComponentV1,
}

struct State {
    kind: Kind,
    components: Vec<String>,
    host: Option<CupertinoHostV1>,
    /// The server's end of the host connection, kept to close it.
    host_socket: Option<UnixStream>,
    /// The component each component connection belongs to.
    component_clients: HashMap<ClientId, String>,
    nonces: HashMap<String, Pending>,
    next_nonce: u64,
    seen: Sender<Seen>,
    display: display::Surfaces,
    /// The layer surface of each surface that has one.
    layer_resources: HashMap<ObjectId, ZwlrLayerSurfaceV1>,
}

struct Data;

impl ClientData for Data {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

impl Server {
    /// Starts the stand-in for a host of `kind` that runs `components`; returns it with the host's end of the host
    /// connection, which [`connect_host`] hands to `Host::connect`.
    pub fn start(kind: Kind, components: &[&str]) -> (Server, UnixStream) {
        Self::start_offering(kind, components, display::Offer::CURRENT)
    }

    /// [`Server::start`] with the display globals at the versions `offer` names.
    pub fn start_offering(
        kind: Kind,
        components: &[&str],
        offer: display::Offer,
    ) -> (Server, UnixStream) {
        let (asks, asked) = mpsc::channel();
        let (seen_tx, seen) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let quit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_quit = Arc::clone(&quit);
        let components: Vec<String> = components.iter().map(|c| c.to_string()).collect();
        let thread = std::thread::Builder::new()
            .name("stand-in-server".into())
            .spawn(move || {
                serve(
                    kind,
                    components,
                    offer,
                    asked,
                    seen_tx,
                    ready_tx,
                    thread_quit,
                )
            })
            .expect("stand-in server thread");
        let host_end = ready.recv().expect("stand-in server starts");
        (
            Server {
                asks,
                seen,
                quit,
                thread: Some(thread),
            },
            host_end,
        )
    }

    /// Sends the shared host the end of a fresh bridge to the separate host of `component`; returns the other end.
    pub fn bus_peer(&self, component: &str) -> UnixStream {
        let (ours, theirs) = UnixStream::pair().expect("socket pair");
        self.asks
            .send(Ask::BusPeer(component.into(), OwnedFd::from(theirs)))
            .unwrap();
        ours
    }

    /// Sends a separate host the end of a fresh bridge to the shared host; returns the other end.
    pub fn bus(&self) -> UnixStream {
        let (ours, theirs) = UnixStream::pair().expect("socket pair");
        self.asks.send(Ask::Bus(OwnedFd::from(theirs))).unwrap();
        ours
    }

    pub fn bus_peer_gone(&self, component: &str) {
        self.asks.send(Ask::BusPeerGone(component.into())).unwrap();
    }

    /// The client end of a connection of its own, as the server hands a program it starts in `WAYLAND_SOCKET`.
    pub fn client(&self) -> UnixStream {
        let (reply, end) = mpsc::channel();
        self.asks.send(Ask::Client(reply)).unwrap();
        end.recv_timeout(PATIENCE)
            .expect("the stand-in makes a client")
    }

    /// Closes the host's connection, as a server that went away.
    pub fn close_host(&self) {
        self.asks.send(Ask::CloseHost).unwrap();
    }

    /// The stand-in's judge of `AnnounceComponent` calls, for a stand-in of `org.cupertino.Session1`.
    pub fn judge(&self) -> Judge {
        Judge(self.asks.clone())
    }

    /// The next thing the stand-in saw that `want` takes, waiting up to [`PATIENCE`].
    pub fn expect(&self, mut want: impl FnMut(&Seen) -> bool) -> Seen {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.seen.recv_timeout(left) {
                Ok(seen) if want(&seen) => return seen,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => panic!("the stand-in server saw nothing wanted"),
                Err(RecvTimeoutError::Disconnected) => panic!("the stand-in server ended"),
            }
        }
    }
}

impl Server {
    /// Everything the stand-in saw, in order, until `done` holds of it, waiting up to [`PATIENCE`].
    pub fn seen_until(&self, mut done: impl FnMut(&[Seen]) -> bool) -> Vec<Seen> {
        let deadline = Instant::now() + PATIENCE;
        let mut seen = Vec::new();
        while !done(&seen) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.seen.recv_timeout(left) {
                Ok(one) => seen.push(one),
                Err(_) => panic!("the stand-in server saw only {seen:?}"),
            }
        }
        seen
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.quit.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The stand-in's loop: answers the clients and the test's asks until it is told to quit.
fn serve(
    kind: Kind,
    components: Vec<String>,
    offer: display::Offer,
    asked: Receiver<Ask>,
    seen: Sender<Seen>,
    ready: Sender<UnixStream>,
    quit: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut display = Display::<State>::new().expect("display");
    let mut handle = display.handle();
    handle.create_global::<State, CupertinoHostV1, ()>(1, ());
    handle.create_global::<State, CupertinoComponentV1, ()>(1, ());
    display::create_globals(&mut handle, offer);
    let (server_end, host_end) = UnixStream::pair().expect("socket pair");
    let host_socket = server_end.try_clone().expect("host socket");
    handle
        .insert_client(server_end, Arc::new(Data))
        .expect("host client");
    let mut state = State {
        kind,
        components,
        host: None,
        host_socket: Some(host_socket),
        component_clients: HashMap::new(),
        nonces: HashMap::new(),
        next_nonce: 0,
        seen,
        display: display::Surfaces::default(),
        layer_resources: HashMap::new(),
    };
    ready.send(host_end).unwrap();
    while !quit.load(std::sync::atomic::Ordering::SeqCst) {
        while let Ok(ask) = asked.try_recv() {
            match ask {
                Ask::Client(reply) => {
                    let (server_end, client_end) = UnixStream::pair().expect("socket pair");
                    handle
                        .insert_client(server_end, Arc::new(Data))
                        .expect("client");
                    let _ = reply.send(client_end);
                }
                ask => answer(&mut state, ask),
            }
        }
        display.dispatch_clients(&mut state).expect("dispatch");
        display.flush_clients().expect("flush");
        let fd = display.backend().poll_fd();
        let mut fds = [rustix::event::PollFd::new(
            &fd,
            rustix::event::PollFlags::IN,
        )];
        let tick = rustix::time::Timespec {
            tv_sec: 0,
            tv_nsec: 5_000_000,
        };
        let _ = rustix::event::poll(&mut fds, Some(&tick));
    }
}

fn answer(state: &mut State, ask: Ask) {
    match ask {
        Ask::BusPeer(component, fd) => {
            if let Some(host) = &state.host {
                host.bus_peer(component, fd.as_fd());
            }
        }
        Ask::Bus(fd) => {
            if let Some(host) = &state.host {
                host.bus(fd.as_fd());
            }
        }
        Ask::BusPeerGone(component) => {
            if let Some(host) = &state.host {
                host.bus_peer_gone(component);
            }
        }
        Ask::Client(_) => {}
        Ask::CloseHost => {
            if let Some(socket) = state.host_socket.take() {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
        }
        Ask::Judge {
            sender,
            nonce,
            accept,
            reply,
        } => {
            let accepted = match state.nonces.remove(&nonce) {
                Some(pending) => {
                    let accepted = pending.name == sender && accept(&pending.component);
                    pending.resource.bus_name(
                        pending.name.clone(),
                        if accepted {
                            BusNameResult::Accepted
                        } else {
                            BusNameResult::Refused
                        },
                    );
                    accepted
                }
                None => false,
            };
            let _ = reply.send(accepted);
        }
    }
}

impl GlobalDispatch<CupertinoHostV1, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<CupertinoHostV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let host = init.init(resource, ());
        host.kind(match state.kind {
            Kind::Shared => WireKind::Shared,
            Kind::Separate => WireKind::Separate,
        });
        for name in &state.components {
            host.component(name.clone());
        }
        host.done();
        state.host = Some(host);
    }
}

impl Dispatch<CupertinoHostV1, ()> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &CupertinoHostV1,
        request: host_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        match request {
            host_v1::Request::GetComponent { id, name } => {
                init.init(id, name);
            }
            host_v1::Request::BusDropped { component } => {
                let _ = state.seen.send(Seen::BusDropped(component));
            }
            host_v1::Request::RequestRestart => {
                let _ = state.seen.send(Seen::RestartRequested);
            }
            _ => {}
        }
    }
}

impl Dispatch<CupertinoHostComponentV1, String> for State {
    fn request(
        state: &mut Self,
        _: &Client,
        resource: &CupertinoHostComponentV1,
        request: host_component_v1::Request,
        name: &String,
        handle: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            host_component_v1::Request::Connect => {
                let (server_end, client_end) = UnixStream::pair().expect("socket pair");
                let mut handle = handle.clone();
                let client = handle
                    .insert_client(server_end, Arc::new(Data))
                    .expect("component client");
                state.component_clients.insert(client.id(), name.clone());
                resource.connection(client_end.as_fd());
            }
            host_component_v1::Request::Failed { cause } => {
                let _ = state.seen.send(Seen::Failed(name.clone(), cause.into()));
            }
            _ => {}
        }
    }
}

impl GlobalDispatch<CupertinoComponentV1, ()> for State {
    fn bind(
        state: &mut Self,
        _: &DisplayHandle,
        client: &Client,
        resource: New<CupertinoComponentV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let object = init.init(resource, ());
        if let Some(name) = state.component_clients.get(&client.id()) {
            object.component(name.clone(), INSTANCE);
            object.hold(HELD.into());
            object.done();
        }
    }
}

impl Dispatch<CupertinoComponentV1, ()> for State {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &CupertinoComponentV1,
        request: component_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            component_v1::Request::Persist { key, .. } => {
                let component = state.component_of(client);
                let _ = state.seen.send(Seen::Persisted { component, key });
                return;
            }
            component_v1::Request::Ready => {
                let _ = state.seen.send(Seen::Ready(state.component_of(client)));
                return;
            }
            _ => {}
        }
        if let component_v1::Request::AnnounceBusName { name } = request {
            let component = state
                .component_clients
                .get(&client.id())
                .cloned()
                .unwrap_or_default();
            if component == NO_NONCE {
                resource.bus_name(name, BusNameResult::Refused);
                return;
            }
            state.next_nonce += 1;
            let nonce = format!("{:032x}", state.next_nonce);
            state.nonces.insert(
                nonce.clone(),
                Pending {
                    component: component.clone(),
                    name: name.clone(),
                    resource: resource.clone(),
                },
            );
            resource.bus_nonce(nonce.clone());
            let _ = state.seen.send(Seen::Announced {
                component,
                name,
                nonce,
            });
        }
    }
}

/// `WAYLAND_SOCKET` is the process's: one host connects at a time.
pub fn environment_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `Host::connect` on `host_end`, as the server hands a host its connection.
pub fn connect_host(host_end: UnixStream) -> Host {
    let _lock = environment_lock();
    std::env::set_var("WAYLAND_SOCKET", host_end.into_raw_fd().to_string());
    Host::connect().expect("host connects")
}

/// Waits on `host` for events until `want` takes one, keeping every event in `log`.
pub fn wait_for(host: &mut Host, mut want: impl FnMut(&mut HostEvent) -> bool) -> HostEvent {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        for mut event in host.wait(Duration::from_millis(50)).expect("host waits") {
            if want(&mut event) {
                return event;
            }
        }
    }
    panic!("the host saw nothing wanted");
}

/// Lets `host` handle what the server sent for a while, as a host's loop does between its other work.
pub fn settle(host: &mut Host) {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(100) {
        host.wait(Duration::from_millis(20)).expect("host waits");
    }
}

/// The connection the stand-in hands out for `component`.
pub fn component_connection(host: &mut Host, component: &str) -> Connection {
    host.connect_component(component);
    let name = ComponentName::from_name(component).expect("component of the table");
    match wait_for(
        host,
        |event| matches!(event, HostEvent::Connection { component, .. } if *component == name),
    ) {
        HostEvent::Connection { connection, .. } => connection,
        _ => unreachable!(),
    }
}

/// Writes one bridge frame: a `u32` little-endian length, then the JSON.
pub fn send_frame(stream: &mut UnixStream, frame: &Value) {
    let bytes = serde_json::to_vec(frame).unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .unwrap();
    stream.write_all(&bytes).unwrap();
}

/// Reads one bridge frame; `None` when the other side closed its end.
pub fn read_frame(stream: &mut UnixStream) -> Option<Value> {
    stream.set_read_timeout(Some(PATIENCE)).unwrap();
    let mut len = [0u8; 4];
    match stream.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
        Err(e) => panic!("bridge frame not read: {e}"),
    }
    let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
    stream.read_exact(&mut buf).unwrap();
    Some(serde_json::from_slice(&buf).unwrap())
}

/// A publication frame.
pub fn message(from: &str, message: Value) -> Value {
    json!({"type": "message", "from": from, "message": message})
}

/// Waits up to [`PATIENCE`] until `done` holds.
pub fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("{what} did not happen");
}
