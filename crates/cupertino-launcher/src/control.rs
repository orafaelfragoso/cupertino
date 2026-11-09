//! `org.cupertino.Launcher1`: the control interface of the `cupertino-launcher` process. The server
//! calls `Toggle` when the launcher shortcut or hot corner fires, and `ToggleApplications` when the
//! application browser is asked for (the Dock's hot corner or gesture, the `app-grid` action). The
//! server started this process, so both answer only the server (this process's parent) and this
//! process itself; any other caller is refused with an `org.cupertino.Error.*` name
//! (`cupertino_peer::check_call`).

use cupertino_peer::{check_call, CallerError, Relation};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::{connection, Connection};

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
    on_call: Box<dyn Fn(Call) + Send + Sync>,
}

impl Control {
    async fn answer(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        member: &'static str,
        call: Call,
    ) -> Result<(), CallerError> {
        check_call(
            connection,
            header,
            "org.cupertino.Launcher1",
            member,
            &[Relation::Itself, Relation::Parent],
        )
        .await?;
        (self.on_call)(call);
        Ok(())
    }
}

#[zbus::interface(name = "org.cupertino.Launcher1")]
impl Control {
    /// Show the panel, or hide it when it is shown. Only the server and this process may.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        self.answer(&header, connection, "Toggle", Call::Toggle)
            .await
    }

    /// Show the application browser (the search field unfolds into it when it is shown), or hide it when it is
    /// shown. Only the server and this process may.
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

/// Export the interface at [`OBJECT_PATH`] and claim [`BUS_NAME`] on the bus `builder` connects to.
/// `on_call` runs on the bus thread for every call taken. The interface is served while the
/// returned connection lives. Fails with `zbus::Error::NameTaken` when another launcher owns the name.
pub fn serve(
    builder: connection::Builder<'static>,
    on_call: impl Fn(Call) + Send + Sync + 'static,
) -> zbus::Result<Connection> {
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                OBJECT_PATH,
                Control {
                    on_call: Box::new(on_call),
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
