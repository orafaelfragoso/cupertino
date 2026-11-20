//! The message bus without a thread of its own: an instance's connection does its work (reading the socket, replies,
//! signals, the calls it serves) on the instance's UI thread, polled by the runtime as it polls the futures of
//! [`Command::Perform`], so nothing waits for the bus there and no executor thread runs beside the UI loop. Calls are
//! made with zbus's async API in a `Command::Perform` future, whose output is the message that carries the reply.
//!
//! A connection belongs to the runtime that made it. When that runtime ends with the connection still open, its socket
//! is shut at once, so the bus sees it go, and what is left on its executor (the tasks that followed its signals, the
//! match rules its dropped streams take back, the socket reader) runs on a thread of its own until the last clone of
//! the connection is dropped, wherever the model that held it is dropped; then the thread ends and keeps nothing. A
//! component needs no teardown of its own: the executor its runtime no longer ticks would otherwise keep each task
//! that holds the connection, and the connection its executor and so the task, for as long as the process runs.

use crate::task::{self, Slot};
use cupertino_ui_core::Command;
use std::future::Future;
use std::net::Shutdown;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use zbus::address::transport::{Transport, UnixSocket};

/// Which bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Session,
    System,
}

/// How long connecting to a bus's socket should take: a local socket accepts at once, and one whose daemon is too
/// busy to take it is reported once it takes longer ([`task::OVERDUE`]).
const CONNECT_DEADLINE: Duration = Duration::from_secs(2);

/// The stack of the thread that finishes a connection its runtime left open: it only runs the connection's tasks to
/// their end, none of which keeps much on the stack.
const ENDED_STACK_BYTES: usize = 256 * 1024;

/// Connects to `bus` from the UI thread without waiting for it: `connected` turns the connection, or why there is
/// none, into a message. The connection does its work on the UI thread for as long as it is open; when it closes (every
/// clone of it dropped, or the bus gone), and at once when it could not be made, the application hears `closed`. When
/// the runtime ends first, the connection is closed and released (the module's documentation).
pub fn connect<M>(
    bus: Bus,
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> M + Send + 'static,
    closed: M,
) -> Vec<Command<M>>
where
    M: Send + 'static,
{
    let open = async move {
        let address = match bus {
            Bus::Session => zbus::Address::session(),
            Bus::System => zbus::Address::system(),
        }?;
        let Some(socket) = socket_address(&address)? else {
            // a bus not on a local socket: closed with its last clone, never shut by the runtime's end
            return Ok((zbus::connection::Builder::address(address)?, None));
        };
        let stream =
            task::io_outcome(CONNECT_DEADLINE, move || UnixStream::connect_addr(&socket)).await?;
        let cut = stream.try_clone()?;
        Ok((
            zbus::connection::Builder::async_io_unix_stream(stream),
            Some(cut),
        ))
    };
    start(
        open,
        |connection| async move { connected(connection) },
        closed,
    )
}

/// [`connect`] over `stream`, a socket the caller already connected to a bus and may shut down (`shutdown(2)`) to
/// cut the connection off. `connected` is a step of its own on the connection before the application hears of it,
/// such as a call that must precede every call of the application: the connection already does its work while that
/// step runs, and the step's output is the message.
pub fn connect_on<M, F>(
    stream: UnixStream,
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> F + Send + 'static,
    closed: M,
) -> Vec<Command<M>>
where
    M: Send + 'static,
    F: Future<Output = M> + Send + 'static,
{
    let open = async move {
        // without a handle to shut, the runtime's end leaves the socket open until the connection's last clone goes
        let cut = stream.try_clone().ok();
        Ok((zbus::connection::Builder::async_io_unix_stream(stream), cut))
    };
    start(open, connected, closed)
}

/// The local socket `address` names, or `None` when it names another transport.
fn socket_address(address: &zbus::Address) -> zbus::Result<Option<SocketAddr>> {
    let Transport::Unix(unix) = address.transport() else {
        return Ok(None);
    };
    let socket = match unix.path() {
        UnixSocket::File(path) => SocketAddr::from_pathname(path)?,
        UnixSocket::Abstract(name) => SocketAddr::from_abstract_name(name.as_encoded_bytes())?,
        // a listening address, which a client cannot connect to; zbus says so
        _ => return Ok(None),
    };
    Ok(Some(socket))
}

/// The two tasks of a connection: making it from what `open` gives (the builder and a handle to shut its socket) and
/// then `connected`, and doing its work until it closes.
fn start<M, F>(
    open: impl Future<Output = zbus::Result<(zbus::connection::Builder<'static>, Option<UnixStream>)>>
        + Send
        + 'static,
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> F + Send + 'static,
    closed: M,
) -> Vec<Command<M>>
where
    M: Send + 'static,
    F: Future<Output = M> + Send + 'static,
{
    // the connection once it is made, `None` when it could not be
    let made = Arc::new(Slot::<Option<Open>>::default());
    let given = Arc::clone(&made);
    let connect = async move {
        let connection = match open.await {
            Ok((builder, cut)) => builder
                .internal_executor(false)
                .build()
                .await
                .map(|connection| (Open::new(&connection, cut), connection)),
            Err(e) => Err(e),
        };
        let connection = match connection {
            Ok((open, connection)) => {
                given.fill(Some(open));
                Ok(connection)
            }
            Err(e) => {
                given.fill(None);
                Err(e)
            }
        };
        connected(connection).await
    };
    let run = async move {
        if let Some(mut open) = made.wait().await {
            open.serve(true).await;
        }
        closed
    };
    vec![
        Command::Perform(Box::pin(connect)),
        Command::Perform(Box::pin(run)),
    ]
}

/// A connection its runtime serves: its executor, a handle to shut its socket, and what tells when its last clone is
/// gone. Dropped before that, as when the runtime ends, it shuts the socket and hands the executor to a thread of its
/// own that runs it until the connection is gone.
struct Open {
    executor: zbus::Executor<'static>,
    cut: Option<UnixStream>,
    /// Ends once the connection's last clone is dropped; `None` once it ended.
    gone: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl Open {
    fn new(connection: &zbus::Connection, cut: Option<UnixStream>) -> Open {
        // the shutdown waits for every other clone: polled once, it listens and lets go of the clone it was given
        let mut gone: Pin<Box<dyn Future<Output = ()> + Send>> =
            Box::pin(connection.clone().graceful_shutdown());
        let listening = gone.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        Open {
            executor: connection.executor().clone(),
            cut,
            gone: listening.is_pending().then_some(gone),
        }
    }

    /// Runs the executor until the connection is gone or, when `until_empty`, it has no task left (its socket
    /// closed).
    async fn serve(&mut self, until_empty: bool) {
        let executor = self.executor.clone();
        while self.gone.is_some() && !(until_empty && executor.is_empty()) {
            let mut tick = std::pin::pin!(executor.tick());
            std::future::poll_fn(|cx| {
                if let Some(gone) = &mut self.gone {
                    if gone.as_mut().poll(cx).is_ready() {
                        self.gone = None;
                        return Poll::Ready(());
                    }
                }
                tick.as_mut().poll(cx)
            })
            .await;
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let Some(gone) = self.gone.take() else {
            return;
        };
        if let Some(cut) = &self.cut {
            // the bus sees the connection go now; its tasks see the socket closed and end
            let _ = cut.shutdown(Shutdown::Both);
        }
        let mut ended = Open {
            executor: self.executor.clone(),
            cut: self.cut.take(),
            gone: Some(gone),
        };
        let finishing = std::thread::Builder::new()
            .name("kit-bus-ended".into())
            .stack_size(ENDED_STACK_BYTES)
            // what is left on the executor goes with it once the connection is gone
            .spawn(move || zbus::block_on(ended.serve(false)));
        if let Err(e) = finishing {
            eprintln!(
                "kit: a bus connection its runtime left open is kept: no thread to finish it: {e}"
            );
        }
    }
}
