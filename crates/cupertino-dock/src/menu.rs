//! The dock's context menus: a tile's and the separator's, as items. CupertinoKit shows them as a popup of the bar
//! ([`crate::view::menu`]), which lays the items out and takes the pointer and the keys.
//!
//! The item lists follow the design tables with these
//! deviations, all approximate: the "Options" submenu is flattened into the top level (no
//! submenus here), "Open at Login" is absent (there is no login-items store), "Show Recents",
//! "Dock Settings" and "Minimize Using" are absent (the recents are `dock.conf`'s `recents`, there
//! is no settings application to open, and the minimize effect is the server's setting), and the Option-key alternates (Hide Others, Force
//! Quit) are not offered. A running application's menu starts with its windows, each of which the item brings
//! forward. The rows, separators and material are the toolkit's menu's ([`crate::view::menu::style`]).

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
    /// Bring the window with this id forward: restored when it is minimized, raised and given the keyboard.
    ShowWindow(u32),
    /// Ask the file manager, the tile with this key, for another window.
    NewWindow(String),
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

/// A window of a tile's application as its menu lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileWindow {
    /// The server's id of the window.
    pub id: u32,
    pub title: String,
}

/// What a window without a title is listed as.
pub const UNTITLED: &str = "Untitled";

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
    /// The windows of its application, in the order they opened.
    pub windows: Vec<TileWindow>,
}

/// The application's windows, each bringing its window forward, and a separator after them; nothing without windows.
fn window_items(f: &TileFacts<'_>) -> Vec<MenuItem> {
    let mut items: Vec<MenuItem> = f
        .windows
        .iter()
        .map(|w| {
            let title = w.title.trim();
            MenuItem::item(
                if title.is_empty() { UNTITLED } else { title },
                MenuCommand::ShowWindow(w.id),
            )
        })
        .collect();
    if !items.is_empty() {
        items.push(MenuItem::separator());
    }
    items
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
        // the file manager's menus have an empty Options submenu and no Quit: running, its windows, New Window, Show
        // All Windows (while it shows a window) and Hide; not running, Open. Its Relaunch is not offered (approximate:
        // the dock has no relaunch path)
        TileKind::App if f.built_in => match f.running_app_id {
            Some(app) => {
                let mut items = not_responding(f, app);
                items.extend(window_items(f));
                items.push(MenuItem::item(
                    "New Window",
                    MenuCommand::NewWindow(f.key.to_string()),
                ));
                items.push(MenuItem::separator());
                items.push(MenuItem {
                    enabled: !f.windows.is_empty(),
                    ..MenuItem::item(
                        "Show All Windows",
                        MenuCommand::ShowAllWindows(app.to_string()),
                    )
                });
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
            if f.running_app_id.is_some() {
                items.extend(window_items(f));
            }
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
