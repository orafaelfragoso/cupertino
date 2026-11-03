//! Publishes an application's menus to the desktop's menu bar over the session bus.
//!
//! The menus are served as `org.gtk.Menus` and their actions as `org.gtk.Actions`: `app.` actions and the
//! application menu on the application object, `win.` actions and the menu bar on one object per window. A window
//! announces the objects with [`DbusProperties`] (`cupertino_app_menu_v1.set_address`). Only what
//! [`MenuBar::exported`] keeps is published; the menu bar adds the desktop's own items. Accelerators name the keys of
//! the active modifier profile, so the menus are exported again when it changes ([`Exporter::set_modifier_map`]).
//! The objects answer only the menu bar the compositor names (`cupertino_app_menu_v1.menu_reader`,
//! [`Exporter::set_reader`]), and their signals go to it alone: any other peer is refused with `AccessDenied`, as is
//! every peer while there is no reader. An application's windows share one connection ([`AppMenus`]).

mod app;
mod objects;
mod window;
mod wire;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

pub use app::{AppMenus, WindowExport};
use cupertino_menu_model::{ActionName, MenuBar, Scope};
use cupertino_modifiers::ModifierMap;
use objects::{action_changes, Actions, Menus, OnActivate, Reader};
pub use window::WindowMenus;
use wire::WireMenus;
use zbus::blocking::{connection, Connection};
use zbus::names::{OwnedUniqueName, UniqueName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

/// A window of the application, numbered by the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u32);

/// A menu item was chosen. `window` is the window whose menus held a `win.` action, `None` for an `app.` action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activation {
    pub window: Option<WindowId>,
    pub action: ActionName,
}

/// Where the menus of a window are, which it announces with `cupertino_app_menu_v1.set_address`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DbusProperties {
    pub application_id: String,
    pub app_menu_path: String,
    pub menubar_path: String,
    pub window_object_path: String,
    pub application_object_path: String,
    pub unique_bus_name: String,
}

/// The menus of one application on one bus connection.
pub struct Exporter {
    conn: Connection,
    app_id: String,
    app_path: String,
    map: Box<dyn ModifierMap + Send + Sync>,
    on_activate: OnActivate,
    reader: Reader,
    /// The exported menus of each window.
    windows: BTreeMap<WindowId, MenuBar>,
}

/// The object path of an application id, as GLib derives it: `org.example.Notes` is `/org/example/Notes`.
fn app_path(app_id: &str) -> String {
    app_id
        .split('.')
        .map(|part| {
            let part: String = part
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            if part.is_empty() {
                "_".to_string()
            } else {
                part
            }
        })
        .fold(String::new(), |path, part| path + "/" + &part)
}

impl Exporter {
    /// Connect with `builder` (`connection::Builder::session()` for the desktop's bus). The connection serves the
    /// application object from the start: zbus then listens for method calls before it reads the socket, so no call
    /// that arrives once an address is announced can be dropped unanswered, as one sent to an object server started
    /// on an open connection can be. `on_activate` runs on the bus connection's thread for each chosen item; it passes
    /// the activation on to the application's own thread.
    pub fn new(
        builder: connection::Builder<'_>,
        app_id: &str,
        map: Box<dyn ModifierMap + Send + Sync>,
        on_activate: impl Fn(Activation) + Send + Sync + 'static,
    ) -> zbus::Result<Exporter> {
        if app_id.is_empty() {
            return Err(zbus::Error::Failure(
                "an application id is required".to_string(),
            ));
        }
        let app_path = app_path(app_id);
        let on_activate: OnActivate = Arc::new(on_activate);
        let reader: Reader = Arc::new(Mutex::new(None));
        let app_actions = Actions {
            scope: Scope::App,
            window: None,
            enabled: BTreeMap::new(),
            on_activate: on_activate.clone(),
            reader: reader.clone(),
        };
        let conn = builder.serve_at(app_path.as_str(), app_actions)?.build()?;
        Ok(Exporter {
            conn,
            app_id: app_id.to_string(),
            app_path,
            map,
            on_activate,
            reader,
            windows: BTreeMap::new(),
        })
    }

    /// The unique bus name of the menu bar the compositor verified, `None` while there is none. Only that connection
    /// may read the menus and choose their items.
    pub fn set_reader(&self, reader: Option<&str>) -> zbus::Result<()> {
        let reader = reader
            .map(|name| UniqueName::try_from(name).map(OwnedUniqueName::from))
            .transpose()?;
        *self.reader.lock().unwrap() = reader;
        Ok(())
    }

    /// `emitter` addressed to the reader alone; `None` while there is no reader.
    fn to_reader(&self, emitter: &SignalEmitter<'_>) -> Option<SignalEmitter<'static>> {
        let reader = self.reader.lock().unwrap().clone()?;
        Some(
            emitter
                .to_owned()
                .set_destination(reader.into_inner().into()),
        )
    }

    fn app_menu_path(&self) -> String {
        format!("{}/menus/appmenu", self.app_path)
    }

    fn window_path(&self, window: WindowId) -> String {
        format!("{}/window/{}", self.app_path, window.0)
    }

    fn menubar_path(&self, window: WindowId) -> String {
        format!("{}/menus/menubar", self.window_path(window))
    }

    /// Publish (or update) the menus of `window` from its full menu model.
    pub fn export(&mut self, window: WindowId, bar: &MenuBar) -> zbus::Result<DbusProperties> {
        self.windows.insert(window, bar.exported());
        self.publish_window(window)?;
        self.publish_app(Some(window))?;
        Ok(DbusProperties {
            application_id: self.app_id.clone(),
            app_menu_path: self.app_menu_path(),
            menubar_path: self.menubar_path(window),
            window_object_path: self.window_path(window),
            application_object_path: self.app_path.clone(),
            unique_bus_name: self
                .conn
                .unique_name()
                .ok_or_else(|| {
                    zbus::Error::Failure("the connection has no unique name".to_string())
                })?
                .to_string(),
        })
    }

    /// Withdraw the menus of a closed window.
    pub fn remove(&mut self, window: WindowId) -> zbus::Result<()> {
        if self.windows.remove(&window).is_none() {
            return Ok(());
        }
        let server = self.conn.object_server();
        server.remove::<Menus, _>(self.menubar_path(window))?;
        server.remove::<Actions, _>(self.window_path(window))?;
        drop(server);
        self.publish_app(self.windows.keys().next_back().copied())
    }

    /// Enable or disable an action of `window` (`win.`) or of the application (`app.`).
    pub fn set_enabled(
        &self,
        window: WindowId,
        action: &ActionName,
        enabled: bool,
    ) -> zbus::Result<()> {
        let path = match action.scope() {
            Scope::App => self.app_path.clone(),
            Scope::Win => self.window_path(window),
        };
        let iface = self.conn.object_server().interface::<_, Actions>(path)?;
        let changed = {
            let mut actions = iface.get_mut();
            match actions.enabled.get_mut(action.name()) {
                Some(e) if *e != enabled => {
                    *e = enabled;
                    true
                }
                Some(_) => false,
                None => {
                    return Err(zbus::Error::Failure(format!(
                        "{} is not exported",
                        action.full()
                    )))
                }
            }
        };
        if let (true, Some(emitter)) = (changed, self.to_reader(iface.signal_emitter())) {
            let enabled_changed = [(action.name().to_string(), enabled)].into_iter().collect();
            zbus::block_on(Actions::changed(
                &emitter,
                Vec::new(),
                enabled_changed,
                Default::default(),
                Default::default(),
            ))?;
        }
        Ok(())
    }

    /// Use another modifier profile: every accelerator is written again.
    pub fn set_modifier_map(
        &mut self,
        map: Box<dyn ModifierMap + Send + Sync>,
    ) -> zbus::Result<()> {
        self.map = map;
        let windows: Vec<WindowId> = self.windows.keys().copied().collect();
        for window in &windows {
            self.publish_window(*window)?;
        }
        self.publish_app(windows.last().copied())
    }

    fn publish_window(&self, window: WindowId) -> zbus::Result<()> {
        let bar = &self.windows[&window];
        let encoded = wire::menu_bar(&bar.menus, self.map.as_ref());
        self.update_menus(&self.menubar_path(window), encoded.menus)?;
        let names = encoded
            .actions
            .iter()
            .chain(&wire::menu(&bar.app, self.map.as_ref()).actions)
            .filter(|a| a.scope() == Scope::Win)
            .map(|a| a.name().to_string())
            .collect();
        self.update_actions(&self.window_path(window), Scope::Win, Some(window), names)
    }

    /// The application menu of `latest`, and the `app.` actions of every window.
    fn publish_app(&self, latest: Option<WindowId>) -> zbus::Result<()> {
        let app_menu = latest
            .map(|w| wire::menu(&self.windows[&w].app, self.map.as_ref()).menus)
            .unwrap_or_default();
        self.update_menus(&self.app_menu_path(), app_menu)?;
        let mut names = BTreeSet::new();
        for bar in self.windows.values() {
            let menus = wire::menu_bar(&bar.menus, self.map.as_ref()).actions;
            let app = wire::menu(&bar.app, self.map.as_ref()).actions;
            names.extend(
                menus
                    .iter()
                    .chain(&app)
                    .filter(|a| a.scope() == Scope::App)
                    .map(|a| a.name().to_string()),
            );
        }
        self.update_actions(&self.app_path, Scope::App, None, names)
    }

    fn update_menus(&self, path: &str, menus: WireMenus) -> zbus::Result<()> {
        let server = self.conn.object_server();
        let iface = match server.interface::<_, Menus>(path) {
            Ok(iface) => iface,
            Err(zbus::Error::InterfaceNotFound) => {
                let reader = self.reader.clone();
                server.at(path, Menus { menus, reader })?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let changes = {
            let mut current = iface.get_mut();
            let changes = wire::changes(&current.menus, &menus);
            current.menus = menus;
            changes
        };
        let Some(emitter) = self.to_reader(iface.signal_emitter()) else {
            return Ok(());
        };
        if changes.is_empty() {
            return Ok(());
        }
        let changes = changes
            .into_iter()
            .map(|(g, m, at, removed, added)| {
                (
                    g,
                    m,
                    at,
                    removed,
                    added.iter().map(|i| i.to_dict()).collect(),
                )
            })
            .collect();
        zbus::block_on(Menus::changed(&emitter, changes))
    }

    /// Export exactly `names` in `scope` at `path`, keeping the enabled state of the actions already there.
    fn update_actions(
        &self,
        path: &str,
        scope: Scope,
        window: Option<WindowId>,
        names: BTreeSet<String>,
    ) -> zbus::Result<()> {
        let server = self.conn.object_server();
        let iface = match server.interface::<_, Actions>(path) {
            Ok(iface) => iface,
            Err(zbus::Error::InterfaceNotFound) => {
                let actions = Actions {
                    scope,
                    window,
                    enabled: names.into_iter().map(|n| (n, true)).collect(),
                    on_activate: self.on_activate.clone(),
                    reader: self.reader.clone(),
                };
                server.at(path, actions)?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let (removed, enabled_changed, added) = {
            let mut actions = iface.get_mut();
            let new: BTreeMap<String, bool> = names
                .into_iter()
                .map(|n| {
                    let enabled = actions.enabled.get(&n).copied().unwrap_or(true);
                    (n, enabled)
                })
                .collect();
            let changes = action_changes(&actions.enabled, &new);
            actions.enabled = new;
            changes
        };
        if removed.is_empty() && enabled_changed.is_empty() && added.is_empty() {
            return Ok(());
        }
        let Some(emitter) = self.to_reader(iface.signal_emitter()) else {
            return Ok(());
        };
        zbus::block_on(Actions::changed(
            &emitter,
            removed,
            enabled_changed,
            std::collections::HashMap::<String, OwnedValue>::new(),
            added,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::ObjectPath;

    #[test]
    fn application_ids_become_object_paths() {
        for (id, path) in [
            ("org.example.Notes", "/org/example/Notes"),
            ("org.example.my-app", "/org/example/my_app"),
            ("a..b", "/a/_/b"),
        ] {
            assert_eq!(app_path(id), path);
            assert!(ObjectPath::try_from(app_path(id)).is_ok());
        }
    }
}
