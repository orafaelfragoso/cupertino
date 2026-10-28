//! What the bars show: the state fed by the server's menu-bar groups and by the session bus, and
//! the [`BarContent`] built from it.
//!
//! [`Shared`] holds what every output's bar shows alike (status items, clock, the announced menus
//! and the menu trees loaded from exporters); a [`BarFeed`] holds what one output's
//! menu-bar object reports (the active application and its window).

use crate::dbus::sni::{self, StatusItem};
use crate::desktop::AppName;
use crate::extras::{ExtraKind, Extras};
use crate::fallback::{self, Chords, FallbackAction, WindowRow};
use crate::item::{MenuItem, Shortcut, ShortcutModifier};
use crate::layout::{BarContent, StatusEntry};
use crate::raster::{PRIVACY_DOT_PT, STATUS_ICON_PT};
use crate::source::{GtkMenuProps, KdeAppMenu, MenuSource, MenuSourceMap, WindowKey};
use crate::worker::Request;
use cupertino_icons::from_argb;
use cupertino_shell_client::{
    AppWindow, BoundAction, GtkMenu, MenuBarUpdate, PrimaryModifier, Shortcuts, WindowMenu,
    WindowState,
};
use std::collections::HashMap;
use tiny_skia::{IntSize, Pixmap};

/// The status slot of the screen capture indicator.
pub const SCREEN_CAPTURE_SLOT: &str = "indicator:screen-capture";

/// A status item with its icon at the size the bars draw.
#[derive(Clone, Debug)]
pub struct StatusState {
    pub item: StatusItem,
    pub icon: Option<Pixmap>,
    /// The item's DBusMenu, loaded when it was announced.
    pub menu: Option<MenuItem>,
}

/// State shared by the bars of all outputs.
#[derive(Default)]
pub struct Shared {
    /// Each bar's announced menu by its key.
    pub sources: MenuSourceMap,
    /// Status items, nearest to the control panel first.
    pub status: Vec<StatusState>,
    /// The bar's own items; they sit between the control panel and the other status items.
    pub extras: Extras,
    /// Icons of the bar's own items at the size the bars draw; `None` for a name the theme lacks.
    pub extra_icons: HashMap<ExtraKind, Option<Pixmap>>,
    /// The server reports a screen or window capture: the indicator shows next to the control panel.
    pub screen_capture: bool,
    /// The chords of the synthetic menus, from the server's shortcuts.
    pub chords: Chords,
    pub clock: String,
    /// Menu trees of the exporters in use. A source is present from the moment its tree is
    /// requested, with an empty tree until the exporter answers.
    layouts: HashMap<MenuSource, MenuItem>,
}

impl Shared {
    /// Show a status item, or drop it when it turned passive.
    pub fn set_status(&mut self, state: StatusState) {
        let existing = self.status.iter().position(|s| s.item.id == state.item.id);
        match (existing, state.item.is_passive()) {
            (Some(i), true) => {
                self.status.remove(i);
            }
            (Some(i), false) => self.status[i] = state,
            (None, true) => {}
            (None, false) => self.status.push(state),
        }
    }

    pub fn remove_status(&mut self, id: &str) {
        self.status.retain(|s| s.item.id != id);
    }

    /// The icon of the status slot `id`: `None` when no item has the id, `Some(None)` for an item without an icon.
    pub fn status_icon(&self, id: &str) -> Option<Option<&Pixmap>> {
        if let Some(kind) = ExtraKind::from_slot_id(id) {
            return self
                .extras
                .shown()
                .contains(&kind)
                .then(|| self.extra_icons.get(&kind).and_then(Option::as_ref));
        }
        self.status
            .iter()
            .find(|s| s.item.id == id)
            .map(|s| s.icon.as_ref())
    }

    /// A menu tree arrived from an exporter. Trees nobody asked for, or that no bar uses any
    /// more, are dropped.
    pub fn apply_layout(&mut self, source: &MenuSource, root: MenuItem) {
        if let Some(tree) = self.layouts.get_mut(source) {
            *tree = root;
        }
    }

    /// Forget the trees of sources no bar uses: the next visit loads them afresh.
    pub fn prune_layouts(&mut self, in_use: &[MenuSource]) {
        self.layouts.retain(|source, _| in_use.contains(source));
    }
}

/// Icon of a status item at `px` device pixels: its pixmap, else the themed icon named by the
/// item (`theme` loads it).
pub fn status_icon(
    item: &StatusItem,
    px: u32,
    theme: impl FnOnce(&str, u32) -> Option<Pixmap>,
) -> Option<Pixmap> {
    let from_pixmap = sni::best_pixmap(item.current_pixmaps(), px as i32)
        .and_then(|p| from_argb(p.width, p.height, &p.argb))
        .and_then(|icon| {
            let size = IntSize::from_wh(icon.width, icon.height)?;
            Pixmap::from_vec(icon.rgba, size)
        });
    from_pixmap.or_else(|| {
        Some(item.current_icon_name())
            .filter(|name| !name.is_empty())
            .and_then(|name| theme(name, px))
    })
}

/// Device pixels of a status icon at `scale` device pixels per point.
pub fn status_icon_px(scale: f64) -> u32 {
    (STATUS_ICON_PT * scale).ceil() as u32
}

/// The active application of one output's bar and how its window exports menus.
pub struct BarFeed {
    /// Identifies the bar's announced menu in [`Shared::sources`].
    key: WindowKey,
    app: Option<AppName>,
    window: WindowState,
    /// The active application's windows, for the window list.
    windows: Vec<AppWindow>,
}

impl BarFeed {
    pub fn new(key: WindowKey) -> Self {
        Self {
            key,
            app: None,
            window: WindowState {
                can_zoom: true,
                workspace: 0,
                workspaces: 1,
            },
            windows: Vec::new(),
        }
    }

    /// Apply a group of the menu-bar object. `names` resolves an application id.
    pub fn apply(
        &mut self,
        update: &MenuBarUpdate,
        shared: &mut Shared,
        names: impl FnOnce(&str) -> AppName,
    ) {
        if let Some(active) = &update.active_app {
            self.app = (!active.app_id.is_empty()).then(|| names(&active.app_id));
        }
        if let Some(menu) = &update.menu {
            match menu {
                WindowMenu::None => shared.sources.clear_window(self.key),
                WindowMenu::DbusMenu { service, path } => shared.sources.set_window_source(
                    self.key,
                    MenuSource::Kde(KdeAppMenu {
                        service: service.clone(),
                        path: path.clone(),
                    }),
                ),
                WindowMenu::Gtk(gtk) => shared
                    .sources
                    .set_window_source(self.key, MenuSource::Gtk(gtk_props(gtk))),
            }
        }
        if let Some(state) = update.window_state {
            self.window = state;
        }
        if let Some(windows) = &update.windows {
            self.windows.clone_from(windows);
        }
        // every bar reports the same server-wide state
        if let Some(active) = update.screen_capture {
            shared.screen_capture = active;
        }
        if let Some(shortcuts) = &update.shortcuts {
            shared.chords = chords(shortcuts);
        }
    }

    /// The window of row `row` of the window list.
    pub fn listed_window(&self, row: usize) -> Option<&AppWindow> {
        self.windows.get(row)
    }

    /// Where the active window's menus come from.
    pub fn source(&self, shared: &Shared) -> MenuSource {
        shared.sources.resolve(self.key)
    }

    /// The bar content: the application menu and the application's menus (those of its exporter
    /// once loaded, else the synthetic Window menu), the status items and the clock. Without an
    /// active window the desktop's menu stands in for the application menu.
    pub fn content(&self, shared: &Shared) -> BarContent {
        let (app_name, app_menu, menus) = match &self.app {
            Some(app) => {
                let source = self.source(shared);
                let menus = if source.is_exported() {
                    shared
                        .layouts
                        .get(&source)
                        .map(top_level_menus)
                        .unwrap_or_default()
                } else {
                    let current = Some(self.window.workspace).filter(|w| *w > 0);
                    let windows: Vec<WindowRow> = self
                        .windows
                        .iter()
                        .map(|w| WindowRow {
                            title: &w.title,
                            active: w.active,
                            minimized: w.minimized,
                        })
                        .collect();
                    fallback::menus(
                        self.window.workspaces,
                        current,
                        self.window.can_zoom,
                        &windows,
                        &shared.chords,
                    )
                };
                (
                    app.name.clone(),
                    fallback::app_menu(&app.name, &shared.chords),
                    menus,
                )
            }
            None => (
                "Desktop".to_string(),
                fallback::desktop_menu(&shared.chords),
                Vec::new(),
            ),
        };
        let indicator = shared.screen_capture.then(|| StatusEntry {
            id: SCREEN_CAPTURE_SLOT.to_owned(),
            width: PRIVACY_DOT_PT,
            menu: None,
        });
        let extras = shared.extras.shown().into_iter().map(|kind| StatusEntry {
            id: kind.slot_id().to_owned(),
            width: STATUS_ICON_PT,
            menu: Some(shared.extras.status_menu(kind)),
        });
        let items = shared.status.iter().map(|s| StatusEntry {
            id: s.item.id.clone(),
            width: STATUS_ICON_PT,
            menu: s.menu.clone().map(|m| MenuItem {
                label: s.item.title.clone(),
                has_submenu: true,
                ..m
            }),
        });
        let status = indicator.into_iter().chain(extras).chain(items).collect();
        let system_menu = shared.extras.session_menu().rows;
        BarContent {
            app_name,
            system_menu,
            app_menu,
            menus,
            status,
            clock: shared.clock.clone(),
            control_panel: true,
        }
    }

    /// The request that loads the active window's menus when it exports some that are not loaded
    /// yet. Asking marks the tree as requested, so the request is made once per source.
    pub fn request_layout(&self, shared: &mut Shared) -> Option<Request> {
        let source = self.source(shared);
        if self.app.is_none() || shared.layouts.contains_key(&source) {
            return None;
        }
        let request = match &source {
            MenuSource::Kde(menu) => Request::FetchDbusMenu(menu.clone()),
            MenuSource::Gtk(props) => Request::FetchGtk(props.clone()),
            MenuSource::Synthetic => return None,
        };
        shared
            .layouts
            .insert(source, MenuItem::submenu(0, "", Vec::new()));
        Some(request)
    }
}

/// The chords of the synthetic menus: the server's binding of each system item, and its primary modifier.
fn chords(shortcuts: &Shortcuts) -> Chords {
    let bound = shortcuts
        .chords
        .iter()
        .filter_map(|(action, chord)| {
            let action = match action {
                BoundAction::CloseWindow => FallbackAction::CloseWindow,
                BoundAction::Minimize => FallbackAction::Minimize,
                BoundAction::Zoom => FallbackAction::Zoom,
                BoundAction::ToggleFullscreen => FallbackAction::ToggleFullscreen,
                BoundAction::HideApp => FallbackAction::Hide,
                BoundAction::HideOthers => FallbackAction::HideOthers,
                BoundAction::QuitApp => FallbackAction::Quit,
                BoundAction::TileLeft => FallbackAction::TileLeft,
                BoundAction::TileRight => FallbackAction::TileRight,
                BoundAction::TileFill => FallbackAction::TileFill,
                BoundAction::ForceQuit | BoundAction::Lock | BoundAction::LogOut => return None,
            };
            Some((action, Shortcut::from_chord(chord)?))
        })
        .collect();
    Chords {
        bound,
        primary: match shortcuts.primary {
            PrimaryModifier::Super => ShortcutModifier::Primary,
            PrimaryModifier::Ctrl => ShortcutModifier::Ctrl,
        },
    }
}

fn gtk_props(gtk: &GtkMenu) -> GtkMenuProps {
    GtkMenuProps {
        bus_name: gtk.bus_name.clone(),
        application_id: gtk.application_id.clone(),
        application_object_path: gtk.application_object_path.clone(),
        window_object_path: gtk.window_object_path.clone(),
        menubar_object_path: gtk.menubar_object_path.clone(),
        app_menu_object_path: gtk.app_menu_object_path.clone(),
    }
}

/// The menus of an exporter's tree: its top-level submenus with a label.
fn top_level_menus(root: &MenuItem) -> Vec<MenuItem> {
    root.children
        .iter()
        .filter(|c| c.visible && !c.separator && !c.label.is_empty())
        .cloned()
        .map(|mut c| {
            c.has_submenu = true;
            c
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus::sni::{IconPixmap, ItemStatus};
    use crate::extras::{BatteryState, WifiState};
    use cupertino_shell_client::ActiveApp;

    fn editor() -> AppName {
        AppName {
            name: "Editor".into(),
            has_entry: true,
        }
    }

    fn update(app_id: &str, menu: Option<WindowMenu>) -> MenuBarUpdate {
        MenuBarUpdate {
            active_app: Some(ActiveApp {
                app_id: app_id.into(),
                title: String::new(),
            }),
            menu,
            window_state: None,
            engage: false,
            screen_capture: None,
            session_agent: None,
            windows: None,
            shortcuts: None,
        }
    }

    fn dbusmenu() -> WindowMenu {
        WindowMenu::DbusMenu {
            service: ":1.5".into(),
            path: "/MenuBar".into(),
        }
    }

    fn kde_menu() -> KdeAppMenu {
        KdeAppMenu {
            service: ":1.5".into(),
            path: "/MenuBar".into(),
        }
    }

    fn kde() -> MenuSource {
        MenuSource::Kde(kde_menu())
    }

    fn exporter_tree() -> MenuItem {
        MenuItem::submenu(
            0,
            "",
            vec![
                MenuItem::submenu(1, "File", vec![MenuItem::new(2, "Open")]),
                MenuItem::submenu(3, "Edit", vec![]),
                MenuItem::new(4, ""),
            ],
        )
    }

    fn status(id: &str, title: &str, status: ItemStatus) -> StatusState {
        StatusState {
            item: StatusItem {
                id: id.into(),
                title: title.into(),
                status,
                ..StatusItem::default()
            },
            icon: None,
            menu: None,
        }
    }

    fn ready(menu: Option<WindowMenu>) -> (BarFeed, Shared) {
        let mut feed = BarFeed::new(1);
        let mut shared = Shared {
            clock: "Wed Oct 21  10:42".into(),
            ..Shared::default()
        };
        feed.apply(&update("org.example.editor", menu), &mut shared, |_| {
            editor()
        });
        (feed, shared)
    }

    const SYNTHETIC_MENUS: [&str; 5] = ["File", "Edit", "View", "Window", "Help"];

    #[test]
    fn an_app_without_exported_menus_gets_the_synthetic_ones() {
        let (feed, shared) = ready(Some(WindowMenu::None));
        let content = feed.content(&shared);
        assert_eq!(content.app_name, "Editor");
        assert!(content
            .app_menu
            .iter()
            .any(|i| i.label == "Quit Editor" || i.label == "About Editor"));
        assert_eq!(
            content
                .menus
                .iter()
                .map(|m| m.label.as_str())
                .collect::<Vec<_>>(),
            SYNTHETIC_MENUS
        );
        assert_eq!(content.clock, "Wed Oct 21  10:42");
        assert!(content.control_panel);
    }

    #[test]
    fn without_an_active_window_the_desktop_menu_shows() {
        let mut feed = BarFeed::new(1);
        let mut shared = Shared::default();
        feed.apply(&update("", None), &mut shared, |_| {
            unreachable!("an empty id is no application")
        });
        let content = feed.content(&shared);
        assert_eq!(content.app_name, "Desktop");
        assert_eq!(
            content
                .app_menu
                .iter()
                .map(|i| i.label.as_str())
                .collect::<Vec<_>>(),
            ["Settings…", "", "Show All"]
        );
        assert!(content.menus.is_empty());
    }

    #[test]
    fn exported_menus_replace_the_synthetic_ones_once_loaded() {
        let (feed, mut shared) = ready(Some(dbusmenu()));
        assert_eq!(feed.source(&shared), kde());
        assert!(
            feed.content(&shared).menus.is_empty(),
            "nothing until the tree arrived"
        );
        assert_eq!(
            feed.request_layout(&mut shared),
            Some(Request::FetchDbusMenu(kde_menu()))
        );
        assert_eq!(
            feed.request_layout(&mut shared),
            None,
            "one request per source"
        );
        shared.apply_layout(&kde(), exporter_tree());
        let content = feed.content(&shared);
        assert_eq!(
            content
                .menus
                .iter()
                .map(|m| (m.label.as_str(), m.has_submenu))
                .collect::<Vec<_>>(),
            [("File", true), ("Edit", true)]
        );
    }

    #[test]
    fn trees_nobody_asked_for_are_dropped() {
        let (feed, mut shared) = ready(Some(dbusmenu()));
        shared.apply_layout(&kde(), exporter_tree());
        assert!(feed.content(&shared).menus.is_empty());
    }

    #[test]
    fn pruned_sources_are_loaded_again_on_the_next_visit() {
        let (feed, mut shared) = ready(Some(dbusmenu()));
        feed.request_layout(&mut shared);
        shared.prune_layouts(&[]);
        assert_eq!(
            feed.request_layout(&mut shared),
            Some(Request::FetchDbusMenu(kde_menu()))
        );
        shared.prune_layouts(&[kde()]);
        assert_eq!(feed.request_layout(&mut shared), None);
    }

    #[test]
    fn the_next_group_replaces_the_announced_menu() {
        let (mut feed, mut shared) = ready(Some(dbusmenu()));
        feed.apply(
            &update("org.example.editor", Some(WindowMenu::None)),
            &mut shared,
            |_| editor(),
        );
        assert_eq!(feed.source(&shared), MenuSource::Synthetic);
    }

    #[test]
    fn another_window_of_the_app_never_shows_the_first_windows_menus() {
        let (mut feed, mut shared) = ready(Some(dbusmenu()));
        shared.layouts.insert(kde(), exporter_tree());
        let mut focus = |menu: WindowMenu| {
            feed.apply(
                &update("org.example.editor", Some(menu)),
                &mut shared,
                |_| editor(),
            );
            feed.content(&shared)
                .menus
                .into_iter()
                .map(|m| m.label)
                .collect::<Vec<_>>()
        };
        let gtk = GtkMenu {
            bus_name: ":1.5".into(),
            application_id: None,
            application_object_path: None,
            window_object_path: None,
            menubar_object_path: Some("/org/example/editor/menus/menubar".into()),
            app_menu_object_path: None,
        };
        assert_eq!(
            focus(WindowMenu::Gtk(gtk)),
            Vec::<String>::new(),
            "its own tree is not loaded yet"
        );
        assert_eq!(focus(WindowMenu::None), SYNTHETIC_MENUS);
        assert_eq!(focus(dbusmenu()), ["File", "Edit"]);
    }

    #[test]
    fn the_window_menu_follows_the_window_state() {
        let (mut feed, mut shared) = ready(Some(WindowMenu::None));
        let state = MenuBarUpdate {
            window_state: Some(WindowState {
                can_zoom: false,
                workspace: 2,
                workspaces: 3,
            }),
            ..MenuBarUpdate::default()
        };
        feed.apply(&state, &mut shared, |_| {
            unreachable!("the application did not change")
        });
        let content = feed.content(&shared);
        let window = content
            .menus
            .iter()
            .find(|m| m.id == fallback::WINDOW_MENU)
            .unwrap();
        let zoom = window.children.iter().find(|i| i.label == "Zoom").unwrap();
        let workspaces = window
            .children
            .iter()
            .find(|i| i.label == "Move to Workspace")
            .unwrap();
        assert!(!zoom.enabled);
        assert_eq!(
            workspaces
                .children
                .iter()
                .map(|i| (i.label.as_str(), i.enabled))
                .collect::<Vec<_>>(),
            [
                ("Workspace 1", true),
                ("Workspace 2", false),
                ("Workspace 3", true)
            ]
        );
    }

    fn window_menu(feed: &BarFeed, shared: &Shared) -> MenuItem {
        feed.content(shared)
            .menus
            .into_iter()
            .find(|m| m.id == fallback::WINDOW_MENU)
            .unwrap()
    }

    #[test]
    fn the_window_list_follows_the_server() {
        let (mut feed, mut shared) = ready(Some(WindowMenu::None));
        let list = |windows: Vec<AppWindow>| MenuBarUpdate {
            windows: Some(windows),
            ..MenuBarUpdate::default()
        };
        let window = |id, title: &str, active| AppWindow {
            id,
            title: title.into(),
            active,
            minimized: false,
        };
        feed.apply(
            &list(vec![
                window(7, "notes.txt", true),
                window(3, "draft.txt", false),
            ]),
            &mut shared,
            |_| unreachable!("the application did not change"),
        );
        let menu = window_menu(&feed, &shared);
        assert!(menu.children.iter().any(|i| i.label == "draft.txt"));
        assert_eq!(feed.listed_window(1).map(|w| w.id), Some(3));
        feed.apply(
            &list(vec![window(3, "draft.txt", true)]),
            &mut shared,
            |_| unreachable!("the application did not change"),
        );
        let menu = window_menu(&feed, &shared);
        let row = menu
            .children
            .iter()
            .find(|i| i.label == "draft.txt")
            .unwrap();
        assert!(row.toggle.is_marked());
        assert!(menu.children.iter().all(|i| i.label != "notes.txt"));
        assert_eq!(feed.listed_window(1), None);
    }

    #[test]
    fn the_servers_shortcuts_become_the_menus_chords() {
        let (mut feed, mut shared) = ready(Some(WindowMenu::None));
        let shortcuts = |primary, quit: &str| MenuBarUpdate {
            shortcuts: Some(Shortcuts {
                chords: [
                    (BoundAction::QuitApp, quit.to_string()),
                    (BoundAction::Minimize, "Super+m".to_string()),
                    (BoundAction::Lock, "Ctrl+Super+q".to_string()),
                ]
                .into_iter()
                .collect(),
                primary,
            }),
            ..MenuBarUpdate::default()
        };
        let chord = |item: Option<&MenuItem>| item?.shortcut.as_ref().map(Shortcut::display);
        let quit = |content: &BarContent| {
            chord(content.app_menu.iter().find(|i| i.label == "Quit Editor"))
        };
        let settings =
            |content: &BarContent| chord(content.app_menu.iter().find(|i| i.label == "Settings…"));
        assert_eq!(quit(&feed.content(&shared)), None, "nothing sent yet");

        feed.apply(
            &shortcuts(PrimaryModifier::Super, "Super+q"),
            &mut shared,
            |_| unreachable!("the application did not change"),
        );
        let content = feed.content(&shared);
        assert_eq!(quit(&content).as_deref(), Some("Super+q"));
        assert_eq!(settings(&content).as_deref(), Some("Super+,"));
        let minimize = window_menu(&feed, &shared);
        assert_eq!(
            chord(minimize.children.iter().find(|i| i.label == "Minimize")).as_deref(),
            Some("Super+m")
        );

        feed.apply(
            &shortcuts(PrimaryModifier::Ctrl, "Ctrl+q"),
            &mut shared,
            |_| unreachable!("the application did not change"),
        );
        let content = feed.content(&shared);
        assert_eq!(quit(&content).as_deref(), Some("Ctrl+q"));
        assert_eq!(settings(&content).as_deref(), Some("Ctrl+,"));
    }

    #[test]
    fn status_items_follow_their_status() {
        let (feed, mut shared) = ready(Some(WindowMenu::None));
        shared.set_status(status(
            ":1.9/StatusNotifierItem",
            "Volume",
            ItemStatus::Active,
        ));
        shared.set_status(status(
            ":1.9/StatusNotifierItem",
            "Volume 50%",
            ItemStatus::Active,
        ));
        shared.set_status(status(
            ":1.3/StatusNotifierItem",
            "Sync",
            ItemStatus::Active,
        ));
        assert_eq!(
            feed.content(&shared)
                .status
                .iter()
                .map(|s| (s.id.as_str(), s.width))
                .collect::<Vec<_>>(),
            [
                (":1.9/StatusNotifierItem", 16.0),
                (":1.3/StatusNotifierItem", 16.0)
            ]
        );
        shared.set_status(status(
            ":1.9/StatusNotifierItem",
            "Volume",
            ItemStatus::Passive,
        ));
        shared.set_status(status(
            ":1.4/StatusNotifierItem",
            "Idle",
            ItemStatus::Passive,
        ));
        assert_eq!(feed.content(&shared).status.len(), 1);
        shared.remove_status(":1.3/StatusNotifierItem");
        assert!(feed.content(&shared).status.is_empty());
    }

    #[test]
    fn the_bars_own_items_sit_next_to_the_control_panel_with_their_menus() {
        let (feed, mut shared) = ready(Some(WindowMenu::None));
        shared.set_status(status(
            ":1.3/StatusNotifierItem",
            "Sync",
            ItemStatus::Active,
        ));
        shared.extras.battery = Some(BatteryState {
            percent: 90,
            charging: false,
            on_battery: true,
        });
        shared.extras.wifi = Some(WifiState {
            enabled: false,
            networks: Vec::new(),
        });
        let content = feed.content(&shared);
        assert_eq!(
            content
                .status
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["extra:wifi", "extra:battery", ":1.3/StatusNotifierItem"]
        );
        let menu = content.status[1].menu.clone().expect("the battery menu");
        assert_eq!(
            (menu.label.as_str(), menu.children[0].label.as_str()),
            ("Battery", "Battery 90%")
        );
        // an item without a themed icon is drawn as a dot, one that is not shown not at all
        assert_eq!(shared.status_icon("extra:battery"), Some(None));
        assert_eq!(shared.status_icon("extra:bluetooth"), None);
    }

    #[test]
    fn the_screen_capture_indicator_sits_nearest_the_control_panel_while_a_capture_runs() {
        let (mut feed, mut shared) = ready(Some(WindowMenu::None));
        shared.extras.battery = Some(BatteryState {
            percent: 90,
            charging: false,
            on_battery: true,
        });
        let capture = |active| MenuBarUpdate {
            screen_capture: Some(active),
            ..MenuBarUpdate::default()
        };
        feed.apply(&capture(true), &mut shared, |_| editor());
        let ids = |feed: &BarFeed, shared: &Shared| {
            feed.content(shared)
                .status
                .iter()
                .map(|s| s.id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&feed, &shared), [SCREEN_CAPTURE_SLOT, "extra:battery"]);
        assert_eq!(feed.content(&shared).status[0].menu, None);
        feed.apply(&capture(false), &mut shared, |_| editor());
        assert_eq!(ids(&feed, &shared), ["extra:battery"]);
    }

    #[test]
    fn a_status_menu_is_titled_with_the_item() {
        let (feed, mut shared) = ready(Some(WindowMenu::None));
        let mut item = status(":1.9/StatusNotifierItem", "Volume", ItemStatus::Active);
        item.menu = Some(MenuItem::submenu(0, "", vec![MenuItem::new(1, "Mute")]));
        shared.set_status(item);
        let menu = feed.content(&shared).status[0].menu.clone().unwrap();
        assert_eq!(
            (menu.label.as_str(), menu.has_submenu, menu.children.len()),
            ("Volume", true, 1)
        );
    }

    #[test]
    fn a_status_icon_prefers_the_pixmap_over_the_theme() {
        let item = StatusItem {
            icon_name: "audio-volume".into(),
            icon_pixmaps: vec![IconPixmap {
                width: 1,
                height: 1,
                argb: vec![255, 255, 0, 0],
            }],
            ..StatusItem::default()
        };
        let icon = status_icon(&item, 16, |_, _| unreachable!("the pixmap is valid"));
        let icon = icon.unwrap();
        assert_eq!(
            (icon.width(), icon.height(), icon.data()),
            (1, 1, &[255, 0, 0, 255][..])
        );
    }

    #[test]
    fn a_status_icon_falls_back_to_the_named_theme_icon() {
        let item = StatusItem {
            icon_name: "audio-volume".into(),
            ..StatusItem::default()
        };
        let icon = status_icon(&item, 32, |name, px| {
            assert_eq!((name, px), ("audio-volume", 32));
            Pixmap::new(32, 32)
        });
        assert_eq!(icon.map(|i| i.width()), Some(32));
        assert!(status_icon(&StatusItem::default(), 32, |_, _| unreachable!(
            "no name to look up"
        ))
        .is_none());
    }

    #[test]
    fn status_icons_are_loaded_at_the_device_size() {
        assert_eq!(
            (
                status_icon_px(1.0),
                status_icon_px(1.5),
                status_icon_px(2.0)
            ),
            (16, 24, 32)
        );
    }
}
