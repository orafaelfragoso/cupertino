//! `com.canonical.AppMenu.Registrar`. Qt, KDE Frameworks, Chromium, Electron and LibreOffice export their menus and hide
//! their in-window menu bar only while the name has an owner. Most also announce the menu through the window's Wayland
//! connection, which the server passes on; Electron registers it here only, for its X11 window or, on Wayland, under a
//! number of its own. The registrar keeps every registration with the connection and process it came from, and the
//! bar matches the active window to one ([`crate::registered`]).
//!
//! A registration lasts until its program unregisters the window or registers it again, its connection leaves the bus,
//! or the bar finds its X11 window gone ([`Registrar::forget`]). A connection whose process the bus cannot name, as one
//! that left while it was asked, is not kept. Several connections may register one window: `GetMenuForWindow` answers
//! with the first.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use zbus::fdo::DBusProxy;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};
use zbus::Connection;

use crate::registered::Registration;

pub const REGISTRAR_NAME: &str = "com.canonical.AppMenu.Registrar";
pub const REGISTRAR_PATH: &str = "/com/canonical/AppMenu/Registrar";

/// Most registrations kept of one connection; past it that connection's oldest goes, so a client that registers without
/// end grows neither the list nor takes the place of other clients' registrations.
const MAX_REGISTRATIONS_PER_CONNECTION: usize = 256;
/// Most registrations kept in all; past it the oldest of the connection that holds the most goes.
const MAX_REGISTRATIONS: usize = 4096;

/// Called with every registration after each change.
pub type Changed = Arc<dyn Fn(Vec<Registration>) + Send + Sync>;

#[derive(Default)]
struct Registry {
    list: Vec<Registration>,
    serial: u64,
}

impl Registry {
    fn register(&mut self, service: String, pid: u32, window: u32, path: String) {
        self.serial += 1;
        // registering a window again keeps the connection's place among those that registered it
        let since = match self
            .list
            .iter()
            .position(|r| r.service == service && r.window == window)
        {
            Some(i) => self.list.remove(i).since,
            None => self.serial,
        };
        let own = self.list.iter().filter(|r| r.service == service).count();
        if own >= MAX_REGISTRATIONS_PER_CONNECTION {
            self.drop_oldest_of(&service);
        } else if self.list.len() >= MAX_REGISTRATIONS {
            let mut counts: std::collections::HashMap<&str, usize> = Default::default();
            for r in &self.list {
                *counts.entry(r.service.as_str()).or_default() += 1;
            }
            if let Some((largest, _)) = counts.into_iter().max_by_key(|(_, n)| *n) {
                let largest = largest.to_string();
                self.drop_oldest_of(&largest);
            }
        }
        self.list.push(Registration {
            service,
            path,
            pid,
            window,
            serial: self.serial,
            since,
        });
    }

    /// Drop the oldest registration of connection `service`.
    fn drop_oldest_of(&mut self, service: &str) {
        if let Some(i) = self.list.iter().position(|r| r.service == service) {
            self.list.remove(i);
        }
    }

    /// The registration that stands for `window`: the connection that registered it first.
    fn menu_for(&self, window: u32) -> Option<&Registration> {
        crate::registered::preferred(self.list.iter().filter(|r| r.window == window), None)
    }

    /// Drop what `drop` names; whether anything went.
    fn remove(&mut self, drop: impl Fn(&Registration) -> bool) -> bool {
        let before = self.list.len();
        self.list.retain(|r| !drop(r));
        self.list.len() != before
    }
}

/// The served registrar; cheap to clone.
#[derive(Clone)]
pub struct Registrar {
    registry: Arc<Mutex<Registry>>,
    changed: Changed,
}

impl Registrar {
    fn update(&self, change: impl FnOnce(&mut Registry) -> bool) {
        let list = {
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            if !change(&mut registry) {
                return;
            }
            registry.list.clone()
        };
        (self.changed)(list);
    }

    /// Forget registrations, by connection and window: those whose X11 window is gone.
    pub fn forget(&self, gone: &[(String, u32)]) {
        self.update(|r| {
            r.remove(|reg| {
                gone.iter()
                    .any(|(s, w)| *s == reg.service && *w == reg.window)
            })
        });
    }

    /// Every registration now.
    pub fn registrations(&self) -> Vec<Registration> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .list
            .clone()
    }
}

/// The object served at [`REGISTRAR_PATH`].
struct Service(Registrar);

#[zbus::interface(name = "com.canonical.AppMenu.Registrar")]
impl Service {
    async fn register_window(
        &self,
        window_id: u32,
        menu_object_path: ObjectPath<'_>,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) {
        let Some(sender) = header.sender() else {
            return;
        };
        // the bus names no process for a connection that already left: nothing to keep
        let pid = process_of(conn, sender.as_str()).await;
        if pid == 0 {
            return;
        }
        let (service, path) = (sender.to_string(), menu_object_path.to_string());
        self.0.update(|r| {
            r.register(service.clone(), pid, window_id, path);
            true
        });
        // a connection that left while its process was asked for has been forgotten before it was kept
        if !has_owner(conn, &service).await {
            self.0.update(|r| r.remove(|reg| reg.service == service));
        }
    }

    fn unregister_window(&self, window_id: u32, #[zbus(header)] header: Header<'_>) {
        let Some(sender) = header.sender() else {
            return;
        };
        self.0
            .update(|r| r.remove(|reg| reg.service == sender.as_str() && reg.window == window_id));
    }

    /// The registration of the window by the connection that registered it first; an empty service and the root path
    /// when none.
    fn get_menu_for_window(&self, window_id: u32) -> (String, OwnedObjectPath) {
        let registry = self.0.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry
            .menu_for(window_id)
            .and_then(|r| {
                Some((
                    r.service.clone(),
                    OwnedObjectPath::try_from(r.path.as_str()).ok()?,
                ))
            })
            .unwrap_or_else(|| {
                (
                    String::new(),
                    OwnedObjectPath::from(ObjectPath::from_static_str_unchecked("/")),
                )
            })
    }
}

/// The process behind the connection `name`, `0` when the bus does not say.
async fn process_of(conn: &Connection, name: &str) -> u32 {
    let Ok(name) = BusName::try_from(name) else {
        return 0;
    };
    match DBusProxy::new(conn).await {
        Ok(bus) => bus.get_connection_unix_process_id(name).await.unwrap_or(0),
        Err(_) => 0,
    }
}

/// Whether the connection `name` is still on the bus; `true` when the bus does not answer, so its leaving is still
/// heard of.
async fn has_owner(conn: &Connection, name: &str) -> bool {
    let Ok(name) = BusName::try_from(name) else {
        return false;
    };
    match DBusProxy::new(conn).await {
        Ok(bus) => bus.name_has_owner(name).await.unwrap_or(true),
        Err(_) => true,
    }
}

/// Serve the registrar on `conn`; toolkits see it only while [`own`] holds the name. `changed` hears of every change,
/// including the registrations of a connection that left the bus.
pub async fn serve(conn: &Connection, changed: Changed) -> zbus::Result<Registrar> {
    let registrar = Registrar {
        registry: Arc::default(),
        changed,
    };
    // subscribed before the object is served, so no connection that registered can leave unseen
    let mut owners = DBusProxy::new(conn)
        .await?
        .receive_name_owner_changed()
        .await?;
    conn.object_server()
        .at(REGISTRAR_PATH, Service(registrar.clone()))
        .await?;
    let follower = registrar.clone();
    conn.executor()
        .spawn(
            async move {
                while let Some(signal) = owners.next().await {
                    let Ok(args) = signal.args() else {
                        continue;
                    };
                    if args.new_owner().is_none() {
                        let name = args.name().as_str();
                        follower.update(|r| r.remove(|reg| reg.service == name));
                    }
                }
            },
            "cupertino-registrar-leavers",
        )
        .detach();
    Ok(registrar)
}

/// Claim the name; `NameTaken` when another process owns it.
pub async fn own(conn: &Connection) -> zbus::Result<()> {
    super::claim_name(conn, REGISTRAR_NAME).await
}

/// Give the name up; a name another process owns stays with it.
pub async fn release(conn: &Connection) -> zbus::Result<()> {
    conn.release_name(REGISTRAR_NAME).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn register(registry: &mut Registry, service: &str, window: u32) {
        registry.register(service.into(), 40, window, format!("/menu/{window}"));
    }

    fn windows_of(registry: &Registry, service: &str) -> Vec<u32> {
        registry
            .list
            .iter()
            .filter(|r| r.service == service)
            .map(|r| r.window)
            .collect()
    }

    #[test]
    fn a_connection_that_registers_without_end_drops_only_its_own_oldest() {
        let mut registry = Registry::default();
        register(&mut registry, ":1.5", 7);
        let flood = MAX_REGISTRATIONS_PER_CONNECTION as u32 + 10;
        for window in 0..flood {
            register(&mut registry, ":1.66", 1000 + window);
        }
        assert_eq!(
            windows_of(&registry, ":1.5"),
            [7],
            "another connection's stays"
        );
        let flooded = windows_of(&registry, ":1.66");
        assert_eq!(flooded.len(), MAX_REGISTRATIONS_PER_CONNECTION);
        assert_eq!(flooded[0], 1010, "its oldest went");
    }

    #[test]
    fn past_the_whole_limit_the_connection_that_holds_the_most_gives_way() {
        let mut registry = Registry::default();
        register(&mut registry, ":1.5", 7);
        let full = MAX_REGISTRATIONS / MAX_REGISTRATIONS_PER_CONNECTION;
        for connection in 0..full {
            for window in 0..MAX_REGISTRATIONS_PER_CONNECTION as u32 {
                register(&mut registry, &format!(":2.{connection}"), window);
            }
        }
        assert_eq!(registry.list.len(), MAX_REGISTRATIONS);
        assert_eq!(windows_of(&registry, ":1.5"), [7]);
    }

    #[test]
    fn a_window_is_the_first_connections_and_registering_again_keeps_its_place() {
        let mut registry = Registry::default();
        register(&mut registry, ":1.5", 7);
        register(&mut registry, ":1.66", 7);
        assert_eq!(registry.menu_for(7).unwrap().service, ":1.5");
        register(&mut registry, ":1.5", 7);
        assert_eq!(
            registry.menu_for(7).unwrap().service,
            ":1.5",
            "registered again: still first"
        );
        assert_eq!(registry.list.len(), 2, "one registration each");
        registry.remove(|r| r.service == ":1.5");
        assert_eq!(registry.menu_for(7).unwrap().service, ":1.66");
        assert!(registry.menu_for(8).is_none());
    }
}
