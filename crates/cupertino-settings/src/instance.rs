//! `org.cupertino.Settings1` (§8): one settings process per session. The running process owns the name and shows the
//! pane `ShowPane` names; a second start calls it and exits.

use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::{connection, Connection};

use crate::PaneId;

pub const BUS_NAME: &str = "org.cupertino.Settings1";
pub const OBJECT_PATH: &str = "/org/cupertino/Settings1";
pub const INTERFACE: &str = "org.cupertino.Settings1";

pub const ABOUT_BUS_NAME: &str = "org.cupertino.Settings.About1";
const ABOUT_PATH: &str = "/org/cupertino/Settings/About1";

struct AboutInstance {
    on_show: Box<dyn Fn(String) + Send + Sync>,
}

#[zbus::interface(name = "org.cupertino.Settings.About1")]
impl AboutInstance {
    fn show(&self, activation_token: String) -> fdo::Result<()> {
        if activation_token.len() > 4096 {
            return Err(fdo::Error::InvalidArgs(
                "activation token exceeds 4096 bytes".into(),
            ));
        }
        (self.on_show)(activation_token);
        Ok(())
    }
}

/// About has a separate owner, so its lifetime and repeat launches never change the Settings pane.
pub fn serve_about(
    builder: connection::Builder<'static>,
    on_show: impl Fn(String) + Send + Sync + 'static,
) -> zbus::Result<Connection> {
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                ABOUT_PATH,
                AboutInstance {
                    on_show: Box::new(on_show),
                },
            )?
            .build()
            .await?;
        match connection
            .request_name_with_flags(ABOUT_BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await?
        {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(connection),
            RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
        }
    })
}

pub fn show_about(
    builder: connection::Builder<'static>,
    activation_token: &str,
) -> zbus::Result<()> {
    zbus::block_on(async {
        builder
            .build()
            .await?
            .call_method(
                Some(ABOUT_BUS_NAME),
                ABOUT_PATH,
                Some(ABOUT_BUS_NAME),
                "Show",
                &(activation_token,),
            )
            .await
            .map(drop)
    })
}

struct Instance {
    on_show: Box<dyn Fn(PaneId, String) + Send + Sync>,
}

#[zbus::interface(name = "org.cupertino.Settings1")]
impl Instance {
    /// Show the pane with the kebab-case id `pane_id` (the `--pane` ids); an unknown id is `InvalidArgs`.
    fn show_pane(&self, pane_id: &str) -> fdo::Result<()> {
        let pane = PaneId::from_key(pane_id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown pane {pane_id:?}")))?;
        (self.on_show)(pane, String::new());
        Ok(())
    }

    /// Show and activate Settings with the caller's grant; the compositor validates and consumes it.
    fn show_pane_with_token(&self, pane_id: &str, activation_token: String) -> fdo::Result<()> {
        if activation_token.len() > 4096 {
            return Err(fdo::Error::InvalidArgs(
                "activation token exceeds 4096 bytes".into(),
            ));
        }
        let pane = PaneId::from_key(pane_id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("unknown pane {pane_id:?}")))?;
        (self.on_show)(pane, activation_token);
        Ok(())
    }
}

/// Export the interface and claim [`BUS_NAME`] on the bus `builder` connects to; `on_show` runs on the bus thread for
/// every accepted call. Served while the returned connection lives. Fails with `zbus::Error::NameTaken` when another
/// settings process owns the name.
pub fn serve(
    builder: connection::Builder<'static>,
    on_show: impl Fn(PaneId, String) + Send + Sync + 'static,
) -> zbus::Result<Connection> {
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                OBJECT_PATH,
                Instance {
                    on_show: Box::new(on_show),
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

/// Ask the running settings process on the bus `builder` connects to to show the pane with the id `pane_id`.
pub fn show(
    builder: connection::Builder<'static>,
    pane_id: &str,
    activation_token: &str,
) -> zbus::Result<()> {
    zbus::block_on(async {
        let connection = builder.build().await?;
        if activation_token.is_empty() {
            connection
                .call_method(
                    Some(BUS_NAME),
                    OBJECT_PATH,
                    Some(INTERFACE),
                    "ShowPane",
                    &(pane_id,),
                )
                .await
                .map(drop)
        } else {
            connection
                .call_method(
                    Some(BUS_NAME),
                    OBJECT_PATH,
                    Some(INTERFACE),
                    "ShowPaneWithToken",
                    &(pane_id, activation_token),
                )
                .await
                .map(drop)
        }
    })
}
