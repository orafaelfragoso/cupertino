//! What the bars show: the state fed by the server's menu-bar groups and by the session bus, and
//! the [`BarContent`] built from it.
//!
//! [`Shared`] holds what every output's bar shows alike (status items, clock, the announced menus
//! and the menu trees loaded from exporters); a [`BarFeed`] holds what one output's
//! menu-bar object reports (the active application and its window).

use crate::dbus::sni::{self, StatusItem};
use crate::desktop::AppName;
use crate::extras::{tray_glyph, ExtraGlyph, ExtraKind, Extras, FrontApp, SystemRow};
use crate::fallback::{self, Chords, FallbackAction, WindowRow};
use crate::item::{ItemId, MenuItem, Shortcut, ShortcutModifier};
use crate::layout::{BarContent, StatusEntry, PRIVACY_DOT_PT, STATUS_ICON_PT};
use crate::merge::{self, Export, Exporter};
use crate::recent::RecentItems;
use crate::source::{GtkMenuProps, KdeAppMenu, MenuSource, MenuSourceMap, WindowKey};
use crate::worker::Request;
use cupertino_defaults::menubar::MenuBarSettings;
use cupertino_icons::from_argb;
use cupertino_kit::core::IconImage;
use cupertino_shell_client::{
    AppWindow, BoundAction, GtkMenu, MenuBarUpdate, PrimaryModifier, Shortcuts, WindowMenu,
    WindowState,
};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use tiny_skia::{IntSize, Pixmap};

/// The status slot of the screen capture indicator.
pub const SCREEN_CAPTURE_SLOT: &str = "indicator:screen-capture";

/// Whether `menubar.conf` lets the bar's own item `kind` show.
/// The id `menubar.conf` lists a status icon by: the application's own id for it, which stays the same from one run
/// to the next, else the name of its bus service.
fn tray_key(item: &crate::dbus::sni::StatusItem) -> &str {
    if item.app_id.is_empty() {
        &item.service
    } else {
        &item.app_id
    }
}

fn kind_shown(conf: &MenuBarSettings, kind: ExtraKind) -> bool {
    match kind {
        ExtraKind::Wifi => conf.shown.wifi,
        ExtraKind::Battery => conf.shown.battery,
        ExtraKind::Bluetooth => conf.shown.bluetooth,
        ExtraKind::Focus => conf.shown.focus,
    }
}

/// A status item with its icon at the size the bars draw.
#[derive(Clone, Debug)]
pub struct StatusState {
    pub item: StatusItem,
    pub icon: Option<IconImage>,
    /// The item's DBusMenu, loaded when it was announced.
    pub menu: Option<MenuItem>,
}

/// What a status slot draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StatusIcon<'a> {
    Image(&'a IconImage),
    /// A symbol of the bar's own items, in the text colour of the bar.
    Symbol(ExtraGlyph),
    /// An item without an icon.
    Dot,
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
    /// The server reports a screen or window capture: the indicator shows next to the control panel.
    pub screen_capture: bool,
    /// The chords of the synthetic menus, from the server's shortcuts.
    pub chords: Chords,
    /// The rows of Recent Items in the system menu, read when it last opened.
    pub recent: RecentItems,
    /// `menubar.conf`: which items show, and how many recent items the system menu lists.
    pub conf: MenuBarSettings,
    pub clock: String,
    /// Menu trees of the exporters in use. A source is present from the moment its tree is
    /// requested, with an empty tree until the exporter answers.
    layouts: HashMap<MenuSource, MenuItem>,
    /// Sources whose load failed: their empty tree stays, so they are not asked for again until the server announces
    /// them again ([`Shared::layout_failed`]).
    failed: HashSet<MenuSource>,
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

    /// What the status slot `id` draws; `None` when no item has the id.
    pub fn status_icon(&self, id: &str) -> Option<StatusIcon<'_>> {
        if let Some(kind) = ExtraKind::from_slot_id(id) {
            return self.extras.shown().contains(&kind).then(|| {
                self.extras
                    .glyph(kind)
                    .map_or(StatusIcon::Dot, StatusIcon::Symbol)
            });
        }
        self.status.iter().find(|s| s.item.id == id).map(|s| {
            // an item standing for one of the bar's radios draws the bar's own symbol
            match tray_glyph(&s.item.app_id, s.item.current_icon_name()) {
                Some(glyph) => StatusIcon::Symbol(glyph),
                None => s.icon.as_ref().map_or(StatusIcon::Dot, StatusIcon::Image),
            }
        })
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
        self.failed.retain(|source| in_use.contains(source));
    }

    /// The load of `source` failed. It is made again when a group announces the source again, not at once, so an
    /// exporter that keeps failing is asked once per group: a CupertinoKit application refuses the bar until it is told
    /// the bar reads its menus, which the server follows with a group.
    pub fn layout_failed(&mut self, source: &MenuSource) {
        if self
            .layouts
            .get(source)
            .is_some_and(|tree| tree.children.is_empty())
        {
            self.failed.insert(source.clone());
        }
    }

    /// A group announced `source`: a load of it that failed is made again.
    fn announced(&mut self, source: &MenuSource) {
        if self.failed.remove(source) {
            self.layouts.remove(source);
        }
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
    /// The active application's id; its last component names the application's own exported menu.
    app_id: String,
    window: WindowState,
    /// The active application's windows, for the window list.
    windows: Vec<AppWindow>,
    /// The bar's server sends the chords of the Edit items (`MenuBar::can_edit`).
    edits: bool,
    /// The bar's server force-quits the active application (`MenuBar::can_force_quit_app`).
    force_quits: bool,
}

impl BarFeed {
    pub fn new(key: WindowKey, edits: bool, force_quits: bool) -> Self {
        Self {
            key,
            edits,
            force_quits,
            app: None,
            app_id: String::new(),
            window: WindowState {
                can_minimize: true,
                can_zoom: true,
                full_screen: false,
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
            self.app_id.clone_from(&active.app_id);
        }
        if let Some(menu) = &update.menu {
            let source = match menu {
                WindowMenu::None => None,
                WindowMenu::DbusMenu { service, path } => Some(MenuSource::Kde(KdeAppMenu {
                    service: service.clone(),
                    path: path.clone(),
                })),
                WindowMenu::Gtk(gtk) => Some(MenuSource::Gtk(gtk_props(gtk))),
            };
            match source {
                None => shared.sources.clear_window(self.key),
                Some(source) => {
                    shared.announced(&source);
                    shared.sources.set_window_source(self.key, source);
                }
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

    /// The active application as the system menu's Force Quit alternate names it; `None` on the desktop.
    pub fn front_app(&self) -> Option<FrontApp<'_>> {
        self.app.as_ref().map(|app| FrontApp {
            app_id: &self.app_id,
            name: &app.name,
            can_force_quit: self.force_quits,
        })
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
    /// once loaded, merged with the bar's own, else the synthetic ones), the status items and the clock. Without an
    /// active window the desktop's menu stands in for the application menu.
    pub fn content(&self, shared: &Shared) -> BarContent {
        let (app_name, app_menu, menus) = match &self.app {
            Some(app) => {
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
                let system = fallback::menus(
                    self.window.workspaces,
                    current,
                    self.window.can_minimize,
                    self.window.can_zoom,
                    self.window.full_screen,
                    &windows,
                    &shared.chords,
                    self.edits,
                );
                let app_menu = fallback::app_menu(&app.name, &shared.chords);
                let (app_menu, menus) = match self.export(shared) {
                    Some(export) => export.merge(app_menu, system, shared.chords.primary),
                    // nothing until the exporter's tree arrived
                    None if self.source(shared).is_exported() => (app_menu, Vec::new()),
                    None => (app_menu, system),
                };
                (app.name.clone(), app_menu, menus)
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
        let extras = shared
            .extras
            .shown()
            .into_iter()
            .filter(|kind| kind_shown(&shared.conf, *kind))
            .map(|kind| StatusEntry {
                id: kind.slot_id().to_owned(),
                width: STATUS_ICON_PT,
                menu: Some(shared.extras.status_menu(kind)),
            });
        let items = shared
            .status
            .iter()
            .filter(|s| {
                let key = tray_key(&s.item);
                !shared.conf.hidden_tray.iter().any(|h| h == key)
            })
            .map(|s| StatusEntry {
                id: s.item.id.clone(),
                width: STATUS_ICON_PT,
                menu: s.menu.clone().map(|m| MenuItem {
                    label: s.item.title.clone(),
                    has_submenu: true,
                    ..m
                }),
            });
        let status = indicator.into_iter().chain(extras).chain(items).collect();
        let system_menu = shared
            .extras
            .system_menu(&shared.chords, &shared.recent, self.front_app())
            .rows;
        BarContent {
            app_name,
            system_menu,
            app_menu,
            menus,
            status,
            clock: if shared.conf.shown.clock {
                shared.clock.clone()
            } else {
                String::new()
            },
            control_panel: true,
        }
    }

    /// The active window's exported tree once it arrived, with the application's names.
    fn export<'a>(&'a self, shared: &'a Shared) -> Option<Export<'a>> {
        let app = self.app.as_ref()?;
        let source = self.source(shared);
        let exporter = match source {
            MenuSource::Kde(_) => Exporter::DbusMenu,
            MenuSource::Gtk(_) => Exporter::Gtk,
            MenuSource::Synthetic => return None,
        };
        let tree = shared
            .layouts
            .get(&source)
            .filter(|t| !t.children.is_empty())?;
        let id_name = self.app_id.rsplit(['.', '/']).next().unwrap_or_default();
        Some(Export {
            tree,
            exporter,
            names: [&app.name, id_name],
            class: merge::client_class(&self.app_id),
        })
    }

    /// The exporter's item the App menu's item of `action` invokes instead of the bar's own action: About, Settings…
    /// and Quit when the exporter has an item of their role.
    pub fn rebound(&self, shared: &Shared, action: FallbackAction) -> Option<ItemId> {
        self.export(shared)?.roles().target(action)
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

/// The chords of the synthetic menus and the system menu: the server's binding of each system item, and its primary
/// modifier.
fn chords(shortcuts: &Shortcuts) -> Chords {
    Chords {
        bound: parsed(shortcuts, fallback_action),
        system: parsed(shortcuts, system_row),
        help_search: shortcuts
            .chord(BoundAction::HelpSearch)
            .and_then(Shortcut::from_chord),
        primary: match shortcuts.primary {
            PrimaryModifier::Super => ShortcutModifier::Primary,
            PrimaryModifier::Ctrl => ShortcutModifier::Ctrl,
        },
    }
}

/// The chord of each bound action `item` maps to an item of a menu.
fn parsed<K: Eq + Hash>(
    shortcuts: &Shortcuts,
    item: fn(BoundAction) -> Option<K>,
) -> HashMap<K, Shortcut> {
    shortcuts
        .chords
        .iter()
        .filter_map(|(action, chord)| Some((item(*action)?, Shortcut::from_chord(chord)?)))
        .collect()
}

/// The item of the synthetic menus that stands for a server action.
fn fallback_action(action: BoundAction) -> Option<FallbackAction> {
    Some(match action {
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
        BoundAction::ForceQuit
        | BoundAction::Lock
        | BoundAction::LogOut
        | BoundAction::HelpSearch => return None,
    })
}

/// The row of the system menu that stands for a server action.
fn system_row(action: BoundAction) -> Option<SystemRow> {
    Some(match action {
        BoundAction::ForceQuit => SystemRow::ForceQuit,
        BoundAction::Lock => SystemRow::Lock,
        BoundAction::LogOut => SystemRow::LogOut,
        _ => return None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbus::sni::{IconPixmap, ItemStatus};
    use crate::extras::{BatteryState, WifiState};
    use crate::fallback::{HELP_MENU, HELP_SEARCH, SEARCH_PLACEHOLDER};
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
            engage_help: false,
            screen_capture: None,
            session_agent: None,
            backdrop_lightness: None,
            backdrop_bands: None,
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
        let mut feed = BarFeed::new(1, true, true);
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
        let mut feed = BarFeed::new(1, true, true);
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
            [
                ("File", true),
                ("Edit", true),
                ("Window", true),
                ("Help", true)
            ],
            "merged with the bar's Window and Help"
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
    fn a_failed_load_is_made_again_only_when_a_group_announces_the_menu_again() {
        let (mut feed, mut shared) = ready(Some(dbusmenu()));
        assert_eq!(
            feed.request_layout(&mut shared),
            Some(Request::FetchDbusMenu(kde_menu()))
        );
        shared.layout_failed(&kde());
        assert_eq!(
            feed.request_layout(&mut shared),
            None,
            "not at once: an exporter that keeps failing is not asked in a loop"
        );
        feed.apply(
            &update("org.example.editor", Some(dbusmenu())),
            &mut shared,
            |_| editor(),
        );
        assert_eq!(
            feed.request_layout(&mut shared),
            Some(Request::FetchDbusMenu(kde_menu()))
        );

        // a tree that arrived is kept when the group announces it again
        shared.apply_layout(&kde(), exporter_tree());
        shared.layout_failed(&kde());
        feed.apply(
            &update("org.example.editor", Some(dbusmenu())),
            &mut shared,
            |_| editor(),
        );
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
        assert_eq!(focus(dbusmenu()), ["File", "Edit", "Window", "Help"]);
    }

    #[test]
    fn the_window_menu_follows_the_window_state() {
        let (mut feed, mut shared) = ready(Some(WindowMenu::None));
        let state = MenuBarUpdate {
            window_state: Some(WindowState {
                can_minimize: false,
                can_zoom: false,
                full_screen: false,
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
        let minimize = window
            .children
            .iter()
            .find(|i| i.label == "Minimize")
            .unwrap();
        let zoom = window.children.iter().find(|i| i.label == "Zoom").unwrap();
        let workspaces = window
            .children
            .iter()
            .find(|i| i.label == "Move to Workspace")
            .unwrap();
        assert!(!minimize.enabled);
        assert!(minimize.alternate.as_ref().is_some_and(|all| all.enabled));
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
        let system = |label: &str| chord(content.system_menu.iter().find(|i| i.label == label));
        assert_eq!(system("Lock Screen").as_deref(), Some("Ctrl+Super+q"));
        assert_eq!(system("Force Quit\u{2026}"), None, "unbound");
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
    fn the_search_row_shows_the_help_search_chord_and_its_placeholder() {
        let (mut feed, mut shared) = ready(Some(WindowMenu::None));
        let search_row = |feed: &BarFeed, shared: &Shared| {
            let content = feed.content(shared);
            let help = content.menus.iter().find(|m| m.id == HELP_MENU).cloned();
            help.and_then(|h| h.children.into_iter().find(|i| i.id == HELP_SEARCH))
                .unwrap()
        };
        let row = search_row(&feed, &shared);
        assert_eq!(row.label, SEARCH_PLACEHOLDER);
        assert!(!row.enabled, "the placeholder is drawn dimmed");
        assert_eq!(row.shortcut, None, "nothing sent yet");
        feed.apply(
            &MenuBarUpdate {
                shortcuts: Some(Shortcuts {
                    chords: [(BoundAction::HelpSearch, "Shift+Super+slash".to_string())]
                        .into_iter()
                        .collect(),
                    primary: PrimaryModifier::Super,
                }),
                ..MenuBarUpdate::default()
            },
            &mut shared,
            |_| unreachable!("the application did not change"),
        );
        let chord = search_row(&feed, &shared).shortcut.unwrap();
        assert_eq!(chord.display(), "Shift+Super+slash");
        assert_eq!(chord.glyphs().key, "/");
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
        // a shown item draws its symbol, one that is not shown nothing at all
        assert!(matches!(
            shared.status_icon("extra:battery"),
            Some(StatusIcon::Symbol(ExtraGlyph {
                symbol: "symbol.battery",
                ..
            }))
        ));
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

    /// Every radio and Focus item, the bar's own and a third-party applet's alike, draws a template symbol of the asset
    /// directory at the bar's status icon size in the text colour of the bar's tone, dark text over a light wallpaper and
    /// light text over a dark one.
    #[test]
    fn every_extra_draws_a_template_symbol_at_the_bar_size_in_the_bar_tint() {
        use crate::extras::{BluetoothState, FocusState, SYMBOL_NAMES};
        use crate::tone::BarTone;
        let mut shared = Shared::default();
        shared.extras.wifi = Some(WifiState {
            enabled: true,
            networks: Vec::new(),
        });
        shared.extras.bluetooth = Some(BluetoothState {
            powered: true,
            devices: Vec::new(),
        });
        shared.extras.focus = Some(FocusState {
            active: true,
            lifetimes: Vec::new(),
        });
        shared.extras.battery = Some(BatteryState {
            percent: 50,
            charging: false,
            on_battery: true,
        });
        // the network and Bluetooth applets of a session without the bar's own services, as their tray items report
        for (id, app, icon) in [
            (
                ":1.7/org/ayatana/NotificationItem/nm_applet",
                "nm-applet",
                "nm-signal-75",
            ),
            (":1.8/org/blueman/sni", "blueman", "blueman-tray"),
        ] {
            shared.set_status(StatusState {
                item: StatusItem {
                    id: id.into(),
                    app_id: app.into(),
                    icon_name: icon.into(),
                    status: ItemStatus::Active,
                    ..StatusItem::default()
                },
                // the icon theme's own picture, which the bar does not draw for these items
                icon: Some(crate::view::icon_image(&Pixmap::new(4, 4).unwrap())),
                menu: None,
            });
        }
        let pack = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/assets/cupertino");
        let (assets, _) = cupertino_assets::Assets::read_pack(std::path::Path::new(pack)).unwrap();
        let slots = [
            ("extra:wifi", "symbol.wifi"),
            ("extra:bluetooth", "symbol.bluetooth"),
            ("extra:focus", "symbol.moon"),
            ("extra:battery", "symbol.battery"),
            (":1.7/org/ayatana/NotificationItem/nm_applet", "symbol.wifi"),
            (":1.8/org/blueman/sni", "symbol.bluetooth"),
        ];
        for (slot, symbol) in slots {
            let Some(StatusIcon::Symbol(glyph)) = shared.status_icon(slot) else {
                panic!("{slot} draws no symbol");
            };
            assert_eq!(glyph.symbol, symbol, "{slot}");
            assert!(SYMBOL_NAMES.contains(&glyph.symbol), "{slot}");
            for scale in [1.0, 2.0] {
                for tone in [BarTone::DarkText, BarTone::LightText] {
                    let palette = tone.palette(false, false);
                    let (name, px, tint) = crate::view::status_symbol(glyph, &palette, scale);
                    assert_eq!(name, symbol);
                    assert_eq!(px, status_icon_px(scale), "{slot} at {scale}x");
                    // the bar's text colour; a dimmed item (Wi-Fi without a joined network) fades it
                    assert_eq!(tint[..3], palette.text[..3], "{slot} in {tone:?}");
                    if glyph.dimmed {
                        assert!(tint[3] < palette.text[3], "{slot} in {tone:?}");
                    } else {
                        assert_eq!(tint[3], palette.text[3], "{slot} in {tone:?}");
                    }
                }
            }
            // the checkout's pack gives it as a template symbol, not a themed picture or a placeholder
            let name = cupertino_assets::IconName::new(symbol).unwrap();
            assert!(
                assets
                    .icon_sources(&name, status_icon_px(2.0))
                    .iter()
                    .any(|(_, source)| matches!(source, cupertino_assets::IconSource::Symbol(_))),
                "{slot}"
            );
        }
    }

    #[test]
    fn a_tray_item_standing_for_a_radio_is_dimmed_while_off_or_unlinked() {
        let glyph = |id: &str, icon: &str| tray_glyph(id, icon).map(|g| (g.symbol, g.dimmed));
        assert_eq!(
            glyph("nm-applet", "nm-signal-100-secure"),
            Some(("symbol.wifi", false))
        );
        assert_eq!(
            glyph("nm-applet", "nm-no-connection"),
            Some(("symbol.wifi", true))
        );
        assert_eq!(
            glyph("nm-applet", "nm-device-wired"),
            Some(("symbol.wifi", true))
        );
        assert_eq!(
            glyph("nm-applet", "nm-stage01-connecting"),
            Some(("symbol.wifi", true))
        );
        assert_eq!(
            glyph("", "network-wireless-signal-good-symbolic"),
            Some(("symbol.wifi", false))
        );
        assert_eq!(
            glyph("", "network-wireless-offline-symbolic"),
            Some(("symbol.wifi", true))
        );
        assert_eq!(
            glyph("blueman", "blueman-tray"),
            Some(("symbol.bluetooth", false))
        );
        assert_eq!(
            glyph("blueman", "blueman-tray-active"),
            Some(("symbol.bluetooth", false))
        );
        assert_eq!(
            glyph("blueman", "blueman-tray-disabled"),
            Some(("symbol.bluetooth", true))
        );
        assert_eq!(
            glyph("", "bluetooth-disabled-symbolic"),
            Some(("symbol.bluetooth", true))
        );
        assert_eq!(glyph("printer-applet", "printer"), None);
        assert_eq!(glyph("steam", "steam_tray_mono"), None);
    }

    /// A tray item's own icon in grey strokes takes the bar's text colour; a coloured one keeps its colours.
    #[test]
    fn a_template_tray_icon_takes_the_bar_tint_and_a_coloured_one_keeps_its_own() {
        use crate::tone::BarTone;
        let solid = |rgba: [u8; 4]| {
            let mut p = Pixmap::new(4, 4).unwrap();
            for px in p.data_mut().as_chunks_mut::<4>().0 {
                *px = rgba;
            }
            crate::view::icon_image(&p)
        };
        let palette = BarTone::DarkText.palette(true, false);
        let text = palette.text.map(|v| (v * 255.0).round() as u8);
        let white = crate::view::status_image(&solid([255, 255, 255, 255]), &palette);
        let first = &white.image.pixels[..4];
        let a = f32::from(first[3]) / 255.0;
        for c in 0..3 {
            let expected = (f32::from(text[c]) * a).round() as i32;
            assert!(
                (i32::from(first[c]) - expected).abs() <= 1,
                "{first:?} {text:?}"
            );
        }
        let red = solid([200, 0, 0, 255]);
        assert_eq!(
            crate::view::status_image(&red, &palette).image.pixels,
            red.image.pixels
        );
    }
}
