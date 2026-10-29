//! The session's place on a shared user bus. Under a display manager the session runs on the user's systemd bus, whose
//! manager and D-Bus activation start portals and services with the environment they hold: the one the last graphical
//! session left (another desktop's `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP`). Once the compositor listens, the variables
//! of this session are put there and `cupertino-session.target` (bound to `graphical-session.target`) is started; the
//! session command stops it again when the compositor ends.
//!
//! A user has one systemd manager for all sessions: when a graphical session is already active (another desktop of the
//! same user), nothing is changed, since its services would lose their display. The exception is a leftover
//! `cupertino-session.target` of an earlier session of ours that ended without stopping it: when no compositor listens on
//! the `WAYLAND_DISPLAY` the manager holds, that one is stopped and the session goes on; when one does, it is a second
//! live session of the user and counts as another session.

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};
use zbus::Connection;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const GRAPHICAL_SESSION: &str = "graphical-session.target";
/// The unit the packages install in the user unit directory.
pub const SESSION_TARGET: &str = "cupertino-session.target";
/// Variables an earlier desktop may have left, removed unless this session publishes its own: without an X display
/// (no `xwayland-satellite`) a program that finds another session's `DISPLAY` would prefer it to Wayland.
const UNSET: [&str; 2] = ["DISPLAY", "XAUTHORITY"];

/// The [`UNSET`] names that `variables` does not set.
fn unset<'a>(variables: &[(&str, &str)]) -> Vec<&'a str> {
    UNSET
        .into_iter()
        .filter(|name| variables.iter().all(|(k, _)| k != name))
        .collect()
}

/// What was done with the shared bus.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The bus has no systemd user manager (a private session bus): nothing to do.
    NoSystemd,
    /// Another graphical session of the user is active: its environment is left alone.
    AnotherSession,
    /// The variables are in the manager and the activation environment, and the target is started. `defaults` names the
    /// defaults that were set because the manager had no value of its own.
    Applied { defaults: Vec<String> },
}

/// The file in `XDG_RUNTIME_DIR` that names the defaults this session put in the manager, one per line, so the session
/// command removes them with the session's variables and leaves a value of the user's own alone.
pub const DEFAULTS_RECORD: &str = "cupertino-session-defaults";

/// Writes [`DEFAULTS_RECORD`] for the defaults `publish` set (removes it when none were).
pub fn record_defaults(defaults: &[String]) {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return;
    };
    if let Err(e) = record_defaults_in(std::path::Path::new(&dir), defaults) {
        tracing::warn!(error = %e, "cannot record the session's default variables: they stay in the user manager after the session");
    }
}

fn record_defaults_in(dir: &std::path::Path, defaults: &[String]) -> std::io::Result<()> {
    let path = dir.join(DEFAULTS_RECORD);
    if defaults.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::write(
        path,
        defaults
            .iter()
            .map(|d| format!("{d}\n"))
            .collect::<String>(),
    )
}

/// The variables the manager's environment sets (`Environment`, `NAME=value` entries).
async fn manager_environment(conn: &Connection) -> zbus::Result<Vec<String>> {
    let reply = call(
        conn,
        SYSTEMD,
        MANAGER_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        &(MANAGER, "Environment"),
    )
    .await?;
    let body = reply.body();
    let value: zbus::zvariant::OwnedValue = body.deserialize()?;
    Vec::<String>::try_from(value).map_err(Into::into)
}

/// The `defaults` whose names `environment` does not set. With `leftover` (a session of ours ended without its cleanup),
/// an entry that holds exactly our default is that session's, not the user's: it counts as unset, so it is set and
/// recorded again and removed at the end of this session.
fn missing<'a>(
    defaults: &[(&'a str, &'a str)],
    environment: &[String],
    leftover: bool,
) -> Vec<(&'a str, &'a str)> {
    defaults
        .iter()
        .copied()
        .filter(|(name, value)| {
            environment.iter().all(|entry| match entry.split_once('=') {
                Some((k, v)) => k != *name || (leftover && v == *value),
                None => true,
            })
        })
        .collect()
}

async fn call<B>(
    conn: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> zbus::Result<zbus::Message>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
{
    conn.call_method(Some(destination), path, Some(interface), method, body)
        .await
}

async fn unit_active(conn: &Connection, name: &str) -> zbus::Result<bool> {
    let unit = match call(conn, SYSTEMD, MANAGER_PATH, MANAGER, "GetUnit", &(name,)).await {
        Ok(reply) => reply.body().deserialize::<OwnedObjectPath>()?,
        // the unit is not loaded: nothing holds it
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str().ends_with("NoSuchUnit") => {
            return Ok(false)
        }
        Err(e) => return Err(e),
    };
    let reply = call(
        conn,
        SYSTEMD,
        unit.as_str(),
        "org.freedesktop.DBus.Properties",
        "Get",
        &(UNIT, "ActiveState"),
    )
    .await?;
    let body = reply.body();
    let state: Value = body.deserialize()?;
    Ok(matches!(
        state.downcast_ref::<&str>(),
        Ok("active" | "activating" | "reloading")
    ))
}

/// Whether the `WAYLAND_DISPLAY` in `environment` is a socket something still listens on: the display of a session that
/// is running, as opposed to a leftover of one that ended. `own` is this session's display: a compositor that took over
/// the socket name an ended session left answers on it too.
fn display_is_live(environment: &[String], own: Option<&str>, runtime_dir: &Path) -> bool {
    let Some(display) = environment
        .iter()
        .find_map(|line| line.strip_prefix("WAYLAND_DISPLAY="))
    else {
        return false;
    };
    Some(display) != own && UnixStream::connect(runtime_dir.join(display)).is_ok()
}

/// Puts `variables` in the user manager's environment and the bus's activation environment and starts the session target,
/// unless the bus has no systemd user manager or another graphical session is active. Each of `defaults` is put there too
/// unless the manager already has a value for it (the user's own choice, e.g. from `environment.d`).
pub async fn publish(
    conn: &Connection,
    variables: &[(&str, &str)],
    defaults: &[(&str, &str)],
) -> zbus::Result<Outcome> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    publish_in(conn, variables, defaults, &runtime_dir).await
}

/// [`publish`] with the directory the displays' sockets are in.
pub async fn publish_in(
    conn: &Connection,
    variables: &[(&str, &str)],
    defaults: &[(&str, &str)],
    runtime_dir: &Path,
) -> zbus::Result<Outcome> {
    let owned = call(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameHasOwner",
        &(SYSTEMD,),
    )
    .await?;
    if !owned.body().deserialize::<bool>()? {
        return Ok(Outcome::NoSystemd);
    }
    let leftover = unit_active(conn, SESSION_TARGET).await?;
    if leftover {
        // the target of a session that is running keeps its services; one whose compositor is gone is a leftover of a
        // session that ended without stopping it (its session command was killed), and is stopped so that this session
        // starts it again with its own variables
        let own = variables
            .iter()
            .find_map(|(k, v)| (*k == "WAYLAND_DISPLAY").then_some(*v));
        if manager_environment(conn)
            .await
            .is_ok_and(|environment| display_is_live(&environment, own, runtime_dir))
        {
            return Ok(Outcome::AnotherSession);
        }
        call(
            conn,
            SYSTEMD,
            MANAGER_PATH,
            MANAGER,
            "StopUnit",
            &(SESSION_TARGET, "replace"),
        )
        .await?;
    } else if unit_active(conn, GRAPHICAL_SESSION).await? {
        return Ok(Outcome::AnotherSession);
    }
    // without the manager's environment no default is set: one could replace a value of the user's own, and the
    // session's own variables still go out
    let defaults = match manager_environment(conn).await {
        Ok(environment) => missing(defaults, &environment, leftover),
        Err(e) => {
            tracing::warn!(error = %e, "cannot read the user manager's environment: the session's defaults are not published");
            Vec::new()
        }
    };
    let variables: Vec<(&str, &str)> = variables
        .iter()
        .copied()
        .chain(defaults.iter().copied())
        .collect();
    let variables = variables.as_slice();
    let unset = unset(variables);
    let assignments: Vec<String> = variables.iter().map(|(k, v)| format!("{k}={v}")).collect();
    call(
        conn,
        SYSTEMD,
        MANAGER_PATH,
        MANAGER,
        "UnsetAndSetEnvironment",
        &(unset.as_slice(), assignments),
    )
    .await?;
    // `UpdateActivationEnvironment` cannot remove a variable: an empty value is the nearest thing
    let mut map: HashMap<&str, &str> = variables.iter().copied().collect();
    map.extend(unset.iter().map(|name| (*name, "")));
    call(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "UpdateActivationEnvironment",
        &(map,),
    )
    .await?;
    let _job: ObjectPath<'_> = call(
        conn,
        SYSTEMD,
        MANAGER_PATH,
        MANAGER,
        "StartUnit",
        &(SESSION_TARGET, "replace"),
    )
    .await?
    .body()
    .deserialize()?;
    Ok(Outcome::Applied {
        defaults: defaults.iter().map(|(k, _)| (*k).to_owned()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_the_manager_already_sets_is_left_to_it() {
        let defaults = [
            ("QT_QPA_PLATFORM", "wayland;xcb"),
            ("ELECTRON_OZONE_PLATFORM_HINT", "auto"),
        ];
        let environment = vec!["PATH=/usr/bin".to_owned(), "QT_QPA_PLATFORM=xcb".to_owned()];
        assert_eq!(
            missing(&defaults, &environment, false),
            [("ELECTRON_OZONE_PLATFORM_HINT", "auto")]
        );
        assert_eq!(missing(&defaults, &[], false), defaults);
    }

    #[test]
    fn our_default_left_by_a_session_without_its_cleanup_is_taken_back() {
        let defaults = [("QT_QPA_PLATFORM", "wayland;xcb")];
        let ours = vec!["QT_QPA_PLATFORM=wayland;xcb".to_owned()];
        let users = vec!["QT_QPA_PLATFORM=xcb".to_owned()];
        assert_eq!(missing(&defaults, &ours, true), defaults);
        assert!(
            missing(&defaults, &users, true).is_empty(),
            "a different value is the user's"
        );
        assert!(
            missing(&defaults, &ours, false).is_empty(),
            "without a leftover session it may be the user's too"
        );
    }

    #[test]
    fn the_record_names_the_defaults_set_and_goes_when_none_were() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DEFAULTS_RECORD);
        record_defaults_in(
            dir.path(),
            &[
                "QT_QPA_PLATFORM".to_owned(),
                "ELECTRON_OZONE_PLATFORM_HINT".to_owned(),
            ],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "QT_QPA_PLATFORM\nELECTRON_OZONE_PLATFORM_HINT\n"
        );
        record_defaults_in(dir.path(), &[]).unwrap();
        assert!(!path.exists());
        record_defaults_in(dir.path(), &[]).unwrap();
    }
}
