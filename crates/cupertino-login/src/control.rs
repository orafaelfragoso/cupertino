//! `org.cupertino.Login1`: the control interface of the lock screen, with one method. The server calls `Lock` to lock
//! the session while no lock screen is connected to ask through `cupertino_session_v1`; the session agent's panels,
//! the power key and the session menu's state are the `agent` component's (`org.cupertino.SessionAgent1` and
//! `SessionMenu` on the host bus).
//!
//! `Lock` checks its caller (`cupertino-peer`): only the server, which started this process, and this process itself
//! may call. An anonymous, sandboxed or unrelated caller, an application the server started included, gets an error of
//! `org.cupertino.Error.*` and nothing happens.

use cupertino_peer::{check_call, CallerError, Relation};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::{connection, Connection};

/// Name, path and interface of the control interface (the callers use these strings).
pub const BUS_NAME: &str = "org.cupertino.Login1";
pub const OBJECT_PATH: &str = "/org/cupertino/Login1";

/// Who may call: the server, which started this process, and this process itself.
const SERVER: &[Relation] = &[Relation::Itself, Relation::Parent];

/// What a caller of the interface asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `Lock`: lock the session (the display slept, the lock shortcut while no lock screen was connected).
    Lock,
}

struct Control {
    on_request: Box<dyn Fn(Request) + Send + Sync>,
}

#[zbus::interface(name = "org.cupertino.Login1")]
impl Control {
    /// Lock the session, or do nothing when it is locked already.
    async fn lock(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), CallerError> {
        check_call(connection, &header, "org.cupertino.Login1", "Lock", SERVER).await?;
        (self.on_request)(Request::Lock);
        Ok(())
    }
}

/// The served interface; it stays on the bus while this lives.
#[derive(Clone)]
pub struct Service {
    connection: Connection,
}

impl Service {
    /// The session bus connection the interface is served on.
    pub fn connection(&self) -> zbus::blocking::Connection {
        zbus::blocking::Connection::from(self.connection.clone())
    }
}

/// Export the interface at [`OBJECT_PATH`] and claim [`BUS_NAME`] on the bus `builder` connects to. `on_request` runs on
/// the bus thread for every call. The interface is served while the returned [`Service`] lives. Fails with `zbus::Error::NameTaken` when another lock screen owns the name.
pub fn serve(
    builder: connection::Builder<'static>,
    on_request: impl Fn(Request) + Send + Sync + 'static,
) -> zbus::Result<Service> {
    serve_with(builder, on_request, true)
}

/// Like [`serve`] without claiming [`BUS_NAME`]: for a process whose name another process holds. The server's calls go to
/// the owner of the name and are refused there; this process then runs on the requests of the server's private
/// connection and of logind.
pub fn serve_unnamed(
    builder: connection::Builder<'static>,
    on_request: impl Fn(Request) + Send + Sync + 'static,
) -> zbus::Result<Service> {
    serve_with(builder, on_request, false)
}

fn serve_with(
    builder: connection::Builder<'static>,
    on_request: impl Fn(Request) + Send + Sync + 'static,
    claim_name: bool,
) -> zbus::Result<Service> {
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                OBJECT_PATH,
                Control {
                    on_request: Box::new(on_request),
                },
            )?
            .build()
            .await?;
        if !claim_name {
            return Ok(Service { connection });
        }
        // after the object exists, so no call finds the name without it; never queue behind the owner
        match connection
            .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await?
        {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
                Ok(Service { connection })
            }
            RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
        }
    })
}
