//! The message bus without a thread of its own: an instance's connection does its work (reading the socket, replies,
//! signals, the calls it serves) on the instance's UI thread, polled by the runtime as it polls the futures of
//! [`Command::Perform`], so nothing waits for the bus there and no executor thread runs beside the UI loop. Calls are
//! made with zbus's async API in a `Command::Perform` future, whose output is the message that carries the reply.

use crate::task::Slot;
use cupertino_ui_core::Command;
use std::future::Future;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

/// Which bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bus {
    Session,
    System,
}

/// Connects to `bus` from the UI thread without waiting for it: `connected` turns the connection, or why there is
/// none, into a message. The connection does its work on the UI thread for as long as it is open; when it closes (every
/// clone of it dropped, or the bus gone), and at once when it could not be made, the application hears `closed`.
pub fn connect<M>(
    bus: Bus,
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> M + Send + 'static,
    closed: M,
) -> Vec<Command<M>>
where
    M: Send + 'static,
{
    let builder = move || match bus {
        Bus::Session => zbus::connection::Builder::session(),
        Bus::System => zbus::connection::Builder::system(),
    };
    start(
        builder,
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
    start(
        move || Ok(zbus::connection::Builder::async_io_unix_stream(stream)),
        connected,
        closed,
    )
}

/// The two tasks of a connection: making it and then `connected`, and doing its work until it closes.
fn start<M, F>(
    builder: impl FnOnce() -> zbus::Result<zbus::connection::Builder<'static>> + Send + 'static,
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> F + Send + 'static,
    closed: M,
) -> Vec<Command<M>>
where
    M: Send + 'static,
    F: Future<Output = M> + Send + 'static,
{
    // the connection's executor once it is made, `None` when it could not be
    let made = Arc::new(Slot::<Option<zbus::Executor<'static>>>::default());
    let given = Arc::clone(&made);
    let connect = async move {
        let connection = match builder() {
            Ok(builder) => builder.internal_executor(false).build().await,
            Err(e) => Err(e),
        };
        given.fill(connection.as_ref().ok().map(|c| c.executor().clone()));
        connected(connection).await
    };
    let run = async move {
        if let Some(executor) = made.wait().await {
            while !executor.is_empty() {
                executor.tick().await;
            }
        }
        closed
    };
    vec![
        Command::Perform(Box::pin(connect)),
        Command::Perform(Box::pin(run)),
    ]
}
