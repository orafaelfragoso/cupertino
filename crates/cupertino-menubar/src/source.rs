//! Mapping from window / app identity to a menu source.
//!
//! The compositor learns about exported menus from several places and feeds them here:
//! * the KDE appmenu Wayland protocol: service name + object path per surface;
//! * the GTK shell protocol: bus name and object paths of `org.gtk.Menus` / `org.gtk.Actions`.
//!
//! [`MenuSourceMap::resolve`] picks the source for a window (its own registration first, then its
//! application's) and falls back to [`MenuSource::Synthetic`], for which
//! [`crate::fallback`] builds the menus.

use std::collections::HashMap;

/// Opaque compositor-side window identifier.
pub type WindowKey = u64;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KdeAppMenu {
    /// Unique or well-known bus name of the exporting process.
    pub service: String,
    /// Object path implementing `com.canonical.dbusmenu`.
    pub path: String,
}

/// Properties announced by a GTK client for one window.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GtkMenuProps {
    /// Unique bus name that owns the object paths.
    pub bus_name: String,
    pub application_id: Option<String>,
    /// Path of the `org.gtk.Actions` / `org.gtk.Menus` object of the application (`app.` actions).
    pub application_object_path: Option<String>,
    /// Path of the object of the window (`win.` actions).
    pub window_object_path: Option<String>,
    /// Path of the menu bar `org.gtk.Menus` object.
    pub menubar_object_path: Option<String>,
    /// Path of the legacy app menu object.
    pub app_menu_object_path: Option<String>,
}

impl GtkMenuProps {
    pub fn has_menu_bar(&self) -> bool {
        self.menubar_object_path.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MenuSource {
    /// DBusMenu exported by the app (KDE appmenu protocol).
    Kde(KdeAppMenu),
    /// GMenuModel exported over D-Bus.
    Gtk(GtkMenuProps),
    /// No exported menu: use the synthetic fallback.
    Synthetic,
}

impl MenuSource {
    /// A source that provides real menus (as opposed to the fallback).
    pub fn is_exported(&self) -> bool {
        matches!(self, MenuSource::Kde(_) | MenuSource::Gtk(_))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowIdentity {
    pub window: WindowKey,
    pub app_id: String,
    pub pid: Option<u32>,
}

#[derive(Debug, Default)]
pub struct MenuSourceMap {
    explicit: HashMap<WindowKey, MenuSource>,
    by_app: HashMap<String, MenuSource>,
}

impl MenuSourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// A window announced its menu (KDE appmenu protocol or GTK properties).
    pub fn set_window_source(&mut self, window: WindowKey, source: MenuSource) {
        match source {
            MenuSource::Synthetic => {
                self.explicit.remove(&window);
            }
            s => {
                self.explicit.insert(window, s);
            }
        }
    }

    pub fn clear_window(&mut self, window: WindowKey) {
        self.explicit.remove(&window);
    }

    /// An app-wide source used for its windows that announced none (an app that exports one
    /// application-level menu).
    pub fn set_app_source(&mut self, app_id: &str, source: Option<MenuSource>) {
        match source {
            Some(s) if s.is_exported() => {
                self.by_app.insert(app_id.to_string(), s);
            }
            _ => {
                self.by_app.remove(app_id);
            }
        }
    }

    /// Forget registrations whose service left the bus.
    pub fn service_vanished(&mut self, service: &str) {
        self.explicit.retain(|_, s| match s {
            MenuSource::Kde(k) => k.service != service,
            MenuSource::Gtk(g) => g.bus_name != service,
            _ => true,
        });
        self.by_app.retain(|_, s| match s {
            MenuSource::Kde(k) => k.service != service,
            MenuSource::Gtk(g) => g.bus_name != service,
            _ => true,
        });
    }

    pub fn resolve(&self, id: &WindowIdentity) -> MenuSource {
        if let Some(s) = self.explicit.get(&id.window) {
            return s.clone();
        }
        self.by_app.get(&id.app_id).cloned().unwrap_or(MenuSource::Synthetic)
    }

    /// The source for an app: the key window's, else the first of its other windows that has an
    /// exported menu, else the synthetic fallback.
    pub fn resolve_app(&self, key_window: &WindowIdentity, others: &[WindowIdentity]) -> MenuSource {
        let first = self.resolve(key_window);
        if first.is_exported() {
            return first;
        }
        others.iter().map(|o| self.resolve(o)).find(MenuSource::is_exported).unwrap_or(first)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kde(service: &str) -> KdeAppMenu {
        KdeAppMenu { service: service.into(), path: "/MenuBar/1".into() }
    }

    fn win(w: u64) -> WindowIdentity {
        WindowIdentity { window: w, app_id: "org.example.app".into(), pid: Some(42) }
    }

    #[test]
    fn unknown_windows_fall_back() {
        let m = MenuSourceMap::new();
        assert_eq!(m.resolve(&win(1)), MenuSource::Synthetic);
        assert!(!MenuSource::Synthetic.is_exported());
    }

    #[test]
    fn explicit_kde_and_gtk_sources() {
        let mut m = MenuSourceMap::new();
        m.set_window_source(1, MenuSource::Kde(kde(":1.55")));
        let gtk = GtkMenuProps { bus_name: ":1.60".into(), menubar_object_path: Some("/org/example/menus/menubar".into()), ..Default::default() };
        m.set_window_source(2, MenuSource::Gtk(gtk.clone()));
        assert_eq!(m.resolve(&win(1)), MenuSource::Kde(kde(":1.55")));
        assert_eq!(m.resolve(&win(2)), MenuSource::Gtk(gtk.clone()));
        assert!(gtk.has_menu_bar());
        m.clear_window(1);
        assert_eq!(m.resolve(&win(1)), MenuSource::Synthetic);
        m.set_window_source(2, MenuSource::Synthetic);
        assert_eq!(m.resolve(&win(2)), MenuSource::Synthetic, "setting the fallback removes the entry");
    }

    #[test]
    fn app_level_source_and_service_disappearance() {
        let mut m = MenuSourceMap::new();
        m.set_app_source("org.example.app", Some(MenuSource::Kde(kde(":1.3"))));
        assert_eq!(m.resolve(&win(9)), MenuSource::Kde(kde(":1.3")));
        m.set_window_source(2, MenuSource::Kde(kde(":1.3")));
        m.set_window_source(3, MenuSource::Kde(kde(":1.4")));
        m.service_vanished(":1.3");
        assert_eq!(m.resolve(&win(9)), MenuSource::Synthetic);
        assert_eq!(m.resolve(&win(3)), MenuSource::Kde(kde(":1.4")), "another service's menu stays");
        assert_eq!(m.resolve(&win(2)), MenuSource::Synthetic);
        m.set_app_source("org.example.app", Some(MenuSource::Synthetic));
        assert_eq!(m.resolve(&win(9)), MenuSource::Synthetic);
    }

    #[test]
    fn app_resolution_prefers_the_key_window_then_other_windows() {
        let mut m = MenuSourceMap::new();
        m.set_window_source(2, MenuSource::Kde(kde(":1.2")));
        assert_eq!(m.resolve_app(&win(1), &[win(2)]), MenuSource::Kde(kde(":1.2")));
        m.set_window_source(1, MenuSource::Kde(kde(":1.1")));
        assert_eq!(m.resolve_app(&win(1), &[win(2)]), MenuSource::Kde(kde(":1.1")));
        let m = MenuSourceMap::new();
        assert_eq!(m.resolve_app(&win(1), &[]), MenuSource::Synthetic);
    }
}
