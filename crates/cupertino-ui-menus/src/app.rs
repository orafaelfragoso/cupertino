//! The menus of all of an application's windows on one session-bus connection: each window gets its own
//! [`WindowId`] and object path on the application's [`Exporter`], and the items chosen in its menus come back to it
//! alone.

use std::collections::BTreeMap;
use std::sync::{mpsc, Arc, Mutex, Weak};

use cupertino_menu_model::{ActionName, MenuBar};
use cupertino_modifiers::ModifierMap;
use zbus::blocking::connection;

use crate::{Activation, DbusProperties, Exporter, WindowId};

type Connect = Box<dyn Fn() -> zbus::Result<connection::Builder<'static>> + Send + Sync>;
type Wake = Box<dyn Fn() + Send + Sync>;

/// The bus connections of a process's applications, one for each application id. A connection is made for the
/// application's first window and closed with its last.
pub struct AppMenus {
    connect: Connect,
    apps: BTreeMap<String, Weak<App>>,
}

/// One application's exporter and the windows its chosen items go to.
struct App {
    exporter: Mutex<Exporter>,
    routes: Arc<Mutex<Routes>>,
}

#[derive(Default)]
struct Routes {
    last: u32,
    windows: BTreeMap<WindowId, Route>,
}

struct Route {
    activations: mpsc::Sender<Activation>,
    wake: Wake,
}

impl Routes {
    /// The window an activation is for: the window of a `win.` action, the newest window for an `app.` action, as
    /// the application menu is the newest window's.
    fn send(&self, activation: Activation) {
        let route = match activation.window {
            Some(window) => self.windows.get(&window),
            None => self.windows.values().next_back(),
        };
        if let Some(route) = route {
            // the receiver is gone only while the window closes
            let _ = route.activations.send(activation);
            (route.wake)();
        }
    }
}

impl Default for AppMenus {
    /// On the session bus.
    fn default() -> AppMenus {
        AppMenus::on(connection::Builder::session)
    }
}

impl AppMenus {
    /// On the bus `connect` builds connections to.
    pub fn on(
        connect: impl Fn() -> zbus::Result<connection::Builder<'static>> + Send + Sync + 'static,
    ) -> AppMenus {
        AppMenus {
            connect: Box::new(connect),
            apps: BTreeMap::new(),
        }
    }

    /// A new window of `app_id`, on the application's connection (made now if it has no window open). `map` is used
    /// only when the connection is made. `wake` runs on the bus connection's thread after each item chosen in the
    /// window's menus.
    pub fn open(
        &mut self,
        app_id: &str,
        map: Box<dyn ModifierMap + Send + Sync>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> zbus::Result<WindowExport> {
        let app = match self.apps.get(app_id).and_then(Weak::upgrade) {
            Some(app) => app,
            None => {
                let routes = Arc::new(Mutex::new(Routes::default()));
                let to = routes.clone();
                let exporter = Exporter::new((self.connect)()?, app_id, map, move |activation| {
                    to.lock().unwrap().send(activation)
                })?;
                let app = Arc::new(App {
                    exporter: Mutex::new(exporter),
                    routes,
                });
                self.apps.retain(|_, app| app.strong_count() > 0);
                self.apps.insert(app_id.to_string(), Arc::downgrade(&app));
                app
            }
        };
        let (tx, activations) = mpsc::channel();
        let window = {
            let mut routes = app.routes.lock().unwrap();
            routes.last += 1;
            let window = WindowId(routes.last);
            routes.windows.insert(
                window,
                Route {
                    activations: tx,
                    wake: Box::new(wake),
                },
            );
            window
        };
        Ok(WindowExport {
            app,
            window,
            activations,
        })
    }
}

/// The menus of one window on its application's connection. Dropping it withdraws them; the other windows' menus
/// stay.
pub struct WindowExport {
    app: Arc<App>,
    window: WindowId,
    activations: mpsc::Receiver<Activation>,
}

impl WindowExport {
    pub fn window(&self) -> WindowId {
        self.window
    }

    /// Publish (or update) the window's menus from its full menu model.
    pub fn export(&self, bar: &MenuBar) -> zbus::Result<DbusProperties> {
        self.app.exporter.lock().unwrap().export(self.window, bar)
    }

    /// Enable or disable an action of the window (`win.`) or of the application (`app.`).
    pub fn set_enabled(&self, action: &ActionName, enabled: bool) -> zbus::Result<()> {
        self.app
            .exporter
            .lock()
            .unwrap()
            .set_enabled(self.window, action, enabled)
    }

    /// The menu bar the compositor verified ([`Exporter::set_reader`]); it reads every window of the application.
    pub fn set_reader(&self, reader: Option<&str>) -> zbus::Result<()> {
        self.app.exporter.lock().unwrap().set_reader(reader)
    }

    /// The items chosen in the window's menus since the last call.
    pub fn activations(&self) -> impl Iterator<Item = Activation> + '_ {
        self.activations.try_iter()
    }
}

impl Drop for WindowExport {
    fn drop(&mut self) {
        self.app.routes.lock().unwrap().windows.remove(&self.window);
        if let Err(e) = self.app.exporter.lock().unwrap().remove(self.window) {
            tracing::warn!(error = %e, "cannot withdraw the menus of a closed window");
        }
    }
}
