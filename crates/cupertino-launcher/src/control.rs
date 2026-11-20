//! `org.cupertino.Launcher1`: the launcher's bus interface, served on the launcher component's own session bus
//! connection, which owns the name (D20). The server calls `Toggle` when the launcher shortcut or hot corner fires, and
//! `ToggleApplications` when the application browser is asked for (the dock's hot corner or gesture, the `app-grid`
//! action).
//!
//! Both answer only the server: the connection that owns `org.cupertino.Session1`, from which the server makes these
//! calls. Any other caller, another connection of the server's process and the components of the shell host included,
//! is refused with `org.cupertino.Error.UntrustedCaller` (`cupertino_peer::CallerError`) and nothing changes.
//!
//! The handlers run on the component's UI thread, which ticks the connection: the check asks the bus daemon who owns
//! `org.cupertino.Session1` and reads nothing else, and an accepted call goes to the component as a [`Call`].

use cupertino_peer::component::SESSION_BUS_NAME;
use cupertino_peer::{CallerError, Denial};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::Connection;

/// Name, path and interface of the control interface (the server calls it by these strings).
pub const BUS_NAME: &str = "org.cupertino.Launcher1";
pub const OBJECT_PATH: &str = "/org/cupertino/Launcher1";

/// A call the interface took.
#[derive(Clone, Debug)]
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
    fn send(&self, call: Call) {
        // the component holds the receiver for as long as the interface is served
        let _ = self.calls.try_send(call);
    }

    /// Hands `call` to the component when the server sent it; a refusal is logged with the method.
    async fn answer(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        member: &str,
        call: Call,
    ) -> Result<(), CallerError> {
        if !from_server(connection, header).await {
            eprintln!("cupertino-launcher: {BUS_NAME}.{member}: the caller is refused: it is not the server");
            return Err(CallerError::from(Denial::Untrusted));
        }
        self.send(call);
        Ok(())
    }
}

/// Whether the call with `header` comes from the server: its sender owns `org.cupertino.Session1` (D20). A call
/// without a sender, or one the bus daemon cannot say the owner for, is not the server's.
async fn from_server(connection: &Connection, header: &Header<'_>) -> bool {
    let Some(sender) = header.sender() else {
        return false;
    };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        return false;
    };
    let Ok(session) = BusName::try_from(SESSION_BUS_NAME) else {
        return false;
    };
    dbus.get_name_owner(session)
        .await
        .is_ok_and(|owner| owner.as_str() == sender.as_str())
}

#[zbus::interface(name = "org.cupertino.Launcher1")]
impl Control {
    /// Show the panel, or hide it when it is shown. Only the server may.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        self.answer(&header, connection, "Toggle", Call::Toggle)
            .await
    }

    /// Show the application browser (the search field unfolds into it when it is shown), or hide it when it is
    /// shown. Only the server may.
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
/// `calls` once its caller is checked. Fails with `zbus::Error::NameTaken` when another launcher owns the name.
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
