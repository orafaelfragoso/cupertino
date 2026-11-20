//! `org.cupertino.SessionAgent1`: what the server asks of the session agent, served on the component's own session bus
//! connection, which also owns the name. Every method is the server's alone: a call from any other connection (D20:
//! only the connection that owns `org.cupertino.Session1` is the server's), another connection of the server's process,
//! a sandboxed or anonymous one included, gets `org.freedesktop.DBus.Error.AccessDenied` and changes nothing.
//!
//! | Method | What the agent does |
//! |---|---|
//! | `RequestLogOut` | shows the log out panel (in place of one that is open; not on a locked session) |
//! | `RequestRestart`, `RequestShutDown` | shows the restart or the shut down panel |
//! | `RequestPowerDialog` | shows the panel a held power key shows |
//! | `PowerKey(b pressed)` | the power key went down or up: a short press sleeps, a hold shows the power panel |
//! | `RefreshCapabilities` | reads again what the machine allows, which the session menu publishes |

use cupertino_kit_shell::bus::{self, Caller};
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::Connection;

use crate::panel::Kind;

/// Name, path and interface of the control interface (the server calls it by these strings).
pub const BUS_NAME: &str = "org.cupertino.SessionAgent1";
pub const OBJECT_PATH: &str = "/org/cupertino/SessionAgent1";

/// What the server asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Panel(Kind),
    PowerKey { pressed: bool },
    RefreshCapabilities,
}

struct Control {
    requests: async_channel::Sender<Request>,
}

impl Control {
    /// Hands `request` to the component when the server sent it.
    async fn accept_from_server(
        &self,
        header: &zbus::message::Header<'_>,
        connection: &Connection,
        request: Request,
    ) -> fdo::Result<()> {
        let allowed = match header.sender() {
            Some(sender) => bus::caller(connection, sender).await == Caller::Server,
            None => false,
        };
        if !allowed {
            return Err(fdo::Error::AccessDenied(format!(
                "only the server may ask the session agent for {request:?}"
            )));
        }
        // the component holds the receiver for as long as the interface is served
        let _ = self.requests.try_send(request);
        Ok(())
    }
}

#[zbus::interface(name = "org.cupertino.SessionAgent1")]
impl Control {
    async fn request_log_out(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Request::Panel(Kind::LogOut))
            .await
    }

    async fn request_restart(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Request::Panel(Kind::Restart))
            .await
    }

    async fn request_shut_down(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Request::Panel(Kind::ShutDown))
            .await
    }

    async fn request_power_dialog(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Request::Panel(Kind::PowerButton))
            .await
    }

    async fn power_key(
        &self,
        pressed: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Request::PowerKey { pressed })
            .await
    }

    async fn refresh_capabilities(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> fdo::Result<()> {
        self.accept_from_server(&header, connection, Request::RefreshCapabilities)
            .await
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; every accepted call goes
/// to `requests`. Fails with `zbus::Error::NameTaken` when another agent owns the name.
pub async fn serve(
    connection: &Connection,
    requests: async_channel::Sender<Request>,
) -> zbus::Result<()> {
    connection
        .object_server()
        .at(OBJECT_PATH, Control { requests })
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
