//! The menus of one application window, published for the desktop's menu bar: the window's objects on the
//! application's session-bus connection, its announcement (`cupertino_app_menu_v1`), the reader the compositor names,
//! and the chosen items handed to the application's loop.

use cupertino_menu_model::{ActionName, MenuBar};
use cupertino_modifiers::ModifierMap;
use cupertino_protocol_app::{AppEffects, AppEvent, AppMenu, MenuAddress};
use cupertino_wayland_client::{Connection, SurfaceId};

use crate::{Activation, AppMenus, WindowExport};

/// The window's menus while they are published.
pub struct WindowMenus {
    /// Dropped first: the objects go before their announcement.
    export: WindowExport,
    _menu: AppMenu,
    surface: SurfaceId,
    shown: bool,
}

impl WindowMenus {
    /// Publish `bar` for the window of `surface` on the connection `apps` has for `app_id`. `None` when the compositor
    /// takes no announcement (app effects version 1) or there is no session bus: the window then keeps showing its
    /// menus itself. A chosen item wakes `conn`'s [`Connection::wait`] and is read with [`WindowMenus::activations`].
    pub fn publish(
        apps: &mut AppMenus,
        conn: &Connection,
        effects: &AppEffects,
        surface: SurfaceId,
        app_id: &str,
        map: Box<dyn ModifierMap + Send + Sync>,
        bar: &MenuBar,
    ) -> Option<WindowMenus> {
        let menu = match effects.menu(conn, surface) {
            Ok(Some(menu)) => menu,
            Ok(None) => return None,
            Err(e) => {
                tracing::warn!(error = %e, "cannot announce the window's menus");
                return None;
            }
        };
        let waker = conn.waker();
        let exported = apps
            .open(app_id, map, move || waker.wake())
            .and_then(|export| export.export(bar).map(|props| (export, props)));
        let (export, props) = match exported {
            Ok(exported) => exported,
            Err(e) => {
                tracing::warn!(error = %e, "cannot publish the window's menus on the session bus");
                return None;
            }
        };
        menu.set_address(&MenuAddress {
            bus_name: props.unique_bus_name,
            application_object_path: props.application_object_path,
            window_object_path: props.window_object_path,
            menubar_object_path: props.menubar_path,
            app_menu_object_path: Some(props.app_menu_path),
        });
        Some(WindowMenus {
            export,
            _menu: menu,
            surface,
            shown: false,
        })
    }

    /// Publish the menus again after the model changed.
    pub fn update(&mut self, bar: &MenuBar) {
        if let Err(e) = self.export.export(bar) {
            tracing::warn!(error = %e, "cannot update the window's menus");
        }
    }

    /// Enable or disable the item of `action`.
    pub fn set_enabled(&self, action: &ActionName, enabled: bool) {
        if let Err(e) = self.export.set_enabled(action, enabled) {
            tracing::warn!(error = %e, action = %action.full(), "cannot change an item of the window's menus");
        }
    }

    /// Take an event of the window's announcement: the reader and whether the bar shows the menus. False for any other
    /// event.
    pub fn handle(&mut self, event: &AppEvent) -> bool {
        match event {
            AppEvent::MenuBar { surface, shown } if *surface == self.surface => {
                self.shown = *shown;
                true
            }
            AppEvent::MenuReader { surface, bus_name } if *surface == self.surface => {
                if let Err(e) = self.export.set_reader(bus_name.as_deref()) {
                    tracing::warn!(error = %e, "the compositor named a reader that is no bus name");
                }
                true
            }
            _ => false,
        }
    }

    /// Whether the desktop's menu bar shows the menus: the window then draws no menu bar of its own. Files and Settings
    /// draw none either way.
    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The items chosen since the last call.
    pub fn activations(&self) -> impl Iterator<Item = Activation> + '_ {
        self.export.activations()
    }
}
