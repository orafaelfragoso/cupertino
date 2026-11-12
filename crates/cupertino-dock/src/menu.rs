//! The dock's context menus: a tile's and the separator's, as items. CupertinoKit shows them as a popup of the bar
//! ([`crate::view::menu`]), which lays the items out and takes the pointer and the keys.
//!
//! The item lists follow the design tables with these
//! deviations, all approximate: the "Options" submenu is flattened into the top level (no
//! submenus here), "Open at Login" is absent (there is no login-items store), "Show Recents",
//! "Dock Settings" and "Minimize Using" are absent (the recents are `dock.conf`'s `recents`, there
//! is no settings application to open, and the minimize effect is the server's setting), and the Option-key alternates (Hide Others, Force
//! Quit) are not offered. The metrics and colours are the menu bar's (approximate).

use std::path::{Path, PathBuf};

use crate::config::{DockConfig, Orientation};
use crate::tiles::TileKind;

/// Gap between a side dock's bar and the popup.
pub const GAP: f64 = 6.0;

/// What a menu item does when picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuCommand {
    /// Pin the tile with this key.
    KeepInDock(String),
    /// Unpin the tile with this key.
    RemoveFromDock(String),
    /// Open the directory that holds the application's desktop entry.
    ShowInFileManager(PathBuf),
    /// The application overview of this application id (`app_command`).
    ShowAllWindows(String),
    Hide(String),
    Quit(String),
    /// Open the Force Quit dialog on the confirmation of this application id.
    ForceQuit(String),
    /// Launch the tile with this key.
    Open(String),
    OpenTrash,
    /// Delete everything in the trash, once asked for again.
    EmptyTrash,
    ToggleAutoHide,
    ToggleMagnification,
    Position(Orientation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    /// `None` for a separator.
    pub command: Option<MenuCommand>,
    pub separator: bool,
    pub enabled: bool,
    /// A radio mark before the label.
    pub checked: bool,
}

impl MenuItem {
    fn item(label: &str, command: MenuCommand) -> Self {
        Self {
            label: label.to_string(),
            command: Some(command),
            separator: false,
            enabled: true,
            checked: false,
        }
    }

    fn disabled(label: &str, command: MenuCommand) -> Self {
        Self {
            enabled: false,
            ..Self::item(label, command)
        }
    }

    fn radio(label: &str, command: MenuCommand, checked: bool) -> Self {
        Self {
            checked,
            ..Self::item(label, command)
        }
    }

    fn separator() -> Self {
        Self {
            label: String::new(),
            command: None,
            separator: true,
            enabled: false,
            checked: false,
        }
    }

    pub fn is_selectable(&self) -> bool {
        !self.separator && self.enabled
    }
}

/// What the menu of a tile depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileFacts<'a> {
    pub kind: TileKind,
    /// The tile's key (desktop file ID, `app_id` or command).
    pub key: &'a str,
    pub pinned: bool,
    /// The built-in file manager tile ([`crate::tiles::Tile::built_in`]).
    pub built_in: bool,
    /// The application id of its windows while it runs.
    pub running_app_id: Option<&'a str>,
    /// The server reports that the application stopped answering.
    pub unresponsive: bool,
    /// The desktop entry file, when the key resolves to one.
    pub entry_path: Option<&'a Path>,
    /// The trash holds something: its menu can empty it.
    pub trash_full: bool,
}

/// The disabled status row that heads the menu of an application that stopped answering.
fn not_responding(f: &TileFacts<'_>, app: &str) -> Vec<MenuItem> {
    if !f.unresponsive {
        return Vec::new();
    }
    vec![
        MenuItem::disabled(
            "Application Not Responding",
            MenuCommand::ForceQuit(app.to_string()),
        ),
        MenuItem::separator(),
    ]
}

/// Force Quit for the menu of the file manager, which has no Quit.
fn force_quit(f: &TileFacts<'_>, app: &str) -> Option<MenuItem> {
    f.unresponsive
        .then(|| MenuItem::item("Force Quit", MenuCommand::ForceQuit(app.to_string())))
}

/// The menu of a tile (running and not running applications, the file manager; the
/// trash). Empty for a separator and a minimized-window tile, which have no menu.
pub fn tile_menu(f: &TileFacts<'_>) -> Vec<MenuItem> {
    match f.kind {
        TileKind::Separator | TileKind::MinimizedWindow => Vec::new(),
        TileKind::Trash => vec![
            MenuItem::item("Open", MenuCommand::OpenTrash),
            MenuItem::separator(),
            MenuItem {
                enabled: f.trash_full,
                ..MenuItem::item("Empty Trash", MenuCommand::EmptyTrash)
            },
        ],
        TileKind::Folder => vec![MenuItem::item(
            &format!("Open \u{201c}{}\u{201d}", crate::stack::folder_name(f.key)),
            MenuCommand::ShowInFileManager(PathBuf::from(f.key)),
        )],
        // the file manager's menus have an empty Options submenu and no Quit:
        // running, Show All Windows and Hide; not running, Open. Its Relaunch is not offered
        // (approximate: the dock has no relaunch path)
        TileKind::App if f.built_in => match f.running_app_id {
            Some(app) => {
                let mut items = not_responding(f, app);
                items.push(MenuItem::item(
                    "Show All Windows",
                    MenuCommand::ShowAllWindows(app.to_string()),
                ));
                items.push(MenuItem::item("Hide", MenuCommand::Hide(app.to_string())));
                items.extend(force_quit(f, app));
                items
            }
            None => vec![MenuItem::item("Open", MenuCommand::Open(f.key.to_string()))],
        },
        TileKind::App | TileKind::Recent => {
            let mut items = f
                .running_app_id
                .map_or_else(Vec::new, |app| not_responding(f, app));
            items.push(if f.pinned {
                MenuItem::item(
                    "Remove from Dock",
                    MenuCommand::RemoveFromDock(f.key.to_string()),
                )
            } else {
                MenuItem::item("Keep in Dock", MenuCommand::KeepInDock(f.key.to_string()))
            });
            if let Some(path) = f.entry_path {
                items.push(MenuItem::item(
                    "Show in File Manager",
                    MenuCommand::ShowInFileManager(path.parent().unwrap_or(path).to_path_buf()),
                ));
            }
            items.push(MenuItem::separator());
            match f.running_app_id {
                Some(app) => {
                    items.push(MenuItem::item(
                        "Show All Windows",
                        MenuCommand::ShowAllWindows(app.to_string()),
                    ));
                    items.push(MenuItem::item("Hide", MenuCommand::Hide(app.to_string())));
                    items.push(if f.unresponsive {
                        MenuItem::item("Force Quit", MenuCommand::ForceQuit(app.to_string()))
                    } else {
                        MenuItem::item("Quit", MenuCommand::Quit(app.to_string()))
                    });
                }
                None => {
                    items.push(MenuItem::disabled(
                        "Show All Windows",
                        MenuCommand::ShowAllWindows(f.key.to_string()),
                    ));
                    items.push(MenuItem::item("Open", MenuCommand::Open(f.key.to_string())));
                }
            }
            items
        }
    }
}

/// The menu of the separator: hiding, magnification and the position, the last as a
/// flattened radio group that only shows the setting: the bar's surface is anchored when the
/// process starts, so the position is `dock.conf`'s (`position`) and the items are disabled.
pub fn separator_menu(cfg: &DockConfig) -> Vec<MenuItem> {
    let position = |label: &str, o: Orientation| MenuItem {
        enabled: false,
        ..MenuItem::radio(label, MenuCommand::Position(o), cfg.orientation == o)
    };
    vec![
        MenuItem::item(
            if cfg.autohide {
                "Turn Hiding Off"
            } else {
                "Turn Hiding On"
            },
            MenuCommand::ToggleAutoHide,
        ),
        MenuItem::item(
            if cfg.magnification {
                "Turn Magnification Off"
            } else {
                "Turn Magnification On"
            },
            MenuCommand::ToggleMagnification,
        ),
        MenuItem::separator(),
        position("Position: Left", Orientation::Left),
        position("Position: Bottom", Orientation::Bottom),
        position("Position: Right", Orientation::Right),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(
        kind: TileKind,
        pinned: bool,
        running: Option<&'a str>,
        path: Option<&'a Path>,
    ) -> TileFacts<'a> {
        TileFacts {
            kind,
            key: "org.example.editor",
            pinned,
            built_in: false,
            running_app_id: running,
            unresponsive: false,
            entry_path: path,
            trash_full: false,
        }
    }

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items
            .iter()
            .map(|i| if i.separator { "-" } else { i.label.as_str() })
            .collect()
    }

    #[test]
    fn an_unresponsive_application_is_headed_by_a_disabled_status_and_offers_force_quit() {
        let mut f = facts(TileKind::App, true, Some("org.example.editor"), None);
        f.unresponsive = true;
        let items = tile_menu(&f);
        assert_eq!(
            labels(&items),
            [
                "Application Not Responding",
                "-",
                "Remove from Dock",
                "-",
                "Show All Windows",
                "Hide",
                "Force Quit"
            ]
        );
        assert!(!items[0].enabled);
        assert_eq!(
            items[6].command,
            Some(MenuCommand::ForceQuit("org.example.editor".into()))
        );
    }

    #[test]
    fn an_unresponsive_file_manager_offers_force_quit_after_hide() {
        let items = tile_menu(&TileFacts {
            kind: TileKind::App,
            key: "cupertino-files",
            pinned: true,
            built_in: true,
            running_app_id: Some("cupertino-files"),
            unresponsive: true,
            entry_path: None,
            trash_full: false,
        });
        assert_eq!(
            labels(&items),
            [
                "Application Not Responding",
                "-",
                "Show All Windows",
                "Hide",
                "Force Quit"
            ]
        );
    }

    #[test]
    fn a_running_pinned_application_offers_the_reference_items() {
        let path = Path::new("/usr/share/applications/org.example.editor.desktop");
        let items = tile_menu(&facts(
            TileKind::App,
            true,
            Some("org.example.editor"),
            Some(path),
        ));
        assert_eq!(
            labels(&items),
            [
                "Remove from Dock",
                "Show in File Manager",
                "-",
                "Show All Windows",
                "Hide",
                "Quit"
            ]
        );
        assert_eq!(
            items[1].command,
            Some(MenuCommand::ShowInFileManager(PathBuf::from(
                "/usr/share/applications"
            )))
        );
        assert_eq!(
            items[3].command,
            Some(MenuCommand::ShowAllWindows("org.example.editor".into()))
        );
        assert!(items.iter().filter(|i| !i.separator).all(|i| i.enabled));
    }

    #[test]
    fn an_idle_unpinned_application_can_be_kept_and_opened_and_its_windows_item_is_disabled() {
        let items = tile_menu(&facts(TileKind::App, false, None, None));
        assert_eq!(
            labels(&items),
            ["Keep in Dock", "-", "Show All Windows", "Open"]
        );
        assert_eq!(
            items[0].command,
            Some(MenuCommand::KeepInDock("org.example.editor".into()))
        );
        assert!(!items[2].enabled && !items[2].is_selectable());
        assert_eq!(
            items[3].command,
            Some(MenuCommand::Open("org.example.editor".into()))
        );
    }

    #[test]
    fn the_file_manager_tile_can_be_neither_kept_nor_removed_nor_quit() {
        let file_manager = |running| TileFacts {
            kind: TileKind::App,
            key: "cupertino-files",
            pinned: true,
            built_in: true,
            running_app_id: running,
            unresponsive: false,
            entry_path: None,
            trash_full: false,
        };
        let running = tile_menu(&file_manager(Some("cupertino-files")));
        assert_eq!(labels(&running), ["Show All Windows", "Hide"]);
        assert_eq!(
            running[1].command,
            Some(MenuCommand::Hide("cupertino-files".into()))
        );
        let idle = tile_menu(&file_manager(None));
        assert_eq!(labels(&idle), ["Open"]);
        assert_eq!(
            idle[0].command,
            Some(MenuCommand::Open("cupertino-files".into()))
        );
    }

    #[test]
    fn the_trash_opens_and_separators_and_minimized_tiles_have_no_menu() {
        assert_eq!(
            tile_menu(&facts(TileKind::Trash, false, None, None))[0].command,
            Some(MenuCommand::OpenTrash)
        );
        assert!(tile_menu(&facts(TileKind::Separator, false, None, None)).is_empty());
        assert!(tile_menu(&facts(TileKind::MinimizedWindow, false, None, None)).is_empty());
    }

    #[test]
    fn the_separator_menu_names_the_current_settings() {
        let cfg = DockConfig {
            autohide: true,
            magnification: false,
            orientation: Orientation::Left,
            ..DockConfig::default()
        };
        let items = separator_menu(&cfg);
        assert_eq!(
            labels(&items),
            [
                "Turn Hiding Off",
                "Turn Magnification On",
                "-",
                "Position: Left",
                "Position: Bottom",
                "Position: Right"
            ]
        );
        assert!(items[3].checked && !items[4].checked && !items[5].checked);
        assert_eq!(
            items[5].command,
            Some(MenuCommand::Position(Orientation::Right))
        );
        assert!(
            items[3..].iter().all(|i| !i.enabled),
            "the position is read from the file, not changed here"
        );
    }
}
