//! `org.cupertino.Launcher1`: the launcher's bus interface, served on the launcher component's own session bus
//! connection, which owns the name (D20). The server calls `Toggle` when the launcher shortcut or hot corner fires, and
//! `ToggleApplications` when the application browser is asked for (the dock's hot corner or gesture, the `app-grid`
//! action).
//!
//! Both answer only the server and the launcher component itself (a call from its own connection); any other caller,
//! another component of the shell host included, is refused with an `org.cupertino.Error.*` name
//! (`cupertino_peer::CallerError`) and nothing changes. The server makes these calls from its bus worker's connection,
//! which is not the one that owns `org.cupertino.Session1`, so the server is known as the host's parent process.
//!
//! The handlers run on the component's UI thread, which ticks the connection, and do no work of their own there: a
//! call goes to the component as a [`Call`], and the component names the caller from the credentials the bus gave for
//! it off the UI thread ([`judge`]), since that reads the caller's `/proc` entries.

use std::path::Path;
use std::sync::Arc;

use cupertino_peer::{own_parent, Caller, CallerError, Relation};
use zbus::fdo::{ConnectionCredentials, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::Connection;

/// Name, path and interface of the control interface (the server calls it by these strings).
pub const BUS_NAME: &str = "org.cupertino.Launcher1";
pub const OBJECT_PATH: &str = "/org/cupertino/Launcher1";

/// The `/proc` the callers are read from.
const PROC: &str = "/proc";

/// Who may call besides the launcher itself: the server, the shell host's parent.
const CALLERS: &[Relation] = &[Relation::Parent];

/// A call the interface took.
#[derive(Clone, Debug)]
pub enum Call {
    /// Name the caller of the credentials (`None`: the bus gave none) for the call that waits on `reply`.
    Judge {
        credentials: Option<Arc<ConnectionCredentials>>,
        reply: async_channel::Sender<Arc<Caller>>,
    },
    /// `Toggle`: the search field opens, or whatever is shown closes.
    Toggle,
    /// `ToggleApplications`: the application browser opens, or closes when it is shown.
    Applications,
}

/// The caller of `credentials`, read from its `/proc` entries: for the I/O pool, never the UI thread.
pub fn judge(credentials: Option<&ConnectionCredentials>) -> Caller {
    match credentials {
        Some(credentials) => Caller::from_credentials(credentials, Path::new(PROC)),
        None => Caller::Anonymous,
    }
}

struct Control {
    calls: async_channel::Sender<Call>,
}

impl Control {
    fn send(&self, call: Call) {
        // the component holds the receiver for as long as the interface is served
        let _ = self.calls.try_send(call);
    }

    /// Hands `call` to the component when the launcher itself or the server sent it; a refusal is logged with the
    /// method.
    async fn answer(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        member: &str,
        call: Call,
    ) -> Result<(), CallerError> {
        let itself = header
            .sender()
            .is_some_and(|sender| connection.unique_name().is_some_and(|own| **own == *sender));
        if !itself {
            let caller = self.caller(connection, header).await;
            caller
                .require_relation(std::process::id(), own_parent(), CALLERS)
                .map_err(|denial| {
                    eprintln!(
                        "cupertino-launcher: {BUS_NAME}.{member}: the caller is refused: {denial:?}"
                    );
                    CallerError::from(denial)
                })?;
        }
        self.send(call);
        Ok(())
    }

    /// The caller of a call with `header`, named by the component; anonymous when the bus or the component gave no
    /// answer.
    async fn caller(&self, connection: &Connection, header: &Header<'_>) -> Arc<Caller> {
        let credentials = credentials(connection, header).await;
        let (reply, answer) = async_channel::bounded(1);
        self.send(Call::Judge { credentials, reply });
        answer
            .recv()
            .await
            .unwrap_or_else(|_| Arc::new(Caller::Anonymous))
    }
}

/// The credentials the bus gives for the sender of a call with `header`; `None` when it gives none.
async fn credentials(
    connection: &Connection,
    header: &Header<'_>,
) -> Option<Arc<ConnectionCredentials>> {
    let sender = header.sender()?;
    let proxy = zbus::fdo::DBusProxy::new(connection).await.ok()?;
    proxy
        .get_connection_credentials(sender.clone().into())
        .await
        .ok()
        .map(Arc::new)
}

#[zbus::interface(name = "org.cupertino.Launcher1")]
impl Control {
    /// Show the panel, or hide it when it is shown. Only the server and the launcher may.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        self.answer(&header, connection, "Toggle", Call::Toggle)
            .await
    }

    /// Show the application browser (the search field unfolds into it when it is shown), or hide it when it is
    /// shown. Only the server and the launcher may.
    async fn toggle_applications(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        self.answer(
            &header,
            connection,
            "ToggleApplications",
            Call::Applications,
        )
        .await
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; every call goes to
/// `calls`, the callers to name first. Fails with `zbus::Error::NameTaken` when another launcher owns the name.
pub async fn serve(
    connection: &Connection,
    calls: async_channel::Sender<Call>,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(OBJECT_PATH, Control { calls })
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
