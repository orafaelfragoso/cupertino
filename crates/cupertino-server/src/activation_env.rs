//! The session's place on a shared user bus. Under a display manager the session runs on the user's systemd bus, whose
//! manager and D-Bus activation start portals and services with the environment they hold: the one the last graphical
//! session left (another desktop's `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP`). Once the compositor listens, the variables
//! of this session are put there and `cupertino-session.target` (bound to `graphical-session.target`) is started; the
//! session command stops it again when the compositor ends.
//!
//! A user has one systemd manager for all sessions: when a graphical session is already active (another desktop of the
//! same user), nothing is changed, since its services would lose their display. The exception is a leftover
//! `cupertino-session.target` of an earlier session of ours that ended without stopping it: that one is stopped and the
//! session goes on.

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};
use zbus::Connection;

const SYSTEMD: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const GRAPHICAL_SESSION: &str = "graphical-session.target";
/// The unit the packages install in the user unit directory.
pub const SESSION_TARGET: &str = "cupertino-session.target";
/// Variables an earlier desktop may have left that this session does not have: Cupertino runs no X server, and a program
/// that finds `DISPLAY` set prefers it to Wayland.
// TODO(D2): once XWayland runs through xwayland-satellite, `DISPLAY` is this session's and is published, not unset.
const UNSET: [&str; 1] = ["DISPLAY"];

/// What was done with the shared bus.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The bus has no systemd user manager (a private session bus): nothing to do.
    NoSystemd,
    /// Another graphical session of the user is active: its environment is left alone.
    AnotherSession,
    /// The variables are in the manager and the activation environment, and the target is started.
    Applied,
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

/// Puts `variables` in the user manager's environment and the bus's activation environment and starts the session target,
/// unless the bus has no systemd user manager or another graphical session is active.
pub async fn publish(conn: &Connection, variables: &[(&str, &str)]) -> zbus::Result<Outcome> {
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
    if unit_active(conn, SESSION_TARGET).await? {
        // a session of ours that ended without stopping its target (its session command was killed): the target is
        // stopped so this session starts it again with its own variables
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
    let assignments: Vec<String> = variables.iter().map(|(k, v)| format!("{k}={v}")).collect();
    call(
        conn,
        SYSTEMD,
        MANAGER_PATH,
        MANAGER,
        "UnsetAndSetEnvironment",
        &(UNSET.as_slice(), assignments),
    )
    .await?;
    // `UpdateActivationEnvironment` cannot remove a variable: an empty value is the nearest thing
    let mut map: HashMap<&str, &str> = variables.iter().copied().collect();
    map.extend(UNSET.iter().map(|name| (*name, "")));
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
    Ok(Outcome::Applied)
}
