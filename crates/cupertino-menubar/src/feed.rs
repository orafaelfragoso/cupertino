//! What the bars show: the state fed by the server's menu-bar groups and by the session bus, and
//! the [`BarContent`] built from it.
//!
//! [`Shared`] holds what every output's bar shows alike (status items, clock, the announced menus
//! and the menu trees loaded from exporters); a [`BarFeed`] holds what one output's
//! menu-bar object reports (the active application and its window).

use crate::dbus::sni::{self, StatusItem};
use crate::desktop::AppName;
use crate::extras::{tray_glyph, tray_radio, ExtraGlyph, ExtraKind, Extras, FrontApp, SystemRow};
use crate::fallback::{self, Chords, FallbackAction, WindowRow};
use crate::icons::Pixels;
use crate::item::{ItemId, MenuItem, Shortcut, ShortcutModifier};
use crate::layout::{BarContent, StatusEntry, PRIVACY_DOT_PT, STATUS_ICON_PT};
use crate::merge::{Export, Exporter};
use crate::recent::RecentItems;
use crate::registered::{
    self, ActiveWindow, Client, Matched, NoX11, Registration, SeenX11, X11Windows,
};
use crate::source::{GtkMenuProps, KdeAppMenu, MenuSource, MenuSourceMap, WindowKey};
use crate::worker::Request;
use cupertino_defaults::menubar::MenuBarSettings;
use cupertino_icons::from_argb;
use cupertino_keyboard::rewrite::ClientClass;
use cupertino_kit::core::IconImage;
use cupertino_kit_shell::menu_bar::{
    ActiveClient, AppWindow, BoundAction, GtkMenu, KeyClass, PrimaryModifier, Shortcuts,
    Update as MenuBarUpdate, WindowMenu, WindowState,
};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

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
    /// The item's DBusMenu as its exporter has it now; `None` without one, and until its tree arrives.
    pub menu: Option<MenuItem>,
}

impl StatusState {
    /// Where the item's menu is exported; `None` for an item without a menu.
    pub fn menu_source(&self) -> Option<KdeAppMenu> {
        Some(KdeAppMenu {
            service: self.item.service.clone(),
            path: self.item.menu_path.clone()?,
        })
    }
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
    /// The menus registered on the session bus, for windows the server announces none for.
    pub registrations: Vec<Registration>,
    /// Registrations whose X11 window is gone, for the registrar to forget ([`Shared::take_gone`]).
    gone: Vec<(String, u32)>,
}

impl Shared {
    /// Show a status item, or drop it when it turned passive. An item read again without its menu keeps the menu it
    /// had while its menu is at the same place: the menu's tree arrives apart from the item ([`Shared::apply_layout`]).
    pub fn set_status(&mut self, mut state: StatusState) {
        let existing = self.status.iter().position(|s| s.item.id == state.item.id);
        match (existing, state.item.is_passive()) {
            (Some(i), true) => {
                self.status.remove(i);
            }
            (Some(i), false) => {
                let old = &mut self.status[i];
                if state.menu.is_none() && old.menu_source() == state.menu_source() {
                    state.menu = old.menu.take();
                }
                *old = state;
            }
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

    /// A menu tree arrived from an exporter: the first, or the tree after the exporter changed it. It replaces the
    /// menu of the status items whose menu the exporter is, and the tree of the application's menus. Trees nobody
    /// asked for, or that no bar uses any more, are dropped.
    pub fn apply_layout(&mut self, source: &MenuSource, root: MenuItem) {
        if let MenuSource::Kde(menu) = source {
            for status in &mut self.status {
                if status.menu_source().as_ref() == Some(menu) {
                    status.menu = Some(root.clone());
                }
            }
        }
        if let Some(tree) = self.layouts.get_mut(source) {
            *tree = root;
        }
    }

    /// Forget the trees of sources no bar uses: the next visit loads them afresh. Returns the sources forgotten, for
    /// the worker to stop following them.
    pub fn prune_layouts(&mut self, in_use: &[MenuSource]) -> Vec<MenuSource> {
        let mut forgotten = Vec::new();
        self.layouts.retain(|source, _| {
            let kept = in_use.contains(source);
            if !kept {
                forgotten.push(source.clone());
            }
            kept
        });
        self.failed.retain(|source| in_use.contains(source));
        forgotten
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

    /// The registrations found gone since the last call.
    pub fn take_gone(&mut self) -> Vec<(String, u32)> {
        std::mem::take(&mut self.gone)
    }

    /// A group announced `source`: a load of it that failed is made again.
    fn announced(&mut self, source: &MenuSource) {
        if self.failed.remove(source) {
            self.layouts.remove(source);
        }
    }
}

/// What the X server of the X11 bridge is asked about the active window of one bar: which of the registered X11
/// windows it is. Answered on the I/O pool, where the connection to the X server lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct X11Query {
    /// The bar whose active window this is.
    pub key: WindowKey,
    registrations: Vec<Registration>,
    bridge: u32,
    title: String,
    windows: Vec<(u32, bool)>,
}

impl X11Query {
    /// The registered menu of the active window, asking `x11`; `seen` keeps the registrations whose window the X
    /// server has had from one answer to the next.
    pub fn resolve(&self, x11: &mut dyn X11Windows, seen: &mut SeenX11) -> Matched {
        let active = ActiveWindow {
            client: Client::X11(self.bridge),
            title: &self.title,
            windows: &self.windows,
        };
        registered::resolve(&self.registrations, &active, x11, seen)
    }
}

/// Icon of a status item at `px` device pixels: its pixmap, else the themed icon named by the
/// item (`theme` loads it).
pub fn status_icon(
    item: &StatusItem,
    px: u32,
    theme: impl FnOnce(&str, u32) -> Option<Pixels>,
) -> Option<Pixels> {
    let from_pixmap = sni::best_pixmap(item.current_pixmaps(), px as i32)
        .and_then(|p| from_argb(p.width, p.height, &p.argb))
        .filter(|icon| icon.width > 0 && icon.height > 0);
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
    /// The active window's title.
    title: String,
    /// The process that shows the active window, when the server says.
    client: Option<ActiveClient>,
    /// How the server passes chords to the active window; the default until the server says.
    keys: KeyClass,
    /// The server announced the active window's menus: no registered menu is looked for.
    announced: bool,
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
            title: String::new(),
            client: None,
            keys: KeyClass::Other,
            announced: false,
        }
    }

    /// Apply a group of the menu-bar object. `names` resolves an application id. The question for the X server, when
    /// the active window is an X11 program's that announced no menu ([`BarFeed::match_registered`]).
    pub fn apply(
        &mut self,
        update: &MenuBarUpdate,
        shared: &mut Shared,
        names: impl FnOnce(&str) -> AppName,
    ) -> Option<X11Query> {
        if let Some(active) = &update.active_app {
            self.app = (!active.app_id.is_empty()).then(|| names(&active.app_id));
            self.app_id.clone_from(&active.app_id);
            self.title.clone_from(&active.title);
        }
        if let Some(client) = update.active_client {
            self.client = Some(client);
        }
        if let Some(keys) = update.key_class {
            self.keys = keys;
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
            self.announced = source.is_some();
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
        self.match_registered(shared)
    }

    /// The active application's name, read afresh for `app_id`: kept while the application is still the active one.
    pub fn set_app_name(&mut self, app_id: &str, name: AppName) {
        if self.app.is_some() && self.app_id == app_id {
            self.app = Some(name);
        }
    }

    /// The active application's id; empty on the desktop.
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// Give the active window the menu it registered on the session bus, when the server announced none for it. Call
    /// after the registrations changed. A window on Wayland is matched at once; an X11 program's needs the X server of
    /// the bridge, asked on the I/O pool: the question is returned, and its answer goes to
    /// [`BarFeed::registered_menu`].
    pub fn match_registered(&self, shared: &mut Shared) -> Option<X11Query> {
        if let Some(query) = self.x11_query(shared) {
            return Some(query);
        }
        if self.announced {
            return None;
        }
        let client = match self.client {
            Some(_) if self.app.is_none() => None,
            Some(ActiveClient::Wayland(pid)) => Some(Client::Wayland(pid)),
            Some(ActiveClient::X11(pid)) => Some(Client::X11(pid)),
            None => None,
        };
        let client = client?;
        let windows: Vec<(u32, bool)> = self.windows.iter().map(|w| (w.id, w.active)).collect();
        let active = ActiveWindow {
            client,
            title: &self.title,
            windows: &windows,
        };
        // a window on Wayland is matched by its process alone: nothing is asked of an X server
        let matched = registered::resolve(
            &shared.registrations,
            &active,
            &mut NoX11,
            &mut SeenX11::new(),
        );
        self.registered_menu(shared, matched);
        None
    }

    /// The question for the X server of the bridge that shows the active window, when it is an X11 program's that
    /// announced no menu; `None` otherwise. An answer to a question that is no longer the current one is not applied.
    pub fn x11_query(&self, shared: &Shared) -> Option<X11Query> {
        if self.announced || self.app.is_none() {
            return None;
        }
        let Some(ActiveClient::X11(bridge)) = self.client else {
            return None;
        };
        Some(X11Query {
            key: self.key,
            registrations: shared.registrations.clone(),
            bridge,
            title: self.title.clone(),
            windows: self.windows.iter().map(|w| (w.id, w.active)).collect(),
        })
    }

    /// Give the active window the registered menu `matched` found, and note the registrations found gone.
    pub fn registered_menu(&self, shared: &mut Shared, matched: Matched) {
        shared.gone.extend(matched.gone);
        match matched.menu {
            Some(menu) => {
                let source = MenuSource::Kde(menu);
                if shared.sources.resolve(self.key) != source {
                    shared.announced(&source);
                    shared.sources.set_window_source(self.key, source);
                }
            }
            None => shared.sources.clear_window(self.key),
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
        let own: Vec<ExtraKind> = shared
            .extras
            .shown()
            .into_iter()
            .filter(|kind| kind_shown(&shared.conf, *kind))
            .collect();
        let extras = own.iter().copied().map(|kind| StatusEntry {
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
            // an applet of a radio the bar shows its own item of would show that radio twice
            .filter(|s| {
                tray_radio(&s.item.app_id, s.item.current_icon_name())
                    .is_none_or(|radio| !own.contains(&radio))
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
        let status = extras.chain(items).collect();
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
            indicator,
            clock: if shared.conf.shown.clock {
                shared.clock.clone()
            } else {
                String::new()
            },
            control_panel: true,
            notice: self.app.is_some()
                && shared
                    .layouts
                    .get(&self.source(shared))
                    .is_some_and(|tree| tree.notice),
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
            class: client_class(self.keys),
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
/// How the server treats an exporter's keys, as the server names it for the active window: first-party for a window of
/// the desktop's toolkit, the terminals' for a terminal's, else the class of every other application.
fn client_class(keys: KeyClass) -> ClientClass {
    match keys {
        KeyClass::Toolkit => ClientClass::FirstParty,
        KeyClass::Terminal => ClientClass::Terminal,
        KeyClass::Other => ClientClass::Other,
    }
}

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
