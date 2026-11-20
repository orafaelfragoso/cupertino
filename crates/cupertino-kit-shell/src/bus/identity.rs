//! The component's session-bus connection and its identity (D20, `docs/architecture/shell-host.md#identity`):
//!
//! 1. The connection is made on a socket the host keeps a handle to, so `Running::shut` cuts it off.
//! 2. Its unique name is announced on the component's Wayland connection (`announce_bus_name`); the server answers
//!    a nonce (`bus_nonce`) there and nowhere else.
//! 3. The nonce goes back from that bus connection (`org.cupertino.Session1.AnnounceComponent`), and the server's
//!    answer (`bus_name`) becomes [`crate::Event::BusIdentity`].
//!
//! Only then does the component get the connection, so none of its calls precedes its identity. A development build
//! serves the debug interface (`org.cupertino.ShellComponentDebug1`, [`crate::crash`]) on the connection before step 2,
//! so a call that finds the component's connection by the name the server accepted is answered from the start.

use std::io;
use std::os::unix::net::UnixStream;
use std::sync::Arc;

use cupertino_kit::bus::{self as kit_bus, Bus};
use cupertino_kit::core::Command;
use cupertino_peer::component::{SESSION_BUS_NAME, SESSION_INTERFACE, SESSION_OBJECT_PATH};

use crate::context;
use crate::link::ComponentLink;
use crate::Event;

/// The method of `org.cupertino.Session1` that proves a component's bus connection with the nonce.
const ANNOUNCE_METHOD: &str = "AnnounceComponent";

pub(super) fn session<M: Send + 'static>(
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> M + Send + 'static,
    closed: M,
) -> Vec<Command<M>> {
    let host = context::with_ctx(|ctx| {
        ctx.link
            .clone()
            .map(|link| (link, Arc::clone(&ctx.sockets), Arc::clone(&ctx.inbox)))
    })
    .flatten();
    let Some((link, sockets, inbox)) = host else {
        // a program of its own: its process is its identity, and it announces nothing
        return kit_bus::connect(Bus::Session, connected, closed);
    };
    let stream = match session_socket() {
        Ok(stream) => stream,
        Err(e) => {
            tracing::warn!(error = %e, "no session bus for the component");
            let error = zbus::Error::InputOutput(Arc::new(e));
            return vec![
                Command::Perform(Box::pin(async move { connected(Err(error)) })),
                Command::Perform(Box::pin(async move { closed })),
            ];
        }
    };
    match stream.try_clone() {
        Ok(handle) => context::keep_socket(&sockets, handle),
        Err(e) => {
            tracing::warn!(error = %e, "the host keeps no handle to the component's bus socket")
        }
    }
    #[cfg(debug_assertions)]
    let serving = crate::crash::Serving::of_thread();
    kit_bus::connect_on(
        stream,
        move |connection| async move {
            if let Ok(bus) = &connection {
                #[cfg(debug_assertions)]
                if let Some(serving) = serving {
                    crate::crash::serve(bus, serving).await;
                }
                let accepted = prove(&link, bus).await;
                inbox.push(Event::BusIdentity { accepted });
            }
            connected(connection)
        },
        closed,
    )
}

/// Announces `bus`'s unique name and proves it; whether the server accepted it as the component's.
async fn prove(link: &ComponentLink, bus: &zbus::Connection) -> bool {
    let Some(name) = bus.unique_name().map(|name| name.to_string()) else {
        return false;
    };
    let Some(nonce) = link.announce(&name).await else {
        // refused without a nonce, or the connection failed: the component is not proved, and gets its connection
        return false;
    };
    let call = bus
        .call_method(
            Some(SESSION_BUS_NAME),
            SESSION_OBJECT_PATH,
            Some(SESSION_INTERFACE),
            ANNOUNCE_METHOD,
            &nonce,
        )
        .await;
    match call {
        // the method returns with the server's answer, which `bus_name` gives on the Wayland side as well
        Ok(_) => link.answer(&name).await.unwrap_or(true),
        Err(e) => {
            tracing::info!(error = %e, "the server did not accept the component's bus name");
            false
        }
    }
}

/// A socket connected to the session bus the environment names (`DBUS_SESSION_BUS_ADDRESS`, else the runtime
/// directory's `bus`).
fn session_socket() -> io::Result<UnixStream> {
    use zbus::address::transport::{Transport, UnixSocket};
    let address = zbus::Address::session().map_err(io::Error::other)?;
    let unsupported = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("session bus address {address} is not a Unix socket a client connects to"),
        )
    };
    match address.transport() {
        Transport::Unix(unix) => match unix.path() {
            UnixSocket::File(path) => UnixStream::connect(path),
            UnixSocket::Abstract(name) => {
                use std::os::linux::net::SocketAddrExt;
                let addr =
                    std::os::unix::net::SocketAddr::from_abstract_name(name.as_encoded_bytes())?;
                UnixStream::connect_addr(&addr)
            }
            _ => Err(unsupported()),
        },
        _ => Err(unsupported()),
    }
}
