//! Mapping from a window to a menu source.
//!
//! The compositor learns about exported menus from several places and feeds them here:
//! * the KDE appmenu Wayland protocol: service name + object path per surface;
//! * the GTK shell protocol: bus name and object paths of `org.gtk.Menus` / `org.gtk.Actions`.
//!
//! [`MenuSourceMap::resolve`] picks the source a window registered, never another window's, and
//! falls back to [`MenuSource::Synthetic`], for which [`crate::fallback`] builds the menus.

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

#[derive(Debug, Default)]
pub struct MenuSourceMap {
    explicit: HashMap<WindowKey, MenuSource>,
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

    pub fn resolve(&self, window: WindowKey) -> MenuSource {
        self.explicit
            .get(&window)
            .cloned()
            .unwrap_or(MenuSource::Synthetic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kde(service: &str) -> KdeAppMenu {
        KdeAppMenu {
            service: service.into(),
            path: "/MenuBar/1".into(),
        }
    }

    #[test]
    fn unknown_windows_fall_back() {
        let m = MenuSourceMap::new();
        assert_eq!(m.resolve(1), MenuSource::Synthetic);
        assert!(!MenuSource::Synthetic.is_exported());
    }

    #[test]
    fn explicit_kde_and_gtk_sources() {
        let mut m = MenuSourceMap::new();
        m.set_window_source(1, MenuSource::Kde(kde(":1.55")));
        let gtk = GtkMenuProps {
            bus_name: ":1.60".into(),
            menubar_object_path: Some("/org/example/menus/menubar".into()),
            ..Default::default()
        };
        m.set_window_source(2, MenuSource::Gtk(gtk.clone()));
        assert_eq!(m.resolve(1), MenuSource::Kde(kde(":1.55")));
        assert_eq!(m.resolve(2), MenuSource::Gtk(gtk.clone()));
        assert!(gtk.has_menu_bar());
        m.clear_window(1);
        assert_eq!(m.resolve(1), MenuSource::Synthetic);
        m.set_window_source(2, MenuSource::Synthetic);
        assert_eq!(
            m.resolve(2),
            MenuSource::Synthetic,
            "setting the fallback removes the entry"
        );
    }
}
