//! The logind session the agent registers for, and whether it is the active one.

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";

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
        let proxy = Proxy::new_owned(bus.clone(), LOGIND, path, SESSION)?;
        let id = proxy.get_property("Id")?;
        Ok(Session { proxy, id })
    }

    pub fn active(&self) -> zbus::Result<bool> {
        self.proxy.get_property("Active")
    }

    /// Calls `changed` with `Active` each time logind changes it, from a thread of its own, and with an error once
    /// logind can no longer tell.
    pub fn watch_active(
        &self,
        changed: impl Fn(Result<bool, String>) + Send + 'static,
    ) -> std::io::Result<()> {
        let proxy = self.proxy.clone();
        std::thread::Builder::new()
            .name("logind-active".to_owned())
            .spawn(move || {
                for change in proxy.receive_property_changed::<bool>("Active") {
                    match change.get() {
                        Ok(active) => changed(Ok(active)),
                        Err(error) => {
                            changed(Err(format!("reading the session's Active: {error}")))
                        }
                    }
                }
                changed(Err(
                    "logind stopped telling whether the session is active".to_owned()
                ));
            })?;
        Ok(())
    }
}
