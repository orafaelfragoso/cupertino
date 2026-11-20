//! `org.cupertino.Launcher1`: the launcher's bus interface, served on the launcher component's own session bus
//! connection, which owns the name (D20). The server calls `Toggle` when the launcher shortcut or hot corner fires, and
//! `ToggleApplications` when the application browser is asked for (the dock's hot corner or gesture, the `app-grid`
//! action).
//!
//! Both answer only the server (the connection that owns `org.cupertino.Session1`) and the launcher component itself;
//! any other caller, another component of the shell host or another connection of the server's process included, gets
//! `org.freedesktop.DBus.Error.AccessDenied` and nothing changes. The handlers do no work of their own: a call goes to
//! the component as a [`Call`].

use cupertino_kit_shell::bus::{self, Caller};
use cupertino_kit_shell::ComponentName;
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::Connection;

/// Name, path and interface of the control interface (the server calls it by these strings).
pub const BUS_NAME: &str = "org.cupertino.Launcher1";
pub const OBJECT_PATH: &str = "/org/cupertino/Launcher1";

/// A call the interface took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// `Toggle`: the search field opens, or whatever is shown closes.
    Toggle,
    /// `ToggleApplications`: the application browser opens, or closes when it is shown.
    Applications,
}

struct Control {
    calls: async_channel::Sender<Call>,
}

impl Control {
    /// Hands `call` to the component when the server or the launcher itself sent it.
    async fn answer(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        member: &str,
        call: Call,
    ) -> fdo::Result<()> {
        let caller = match header.sender() {
            Some(sender) => bus::caller(connection, sender).await,
            None => Caller::Other,
        };
        if !matches!(
            caller,
            Caller::Server | Caller::Component(ComponentName::LAUNCHER)
        ) {
            return Err(fdo::Error::AccessDenied(format!(
                "only the server may call {BUS_NAME}.{member}"
            )));
        }
        // the component holds the receiver for as long as the interface is served
        let _ = self.calls.try_send(call);
        Ok(())
    }
}

#[zbus::interface(name = "org.cupertino.Launcher1")]
impl Control {
    /// Show the panel, or hide it when it is shown. Only the server and the launcher may.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.answer(&header, connection, "Toggle", Call::Toggle)
            .await
    }

    /// Show the application browser (the search field unfolds into it when it is shown), or hide it when it is
    /// shown. Only the server and the launcher may.
    async fn toggle_applications(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.answer(
            &header,
            connection,
            "ToggleApplications",
            Call::Applications,
        )
        .await
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; every accepted call goes
/// to `calls`. Fails with `zbus::Error::NameTaken` when another launcher owns the name.
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
