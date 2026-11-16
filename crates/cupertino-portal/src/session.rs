//! The `org.freedesktop.impl.portal.Session` object of one session of ScreenCast or GlobalShortcuts, at the session's
//! handle while it lives. Only the frontend may close it.

use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{interface, Connection};

use crate::request::{answered, from_frontend, PortalError};

/// What a session does when the frontend closes it, given the connection the call came on.
type OnClose = Box<dyn Fn(&Connection) + Send + Sync>;

pub struct Session {
    closed: async_channel::Sender<()>,
    on_close: OnClose,
}

impl Session {
    /// A session whose `Close` sends on `closed` (so a call still waiting for the person returns) and then runs
    /// `on_close`, which ends the session; the object must be taken off the bus after the call returns, not from
    /// inside it.
    pub fn new(
        closed: async_channel::Sender<()>,
        on_close: impl Fn(&Connection) + Send + Sync + 'static,
    ) -> Self {
        Self {
            closed,
            on_close: Box::new(on_close),
        }
    }
}

#[interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), PortalError> {
        let result = async {
            from_frontend(&header, connection).await?;
            let _ = self.closed.try_send(());
            (self.on_close)(connection);
            Ok(())
        }
        .await;
        answered("org.freedesktop.impl.portal.Session", "Close", None, result)
    }

    /// The session ended without the frontend asking.
    #[zbus(signal)]
    pub async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}
