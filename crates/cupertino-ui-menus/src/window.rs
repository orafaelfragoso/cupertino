//! The menus of one application window, published for the desktop's menu bar: the exporter on the session bus, the
//! window's announcement (`cupertino_app_menu_v1`), the reader the compositor names, and the chosen items handed to the
//! application's loop.

use std::sync::mpsc;

use cupertino_menu_model::{ActionName, MenuBar};
use cupertino_modifiers::ModifierMap;
use cupertino_protocol_app::{AppEffects, AppEvent, AppMenu, MenuAddress};
use cupertino_wayland_client::{Connection, SurfaceId};

use crate::{Activation, Exporter, WindowId};

/// The window's menus while they are published.
pub struct WindowMenus {
    exporter: Exporter,
    /// Dropping it withdraws the announcement.
    _menu: AppMenu,
    surface: SurfaceId,
    window: WindowId,
    activations: mpsc::Receiver<Activation>,
    shown: bool,
}

impl WindowMenus {
    /// Publish `bar` for the window of `surface`. `None` when the compositor takes no announcement (app effects
    /// version 1) or there is no session bus: the window then keeps showing its menus itself. A chosen item wakes
    /// `conn`'s [`Connection::wait`] and is read with [`WindowMenus::activations`].
    pub fn publish(
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
        let (tx, activations) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        let waker = conn.waker();
        let exported = zbus::blocking::connection::Builder::session().and_then(|builder| {
            let mut exporter = Exporter::new(builder, app_id, map, move |activation| {
                // the receiver is gone only while the window closes
                let _ = tx.lock().unwrap().send(activation);
                waker.wake();
            })?;
            let props = exporter.export(WindowId(1), bar)?;
            Ok((exporter, props))
        });
        let (exporter, props) = match exported {
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
            exporter,
            _menu: menu,
            surface,
            window: WindowId(1),
            activations,
            shown: false,
        })
    }

    /// Publish the menus again after the model changed.
    pub fn update(&mut self, bar: &MenuBar) {
        if let Err(e) = self.exporter.export(self.window, bar) {
            tracing::warn!(error = %e, "cannot update the window's menus");
        }
    }

    /// Enable or disable the item of `action`.
    pub fn set_enabled(&self, action: &ActionName, enabled: bool) {
        if let Err(e) = self.exporter.set_enabled(self.window, action, enabled) {
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
                if let Err(e) = self.exporter.set_reader(bus_name.as_deref()) {
                    tracing::warn!(error = %e, "the compositor named a reader that is no bus name");
                }
                true
            }
            _ => false,
        }
    }

    /// Whether the desktop's menu bar shows the menus: the window then draws no menu bar of its own.
    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The items chosen since the last call.
    pub fn activations(&self) -> impl Iterator<Item = Activation> + '_ {
        self.activations.try_iter()
    }
}
