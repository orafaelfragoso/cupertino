//! What every dialog of the backend shares: only xdg-desktop-portal may call it, and the frontend may end a call early
//! through the `org.freedesktop.impl.portal.Request` object at the call's handle.

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};
use zbus::{interface, Connection};

/// The name of the frontend, the only caller of the backend.
pub const FRONTEND: &str = "org.freedesktop.portal.Desktop";

/// How a call ended, as the frontend reads `response`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Response {
    /// The user went ahead.
    Success = 0,
    /// The user cancelled.
    Cancelled = 1,
    /// The call ended another way: closed by the frontend, or the dialog went away.
    Other = 2,
}

/// Why a call was refused before any dialog showed.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.DBus.Error")]
pub enum PortalError {
    #[zbus(error)]
    ZBus(zbus::Error),
    AccessDenied(String),
    InvalidArgs(String),
    LimitsExceeded(String),
}

/// `result` of `method` of `interface`, which `app_id` asked for through the frontend, as it was; a refusal is recorded
/// as `portal.request.failed` with its D-Bus error name.
pub fn answered<T>(
    interface: &'static str,
    method: &'static str,
    app_id: Option<&str>,
    result: Result<T, PortalError>,
) -> Result<T, PortalError> {
    if let Err(error) = &result {
        let mut event = diag::event(
            Severity::Warn,
            "portal.request.failed",
            "a portal request was refused",
        )
        .field("interface", Value::Public(interface))
        .field("method", Value::Public(method));
        if let Some(app_id) = app_id {
            event = event.field("app_id", Value::Public(app_id));
        }
        event
            .field(
                "error",
                Value::Public(zbus::DBusError::name(error).as_str()),
            )
            .emit();
    }
    result
}

/// Refuses a call that does not come from the current owner of [`FRONTEND`], so no application can skip the
/// frontend's permission store.
pub async fn from_frontend(
    header: &Header<'_>,
    connection: &Connection,
) -> Result<(), PortalError> {
    let denied =
        || PortalError::AccessDenied("only xdg-desktop-portal may call the backend".to_owned());
    let sender = header.sender().ok_or_else(denied)?;
    let owner = DBusProxy::new(connection)
        .await?
        .get_name_owner(BusName::try_from(FRONTEND).map_err(zbus::Error::from)?)
        .await
        .map_err(|_| denied())?;
    if *sender == *owner {
        Ok(())
    } else {
        Err(denied())
    }
}

/// The `Request` object of one call, at its handle while the call runs.
pub struct Request {
    closed: async_channel::Sender<()>,
}

#[interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    async fn close(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), PortalError> {
        let result = async {
            from_frontend(&header, connection).await?;
            let _ = self.closed.try_send(());
            Ok(())
        }
        .await;
        answered("org.freedesktop.impl.portal.Request", "Close", None, result)
    }
}

/// Serves the `Request` object at `handle` until dropped; [`Exported::closed`] ends when the frontend closes it.
pub struct Exported {
    connection: Connection,
    handle: OwnedObjectPath,
    closed: async_channel::Receiver<()>,
}

impl Exported {
    pub async fn at(connection: &Connection, handle: ObjectPath<'_>) -> Result<Self, PortalError> {
        let (sender, closed) = async_channel::bounded(1);
        let added = connection
            .object_server()
            .at(&handle, Request { closed: sender })
            .await?;
        if !added {
            return Err(PortalError::InvalidArgs(format!(
                "{handle} is already in use"
            )));
        }
        Ok(Self {
            connection: connection.clone(),
            handle: handle.into(),
            closed,
        })
    }

    pub fn handle(&self) -> &OwnedObjectPath {
        &self.handle
    }

    /// Resolves when the frontend calls `Close`.
    pub async fn closed(&self) {
        let _ = self.closed.recv().await;
    }

    /// Stop serving the object.
    pub async fn remove(self) {
        let _ = self
            .connection
            .object_server()
            .remove::<Request, _>(&self.handle)
            .await;
    }
}
