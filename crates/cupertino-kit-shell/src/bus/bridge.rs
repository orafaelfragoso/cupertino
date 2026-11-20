//! The host bus across hosts (`docs/architecture/shell-host.md#the-host-bus-and-its-bridge`): a separate host reaches
//! the shared host's bus over a socket pair the server makes, one end to each side (`bus_peer` to the shared host,
//! `bus` to the separate host). Each end has a reader thread, which checks every frame against the bridge's rules
//! and hands the publications to the bus, and a writer thread, so a publication never waits for a socket.
//!
//! A bridge that closes or breaks a rule is closed and logged, and its side reports it with `bus_dropped`; the
//! server then sends a fresh pair. An end the host closes itself (a fresh pair replaced it, or `bus_peer_gone`) is
//! not reported.

use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use cupertino_protocols::client::host::cupertino_host_v1::CupertinoHostV1;
use serde::{Deserialize, Serialize};
use wayland_client::Connection as WlConnection;

use super::hub::BusHub;
use super::{Message, Sender, Topic, MAX_BRIDGE_FRAME_BYTES};
use crate::hosting::Kind;
use crate::ComponentName;

/// Version of the bridge's frames that `hello` and `welcome` carry; a bridge of another version is closed.
pub const BRIDGE_VERSION: u32 = 1;

/// Frames waiting to be written on one bridge, and publications a separate host keeps while it has no bridge (the
/// newest are kept).
pub const MAX_BRIDGE_QUEUE: usize = 256;

/// Name of a bridge's reader thread.
const READER_THREAD: &str = "host-bus-read";
/// Name of a bridge's writer thread.
const WRITER_THREAD: &str = "host-bus-write";

/// One frame of the bridge: a `u32` little-endian length, then JSON with a `type` tag.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Frame {
    /// The separate host's first frame.
    Hello {
        version: u32,
        component: ComponentName,
    },
    /// The shared host's answer to a hello it accepts.
    Welcome { version: u32 },
    /// A publication, both ways.
    Message { from: Sender, message: Message },
}

/// Encodes `frame` with its length, enforcing [`MAX_BRIDGE_FRAME_BYTES`].
fn encode(frame: &Frame) -> io::Result<Vec<u8>> {
    let payload =
        serde_json::to_vec(frame).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if payload.len() > MAX_BRIDGE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "host bus frame {} bytes exceeds {MAX_BRIDGE_FRAME_BYTES} byte limit",
                payload.len()
            ),
        ));
    }
    let mut bytes = Vec::with_capacity(4 + payload.len());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// Reads one frame; an unknown tag or field is an error.
fn read_frame(reader: &mut impl Read) -> io::Result<Frame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_BRIDGE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("host bus frame {len} bytes exceeds {MAX_BRIDGE_FRAME_BYTES} byte limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Where a publication entered this host's bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Came {
    /// From a component or the host of this process.
    Here,
    /// Over the shared host's bridge to the separate host that runs this component.
    Peer(ComponentName),
    /// Over a separate host's bridge to the shared host.
    Uplink,
}

/// Which end of a bridge a link is.
#[derive(Clone, Copy, Debug)]
enum Side {
    /// The shared host's end of the bridge to the separate host of this component.
    Peer(ComponentName),
    /// A separate host's end, for its component.
    Uplink(ComponentName),
}

/// Frames waiting for a bridge's writer thread.
#[derive(Default)]
struct Outbox {
    state: Mutex<OutboxState>,
    ready: Condvar,
}

#[derive(Default)]
struct OutboxState {
    frames: VecDeque<Vec<u8>>,
    closed: bool,
    warned: bool,
}

impl Outbox {
    fn lock(&self) -> MutexGuard<'_, OutboxState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, frame: Vec<u8>) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        if state.frames.len() >= MAX_BRIDGE_QUEUE {
            state.frames.pop_front();
            if !state.warned {
                state.warned = true;
                tracing::warn!(
                    "host bus bridge holds {MAX_BRIDGE_QUEUE} frames; dropping the oldest"
                );
            }
        }
        state.frames.push_back(frame);
        self.ready.notify_one();
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// The next frame to write; `None` once the outbox is closed.
    fn next(&self) -> Option<Vec<u8>> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(frame) = state.frames.pop_front() {
                return Some(frame);
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// This host's end of one bridge.
struct Link {
    id: u64,
    socket: UnixStream,
    outbox: Arc<Outbox>,
    /// Past the handshake: publications go to it.
    live: bool,
    /// Closed by this host: its end is not reported.
    retired: Arc<AtomicBool>,
}

impl Link {
    fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
        self.outbox.close();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }

    fn send(&self, frame: &Frame) {
        match encode(frame) {
            Ok(bytes) => self.outbox.push(bytes),
            Err(e) => tracing::warn!(error = %e, "host bus frame not sent"),
        }
    }
}

/// How a side reports a closed bridge to the server (`bus_dropped`).
#[derive(Clone)]
struct Reporter {
    host: CupertinoHostV1,
    conn: WlConnection,
}

impl Reporter {
    fn dropped(&self, component: ComponentName) {
        self.host.bus_dropped(component.as_str().to_owned());
        let _ = self.conn.flush();
    }
}

/// The bridges of one host: the shared host's peers, or a separate host's one bridge to the shared host.
#[derive(Default)]
pub(crate) struct Bridges {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    next_id: u64,
    kind: Option<Kind>,
    reporter: Option<Reporter>,
    /// The shared host's ends, by the component of the separate host at the other end.
    peers: HashMap<ComponentName, Link>,
    /// A separate host's end.
    uplink: Option<Link>,
    /// A separate host's component, once its list names it.
    own: Option<ComponentName>,
    /// A separate host's end that came before its list named the component.
    waiting: Option<UnixStream>,
    /// A separate host's component's publications while it has no bridge.
    pending: VecDeque<Message>,
    warned_pending: bool,
}

impl Bridges {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// What this host is: a separate host keeps its component's publications while it has no bridge.
    pub(crate) fn set_kind(&self, kind: Kind) {
        self.lock().kind = Some(kind);
    }

    /// The host object and connection `bus_dropped` goes on.
    pub(crate) fn set_reporter(&self, host: CupertinoHostV1, conn: WlConnection) {
        self.lock().reporter = Some(Reporter { host, conn });
    }

    /// Hands a publication that entered the bus to the bridges it goes on: on the shared host to every bridge but
    /// the one it came over, on a separate host its component's to the shared host. Called under the bus's lock.
    pub(super) fn forward(&self, from: Sender, message: &Message, came: Came) {
        let mut guard = self.lock();
        let state = &mut *guard;
        let mut frame = None;
        let mut bytes = || {
            frame
                .get_or_insert_with(|| {
                    encode(&Frame::Message {
                        from,
                        message: message.clone(),
                    })
                    .map_err(|e| tracing::warn!(error = %e, "host bus frame not sent"))
                    .ok()
                })
                .clone()
        };
        for (component, link) in &state.peers {
            if link.live && came != Came::Peer(*component) {
                if let Some(bytes) = bytes() {
                    link.outbox.push(bytes);
                }
            }
        }
        let Sender::Component(publisher) = from else {
            return;
        };
        if came != Came::Here
            || state.kind != Some(Kind::Separate)
            || message.topic() == Topic::Host
            || state.own.is_some_and(|own| own != publisher)
        {
            return;
        }
        match state.uplink.as_ref().filter(|link| link.live) {
            Some(link) => {
                if let Some(bytes) = bytes() {
                    link.outbox.push(bytes);
                }
            }
            None => {
                if state.pending.len() >= MAX_BRIDGE_QUEUE {
                    state.pending.pop_front();
                    if !state.warned_pending {
                        state.warned_pending = true;
                        tracing::warn!(
                            "separate host holds {MAX_BRIDGE_QUEUE} publications for the shared host; dropping the oldest"
                        );
                    }
                }
                state.pending.push_back(message.clone());
            }
        }
    }
}

/// The shared host's end of a fresh bridge to the separate host that runs `component` (`bus_peer`): the end it
/// held before is closed.
pub(crate) fn peer(hub: &Arc<BusHub>, component: ComponentName, stream: UnixStream) {
    let mut state = hub.bridges().lock();
    if let Some(old) = state.peers.remove(&component) {
        old.retire();
    }
    state.next_id += 1;
    let id = state.next_id;
    match open(hub, stream, id, Side::Peer(component), None) {
        Ok(link) => {
            state.peers.insert(component, link);
        }
        Err(e) => {
            tracing::warn!(component = component.as_str(), error = %e, "host bus bridge not opened")
        }
    }
}

/// The separate host that ran `component` ended (`bus_peer_gone`): its bridge closes and its retained
/// publications go.
pub(crate) fn peer_gone(hub: &Arc<BusHub>, component: ComponentName) {
    if let Some(old) = hub.bridges().lock().peers.remove(&component) {
        old.retire();
    }
    hub.forget(Sender::Component(component));
}

/// A separate host's end of a fresh bridge to the shared host (`bus`): the end it held before is closed.
pub(crate) fn uplink(hub: &Arc<BusHub>, stream: UnixStream) {
    let mut state = hub.bridges().lock();
    if let Some(old) = state.uplink.take() {
        old.retire();
    }
    match state.own {
        Some(own) => start_uplink(hub, &mut state, own, stream),
        None => state.waiting = Some(stream),
    }
}

/// A separate host's component, from its list: a bridge that waited for it says hello now.
pub(crate) fn set_own(hub: &Arc<BusHub>, component: ComponentName) {
    let mut state = hub.bridges().lock();
    state.own = Some(component);
    if let Some(stream) = state.waiting.take() {
        start_uplink(hub, &mut state, component, stream);
    }
}

fn start_uplink(hub: &Arc<BusHub>, state: &mut State, own: ComponentName, stream: UnixStream) {
    state.next_id += 1;
    let hello = encode(&Frame::Hello {
        version: BRIDGE_VERSION,
        component: own,
    });
    match hello.and_then(|hello| open(hub, stream, state.next_id, Side::Uplink(own), Some(hello))) {
        Ok(link) => state.uplink = Some(link),
        Err(e) => tracing::warn!(error = %e, "host bus bridge not opened"),
    }
}

/// Starts the reader and writer threads of one end; `first` is written before anything else.
fn open(
    hub: &Arc<BusHub>,
    stream: UnixStream,
    id: u64,
    side: Side,
    first: Option<Vec<u8>>,
) -> io::Result<Link> {
    let outbox = Arc::new(Outbox::default());
    if let Some(first) = first {
        outbox.push(first);
    }
    let retired = Arc::new(AtomicBool::new(false));
    let mut writer = stream.try_clone()?;
    let mut reader = stream.try_clone()?;
    let write_box = Arc::clone(&outbox);
    std::thread::Builder::new()
        .name(WRITER_THREAD.into())
        .spawn(move || {
            while let Some(frame) = write_box.next() {
                if writer.write_all(&frame).is_err() {
                    // the reader sees the end and reports it
                    let _ = writer.shutdown(std::net::Shutdown::Both);
                    return;
                }
            }
        })?;
    let read_hub = Arc::clone(hub);
    std::thread::Builder::new()
        .name(READER_THREAD.into())
        .spawn(move || {
            let result = match side {
                Side::Peer(component) => read_peer(&read_hub, &mut reader, component, id),
                Side::Uplink(own) => read_uplink(&read_hub, &mut reader, own, id),
            };
            end(&read_hub, side, id, result);
        })?;
    Ok(Link {
        id,
        socket: stream,
        outbox,
        live: false,
        retired,
    })
}

/// Reads the next frame; `None` when the other side closed its end.
fn next_frame(reader: &mut UnixStream) -> Result<Option<Frame>, String> {
    match read_frame(reader) {
        Ok(frame) => Ok(Some(frame)),
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// The shared host's reader: the hello of the component the server named, then that component's publications.
fn read_peer(
    hub: &BusHub,
    reader: &mut UnixStream,
    component: ComponentName,
    id: u64,
) -> Result<(), String> {
    match next_frame(reader)? {
        None => return Ok(()),
        Some(Frame::Hello {
            version,
            component: named,
        }) if version == BRIDGE_VERSION && named == component => {}
        Some(Frame::Hello {
            version,
            component: named,
        }) => {
            return Err(format!(
                "hello of {} version {version} on the bridge of {} version {BRIDGE_VERSION}",
                named.as_str(),
                component.as_str()
            ))
        }
        Some(_) => return Err("the first frame is no hello".into()),
    }
    welcome(hub, Side::Peer(component), id);
    while let Some(frame) = next_frame(reader)? {
        let Frame::Message { from, message } = frame else {
            return Err("a frame other than a message after the hello".into());
        };
        if from != Sender::Component(component) {
            return Err(format!(
                "a message from another sender than {}",
                component.as_str()
            ));
        }
        if message.topic() == Topic::Host {
            return Err("a message of the Host topic".into());
        }
        hub.deliver_bridged(from, message, Came::Peer(component));
    }
    Ok(())
}

/// A separate host's reader: the shared host's welcome, then the publications of the other hosts.
fn read_uplink(
    hub: &BusHub,
    reader: &mut UnixStream,
    own: ComponentName,
    id: u64,
) -> Result<(), String> {
    match next_frame(reader)? {
        None => return Ok(()),
        Some(Frame::Welcome { version }) if version == BRIDGE_VERSION => {}
        Some(Frame::Welcome { version }) => {
            return Err(format!(
                "welcome of version {version}, not {BRIDGE_VERSION}"
            ))
        }
        Some(_) => return Err("the first frame is no welcome".into()),
    }
    welcome(hub, Side::Uplink(own), id);
    while let Some(frame) = next_frame(reader)? {
        let Frame::Message { from, message } = frame else {
            return Err("a frame other than a message after the welcome".into());
        };
        if from == Sender::Component(own) {
            return Err("a message in this host's own component's name".into());
        }
        if message.topic() == Topic::Host && from != Sender::Host {
            return Err("a message of the Host topic from a component".into());
        }
        hub.deliver_bridged(from, message, Came::Uplink);
    }
    Ok(())
}

/// Ends the handshake of link `id`: the shared host answers `welcome` and every retained publication; a separate
/// host sends its component's retained publications again and then what waited for the bridge. Under the bus's lock,
/// so no publication slips between what is sent here and what the bridge is given from now on.
fn welcome(hub: &BusHub, side: Side, id: u64) {
    let inner = hub.lock();
    let mut guard = hub.bridges().lock();
    let state = &mut *guard;
    match side {
        Side::Peer(component) => {
            let Some(link) = state.peers.get_mut(&component).filter(|link| link.id == id) else {
                return;
            };
            link.send(&Frame::Welcome {
                version: BRIDGE_VERSION,
            });
            for (from, message) in inner.retained(|from| from != Sender::Component(component)) {
                link.send(&Frame::Message { from, message });
            }
            link.live = true;
        }
        Side::Uplink(own) => {
            let Some(link) = state.uplink.as_mut().filter(|link| link.id == id) else {
                return;
            };
            for (from, message) in inner.retained(|from| from == Sender::Component(own)) {
                link.send(&Frame::Message { from, message });
            }
            for message in state.pending.drain(..) {
                link.send(&Frame::Message {
                    from: Sender::Component(own),
                    message,
                });
            }
            link.live = true;
        }
    }
}

/// A reader ended: unless this host closed the link itself, the link goes and the server hears `bus_dropped`.
fn end(hub: &BusHub, side: Side, id: u64, result: Result<(), String>) {
    let (component, report) = {
        let mut guard = hub.bridges().lock();
        let state = &mut *guard;
        let (component, slot) = match side {
            Side::Peer(component) => (
                component,
                state
                    .peers
                    .get(&component)
                    .filter(|link| link.id == id)
                    .is_some()
                    .then(|| state.peers.remove(&component))
                    .flatten(),
            ),
            Side::Uplink(own) => (
                own,
                state
                    .uplink
                    .as_ref()
                    .filter(|link| link.id == id)
                    .is_some()
                    .then(|| state.uplink.take())
                    .flatten(),
            ),
        };
        match slot {
            Some(link) if !link.retired.load(Ordering::SeqCst) => {
                link.retire();
                (component, state.reporter.clone())
            }
            Some(link) => {
                link.retire();
                return;
            }
            None => return,
        }
    };
    match result {
        Ok(()) => tracing::info!(component = component.as_str(), "host bus bridge closed"),
        Err(e) => {
            tracing::warn!(component = component.as_str(), error = %e, "host bus bridge broke a rule; closed")
        }
    }
    if let Some(reporter) = report {
        reporter.dropped(component);
    }
}
