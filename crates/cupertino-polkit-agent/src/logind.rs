//! The logind session the agent registers for, and whether it is the active one.
//!
//! `Active` is read from logind and followed through `PropertiesChanged` signals that only the connection owning
//! logind's name may send ([`from_owner`]): a forged `Active` would dismiss the prompt, or show
//! it while the session is in the background. The proxy keeps no cache: `Active` is read from logind when asked.

use std::collections::HashMap;

use cupertino_peer::signal::from_owner;
use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::message::Type;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::MatchRule;

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// The session of this process: `XDG_SESSION_ID`, else the one logind finds by the process id.
pub struct Session {
    proxy: Proxy<'static>,
    pub id: String,
}

impl Session {
    pub fn find(bus: &Connection) -> zbus::Result<Session> {
        let reply = match std::env::var("XDG_SESSION_ID") {
            Ok(id) if !id.is_empty() => bus.call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER),
                "GetSession",
                &(id,),
            )?,
            _ => bus.call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER),
                "GetSessionByPID",
                &(std::process::id(),),
            )?,
        };
        let path: OwnedObjectPath = reply.body().deserialize()?;
        Self::at(bus, path)
    }

    fn at(bus: &Connection, path: OwnedObjectPath) -> zbus::Result<Session> {
        let proxy = Builder::<Proxy>::new(bus)
            .destination(LOGIND)?
            .path(path)?
            .interface(SESSION)?
            .cache_properties(CacheProperties::No)
            .build()?;
        let id = proxy.get_property("Id")?;
        Ok(Session { proxy, id })
    }

    pub fn active(&self) -> zbus::Result<bool> {
        self.proxy.get_property("Active")
    }

    /// Calls `changed` with `Active` each time logind changes it, from a thread of its own, and with an error once
    /// logind can no longer tell. A change sent by any connection but the one owning logind's name is ignored.
    pub fn watch_active(
        &self,
        changed: impl Fn(Result<bool, String>) + Send + 'static,
    ) -> zbus::Result<()> {
        let connection = self.proxy.connection().clone();
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(LOGIND)?
            .path(self.proxy.path().to_owned())?
            .interface(PROPERTIES)?
            .member("PropertiesChanged")?
            .arg(0, SESSION)?
            .build();
        let changes = from_owner(&connection, LOGIND, rule, |_| {
            eprintln!("cupertino-polkit-agent: ignored a session change that logind did not send")
        })?;
        std::thread::Builder::new()
            .name("logind-active".to_owned())
            .spawn(move || {
                for change in changes {
                    let change = match change {
                        Ok(change) => change,
                        Err(error) => {
                            return changed(Err(format!("reading the session's Active: {error}")))
                        }
                    };
                    match active_of(&change) {
                        Ok(Some(active)) => changed(Ok(active)),
                        Ok(None) => {}
                        Err(error) => {
                            changed(Err(format!("reading the session's Active: {error}")))
                        }
                    }
                }
                changed(Err(
                    "logind stopped telling whether the session is active".to_owned()
                ));
            })
            .map_err(|error| zbus::Error::Failure(error.to_string()))?;
        Ok(())
    }
}

/// The new `Active` in a `PropertiesChanged` of the session, `None` when it names other properties only.
fn active_of(change: &zbus::Message) -> zbus::Result<Option<bool>> {
    let (_, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
        change.body().deserialize()?;
    changed
        .get("Active")
        .map(|value| bool::try_from(value).map_err(zbus::Error::from))
        .transpose()
}
