//! The session's place on a shared user bus. Under a display manager the session runs on the user's systemd bus, whose
//! manager and D-Bus activation start portals and services with the environment they hold: the one the last graphical
//! session left (another desktop's `WAYLAND_DISPLAY` and `XDG_CURRENT_DESKTOP`). Once the compositor listens, the variables
//! of this session are put there and `cupertino-session.target` (bound to `graphical-session.target`) is started; the
//! session command stops it again when the compositor ends.
//!
//! A user has one systemd manager for all sessions: when a graphical session is already active (another desktop of the
//! same user), nothing is changed, since its services would lose their display.

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

async fn graphical_session_active(conn: &Connection) -> zbus::Result<bool> {
    let unit = match call(
        conn,
        SYSTEMD,
        MANAGER_PATH,
        MANAGER,
        "GetUnit",
        &(GRAPHICAL_SESSION,),
    )
    .await
    {
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
    if graphical_session_active(conn).await? {
        return Ok(Outcome::AnotherSession);
    }
    let assignments: Vec<String> = variables.iter().map(|(k, v)| format!("{k}={v}")).collect();
    call(
        conn,
        SYSTEMD,
        MANAGER_PATH,
        MANAGER,
        "SetEnvironment",
        &(assignments,),
    )
    .await?;
    let map: HashMap<&str, &str> = variables.iter().copied().collect();
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
