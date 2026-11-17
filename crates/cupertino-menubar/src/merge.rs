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
use cupertino_keyboard::modifiers::{ModifierMap, Profile, RawMods};
use cupertino_keyboard::rewrite::{
    is_printable, pressed_for, ClientClass, KeyPress, SYM_KP_ENTER, SYM_RETURN,
};
use cupertino_menu_model::{ActionName, Chord, Role};

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
    /// How the server treats the application's keys, as it names them for the active window; its accelerators show the
    /// keys pressed.
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
