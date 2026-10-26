//! The dock's context menus: a tile's and
//! the separator's, as items, their layout as a popup beside the bar, and the hit test.
//! Drawing is in `render::paint_menu`; the process opens the popup (`app.rs`).
//!
//! The item lists follow the design tables with these
//! deviations, all approximate: the "Options" submenu is flattened into the top level (no
//! submenus here), "Open at Login" is absent (there is no login-items store), "Show Recents",
//! "Dock Settings" and "Minimize Using" are absent (no recents source, no settings application;
//! the minimize effect is the server's setting), and the Option-key alternates (Hide Others, Force
//! Quit) are not offered. The metrics and colours are the menu bar's (approximate).

use std::path::{Path, PathBuf};

use cupertino_wm::geometry::{Point, Rect};

use crate::config::{DockConfig, Orientation};
use crate::tiles::TileKind;

/// Row metrics, as the menu bar's popups (approximate).
pub const ROW_HEIGHT: f64 = 22.0;
pub const SEPARATOR_HEIGHT: f64 = 9.0;
pub const PADDING_Y: f64 = 5.0;
/// Horizontal inset of the labels inside the popup.
pub const TEXT_INSET: f64 = 14.0;
pub const MIN_WIDTH: f64 = 120.0;
/// The popup keeps this far from the output's edges.
pub const SCREEN_MARGIN: f64 = 4.0;
/// Gap between the bar and the popup.
pub const GAP: f64 = 6.0;
pub const RADIUS: f64 = 8.0;
pub const FONT_PT: f64 = 13.0;

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
    /// Launch the tile with this key.
    Open(String),
    OpenTrash,
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
        Self { label: label.to_string(), command: Some(command), separator: false, enabled: true, checked: false }
    }

    fn disabled(label: &str, command: MenuCommand) -> Self {
        Self { enabled: false, ..Self::item(label, command) }
    }

    fn radio(label: &str, command: MenuCommand, checked: bool) -> Self {
        Self { checked, ..Self::item(label, command) }
    }

    fn separator() -> Self {
        Self { label: String::new(), command: None, separator: true, enabled: false, checked: false }
    }

    pub fn is_selectable(&self) -> bool {
        !self.separator && self.enabled
    }

    fn height(&self) -> f64 {
        if self.separator {
            SEPARATOR_HEIGHT
        } else {
            ROW_HEIGHT
        }
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
    /// The desktop entry file, when the key resolves to one.
    pub entry_path: Option<&'a Path>,
}

/// The menu of a tile (running and not running applications, the file manager; the
/// trash). Empty for a separator and a minimized-window tile, which have no menu.
pub fn tile_menu(f: &TileFacts<'_>) -> Vec<MenuItem> {
    match f.kind {
        TileKind::Separator | TileKind::MinimizedWindow => Vec::new(),
        TileKind::Trash => vec![MenuItem::item("Open", MenuCommand::OpenTrash)],
        // the file manager's menus have an empty Options submenu and no Quit:
        // running, Show All Windows and Hide; not running, Open. Its Relaunch is not offered
        // (approximate: the dock has no relaunch path)
        TileKind::App if f.built_in => match f.running_app_id {
            Some(app) => vec![MenuItem::item("Show All Windows", MenuCommand::ShowAllWindows(app.to_string())), MenuItem::item("Hide", MenuCommand::Hide(app.to_string()))],
            None => vec![MenuItem::item("Open", MenuCommand::Open(f.key.to_string()))],
        },
        TileKind::App | TileKind::Recent => {
            let mut items = vec![if f.pinned { MenuItem::item("Remove from Dock", MenuCommand::RemoveFromDock(f.key.to_string())) } else { MenuItem::item("Keep in Dock", MenuCommand::KeepInDock(f.key.to_string())) }];
            if let Some(path) = f.entry_path {
                items.push(MenuItem::item("Show in File Manager", MenuCommand::ShowInFileManager(path.parent().unwrap_or(path).to_path_buf())));
            }
            items.push(MenuItem::separator());
            match f.running_app_id {
                Some(app) => {
                    items.push(MenuItem::item("Show All Windows", MenuCommand::ShowAllWindows(app.to_string())));
                    items.push(MenuItem::item("Hide", MenuCommand::Hide(app.to_string())));
                    items.push(MenuItem::item("Quit", MenuCommand::Quit(app.to_string())));
                }
                None => {
                    items.push(MenuItem::disabled("Show All Windows", MenuCommand::ShowAllWindows(f.key.to_string())));
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
    let position = |label: &str, o: Orientation| MenuItem { enabled: false, ..MenuItem::radio(label, MenuCommand::Position(o), cfg.orientation == o) };
    vec![
        MenuItem::item(if cfg.autohide { "Turn Hiding Off" } else { "Turn Hiding On" }, MenuCommand::ToggleAutoHide),
        MenuItem::item(if cfg.magnification { "Turn Magnification Off" } else { "Turn Magnification On" }, MenuCommand::ToggleMagnification),
        MenuItem::separator(),
        position("Position: Left", Orientation::Left),
        position("Position: Bottom", Orientation::Bottom),
        position("Position: Right", Orientation::Right),
    ]
}

/// A laid-out menu, in the coordinates of the bar surface it pops up from.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuLayout {
    pub rect: Rect,
    /// One rectangle per item, in the same coordinates.
    pub rows: Vec<Rect>,
}

/// Lay the menu out beside the tile rectangle `anchor` (bar-surface coordinates): above it for a
/// bottom dock, to its right or left for a side dock, kept inside `screen` (the output, in the
/// same coordinates) by [`SCREEN_MARGIN`]. `measure` gives a label's width in points.
pub fn layout(items: &[MenuItem], anchor: &Rect, screen: &Rect, orientation: Orientation, measure: &dyn Fn(&str) -> f64) -> MenuLayout {
    let widest = items.iter().filter(|i| !i.separator).map(|i| measure(&i.label)).fold(0.0, f64::max);
    let w = (widest + 2.0 * TEXT_INSET).max(MIN_WIDTH).ceil();
    let h = items.iter().map(MenuItem::height).sum::<f64>() + 2.0 * PADDING_Y;
    let (x, y) = match orientation {
        Orientation::Bottom => (anchor.cx() - w / 2.0, anchor.y - GAP - h),
        Orientation::Left => (anchor.right() + GAP, anchor.cy() - h / 2.0),
        Orientation::Right => (anchor.x - GAP - w, anchor.cy() - h / 2.0),
    };
    let inner = screen.inset(SCREEN_MARGIN);
    let x = x.max(inner.x).min(inner.right() - w).round();
    let y = y.max(inner.y).min(inner.bottom() - h).round();
    let rect = Rect::new(x, y, w, h);
    let mut rows = Vec::with_capacity(items.len());
    let mut top = y + PADDING_Y;
    for item in items {
        rows.push(Rect::new(x, top, w, item.height()));
        top += item.height();
    }
    MenuLayout { rect, rows }
}

impl MenuLayout {
    /// The selectable item under `p`.
    pub fn hit(&self, items: &[MenuItem], p: Point) -> Option<usize> {
        self.rows.iter().position(|r| r.contains(p)).filter(|i| items[*i].is_selectable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(kind: TileKind, pinned: bool, running: Option<&'a str>, path: Option<&'a Path>) -> TileFacts<'a> {
        TileFacts { kind, key: "org.example.editor", pinned, built_in: false, running_app_id: running, entry_path: path }
    }

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| if i.separator { "-" } else { i.label.as_str() }).collect()
    }

    #[test]
    fn a_running_pinned_application_offers_the_reference_items() {
        let path = Path::new("/usr/share/applications/org.example.editor.desktop");
        let items = tile_menu(&facts(TileKind::App, true, Some("org.example.editor"), Some(path)));
        assert_eq!(labels(&items), ["Remove from Dock", "Show in File Manager", "-", "Show All Windows", "Hide", "Quit"]);
        assert_eq!(items[1].command, Some(MenuCommand::ShowInFileManager(PathBuf::from("/usr/share/applications"))));
        assert_eq!(items[3].command, Some(MenuCommand::ShowAllWindows("org.example.editor".into())));
        assert!(items.iter().filter(|i| !i.separator).all(|i| i.enabled));
    }

    #[test]
    fn an_idle_unpinned_application_can_be_kept_and_opened_and_its_windows_item_is_disabled() {
        let items = tile_menu(&facts(TileKind::App, false, None, None));
        assert_eq!(labels(&items), ["Keep in Dock", "-", "Show All Windows", "Open"]);
        assert_eq!(items[0].command, Some(MenuCommand::KeepInDock("org.example.editor".into())));
        assert!(!items[2].enabled && !items[2].is_selectable());
        assert_eq!(items[3].command, Some(MenuCommand::Open("org.example.editor".into())));
    }

    #[test]
    fn the_file_manager_tile_can_be_neither_kept_nor_removed_nor_quit() {
        let file_manager = |running| TileFacts { kind: TileKind::App, key: "cupertino-files", pinned: true, built_in: true, running_app_id: running, entry_path: None };
        let running = tile_menu(&file_manager(Some("cupertino-files")));
        assert_eq!(labels(&running), ["Show All Windows", "Hide"]);
        assert_eq!(running[1].command, Some(MenuCommand::Hide("cupertino-files".into())));
        let idle = tile_menu(&file_manager(None));
        assert_eq!(labels(&idle), ["Open"]);
        assert_eq!(idle[0].command, Some(MenuCommand::Open("cupertino-files".into())));
    }

    #[test]
    fn the_trash_opens_and_separators_and_minimized_tiles_have_no_menu() {
        assert_eq!(tile_menu(&facts(TileKind::Trash, false, None, None))[0].command, Some(MenuCommand::OpenTrash));
        assert!(tile_menu(&facts(TileKind::Separator, false, None, None)).is_empty());
        assert!(tile_menu(&facts(TileKind::MinimizedWindow, false, None, None)).is_empty());
    }

    #[test]
    fn the_separator_menu_names_the_current_settings() {
        let cfg = DockConfig { autohide: true, magnification: false, orientation: Orientation::Left, ..DockConfig::default() };
        let items = separator_menu(&cfg);
        assert_eq!(labels(&items), ["Turn Hiding Off", "Turn Magnification On", "-", "Position: Left", "Position: Bottom", "Position: Right"]);
        assert!(items[3].checked && !items[4].checked && !items[5].checked);
        assert_eq!(items[5].command, Some(MenuCommand::Position(Orientation::Right)));
        assert!(items[3..].iter().all(|i| !i.enabled), "the position is read from the file, not changed here");
    }

    #[test]
    fn the_popup_sits_above_a_bottom_dock_tile_and_stays_on_the_output() {
        let items = tile_menu(&facts(TileKind::App, true, Some("a"), None));
        let measure = |s: &str| s.len() as f64 * 7.0;
        // the bar surface is 100 pt tall at the bottom of a 1000 × 700 output
        let screen = Rect::new(0.0, -600.0, 1000.0, 700.0);
        let tile = Rect::new(400.0, 30.0, 64.0, 64.0);
        let l = layout(&items, &tile, &screen, Orientation::Bottom, &measure);
        assert_eq!(l.rect.w, (16.0 * 7.0 + 2.0 * TEXT_INSET).ceil(), "as wide as 'Remove from Dock' plus the insets");
        assert_eq!(l.rect.bottom(), tile.y - GAP);
        assert!((l.rect.cx() - tile.cx()).abs() <= 0.5);
        assert_eq!(l.rows.len(), items.len());
        assert_eq!(l.rows[0].y, l.rect.y + PADDING_Y);
        assert_eq!(l.rows[1].h, SEPARATOR_HEIGHT);
        assert_eq!(l.rect.h, 4.0 * ROW_HEIGHT + SEPARATOR_HEIGHT + 2.0 * PADDING_Y);
        // a tile at the edge: the popup is pushed inside
        let edge = layout(&items, &Rect::new(980.0, 30.0, 64.0, 64.0), &screen, Orientation::Bottom, &measure);
        assert_eq!(edge.rect.right(), screen.right() - SCREEN_MARGIN);
        // a side dock puts it beside the tile
        let side = layout(&items, &Rect::new(10.0, 300.0, 64.0, 64.0), &Rect::new(0.0, 0.0, 1000.0, 700.0), Orientation::Left, &measure);
        assert_eq!(side.rect.x, 74.0 + GAP);
    }

    #[test]
    fn hits_land_on_selectable_rows_only() {
        let items = tile_menu(&facts(TileKind::App, false, None, None));
        let l = layout(&items, &Rect::new(400.0, 30.0, 64.0, 64.0), &Rect::new(0.0, -600.0, 1000.0, 700.0), Orientation::Bottom, &|s| s.len() as f64 * 7.0);
        let centre = |r: &Rect| Point::new(r.cx(), r.cy());
        assert_eq!(l.hit(&items, centre(&l.rows[0])), Some(0));
        assert_eq!(l.hit(&items, centre(&l.rows[1])), None, "the separator");
        assert_eq!(l.hit(&items, centre(&l.rows[2])), None, "the disabled item");
        assert_eq!(l.hit(&items, centre(&l.rows[3])), Some(3));
        assert_eq!(l.hit(&items, Point::new(l.rect.x - 1.0, l.rect.cy())), None);
    }
}
