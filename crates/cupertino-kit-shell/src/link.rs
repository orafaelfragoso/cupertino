//! A component's own object on its Wayland connection, `cupertino_component_v1`
//! (`docs/architecture/shell-host.md#identity`): what the server says of the component when it starts (its instance
//! number and the keys of the surfaces it holds from the instance before), and the announcement of the component's
//! session-bus name (D20).
//!
//! The object lives on an event queue of its own on the component's connection, next to the queues of whatever else
//! the component's loop runs there. A wait for the server's answer dispatches that queue and, while nobody else reads
//! the connection, reads it itself when it turns readable, so the wait is woken by the socket and never polls.

use std::collections::HashMap;
use std::future::poll_fn;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use async_io::Async;
use cupertino_protocols::client::host::cupertino_component_v1::{
    self, BusNameResult, CupertinoComponentV1,
};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection as WlConnection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};

use crate::Error;

/// Version of `cupertino_component_v1` this crate was built with: a connection that offers another starts no
/// component (`Error::Version`).
pub(crate) const COMPONENT_PROTOCOL_VERSION: u32 = 1;

/// What the server says of a component when its connection opens.
pub(crate) struct Opened {
    /// The instance number, part of the name of every transaction the instance creates.
    pub instance: u32,
    /// The keys of the persistent surfaces the server holds from the instance before.
    pub holds: Vec<String>,
    /// The version the connection's registry offers each global at, by interface name.
    pub globals: HashMap<String, u32>,
}

#[derive(Default)]
struct LinkState {
    /// The version the registry offers `cupertino_component_v1` at.
    offered: Option<u32>,
    /// The version the registry offers each global at, by interface name.
    globals: HashMap<String, u32>,
    object: Option<CupertinoComponentV1>,
    instance: u32,
    holds: Vec<String>,
    /// The nonces of the announcements, in the order the server answered them.
    nonces: std::collections::VecDeque<String>,
    /// The server's answer for each announced name.
    answers: HashMap<String, bool>,
    /// The connection failed: no answer comes.
    failed: bool,
}

struct Inner {
    queue: EventQueue<LinkState>,
    state: LinkState,
}

/// The component's `cupertino_component_v1` on its connection.
pub(crate) struct ComponentLink {
    conn: WlConnection,
    inner: Mutex<Inner>,
    /// The connection's socket, as the reactor of the bus tasks sees it.
    readable: Option<Async<OwnedFd>>,
}

impl ComponentLink {
    /// Opens the link on `stream`, the component's connection: binds `cupertino_component_v1` and waits for what
    /// the server says of the component. Blocks the calling thread for the round trips: the host's main thread, which
    /// may wait, before the component's thread starts.
    pub(crate) fn open(stream: UnixStream) -> Result<(Arc<ComponentLink>, Opened), Error> {
        let conn = WlConnection::from_socket(stream)
            .map_err(|e| Error::Protocol(format!("component connection failed: {e}")))?;
        let mut queue = conn.new_event_queue::<LinkState>();
        let qh = queue.handle();
        let _registry = conn.display().get_registry(&qh, ());
        let mut state = LinkState::default();
        queue
            .roundtrip(&mut state)
            .map_err(|e| Error::Protocol(format!("component registry roundtrip failed: {e}")))?;
        match state.offered {
            Some(COMPONENT_PROTOCOL_VERSION) => {}
            Some(found) => {
                return Err(Error::Version {
                    expected: COMPONENT_PROTOCOL_VERSION,
                    found,
                })
            }
            None => {
                return Err(Error::Protocol(
                    "the component's connection offers no cupertino_component_v1".into(),
                ))
            }
        }
        queue
            .roundtrip(&mut state)
            .map_err(|e| Error::Protocol(format!("component roundtrip failed: {e}")))?;
        let opened = Opened {
            instance: state.instance,
            holds: std::mem::take(&mut state.holds),
            globals: std::mem::take(&mut state.globals),
        };
        let readable = conn
            .backend()
            .poll_fd()
            .try_clone_to_owned()
            .ok()
            .and_then(|fd| Async::new(fd).ok());
        let link = ComponentLink {
            conn,
            inner: Mutex::new(Inner { queue, state }),
            readable,
        };
        Ok((Arc::new(link), opened))
    }

    /// The component's Wayland connection, for the queues of its loop.
    pub(crate) fn connection(&self) -> &WlConnection {
        &self.conn
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Announces `name`, the unique name of the component's session-bus connection, and gives the nonce the server
    /// answers with; `None` when the connection failed first.
    pub(crate) async fn announce(&self, name: &str) -> Option<String> {
        {
            let mut inner = self.lock();
            inner.state.nonces.clear();
            inner.state.answers.remove(name);
            let object = inner.state.object.clone()?;
            object.announce_bus_name(name.to_owned());
        }
        if let Err(e) = self.conn.flush() {
            tracing::warn!(error = %e, "bus name announcement not sent");
            return None;
        }
        poll_fn(|cx| {
            self.poll_until(cx, |state| match state.nonces.pop_front() {
                Some(nonce) => Some(Some(nonce)),
                None => state.failed.then_some(None),
            })
        })
        .await
    }

    /// The server's answer to the announcement of `name`: whether it is the component's; `None` when the
    /// connection failed first.
    pub(crate) async fn answer(&self, name: &str) -> Option<bool> {
        poll_fn(|cx| {
            self.poll_until(cx, |state| match state.answers.remove(name) {
                Some(accepted) => Some(Some(accepted)),
                None => state.failed.then_some(None),
            })
        })
        .await
    }

    /// Persists the layer surface `surface` under `key` (`persist`), before its first buffer.
    pub(crate) fn persist(&self, surface: &WlSurface, key: &str) {
        if let Some(object) = self.lock().state.object.clone() {
            object.persist(surface, key.to_owned());
        }
    }

    /// Tells the server the component shows what it shows at start (`ready`).
    pub(crate) fn ready(&self) {
        if let Some(object) = self.lock().state.object.clone() {
            object.ready();
        }
    }

    /// Handles what the component's loop read for the link's queue, as that loop does on each turn: the waits of the
    /// bus tasks find their answers there, and a failed connection fails them.
    pub(crate) fn dispatch_pending(&self) {
        let mut guard = self.lock();
        let Inner { queue, state } = &mut *guard;
        if let Err(e) = queue.dispatch_pending(state) {
            tracing::warn!(error = %e, "component connection failed");
            state.failed = true;
        }
    }

    /// Dispatches the link's queue until `check` has its value, reading the connection when it turns readable.
    fn poll_until<T>(
        &self,
        cx: &mut Context<'_>,
        mut check: impl FnMut(&mut LinkState) -> Option<T>,
    ) -> Poll<T> {
        loop {
            {
                let mut guard = self.lock();
                let Inner { queue, state } = &mut *guard;
                if let Poll::Ready(Err(e)) = queue.poll_dispatch_pending(cx, state) {
                    tracing::warn!(error = %e, "component connection failed");
                    state.failed = true;
                }
                if let Some(value) = check(state) {
                    return Poll::Ready(value);
                }
            }
            // another reader of the connection (the component's loop) dispatches into this queue and wakes it
            let Some(readable) = &self.readable else {
                return Poll::Pending;
            };
            match readable.poll_readable(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => {
                    tracing::warn!(error = %e, "component connection cannot be watched");
                    self.lock().state.failed = true;
                    continue;
                }
                Poll::Ready(Ok(())) => {}
            }
            match self.conn.prepare_read() {
                Some(guard) => match guard.read() {
                    Ok(_) => {}
                    Err(WaylandError::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "component connection failed");
                        self.lock().state.failed = true;
                    }
                },
                // events of another queue wait for their dispatch: look again on the next turn
                None => {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
            }
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for LinkState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
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
            let offered = state.globals.entry(interface.clone()).or_insert(version);
            *offered = (*offered).max(version);
            if interface == CupertinoComponentV1::interface().name {
                state.offered = Some(version);
                if version == COMPONENT_PROTOCOL_VERSION && state.object.is_none() {
                    state.object = Some(registry.bind(name, version, qh, ()));
                }
            }
        }
    }
}

impl Dispatch<CupertinoComponentV1, ()> for LinkState {
    fn event(
        state: &mut Self,
        _: &CupertinoComponentV1,
        event: cupertino_component_v1::Event,
        _: &(),
        _: &WlConnection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            cupertino_component_v1::Event::Component { instance, .. } => state.instance = instance,
            cupertino_component_v1::Event::Hold { key } => state.holds.push(key),
            cupertino_component_v1::Event::BusNonce { nonce } => state.nonces.push_back(nonce),
            cupertino_component_v1::Event::BusName { name, result } => {
                let accepted = matches!(result, WEnum::Value(BusNameResult::Accepted));
                state.answers.insert(name, accepted);
            }
            _ => {}
        }
    }
}
