//! The logind session the prompts belong to, and whether it is the active one (D13).
//!
//! The session is this process's: `XDG_SESSION_ID`, else the one logind finds by the process id. `Active` is read from
//! logind and followed through `PropertiesChanged`, which the proxy takes only from the connection that owns logind's
//! name: a forged `Active` would dismiss a prompt, or show one while the session is in the background. Everything runs
//! on the system bus connection Kit's loop ticks.

use std::collections::HashMap;

use futures_lite::StreamExt;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::{Connection, Proxy};

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// The variable a login (`pam_systemd`) sets to the session's id.
const SESSION_VARIABLE: &str = "XDG_SESSION_ID";

/// This process's session as logind names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub path: OwnedObjectPath,
    /// The id polkitd registers the agent for.
    pub id: String,
    /// Whether it is the active session of its seat now.
    pub active: bool,
}

fn proxy<'a>(
    system: &Connection,
    path: OwnedObjectPath,
    interface: &'static str,
) -> zbus::Result<zbus::proxy::Builder<'a, Proxy<'a>>> {
    Ok(zbus::proxy::Builder::<Proxy<'_>>::new(system)
        .destination(LOGIND)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No))
}

/// The session of this process, its id and whether it is active.
pub async fn find(system: &Connection) -> zbus::Result<Session> {
    let manager = proxy(system, OwnedObjectPath::try_from(MANAGER_PATH)?, MANAGER)?
        .build()
        .await?;
    let path: OwnedObjectPath = match std::env::var(SESSION_VARIABLE) {
        Ok(id) if !id.is_empty() => manager.call("GetSession", &(id.as_str(),)).await?,
        _ => {
            manager
                .call("GetSessionByPID", &(std::process::id(),))
                .await?
        }
    };
    let session = proxy(system, path.clone(), SESSION)?.build().await?;
    let id: String = session.get_property("Id").await?;
    let active: bool = session.get_property("Active").await?;
    Ok(Session { path, id, active })
}

/// Hands each new `Active` of the session at `path` to `changed` until the stream ends; returns why. `Active` is read
/// once more after the changes are subscribed to, so none is missed between [`find`] and the subscription.
pub async fn follow_active(
    system: &Connection,
    path: OwnedObjectPath,
    changed: impl Fn(bool),
) -> String {
    let properties = match proxy(system, path.clone(), PROPERTIES) {
        Ok(builder) => builder.build().await,
        Err(e) => Err(e),
    };
    let properties = match properties {
        Ok(properties) => properties,
        Err(e) => return format!("reaching the login session: {e}"),
    };
    let mut changes = match properties.receive_signal("PropertiesChanged").await {
        Ok(changes) => changes,
        Err(e) => return format!("subscribing to the session's changes: {e}"),
    };
    match active(system, path).await {
        Ok(now) => changed(now),
        Err(e) => return format!("reading whether the session is active: {e}"),
    }
    while let Some(change) = changes.next().await {
        match active_of(&change) {
            Ok(Some(active)) => changed(active),
            Ok(None) => {}
            Err(e) => {
                eprintln!("cupertino-prompts: a change of the session that does not read: {e}")
            }
        }
    }
    "logind stopped telling whether the session is active".to_owned()
}

/// The new `Active` in a `PropertiesChanged` of the session, `None` when it names other properties only.
fn active_of(change: &zbus::Message) -> zbus::Result<Option<bool>> {
    let (interface, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
        change.body().deserialize()?;
    if interface != SESSION {
        return Ok(None);
    }
    changed
        .get("Active")
        .map(|value| bool::try_from(value).map_err(zbus::Error::from))
        .transpose()
}

/// Whether the session at `path` is active now.
pub async fn active(system: &Connection, path: OwnedObjectPath) -> zbus::Result<bool> {
    proxy(system, path, SESSION)?
        .build()
        .await?
        .get_property("Active")
        .await
}
