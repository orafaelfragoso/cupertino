//! `org.cupertino.ControlPanel1`: the control interface of the `cupertino-controlcenter` process. The server (shortcut)
//! and the menu bar (status item click) call `Toggle`. The menu bar is no relation of this process (the server started
//! both), so `Toggle` answers any unsandboxed process of the person and refuses a Flatpak, a Snap and a caller the bus
//! cannot name (`cupertino_peer::check_call`), as `org.cupertino.NotificationCenter1` does for the same callers.

use cupertino_peer::{check_call, CallerError, Relation};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::{connection, Connection};

/// Who may toggle the panel: the server (the parent), the menu bar (no relation) and this process; never a sandboxed or
/// anonymous caller.
const CALLERS: &[Relation] = &[Relation::Itself, Relation::Parent, Relation::Other];

/// Name, path and interface of the control interface (the callers use these strings).
pub const BUS_NAME: &str = "org.cupertino.ControlPanel1";
pub const OBJECT_PATH: &str = "/org/cupertino/ControlPanel1";

struct Control {
    on_toggle: Box<dyn Fn() + Send + Sync>,
}

#[zbus::interface(name = "org.cupertino.ControlPanel1")]
impl Control {
    /// Open the panel, or dismiss it when it is open. Refused to a sandboxed or anonymous caller.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        check_call(
            connection,
            &header,
            "org.cupertino.ControlPanel1",
            "Toggle",
            CALLERS,
        )
        .await?;
        (self.on_toggle)();
        Ok(())
    }
}

/// Export the interface at [`OBJECT_PATH`] and claim [`BUS_NAME`] on the bus `builder` connects to. `on_toggle` runs on
/// the bus thread for every `Toggle` call. The interface is served while the returned connection lives. Fails with
/// `zbus::Error::NameTaken` when another panel owns the name.
pub fn serve(
    builder: connection::Builder<'static>,
    on_toggle: impl Fn() + Send + Sync + 'static,
) -> zbus::Result<Connection> {
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                OBJECT_PATH,
                Control {
                    on_toggle: Box::new(on_toggle),
                },
            )?
            .build()
            .await?;
        // after the object exists, so no call finds the name without it; never queue behind the owner
        match connection
            .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await?
        {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(connection),
            RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
        }
    })
}
