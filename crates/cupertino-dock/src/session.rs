//! What the dock shows of the session: the windows the server reports, mapped to the tiles of the
//! applications they belong to, and what a click on a tile asks for.

use std::collections::BTreeMap;

use cupertino_wm::ids::WindowId;

use crate::conf::FILE_MANAGER;
use crate::desktop::{exec_command, exec_command_with, DesktopIndex};
use crate::dock::Dock;
use crate::tiles::{TileId, TileKind};

/// A window the server reported.
pub struct Window {
    /// Application id as the server reports it.
    pub app_id: String,
    /// Key of the tile of the application: a pinned tile that stands for it, else `app_id`.
    pub key: String,
    pub title: String,
    pub minimized: bool,
}

/// What a click on a tile asks the host to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activation {
    /// Raise the windows of a running application, by the application id the server reported.
    ActivateApp(String),
    /// Restore a minimized window out of its tile.
    RestoreWindow(u32),
    /// Start the application of the tile with this key.
    Launch(String),
    /// Open the trash.
    OpenTrash,
}

/// The command that starts an application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub argv: Vec<String>,
    /// The entry asks to run inside a terminal emulator.
    pub terminal: bool,
}

/// The dock model together with the windows it reflects.
pub struct Session {
    pub dock: Dock,
    desktop: DesktopIndex,
    /// Windows by server id.
    windows: BTreeMap<u32, Window>,
    /// The server's first list of windows was taken in: an application that appears after that
    /// has just started, one in the list was running before the dock was.
    live: bool,
}

impl Session {
    pub fn new(dock: Dock, desktop: DesktopIndex) -> Self {
        Self {
            dock,
            desktop,
            windows: BTreeMap::new(),
            live: false,
        }
    }

    /// The windows the server listed when the dock started are in: from now on a window of an
    /// application with none is a launch.
    pub fn list_taken(&mut self) {
        self.live = true;
    }

    /// The desktop entries the tiles resolve through.
    pub fn desktop(&self) -> &DesktopIndex {
        &self.desktop
    }

    /// Key of the tile an application id stands for: the tile of that id, else the tile pinned
    /// by the id of the same desktop entry or by one of its aliases, else the id itself.
    pub fn key_of(&self, app_id: &str) -> String {
        if self.dock.app_tile(app_id).is_some() {
            return app_id.to_string();
        }
        if let Some(e) = self.desktop.find(app_id) {
            for candidate in [e.id.as_str(), e.startup_wm_class.as_str()] {
                if !candidate.is_empty() && self.dock.app_tile(candidate).is_some() {
                    return candidate.to_string();
                }
            }
            let aliased = self.dock.tiles().into_iter().find(|t| {
                t.kind == TileKind::App && self.desktop.find(&t.key).is_some_and(|x| x.id == e.id)
            });
            if let Some(t) = aliased {
                return t.key.clone();
            }
        }
        app_id.to_string()
    }

    /// A window appeared or one of its properties changed.
    pub fn window(&mut self, id: u32, app_id: &str, title: &str, minimized: bool, now: f64) {
        if self.windows.get(&id).is_some_and(|w| w.app_id != app_id) {
            self.window_closed(id, now);
        }
        let key = self.key_of(app_id);
        if !self.windows.values().any(|w| w.key == key) {
            if self.live {
                self.dock.app_appeared(&key, now);
            } else {
                self.dock.app_started(&key, now);
            }
        }
        let was_minimized = self
            .windows
            .insert(
                id,
                Window {
                    app_id: app_id.to_string(),
                    key: key.clone(),
                    title: title.to_string(),
                    minimized,
                },
            )
            .is_some_and(|w| w.minimized);
        match (was_minimized, minimized) {
            (false, true) => self
                .dock
                .window_minimized(WindowId(u64::from(id)), &key, title, now),
            (true, false) => self.dock.window_restored(WindowId(u64::from(id)), now),
            _ => {}
        }
    }

    /// A window went away; the application leaves the running state with its last window.
    pub fn window_closed(&mut self, id: u32, now: f64) {
        let Some(window) = self.windows.remove(&id) else {
            return;
        };
        if window.minimized {
            self.dock.window_restored(WindowId(u64::from(id)), now);
        }
        if !self.windows.values().any(|w| w.key == window.key) {
            self.dock.app_quit(&window.key, now);
        }
    }

    /// The ids of the windows the server reported, in order.
    pub fn window_ids(&self) -> Vec<u32> {
        self.windows.keys().copied().collect()
    }

    /// A window the server reported, by its id.
    pub fn window_info(&self, id: u32) -> Option<&Window> {
        self.windows.get(&id)
    }

    /// The application id of the windows an application tile stands for; `None` while none runs.
    pub fn running_app_id(&self, id: TileId) -> Option<&str> {
        let tile = self.dock.tile(id)?;
        self.windows
            .values()
            .find(|w| w.key == tile.key)
            .map(|w| w.app_id.as_str())
    }

    /// The server started a program of this application: its tile bounces until a window appears.
    pub fn launching(&mut self, app_id: &str, now: f64) {
        let key = self.key_of(app_id);
        self.dock.app_launching(&key, now);
    }

    /// The server refused an application's activation: its tile hops once (informational
    /// attention). The application is named as the server reports it.
    pub fn attention(&mut self, app_id: &str, now: f64) {
        let key = self.key_of(app_id);
        self.dock.attention(&key, false, now);
    }

    /// What a click on tile `id` asks for; `None` for a separator or a tile that is gone.
    pub fn activation(&self, id: TileId) -> Option<Activation> {
        let tile = self.dock.tile(id)?;
        match tile.kind {
            TileKind::MinimizedWindow => tile.window.map(|w| {
                Activation::RestoreWindow(
                    u32::try_from(w.0).expect("window ids come from the server as u32"),
                )
            }),
            TileKind::App | TileKind::Recent => {
                Some(match self.windows.values().find(|w| w.key == tile.key) {
                    Some(w) => Activation::ActivateApp(w.app_id.clone()),
                    None => Activation::Launch(tile.key.clone()),
                })
            }
            TileKind::Trash => Some(Activation::OpenTrash),
            TileKind::Separator => None,
        }
    }

    /// The command that starts the application of tile `key`: the `Exec` of its desktop entry, or
    /// the key itself as a command when there is no entry.
    pub fn launch(&self, key: &str) -> Launch {
        match self.desktop.find(key) {
            Some(e) => Launch {
                argv: exec_command(&e.exec),
                terminal: e.terminal,
            },
            None => Launch {
                argv: vec![key.to_string()],
                terminal: false,
            },
        }
    }

    /// The command that opens `target` (a folder or `trash:///`) in the session's file manager
    /// ([`FILE_MANAGER`]); `None` when it is not installed.
    pub fn file_manager_command(&self, target: &str) -> Option<Vec<String>> {
        self.desktop
            .find(FILE_MANAGER)
            .map(|e| exec_command_with(&e.exec, target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DockConfig;
    use crate::desktop::parse_entry;
    use std::path::Path;

    #[test]
    fn the_trash_and_folders_open_in_the_session_file_manager_when_installed() {
        let entry = |id: &str, exec: &str| {
            parse_entry(
                id,
                Path::new("x"),
                &format!("[Desktop Entry]\nName={id}\nExec={exec}\n"),
            )
            .unwrap()
        };
        let host = DesktopIndex {
            entries: vec![entry("org.gnome.Nautilus", "nautilus --new-window %U")],
        };
        assert_eq!(
            Session::new(Dock::new(DockConfig::default()), host.clone())
                .file_manager_command("trash:///"),
            None
        );
        let mut session_apps = host;
        session_apps
            .entries
            .push(entry("cupertino-files", "cupertino-files %U"));
        let session = Session::new(Dock::new(DockConfig::default()), session_apps);
        assert_eq!(
            session.file_manager_command("trash:///"),
            Some(vec!["cupertino-files".to_string(), "trash:///".to_string()])
        );
        assert_eq!(
            session.file_manager_command("/usr/share/applications"),
            Some(vec![
                "cupertino-files".to_string(),
                "/usr/share/applications".to_string()
            ])
        );
    }
}
