//! `org.cupertino.Settings1` (§8): one settings process per session. The running process owns the name and shows the
//! pane `ShowPane` names; a second start calls it and exits.

use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::{connection, Connection};

use crate::PaneId;

pub const BUS_NAME: &str = "org.cupertino.Settings1";
pub const OBJECT_PATH: &str = "/org/cupertino/Settings1";
pub const INTERFACE: &str = "org.cupertino.Settings1";

struct Instance {
    on_show: Box<dyn Fn(PaneId) + Send + Sync>,
}

#[zbus::interface(name = "org.cupertino.Settings1")]
impl Instance {
    /// Show the pane with the kebab-case id `pane_id` (the `--pane` ids); an unknown id is `InvalidArgs`.
    fn show_pane(&self, pane_id: &str) -> fdo::Result<()> {
        let pane = PaneId::from_key(pane_id).ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown pane {pane_id:?}")))?;
        (self.on_show)(pane);
        Ok(())
    }
}

/// Export the interface and claim [`BUS_NAME`] on the bus `builder` connects to; `on_show` runs on the bus thread for
/// every accepted call. Served while the returned connection lives. Fails with `zbus::Error::NameTaken` when another
/// settings process owns the name.
pub fn serve(builder: connection::Builder<'static>, on_show: impl Fn(PaneId) + Send + Sync + 'static) -> zbus::Result<Connection> {
    zbus::block_on(async {
        let connection = builder.serve_at(OBJECT_PATH, Instance { on_show: Box::new(on_show) })?.build().await?;
        // after the object exists, so no call finds the name without it; never queue behind the owner
        match connection.request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into()).await? {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(connection),
            RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
        }
    })
}

/// Ask the running settings process on the bus `builder` connects to to show the pane with the id `pane_id`.
pub fn show(builder: connection::Builder<'static>, pane_id: &str) -> zbus::Result<()> {
    zbus::block_on(async {
        let connection = builder.build().await?;
        connection.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), "ShowPane", &(pane_id,)).await.map(drop)
    })
}
