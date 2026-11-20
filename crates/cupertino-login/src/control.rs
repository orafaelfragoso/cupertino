//! `org.cupertino.Login1`: the control interface of the lock screen, with one method. The server calls `Lock` to lock
//! the session while no lock screen is connected to ask through `cupertino_session_v1`; the session agent's panels,
//! the power key and the session menu's state are the `agent` component's (`org.cupertino.SessionAgent1` and
//! `SessionMenu` on the host bus).
//!
//! `Lock` checks its caller (`cupertino-peer`): only the server, which started this process, and this process itself
//! may call. An anonymous, sandboxed or unrelated caller, an application the server started included, gets an error of
//! `org.cupertino.Error.*` and nothing happens. The interface is served on the session bus connection the lock
//! screen's loop ticks (`cupertino_kit::bus`).

use cupertino_peer::{check_call, CallerError, Relation};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::Connection;

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

/// Export the interface at [`OBJECT_PATH`] on `connection` and claim [`BUS_NAME`]. `on_request` runs on the
/// connection's executor for every call; it must not wait. The interface is served while the connection is open.
/// Fails with `zbus::Error::NameTaken` when another lock screen owns the name: the interface is served all the same,
/// unnamed, and the server's calls go to the owner of the name and are refused there; this process then runs on the
/// requests of the server's own connection and of logind.
pub async fn serve(
    connection: &Connection,
    on_request: impl Fn(Request) + Send + Sync + 'static,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(
            OBJECT_PATH,
            Control {
                on_request: Box::new(on_request),
            },
        )
        .await?;
    // after the object exists, so no call finds the name without it; never queue behind the owner
    match connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await?
    {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(()),
        RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
    }
}
