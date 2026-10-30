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

/// Hand `activation_token` to the running About. Only an owner that runs this very program gets it: another program
/// holding the name would otherwise receive the launcher's focus grant. The call goes to the owner that was checked.
pub fn show_about(
    builder: connection::Builder<'static>,
    activation_token: &str,
) -> zbus::Result<()> {
    zbus::block_on(async {
        let connection = builder.build().await?;
        let owner = checked_owner(&connection, ABOUT_BUS_NAME).await?;
        connection
            .call_method(
                Some(owner.as_str()),
                ABOUT_PATH,
                Some(ABOUT_BUS_NAME),
                "Show",
                &(activation_token,),
            )
            .await
            .map(drop)
    })
}

/// The owner of `name`, when it runs this very program; refused (and logged) otherwise, so another program holding the
/// name never receives a launch's activation token.
async fn checked_owner(
    connection: &Connection,
    name: &'static str,
) -> zbus::Result<zbus::names::OwnedUniqueName> {
    let daemon = fdo::DBusProxy::new(connection).await?;
    let owner = daemon
        .get_name_owner(name.try_into().expect("a valid bus name"))
        .await?;
    let pid = daemon
        .get_connection_unix_process_id(owner.as_ref().into())
        .await?;
    if runs_this_program(pid) {
        Ok(owner)
    } else {
        eprintln!("cupertino-settings: {name} is owned by another program; the launch is not handed to it");
        Err(zbus::Error::Failure(format!(
            "{name} is owned by another program"
        )))
    }
}

/// Whether process `pid` runs the same executable as this process.
fn runs_this_program(pid: u32) -> bool {
    match (
        std::fs::read_link(format!("/proc/{pid}/exe")),
        std::env::current_exe(),
    ) {
        (Ok(owner), Ok(own)) => same_program(&owner, &own),
        _ => false,
    }
}

/// Whether `owner`, a process's executable link, is `own`. A binary replaced by a package upgrade while it runs reads
/// "<path> (deleted)"; it still ran from the same installed path.
fn same_program(owner: &std::path::Path, own: &std::path::Path) -> bool {
    let owner = owner.as_os_str().as_encoded_bytes();
    let owner = owner.strip_suffix(b" (deleted)").unwrap_or(owner);
    owner == own.as_os_str().as_encoded_bytes()
}

/// Whether a hand-over failed because About's owner left the bus after the name was found taken.
fn owner_left(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
        ),
        zbus::Error::FDO(e) => matches!(
            **e,
            fdo::Error::ServiceUnknown(_) | fdo::Error::NameHasNoOwner(_)
        ),
        _ => false,
    }
}

/// Serve About, or hand the launch to the About that runs (`Ok(None)`). An owner that leaves between the two, as an
/// About quitting while it is launched again, lets this launch claim the name once more.
pub fn serve_or_show_about<C>(
    mut serve: impl FnMut() -> zbus::Result<C>,
    mut show: impl FnMut() -> zbus::Result<()>,
) -> zbus::Result<Option<C>> {
    let mut retried = false;
    loop {
        match serve() {
            Ok(connection) => return Ok(Some(connection)),
            Err(zbus::Error::NameTaken) => match show() {
                Ok(()) => return Ok(None),
                Err(e) if owner_left(&e) && !retried => retried = true,
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
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

/// Ask the running settings process on the bus `builder` connects to to show the pane with the id `pane_id`. A launch
/// with an activation token hands it only to an owner that runs this very program, and calls the owner it checked.
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
            let owner = checked_owner(&connection, BUS_NAME).await?;
            connection
                .call_method(
                    Some(owner.as_str()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn left() -> zbus::Error {
        zbus::Error::FDO(Box::new(fdo::Error::NameHasNoOwner("gone".into())))
    }

    #[test]
    fn a_taken_name_hands_the_launch_over() {
        let mut serves = 0;
        let outcome = serve_or_show_about(
            || -> zbus::Result<()> {
                serves += 1;
                Err(zbus::Error::NameTaken)
            },
            || Ok(()),
        );
        assert!(matches!(outcome, Ok(None)));
        assert_eq!(serves, 1);
    }

    #[test]
    fn an_owner_that_left_lets_the_launch_claim_the_name_once_more() {
        let mut serves = 0;
        let outcome = serve_or_show_about(
            || {
                serves += 1;
                if serves == 1 {
                    Err(zbus::Error::NameTaken)
                } else {
                    Ok("owner")
                }
            },
            || Err(left()),
        );
        assert!(matches!(outcome, Ok(Some("owner"))));
        assert_eq!(serves, 2);
    }

    #[test]
    fn the_name_is_claimed_again_only_once_and_other_failures_are_not_retried() {
        let mut serves = 0;
        let outcome = serve_or_show_about(
            || -> zbus::Result<()> {
                serves += 1;
                Err(zbus::Error::NameTaken)
            },
            || Err(left()),
        );
        assert!(outcome.is_err());
        assert_eq!(serves, 2);

        let mut serves = 0;
        let outcome = serve_or_show_about(
            || -> zbus::Result<()> {
                serves += 1;
                Err(zbus::Error::NameTaken)
            },
            || Err(zbus::Error::Failure("refused".into())),
        );
        assert!(matches!(outcome, Err(zbus::Error::Failure(_))));
        assert_eq!(serves, 1);
    }

    #[test]
    fn a_binary_replaced_by_an_upgrade_is_still_this_program() {
        use std::path::Path;
        let own = Path::new("/usr/bin/cupertino-settings");
        assert!(same_program(own, own));
        assert!(same_program(
            Path::new("/usr/bin/cupertino-settings (deleted)"),
            own
        ));
        assert!(!same_program(Path::new("/usr/bin/other"), own));
        assert!(!same_program(Path::new("/usr/bin/other (deleted)"), own));
        assert!(!same_program(
            Path::new("/usr/bin/cupertino-settings (deleted) (deleted)"),
            own
        ));
    }

    #[test]
    fn only_this_program_counts_as_the_owner() {
        assert!(runs_this_program(std::process::id()));
        let other = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let pid = other.id();
        let mut other = other;
        assert!(!runs_this_program(pid));
        other.kill().unwrap();
        other.wait().unwrap();
        assert!(!runs_this_program(u32::MAX));
    }
}
