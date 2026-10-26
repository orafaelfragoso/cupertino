//! `org.cupertino.Launcher1`: the control interface of the `cupertino-launcher` process. The server
//! calls `Toggle` when the launcher shortcut or hot corner fires.

use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::{connection, Connection};

/// Name, path and interface of the control interface (the server calls it by these strings).
pub const BUS_NAME: &str = "org.cupertino.Launcher1";
pub const OBJECT_PATH: &str = "/org/cupertino/Launcher1";

struct Control {
    on_toggle: Box<dyn Fn() + Send + Sync>,
}

#[zbus::interface(name = "org.cupertino.Launcher1")]
impl Control {
    /// Show the panel, or hide it when it is shown.
    fn toggle(&self) {
        (self.on_toggle)();
    }
}

/// Export the interface at [`OBJECT_PATH`] and claim [`BUS_NAME`] on the bus `builder` connects to.
/// `on_toggle` runs on the bus thread for every `Toggle` call. The interface is served while the
/// returned connection lives. Fails with `zbus::Error::NameTaken` when another launcher owns the name.
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
