//! The bar's menus for an application that exports its own: the exporter's tree merged with the bar's App, Window
//! and Help menus.
//!
//! * Roles: the exporter's About, Preferences, Quit, Close and Full Screen items. A GMenuModel item has the role its
//!   action names (`app.about`; `app.preferences` or `app.settings`; `app.quit`; `win.close`; `win.fullscreen`), a
//!   DBusMenu item the role its English label names ([`label_role`]).
//! * The App menu is the bar's. About, Settings… and Quit invoke the exporter's item of their role when it has one
//!   ([`Roles`]); those items leave the menu they were in when it is the application's own menu, File or Edit. The
//!   rest of the application's own menu (the GMenuModel application menu, or the DBusMenu menu titled with the
//!   application's name) goes between About and Settings….
//! * File, Edit, View and any other menu show as exported, View with Enter Full Screen appended when it has no full
//!   screen item. Window is the bar's, with the exporter's items that are no duplicates below; Help is the bar's,
//!   with the exporter's items below the search row. The order is File, Edit, View, the others, Window, Help.
//!
//! The bar's items keep their stable ids and the exporter's keep theirs, so an open menu survives a layout update; an
//! activated item is routed by its [`crate::item::Origin`], which the readers set, never by its id.

use crate::fallback::{self, FallbackAction};
use crate::item::{key_char, split_mnemonic, ItemId, MenuItem, Shortcut, ShortcutModifier};
use cupertino_keyboard::rewrite::{
    is_printable, pressed_for, ClientClass, KeyPress, SYM_KP_ENTER, SYM_RETURN,
};
use cupertino_menu_model::{ActionName, Chord, Role};
use cupertino_modifiers::{ModifierMap, Profile, RawMods};

/// The top-level item of a GMenuModel tree that holds its application menu (`app_menu_object_path`).
pub const GTK_APP_MENU: ItemId = ItemId(-1);

/// Separator between the application's own items and Settings… in the App menu.
const APP_ITEMS_SEPARATOR: i32 = -1107;
/// Separator before the Enter Full Screen item the bar appends to an exported View menu.
const FULL_SCREEN_SEPARATOR: i32 = -1108;
/// Separator before the exporter's items in the Window menu.
const WINDOW_ITEMS_SEPARATOR: i32 = -1109;
/// Separator before the exporter's items in the Help menu.
const HELP_ITEMS_SEPARATOR: i32 = -1110;

/// Labels of a Window menu item the bar's Window menu already has, lower case.
const WINDOW_DUPLICATES: [&str; 8] = [
    "minimize",
    "zoom",
    "maximize",
    "close",
    "close window",
    "bring all to front",
    "move to workspace",
    "move to desktop",
];

/// How an exporter's items name their role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exporter {
    /// GMenuModel: by the item's action.
    Gtk,
    /// DBusMenu: by the item's label.
    DbusMenu,
}

/// An exporter's tree, with what the bar knows of the application.
#[derive(Clone, Copy, Debug)]
pub struct Export<'a> {
    pub tree: &'a MenuItem,
    pub exporter: Exporter,
    /// The application's names: its desktop entry's `Name` and the last component of its id. A DBusMenu menu titled
    /// with either is the application's own menu, and "Configure <name>" its preferences.
    pub names: [&'a str; 2],
    /// How the server treats the application's keys ([`client_class`]); its accelerators show the keys pressed.
    pub class: ClientClass,
}

/// The exporter's items the App menu's About, Settings… and Quit invoke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Roles {
    pub about: Option<ItemId>,
    pub preferences: Option<ItemId>,
    pub quit: Option<ItemId>,
}

impl Roles {
    /// The exporter's item the bar's item of `action` invokes; `None` when the bar's own action runs.
    pub fn target(&self, action: FallbackAction) -> Option<ItemId> {
        match action {
            FallbackAction::About => self.about,
            FallbackAction::Settings => self.preferences,
            FallbackAction::Quit => self.quit,
            _ => None,
        }
    }
}

/// The exporter's menus by their place in the bar.
#[derive(Default)]
struct Parts {
    /// The application's own menu.
    app: Option<MenuItem>,
    file: Option<MenuItem>,
    edit: Option<MenuItem>,
    view: Option<MenuItem>,
    window: Option<MenuItem>,
    help: Option<MenuItem>,
    others: Vec<MenuItem>,
}

impl Parts {
    /// The menus in the order roles are looked for: the application's own menu, File and Edit first.
    fn in_role_order(&self) -> impl Iterator<Item = &MenuItem> {
        [&self.app, &self.file, &self.edit, &self.view]
            .into_iter()
            .flatten()
            .chain(&self.others)
            .chain([&self.window, &self.help].into_iter().flatten())
    }
}

impl Export<'_> {
    /// The role of an item: a visible item that opens no submenu.
    pub fn role(&self, item: &MenuItem) -> Option<Role> {
        if item.separator || item.has_submenu || !item.visible {
            return None;
        }
        match self.exporter {
            Exporter::Gtk => Role::of(&ActionName::parse(item.action.as_deref()?)?),
            Exporter::DbusMenu => label_role(&item.label, &self.names),
        }
    }

    /// The exporter's items the App menu's items invoke: the first item of each role, looked for in the
    /// application's own menu, File, Edit, then the other menus in bar order.
    pub fn roles(&self) -> Roles {
        roles(self, &self.parts())
    }

    /// The App menu and the application's menus: `app_menu` and `system` are the bar's own
    /// ([`fallback::app_menu`], [`fallback::menus`]); `primary` shows accelerators as [`fallback::Chords::primary`].
    pub fn merge(
        &self,
        mut app_menu: Vec<MenuItem>,
        system: Vec<MenuItem>,
        primary: ShortcutModifier,
    ) -> (Vec<MenuItem>, Vec<MenuItem>) {
        let mut parts = self.parts();
        for menu in [
            &mut parts.app,
            &mut parts.file,
            &mut parts.edit,
            &mut parts.view,
            &mut parts.window,
            &mut parts.help,
        ]
        .into_iter()
        .flatten()
        .chain(&mut parts.others)
        {
            show_accels(menu, primary, self.class);
        }
        let roles = roles(self, &parts);
        for slot in [&mut parts.app, &mut parts.file, &mut parts.edit] {
            let Some(menu) = slot.as_mut() else { continue };
            let loaded = !menu.children.is_empty();
            menu.children.retain(|c| {
                !matches!(
                    self.role(c),
                    Some(Role::About | Role::Preferences | Role::Quit)
                )
            });
            collapse_separators(&mut menu.children);
            // a menu still to be loaded stays; one the removal emptied goes
            if loaded && menu.children.is_empty() {
                *slot = None;
            }
        }

        let settings = FallbackAction::Settings.item_id();
        if let Some(item) = app_menu.iter_mut().find(|i| i.id == settings) {
            item.enabled = roles
                .preferences
                .and_then(|id| self.tree.find(id))
                .is_some_and(|i| i.enabled);
        }
        let own = parts.app.map(|m| m.children).unwrap_or_default();
        if let Some(at) = app_menu.iter().position(|i| i.id == settings) {
            if !own.is_empty() {
                let mut tail = app_menu.split_off(at);
                app_menu.extend(own);
                app_menu.push(MenuItem::separator(APP_ITEMS_SEPARATOR));
                app_menu.append(&mut tail);
            }
        }

        let take = |id: ItemId| system.iter().find(|m| m.id == id).cloned();
        let mut menus: Vec<MenuItem> = [parts.file, parts.edit].into_iter().flatten().collect();
        if let Some(mut view) = parts.view {
            // a View still to be loaded is left as it is, so it loads when it opens
            let full_screen = take(fallback::VIEW_MENU)
                .and_then(|v| v.children.into_iter().next())
                .filter(|_| {
                    !view.children.is_empty()
                        && !view
                            .children
                            .iter()
                            .any(|c| self.role(c) == Some(Role::FullScreen))
                });
            if let Some(item) = full_screen {
                view.children
                    .push(MenuItem::separator(FULL_SCREEN_SEPARATOR));
                view.children.push(item);
            }
            menus.push(view);
        }
        menus.extend(parts.others);
        if let Some(mut window) = take(fallback::WINDOW_MENU) {
            let kept = parts.window.map(|w| {
                w.children
                    .into_iter()
                    .filter(|c| !self.duplicates_window_item(c))
                    .collect()
            });
            below(&mut window, WINDOW_ITEMS_SEPARATOR, kept);
            menus.push(window);
        }
        if let Some(mut help) = take(fallback::HELP_MENU) {
            below(
                &mut help,
                HELP_ITEMS_SEPARATOR,
                parts.help.map(|h| h.children),
            );
            menus.push(help);
        }
        (app_menu, menus)
    }

    /// An item of the exporter's Window menu that the bar's Window menu already has, by role or label.
    fn duplicates_window_item(&self, item: &MenuItem) -> bool {
        self.role(item) == Some(Role::Close)
            || WINDOW_DUPLICATES.contains(&plain_label(&item.label).as_str())
    }

    /// The exporter's labelled top-level menus by their place in the bar.
    fn parts(&self) -> Parts {
        let mut parts = Parts::default();
        for top in &self.tree.children {
            if !top.visible || top.separator {
                continue;
            }
            if self.exporter == Exporter::Gtk && top.id == GTK_APP_MENU {
                parts.app = Some(top.clone());
                continue;
            }
            if top.label.is_empty() {
                continue;
            }
            let menu = MenuItem {
                has_submenu: true,
                ..top.clone()
            };
            let title = plain_label(&top.label);
            if self.exporter == Exporter::DbusMenu
                && parts.app.is_none()
                && self
                    .names
                    .iter()
                    .any(|n| !n.is_empty() && n.to_lowercase() == title)
            {
                parts.app = Some(menu);
                continue;
            }
            let slot = match title.as_str() {
                "file" => &mut parts.file,
                "edit" => &mut parts.edit,
                "view" => &mut parts.view,
                "window" => &mut parts.window,
                "help" => &mut parts.help,
                _ => {
                    parts.others.push(menu);
                    continue;
                }
            };
            match slot {
                None => *slot = Some(menu),
                Some(_) => parts.others.push(menu),
            }
        }
        parts
    }
}

fn roles(export: &Export, parts: &Parts) -> Roles {
    let first = |role: Role| {
        parts
            .in_role_order()
            .flat_map(|m| &m.children)
            .find(|c| export.role(c) == Some(role))
            .map(|c| c.id)
    };
    Roles {
        about: first(Role::About),
        preferences: first(Role::Preferences),
        quit: first(Role::Quit),
    }
}

/// `items` below the bar's own items of `menu`, after a separator; nothing when there are none.
fn below(menu: &mut MenuItem, separator: i32, items: Option<Vec<MenuItem>>) {
    let mut items = items.unwrap_or_default();
    collapse_separators(&mut items);
    if !items.is_empty() {
        menu.children.push(MenuItem::separator(separator));
        menu.children.append(&mut items);
    }
}

/// Drop leading, trailing and doubled separators.
fn collapse_separators(items: &mut Vec<MenuItem>) {
    let mut previous_separator = true;
    items.retain(|i| {
        let keep = !(i.separator && previous_separator);
        if keep {
            previous_separator = i.separator;
        }
        keep
    });
    if items.last().is_some_and(|i| i.separator) {
        items.pop();
    }
}

/// A label compared with the role lists: without its mnemonic marker and a trailing ellipsis, in lower case.
fn plain_label(label: &str) -> String {
    let (label, _) = split_mnemonic(label);
    let label = label.trim();
    let label = label
        .strip_suffix('…')
        .or_else(|| label.strip_suffix("..."))
        .unwrap_or(label);
    label.trim().to_lowercase()
}

/// The role an English DBusMenu label names, with the mnemonic marker, a trailing ellipsis and case ignored: about
/// starts with "About"; quit is "Quit", "Quit" and a name, or "Exit"; preferences is "Preferences", "Settings",
/// "Options" or "Configure" and one of `names` (other "Configure" items are not the application's settings); close
/// is "Close" or "Close Window"; full screen is "Full Screen" or "Enter Full Screen". Any other label has none.
pub fn label_role(label: &str, names: &[&str]) -> Option<Role> {
    let label = plain_label(label);
    let words = |prefix: &str| {
        label
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(' '))
    };
    let names_app = |rest: &str| {
        names
            .iter()
            .any(|n| !n.is_empty() && n.to_lowercase() == rest.trim())
    };
    Some(match label.as_str() {
        "about" => Role::About,
        "quit" | "exit" => Role::Quit,
        "preferences" | "settings" | "options" => Role::Preferences,
        "close" | "close window" => Role::Close,
        "full screen" | "enter full screen" | "exit full screen" => Role::FullScreen,
        _ if words("about").is_some() => Role::About,
        _ if words("quit").is_some_and(|rest| !rest.trim().is_empty()) => Role::Quit,
        _ if words("configure").is_some_and(names_app) => Role::Preferences,
        _ => return None,
    })
}

/// The server's modifier profile, named by the key its convention chords show as primary
/// ([`fallback::Chords::primary`]): Super under `cupertino`, Ctrl under `classic-pc`.
fn profile(primary: ShortcutModifier) -> Profile {
    match primary {
        ShortcutModifier::Ctrl => Profile::classic_pc(),
        _ => Profile::cupertino(),
    }
}

/// How the server treats an exporter's keys: [`ClientClass::FirstParty`] for a `cupertino-*` application id, as the
/// server decides, else [`ClientClass::Other`] (the bar knows neither the terminals nor the applications whose keys
/// the server leaves alone).
pub fn client_class(app_id: &str) -> ClientClass {
    if app_id.starts_with("cupertino-") {
        ClientClass::FirstParty
    } else {
        ClientClass::Other
    }
}

/// The shortcut a GMenuModel accelerator of a `class` client shows: the keys the person presses under the profile
/// `primary` names. The accelerator names the keys the application receives; for a third-party application the
/// server makes Ctrl chords of Super chords ([`pressed_for`]), so the bar shows the Super chord. `None` for a
/// malformed accelerator.
pub fn accel_shortcut(
    accel: &str,
    primary: ShortcutModifier,
    class: ClientClass,
) -> Option<Shortcut> {
    let profile = profile(primary);
    let chord = Chord::from_accel(accel, &profile)?;
    let raw = chord
        .modifiers
        .iter()
        .try_fold(RawMods::default(), |raw, m| {
            Some(raw | profile.physical(m)?.into())
        })?;
    let name = chord.key.name();
    let ch = key_char(name);
    let received = KeyPress {
        evdev: 0,
        sym: match name {
            "Return" => SYM_RETURN,
            "KP_Enter" => SYM_KP_ENTER,
            _ => ch.map_or(0, u32::from),
        },
        printable: is_printable(ch),
        is_modifier: false,
        raw,
    };
    let pressed = pressed_for(&profile, class, &received).raw;
    let mut modifiers: Vec<ShortcutModifier> = [
        (RawMods::CTRL, ShortcutModifier::Ctrl),
        (RawMods::ALT, ShortcutModifier::Alt),
        (RawMods::SHIFT, ShortcutModifier::Shift),
        (RawMods::SUPER, ShortcutModifier::Primary),
    ]
    .into_iter()
    .filter(|(bit, _)| pressed.contains(*bit))
    .map(|(_, m)| m)
    .collect();
    modifiers.sort();
    Some(Shortcut {
        modifiers,
        key: chord.key.label(),
    })
}

/// Show the accelerators of `menu`'s items.
fn show_accels(menu: &mut MenuItem, primary: ShortcutModifier, class: ClientClass) {
    if let Some(accel) = &menu.accel {
        menu.shortcut = accel_shortcut(accel, primary, class);
    }
    for child in &mut menu.children {
        show_accels(child, primary, class);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback::Chords;

    fn labels(items: &[MenuItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    fn ids(items: &[MenuItem]) -> Vec<i32> {
        items.iter().map(|i| i.id.0).collect()
    }

    fn gtk(id: i32, label: &str, action: &str) -> MenuItem {
        MenuItem {
            action: Some(action.to_owned()),
            ..MenuItem::new(id, label)
        }
    }

    /// A DBusMenu exporter: its own menu titled "Editor", File and Edit with role items, View, Tools, Window, Help.
    fn dbusmenu_tree() -> MenuItem {
        MenuItem::submenu(
            0,
            "",
            vec![
                MenuItem::submenu(
                    1,
                    "Editor",
                    vec![
                        MenuItem::new(2, "About Editor"),
                        MenuItem::separator(3),
                        MenuItem::new(4, "Check for Updates"),
                        MenuItem::separator(5),
                        MenuItem::new(6, "Preferences…"),
                        MenuItem::separator(7),
                        MenuItem::new(8, "Quit"),
                    ],
                ),
                MenuItem::submenu(
                    10,
                    "File",
                    vec![
                        MenuItem::new(11, "New"),
                        MenuItem::separator(12),
                        MenuItem::new(13, "Close Window"),
                        MenuItem::separator(14),
                        MenuItem::new(15, "Quit Editor"),
                    ],
                ),
                MenuItem::submenu(
                    20,
                    "Edit",
                    vec![
                        MenuItem::new(21, "Undo"),
                        MenuItem::separator(22),
                        MenuItem::new(23, "Copy"),
                        MenuItem::separator(24),
                        MenuItem::new(25, "Settings"),
                    ],
                ),
                MenuItem::submenu(30, "View", vec![MenuItem::new(31, "Zoom In")]),
                MenuItem::submenu(40, "Tools", vec![MenuItem::new(41, "Spelling")]),
                MenuItem::submenu(
                    50,
                    "Window",
                    vec![
                        MenuItem::new(51, "Minimize"),
                        MenuItem::new(52, "Close Window"),
                        MenuItem::separator(53),
                        MenuItem::new(54, "Split View"),
                        MenuItem::new(55, "Move to Desktop"),
                    ],
                ),
                MenuItem::submenu(
                    60,
                    "Help",
                    vec![
                        MenuItem::new(61, "Editor Handbook"),
                        MenuItem::new(62, "About KDE"),
                    ],
                ),
            ],
        )
    }

    /// A GMenuModel exporter: an application menu, File, View with its full screen item and Help.
    fn gtk_tree() -> MenuItem {
        MenuItem::submenu(
            0,
            "",
            vec![
                MenuItem::submenu(
                    GTK_APP_MENU.0,
                    "",
                    vec![
                        gtk(100, "About Editor", "app.about"),
                        MenuItem::separator(101),
                        gtk(102, "Keyboard Shortcuts", "win.show-help-overlay"),
                        MenuItem::separator(103),
                        gtk(104, "Settings", "app.settings"),
                        MenuItem {
                            accel: Some("<Primary>q".into()),
                            ..gtk(105, "Quit", "app.quit")
                        },
                    ],
                ),
                MenuItem::submenu(
                    1,
                    "File",
                    vec![
                        MenuItem {
                            accel: Some("<Primary>n".into()),
                            ..gtk(2, "New", "app.new")
                        },
                        gtk(3, "Close", "win.close"),
                    ],
                ),
                MenuItem::submenu(4, "View", vec![gtk(5, "Fullscreen", "win.fullscreen")]),
                MenuItem::submenu(6, "Help", vec![gtk(7, "Contents", "app.help")]),
            ],
        )
    }

    fn export(tree: &MenuItem, exporter: Exporter) -> Export<'_> {
        Export {
            tree,
            exporter,
            names: ["Editor", "editor"],
            class: ClientClass::Other,
        }
    }

    fn merged(export: &Export) -> (Vec<MenuItem>, Vec<MenuItem>) {
        let chords = Chords::default();
        export.merge(
            fallback::app_menu("Editor", &chords),
            fallback::menus(1, Some(1), true, true, false, &[], &chords, true),
            chords.primary,
        )
    }

    fn menu<'a>(menus: &'a [MenuItem], label: &str) -> &'a MenuItem {
        menus.iter().find(|m| m.label == label).unwrap()
    }

    #[test]
    fn a_dbusmenu_label_names_its_role_without_marker_ellipsis_or_case() {
        let names = ["Editor", "editor"];
        let cases = [
            ("About", Some(Role::About)),
            ("&About Editor", Some(Role::About)),
            ("about kde", Some(Role::About)),
            ("_Quit", Some(Role::Quit)),
            ("Quit Editor", Some(Role::Quit)),
            ("E&xit", Some(Role::Quit)),
            ("Preferences…", Some(Role::Preferences)),
            ("SETTINGS...", Some(Role::Preferences)),
            ("&Options", Some(Role::Preferences)),
            ("Configure Editor…", Some(Role::Preferences)),
            ("&Close", Some(Role::Close)),
            ("Close Window", Some(Role::Close)),
            ("Full Screen", Some(Role::FullScreen)),
            ("Enter Full Screen", Some(Role::FullScreen)),
            ("Exit Full Screen", Some(Role::FullScreen)),
            ("Configure Toolbars…", None),
            ("Configure", None),
            ("Quitting Time", None),
            ("Abouts", None),
            ("Close All", None),
            ("Préférences", None),
            ("Open…", None),
        ];
        for (label, role) in cases {
            assert_eq!(label_role(label, &names), role, "{label}");
        }
    }

    #[test]
    fn a_gmenumodel_item_has_the_role_its_action_names() {
        let tree = MenuItem::default();
        let e = export(&tree, Exporter::Gtk);
        let role = |action: &str| e.role(&gtk(1, "Anything", action));
        assert_eq!(role("app.about"), Some(Role::About));
        assert_eq!(role("app.preferences"), Some(Role::Preferences));
        assert_eq!(role("app.settings"), Some(Role::Preferences));
        assert_eq!(role("app.quit"), Some(Role::Quit));
        assert_eq!(role("win.close"), Some(Role::Close));
        assert_eq!(role("win.fullscreen"), Some(Role::FullScreen));
        assert_eq!(role("win.quit"), None, "the scope counts");
        assert_eq!(
            e.role(&MenuItem::new(1, "Quit")),
            None,
            "the label does not"
        );
        let submenu = MenuItem {
            action: Some("app.quit".into()),
            ..MenuItem::submenu(1, "Quit", vec![])
        };
        assert_eq!(e.role(&submenu), None, "a submenu has no role");
    }

    #[test]
    fn the_app_menus_about_settings_and_quit_invoke_the_exporters_items() {
        let tree = dbusmenu_tree();
        let e = export(&tree, Exporter::DbusMenu);
        assert_eq!(
            e.roles(),
            Roles {
                about: Some(ItemId(2)),
                preferences: Some(ItemId(6)),
                quit: Some(ItemId(8)),
            },
            "the application's own menu is looked in first"
        );
        let tree = gtk_tree();
        let roles = export(&tree, Exporter::Gtk).roles();
        assert_eq!(
            [
                FallbackAction::About,
                FallbackAction::Settings,
                FallbackAction::Quit,
                FallbackAction::Hide
            ]
            .map(|a| roles.target(a)),
            [
                Some(ItemId(100)),
                Some(ItemId(104)),
                Some(ItemId(105)),
                None
            ]
        );
        let (app, _) = merged(&export(&tree, Exporter::Gtk));
        let settings = app.iter().find(|i| i.label == "Settings…").unwrap();
        assert!(settings.enabled, "Settings… acts once bound");
    }

    #[test]
    fn without_role_items_the_app_menu_keeps_its_own_actions() {
        let tree = MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(1, "File", vec![MenuItem::new(2, "New")])],
        );
        let e = export(&tree, Exporter::DbusMenu);
        assert_eq!(e.roles(), Roles::default());
        let (app, _) = merged(&e);
        assert_eq!(app, fallback::app_menu("Editor", &Chords::default()));
        assert!(!app.iter().find(|i| i.label == "Settings…").unwrap().enabled);
    }

    #[test]
    fn role_items_leave_file_and_edit_and_their_separators_collapse() {
        let tree = dbusmenu_tree();
        let (_, menus) = merged(&export(&tree, Exporter::DbusMenu));
        assert_eq!(ids(&menu(&menus, "File").children), [11, 12, 13]);
        assert_eq!(ids(&menu(&menus, "Edit").children), [21, 22, 23]);
        let help = menu(&menus, "Help");
        assert!(
            help.children.iter().any(|i| i.id == ItemId(62)),
            "a role item elsewhere stays where it is"
        );

        let only_quit = MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(1, "File", vec![MenuItem::new(2, "Quit")])],
        );
        let (_, menus) = merged(&export(&only_quit, Exporter::DbusMenu));
        assert_eq!(labels(&menus), ["Window", "Help"], "an emptied menu goes");
    }

    #[test]
    fn the_gtk_application_menu_goes_between_about_and_settings() {
        let tree = gtk_tree();
        let (app, menus) = merged(&export(&tree, Exporter::Gtk));
        assert_eq!(
            labels(&app),
            [
                "About Editor",
                "",
                "Keyboard Shortcuts",
                "",
                "Settings…",
                "",
                "Hide Editor",
                "Hide Others",
                "Show All",
                "",
                "Quit Editor"
            ]
        );
        assert_eq!(
            ids(&app),
            [-1000, -1100, 102, -1107, -1005, -1104, -1001, -1002, -1003, -1101, -1004]
        );
        assert!(menus.iter().all(|m| m.id != GTK_APP_MENU));
    }

    #[test]
    fn the_dbusmenu_menu_titled_with_the_apps_name_or_id_is_its_own() {
        let tree = dbusmenu_tree();
        for names in [["Editor", "kate"], ["Text Editor", "editor"]] {
            let e = Export {
                names,
                ..export(&tree, Exporter::DbusMenu)
            };
            let (app, menus) = merged(&e);
            assert_eq!(ids(&app)[..5], [-1000, -1100, 4, -1107, -1005], "{names:?}");
            assert!(menus.iter().all(|m| m.label != "Editor"), "{names:?}");
        }
        let e = Export {
            names: ["Writer", "writer"],
            ..export(&tree, Exporter::DbusMenu)
        };
        let (app, menus) = merged(&e);
        assert_eq!(app, {
            let mut own = fallback::app_menu("Editor", &Chords::default());
            own[2].enabled = true;
            own
        });
        assert!(
            menus.iter().any(|m| m.label == "Editor"),
            "shown as exported"
        );
    }

    #[test]
    fn view_gets_enter_full_screen_unless_it_has_a_full_screen_item() {
        let tree = dbusmenu_tree();
        let (_, menus) = merged(&export(&tree, Exporter::DbusMenu));
        let view = menu(&menus, "View");
        assert_eq!(labels(&view.children), ["Zoom In", "", "Enter Full Screen"]);
        assert_eq!(ids(&view.children), [31, -1108, -1006]);

        let tree = gtk_tree();
        let (_, menus) = merged(&export(&tree, Exporter::Gtk));
        assert_eq!(ids(&menu(&menus, "View").children), [5]);

        let labelled = MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(
                1,
                "View",
                vec![MenuItem::new(2, "Full Screen")],
            )],
        );
        let (_, menus) = merged(&export(&labelled, Exporter::DbusMenu));
        assert_eq!(ids(&menu(&menus, "View").children), [2]);

        let unloaded = MenuItem::submenu(
            0,
            "",
            vec![
                MenuItem::submenu(1, "File", vec![]),
                MenuItem::submenu(2, "View", vec![]),
            ],
        );
        let (_, menus) = merged(&export(&unloaded, Exporter::DbusMenu));
        assert!(
            menus[..2].iter().all(MenuItem::needs_load),
            "menus still to be loaded are left to load when they open"
        );
    }

    #[test]
    fn window_is_the_bars_with_the_exporters_other_items_below() {
        let tree = dbusmenu_tree();
        let (_, menus) = merged(&export(&tree, Exporter::DbusMenu));
        let window = menu(&menus, "Window");
        let system = fallback::window_menu(1, Some(1), true, true, &[], &Chords::default());
        assert_eq!(window.id, fallback::WINDOW_MENU);
        assert_eq!(
            window.children[..system.children.len()],
            system.children[..]
        );
        assert_eq!(
            ids(&window.children[system.children.len()..]),
            [-1109, 54],
            "Minimize, Close Window and Move to Desktop are the bar's already"
        );

        let gtk_close = MenuItem::submenu(
            0,
            "",
            vec![MenuItem::submenu(
                1,
                "Window",
                vec![gtk(2, "Shut", "win.close"), gtk(3, "Tabs", "win.tabs")],
            )],
        );
        let (_, menus) = merged(&export(&gtk_close, Exporter::Gtk));
        let window = menu(&menus, "Window");
        assert_eq!(
            ids(&window.children[system.children.len()..]),
            [-1109, 3],
            "by role"
        );
    }

    #[test]
    fn help_keeps_the_exporters_items_below_the_search_row() {
        let tree = dbusmenu_tree();
        let (_, menus) = merged(&export(&tree, Exporter::DbusMenu));
        let help = menu(&menus, "Help");
        assert_eq!(help.id, fallback::HELP_MENU);
        assert_eq!(ids(&help.children), [-1030, -1110, 61, 62]);
    }

    #[test]
    fn the_menus_keep_the_bars_order() {
        let mut tree = dbusmenu_tree();
        tree.children.reverse();
        let (_, menus) = merged(&export(&tree, Exporter::DbusMenu));
        assert_eq!(
            labels(&menus),
            ["File", "Edit", "View", "Tools", "Window", "Help"]
        );
    }

    #[test]
    fn a_gtk_app_with_only_an_application_menu_gets_window_and_help() {
        let mut tree = gtk_tree();
        tree.children.truncate(1);
        let (app, menus) = merged(&export(&tree, Exporter::Gtk));
        assert_eq!(labels(&menus), ["Window", "Help"]);
        assert_eq!(
            ids(&menus),
            [fallback::WINDOW_MENU.0, fallback::HELP_MENU.0]
        );
        assert!(app.iter().any(|i| i.label == "Keyboard Shortcuts"));
    }

    #[test]
    fn ids_stay_across_a_layout_update() {
        let before = dbusmenu_tree();
        let mut after = dbusmenu_tree();
        let file = after.find_mut(ItemId(10)).unwrap();
        file.children.insert(1, MenuItem::new(16, "Open…"));
        after.find_mut(ItemId(31)).unwrap().label = "Zoom In Further".into();
        let flat = |(app, menus): (Vec<MenuItem>, Vec<MenuItem>)| {
            fn walk(items: &[MenuItem], out: &mut Vec<(i32, String)>) {
                for i in items {
                    out.push((i.id.0, i.label.clone()));
                    walk(&i.children, out);
                }
            }
            let mut out = Vec::new();
            walk(&app, &mut out);
            walk(&menus, &mut out);
            out
        };
        let old = flat(merged(&export(&before, Exporter::DbusMenu)));
        let new = flat(merged(&export(&after, Exporter::DbusMenu)));
        let kept: Vec<_> = new
            .iter()
            .filter(|(id, _)| *id != 16)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(kept, old.iter().map(|(id, _)| *id).collect::<Vec<_>>());
        assert!(new.contains(&(31, "Zoom In Further".into())));
    }

    fn shown(accel: &str, primary: ShortcutModifier, class: ClientClass) -> Option<String> {
        accel_shortcut(accel, primary, class).map(|s| s.display())
    }

    #[test]
    fn a_third_party_apps_accelerators_show_the_keys_pressed() {
        use ShortcutModifier::{Ctrl, Primary};
        let other = ClientClass::Other;
        let cases = [
            ("<Primary>q", "Super+Q", "Ctrl+Q"),
            ("<Control><Shift>s", "Shift+Super+S", "Ctrl+Shift+S"),
            ("<Primary>Return", "Super+Return", "Ctrl+Return"),
            ("<Primary>comma", "Super+,", "Ctrl+,"),
            ("<Primary>F5", "Ctrl+F5", "Ctrl+F5"),
            ("<Primary>Escape", "Ctrl+Esc", "Ctrl+Esc"),
            ("<Alt>F4", "Alt+F4", "Alt+F4"),
            // a Super chord reaches the application from the Ctrl one
            ("<Super>q", "Ctrl+Q", "Super+Q"),
        ];
        for (accel, cupertino, classic) in cases {
            assert_eq!(
                shown(accel, Primary, other).as_deref(),
                Some(cupertino),
                "{accel}"
            );
            assert_eq!(
                shown(accel, Ctrl, other).as_deref(),
                Some(classic),
                "{accel}"
            );
        }
        assert_eq!(shown("<Hyper>x", Primary, other), None);
        assert_eq!(shown("<Primary>", Primary, other), None);
        assert_eq!(shown("<Primary", Ctrl, other), None);

        let tree = gtk_tree();
        let chords = Chords::default();
        let (_, menus) = export(&tree, Exporter::Gtk).merge(
            fallback::app_menu("Editor", &chords),
            fallback::menus(1, Some(1), true, true, false, &[], &chords, true),
            Primary,
        );
        let new = &menu(&menus, "File").children[0];
        assert_eq!(
            new.shortcut.as_ref().map(Shortcut::display).as_deref(),
            Some("Super+N"),
            "the merge shows the keys pressed"
        );
    }

    #[test]
    fn a_first_party_apps_accelerators_show_as_they_are() {
        use ShortcutModifier::{Ctrl, Primary};
        assert_eq!(client_class("cupertino-files"), ClientClass::FirstParty);
        assert_eq!(client_class("org.example.editor"), ClientClass::Other);
        let first = ClientClass::FirstParty;
        for primary in [Primary, Ctrl] {
            assert_eq!(
                shown("<Super>q", primary, first).as_deref(),
                Some("Super+Q")
            );
            assert_eq!(
                shown("<Primary>q", primary, first).as_deref(),
                Some("Ctrl+Q")
            );
        }
    }
}
