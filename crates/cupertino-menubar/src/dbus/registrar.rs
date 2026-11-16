//! `com.canonical.AppMenu.Registrar`. Qt, KDE Frameworks, Chromium, Electron and LibreOffice export their menus and hide
//! their in-window menu bar only while the name has an owner. Most also announce the menu through the window's Wayland
//! connection, which the server passes on; Electron registers it here only, for its X11 window or, on Wayland, under a
//! number of its own. The registrar keeps every registration with the connection and process it came from, and the
//! bar matches the active window to one ([`crate::registered`]).
//!
//! A registration lasts until its program unregisters the window or registers it again, its connection leaves the bus,
//! or the bar finds its X11 window gone ([`Registrar::forget`]).

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

/// Most registrations kept; past it the oldest goes, so a client that registers without end cannot grow the list.
const MAX_REGISTRATIONS: usize = 1024;

/// Called with every registration after each change.
pub type Changed = Arc<dyn Fn(Vec<Registration>) + Send + Sync>;

#[derive(Default)]
struct Registry {
    list: Vec<Registration>,
    serial: u64,
}

impl Registry {
    fn register(&mut self, service: String, pid: u32, window: u32, path: String) {
        self.list
            .retain(|r| !(r.service == service && r.window == window));
        if self.list.len() >= MAX_REGISTRATIONS {
            self.list.remove(0);
        }
        self.serial += 1;
        self.list.push(Registration {
            service,
            path,
            pid,
            window,
            serial: self.serial,
        });
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
        let pid = process_of(conn, sender.as_str()).await;
        let (service, path) = (sender.to_string(), menu_object_path.to_string());
        self.0.update(|r| {
            r.register(service, pid, window_id, path);
            true
        });
    }

    fn unregister_window(&self, window_id: u32, #[zbus(header)] header: Header<'_>) {
        let Some(sender) = header.sender() else {
            return;
        };
        self.0
            .update(|r| r.remove(|reg| reg.service == sender.as_str() && reg.window == window_id));
    }

    /// The latest registration of the window from any connection; an empty service and the root path when none.
    fn get_menu_for_window(&self, window_id: u32) -> (String, OwnedObjectPath) {
        let registry = self.0.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry
            .list
            .iter()
            .filter(|r| r.window == window_id)
            .max_by_key(|r| r.serial)
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
