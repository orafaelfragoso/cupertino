//! The menus Files publishes to the desktop's menu bar (menu bar spec §8): only what the window can do, and no Help
//! menu, which the bar adds. Its Window menu holds only Show Progress, which the bar lists below its own items. Each
//! item's chord is the window's own key for it. Files exports no Quit: each
//! window is a process of its own, so the bar's Quit, which closes every window of the application, quits Files.

use cupertino_menu_model::standard::{self, Features};
use cupertino_menu_model::{ActionName, Chord, Entry, Item, Menu, MenuBar, Scope};
use cupertino_modifiers::{Modifier, Mods};

use crate::menu_spec::sort_choices;
use crate::sort::SortKey;
use crate::window::ViewMode;

/// What a menu item asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    NewWindow,
    NewFolder,
    NewFolderWithSelection,
    Open,
    OpenInNewWindow,
    CloseWindow,
    MoveToTrash,
    PutBack,
    Rename,
    Duplicate,
    MakeAlias,
    ShowOriginal,
    Eject,
    Copy,
    Paste,
    MoveHere,
    SelectAll,
    View(ViewMode),
    SortBy(SortKey, bool),
    TogglePathBar,
    ToggleStatusBar,
    /// Hide the sidebar, or show it again.
    ToggleSidebar,
    FullScreen,
    Back,
    Forward,
    EnclosingFolder,
    /// Show the files opened lately.
    Recents,
    /// Show the home folder.
    Home,
    /// Show the progress window of the file operations.
    ShowProgress,
    /// Open a tab on the folder shown, after the tab shown.
    NewTab,
    /// Close the tab shown; the last tab closes the window.
    CloseTab,
    NextTab,
    PreviousTab,
    /// Show the tab bar while the window has one tab, or hide it again.
    ToggleTabBar,
    /// Preview the selection in the preview window, or close it.
    Peek,
}

/// What the menus depend on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    /// Number of selected items.
    pub count: usize,
    /// Every selected item is a folder.
    pub all_folders: bool,
    /// The only selected item is a link.
    pub single_link: bool,
    /// The folder shown is the trash's.
    pub in_trash: bool,
    pub path_bar: bool,
    pub status_bar: bool,
    /// The sidebar is shown, not collapsed.
    pub sidebar: bool,
    pub full_screen: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub has_parent: bool,
    /// The name of the removable file system the folder shown is on.
    pub eject: Option<String>,
    /// The folder shown is on read-only media: nothing can be made, moved or renamed in it.
    pub read_only: bool,
    /// Copy put items on the pasteboard.
    pub can_paste: bool,
    /// The pasteboard's items may be moved, not only copied (Move Item Here).
    pub can_move: bool,
    /// How many tabs the window has.
    pub tabs: usize,
    /// The tab bar shows while the window has one tab.
    pub tab_bar: bool,
    /// A file operation runs, or one that failed is listed in the progress window.
    pub jobs: bool,
}

/// The sort choices the window has data for, with their action names.
fn sorts() -> impl Iterator<Item = (&'static str, SortKey, bool)> {
    sort_choices()
        .into_iter()
        .filter_map(|(title, choice)| choice.map(|(key, ascending)| (title, key, ascending)))
}

fn sort_name(key: SortKey) -> &'static str {
    match key {
        SortKey::None => "sort-none",
        SortKey::Name => "sort-name",
        SortKey::Kind => "sort-kind",
        SortKey::DateModified => "sort-date-modified",
        SortKey::DateCreated => "sort-date-created",
        SortKey::DateAccessed => "sort-date-accessed",
        SortKey::Size => "sort-size",
        SortKey::Tags => "sort-tags",
    }
}

impl MenuAction {
    /// The action the item of `self` is published with.
    pub fn name(self) -> ActionName {
        use MenuAction::*;
        let (scope, name) = match self {
            NewWindow => (Scope::App, "new"),
            NewFolder => (Scope::Win, "new-folder"),
            NewFolderWithSelection => (Scope::Win, "new-folder-with-selection"),
            Open => (Scope::Win, "open"),
            OpenInNewWindow => (Scope::Win, "open-in-new-window"),
            CloseWindow => (Scope::Win, "close"),
            MoveToTrash => (Scope::Win, "move-to-trash"),
            PutBack => (Scope::Win, "put-back"),
            Rename => (Scope::Win, "rename"),
            Duplicate => (Scope::Win, "duplicate"),
            MakeAlias => (Scope::Win, "make-alias"),
            ShowOriginal => (Scope::Win, "show-original"),
            Eject => (Scope::Win, "eject"),
            Copy => (Scope::Win, "copy"),
            Paste => (Scope::Win, "paste"),
            MoveHere => (Scope::Win, "move-here"),
            SelectAll => (Scope::Win, "select-all"),
            View(ViewMode::Icons) => (Scope::Win, "view-icons"),
            View(ViewMode::List) => (Scope::Win, "view-list"),
            View(ViewMode::Columns) => (Scope::Win, "view-columns"),
            View(ViewMode::Gallery) => (Scope::Win, "view-gallery"),
            SortBy(key, _) => (Scope::Win, sort_name(key)),
            TogglePathBar => (Scope::Win, "toggle-path-bar"),
            ToggleStatusBar => (Scope::Win, "toggle-status-bar"),
            ToggleSidebar => (Scope::Win, "toggle-sidebar"),
            FullScreen => (Scope::Win, "fullscreen"),
            Back => (Scope::Win, "go-back"),
            Forward => (Scope::Win, "go-forward"),
            EnclosingFolder => (Scope::Win, "go-enclosing-folder"),
            Recents => (Scope::Win, "go-recents"),
            Home => (Scope::Win, "go-home"),
            ShowProgress => (Scope::Win, "show-progress"),
            NewTab => (Scope::Win, "new-tab"),
            CloseTab => (Scope::Win, "close-tab"),
            NextTab => (Scope::Win, "next-tab"),
            PreviousTab => (Scope::Win, "previous-tab"),
            ToggleTabBar => (Scope::Win, "toggle-tab-bar"),
            Peek => (Scope::Win, "peek"),
        };
        ActionName::new(scope, name).expect("the menu's action names are valid")
    }

    /// The action of a chosen item; `None` for a name the menus do not publish.
    pub fn from_name(name: &ActionName) -> Option<MenuAction> {
        use MenuAction::*;
        let fixed = [
            NewWindow,
            NewFolder,
            NewFolderWithSelection,
            Open,
            OpenInNewWindow,
            CloseWindow,
            MoveToTrash,
            PutBack,
            Rename,
            Duplicate,
            MakeAlias,
            ShowOriginal,
            Eject,
            Copy,
            Paste,
            MoveHere,
            SelectAll,
            View(ViewMode::Icons),
            View(ViewMode::List),
            View(ViewMode::Columns),
            View(ViewMode::Gallery),
            TogglePathBar,
            ToggleStatusBar,
            ToggleSidebar,
            FullScreen,
            Back,
            Forward,
            EnclosingFolder,
            Recents,
            Home,
            ShowProgress,
            NewTab,
            CloseTab,
            NextTab,
            PreviousTab,
            ToggleTabBar,
            Peek,
        ];
        fixed
            .into_iter()
            .chain(sorts().map(|(_, key, ascending)| SortBy(key, ascending)))
            .find(|action| action.name() == *name)
    }
}

fn chord(modifiers: &[Modifier], key: &str) -> Option<Chord> {
    Some(Chord::new(modifiers, key).expect("the menu's chords name keysyms"))
}

fn item(label: &str, action: MenuAction) -> Entry {
    Entry::Item(Item::new(label, action.name(), action.chord()))
}

/// An item with the picture `symbol` before its label.
fn item_with_icon(label: &str, action: MenuAction, symbol: &str) -> Entry {
    Entry::Item(Item::new(label, action.name(), action.chord()).with_icon(symbol))
}

impl MenuAction {
    /// The keys that do `self` in a window, shown beside its item in every menu that offers it. Full screen's are
    /// the standard View menu's.
    pub fn chord(self) -> Option<Chord> {
        use MenuAction::*;
        use Modifier::{Alt, Ctrl, Primary, Shift};
        match self {
            NewWindow => chord(&[Primary], "n"),
            NewFolder => chord(&[Shift, Primary], "n"),
            NewFolderWithSelection => chord(&[Ctrl, Primary], "n"),
            Open => chord(&[Primary], "o"),
            CloseWindow => chord(&[Primary], "w"),
            MoveToTrash => chord(&[Primary], "BackSpace"),
            Duplicate => chord(&[Primary], "d"),
            MakeAlias => chord(&[Ctrl, Primary], "a"),
            ShowOriginal => chord(&[Primary], "r"),
            Eject => chord(&[Primary], "e"),
            Copy => chord(&[Primary], "c"),
            Paste => chord(&[Primary], "v"),
            MoveHere => chord(&[Alt, Primary], "v"),
            SelectAll => chord(&[Primary], "a"),
            View(ViewMode::Icons) => chord(&[Primary], "1"),
            View(ViewMode::List) => chord(&[Primary], "2"),
            View(ViewMode::Columns) => chord(&[Primary], "3"),
            View(ViewMode::Gallery) => chord(&[Primary], "4"),
            TogglePathBar => chord(&[Alt, Primary], "p"),
            ToggleStatusBar => chord(&[Primary], "slash"),
            ToggleSidebar => chord(&[Alt, Primary], "s"),
            Back => chord(&[Primary], "bracketleft"),
            Forward => chord(&[Primary], "bracketright"),
            EnclosingFolder => chord(&[Primary], "Up"),
            Home => chord(&[Shift, Primary], "h"),
            NewTab => chord(&[Primary], "t"),
            CloseTab => chord(&[Primary], "w"),
            NextTab => chord(&[Ctrl], "Tab"),
            PreviousTab => chord(&[Ctrl, Shift], "Tab"),
            ToggleTabBar => chord(&[Shift, Primary], "t"),
            Peek => chord(&[Primary], "y"),
            // Shift+Primary+F is the system's full-screen shortcut, which the compositor takes before any window
            Recents => None,
            OpenInNewWindow | PutBack | Rename | SortBy(..) | FullScreen | ShowProgress => None,
        }
    }
}

/// `chord` as a menu row shows it: the modifier symbols in the order control, option, shift, command, then the key,
/// the common named keys as symbols (Backspace as the delete-left arrow).
pub fn glyphs(chord: &Chord) -> String {
    let mut text: String = [
        (Mods::CTRL, '\u{2303}'),
        (Mods::ALT, '\u{2325}'),
        (Mods::SHIFT, '\u{21e7}'),
        (Mods::PRIMARY, '\u{2318}'),
    ]
    .into_iter()
    .filter(|(m, _)| chord.modifiers.contains(*m))
    .map(|(_, glyph)| glyph)
    .collect();
    match chord.key.name() {
        "BackSpace" => text.push('\u{232b}'),
        "Delete" => text.push('\u{2326}'),
        "Return" => text.push('\u{21a9}'),
        "Tab" => text.push('\u{21e5}'),
        "Escape" => text.push('\u{238b}'),
        "Up" => text.push('\u{2191}'),
        "Down" => text.push('\u{2193}'),
        "Left" => text.push('\u{2190}'),
        "Right" => text.push('\u{2192}'),
        _ => text.push_str(&chord.key.label()),
    }
    text
}

/// The menus for `state`. Put Back is there only in the trash, Eject only on a removable file system, and the toggles
/// name what they will do.
pub fn menu_bar(state: &MenuState) -> MenuBar {
    use MenuAction::*;
    let standard = standard::menu_bar(
        "Files",
        &Features {
            full_screen: state.full_screen,
            ..Features::default()
        },
    );
    let full_screen = standard
        .menus
        .iter()
        .flat_map(|menu| menu.sections.iter().flatten())
        .find(|entry| matches!(entry, Entry::Item(item) if item.command == cupertino_menu_model::Command::Action(FullScreen.name())))
        .cloned()
        .expect("the standard View menu has full screen");
    let file = Menu::new(
        "File",
        vec![
            vec![
                item("New Window", NewWindow),
                item("New Folder", NewFolder),
                item("New Folder with Selection", NewFolderWithSelection),
                item("New Tab", NewTab),
            ],
            [
                Some(item("Open", Open)),
                Some(item("Open in New Window", OpenInNewWindow)),
                // with tabs, the primary key and W closes the tab and Shift with them the window
                Some(Entry::Item(Item::new(
                    "Close Window",
                    CloseWindow.name(),
                    match state.tabs > 1 {
                        true => chord(&[Modifier::Shift, Modifier::Primary], "w"),
                        false => CloseWindow.chord(),
                    },
                ))),
                (state.tabs > 1).then(|| item("Close Tab", CloseTab)),
            ]
            .into_iter()
            .flatten()
            .collect(),
            [
                Some(item_with_icon("Move to Trash", MoveToTrash, "symbol.trash")),
                state.in_trash.then(|| item("Put Back", PutBack)),
                Some(item("Rename", Rename)),
                Some(item("Duplicate", Duplicate)),
                Some(item("Make Alias", MakeAlias)),
                Some(item("Quick Preview", Peek)),
                Some(item("Show Original", ShowOriginal)),
            ]
            .into_iter()
            .flatten()
            .collect(),
            state
                .eject
                .iter()
                .map(|name| item_with_icon(&format!("Eject “{name}”"), Eject, "symbol.eject"))
                .collect(),
        ]
        .into_iter()
        .filter(|section: &Vec<Entry>| !section.is_empty())
        .collect(),
    );
    let edit = Menu::new(
        "Edit",
        vec![
            vec![
                item("Copy", Copy),
                item("Paste Item", Paste),
                item("Move Item Here", MoveHere),
            ],
            vec![item("Select All", SelectAll)],
        ],
    );
    let sort_by = Menu::new(
        "Sort By",
        vec![sorts()
            .map(|(title, key, ascending)| item(title, SortBy(key, ascending)))
            .collect()],
    );
    let view = Menu::new(
        "View",
        vec![
            vec![
                item_with_icon("as Icons", View(ViewMode::Icons), "symbol.grid"),
                item_with_icon("as List", View(ViewMode::List), "symbol.list"),
                item_with_icon("as Columns", View(ViewMode::Columns), "symbol.columns"),
                item_with_icon("as Gallery", View(ViewMode::Gallery), "symbol.gallery"),
            ],
            vec![Entry::Submenu(sort_by)],
            vec![item(
                if state.tabs > 1 || state.tab_bar {
                    "Hide Tab Bar"
                } else {
                    "Show Tab Bar"
                },
                ToggleTabBar,
            )],
            vec![item(
                if state.sidebar {
                    "Hide Sidebar"
                } else {
                    "Show Sidebar"
                },
                ToggleSidebar,
            )],
            vec![
                item(
                    if state.path_bar {
                        "Hide Path Bar"
                    } else {
                        "Show Path Bar"
                    },
                    TogglePathBar,
                ),
                item(
                    if state.status_bar {
                        "Hide Status Bar"
                    } else {
                        "Show Status Bar"
                    },
                    ToggleStatusBar,
                ),
            ],
            vec![full_screen],
        ],
    );
    let go = Menu::new(
        "Go",
        vec![
            vec![
                item("Back", Back),
                item("Forward", Forward),
                item("Enclosing Folder", EnclosingFolder),
            ],
            vec![
                item_with_icon("Recents", Recents, "symbol.recents"),
                item_with_icon("Home", Home, "symbol.house"),
            ],
        ],
    );
    let window = Menu::new(
        "Window",
        vec![
            vec![
                item("Show Previous Tab", PreviousTab),
                item("Show Next Tab", NextTab),
            ],
            vec![item("Show Progress", ShowProgress)],
        ],
    );
    MenuBar {
        app: Menu::new(&standard.app.title, Vec::new()),
        menus: vec![file, edit, view, go, window],
    }
}

/// The keysym of a chord's key name: a letter or digit, or one of the named keys the menus use; `None` for another
/// name.
pub fn keysym(name: &str) -> Option<u32> {
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return c
            .is_ascii_alphanumeric()
            .then(|| c.to_ascii_lowercase() as u32);
    }
    Some(match name {
        "BackSpace" => 0xff08,
        "Return" => 0xff0d,
        "Tab" => 0xff09,
        "Escape" => 0xff1b,
        "Left" => 0xff51,
        "Up" => 0xff52,
        "Right" => 0xff53,
        "Down" => 0xff54,
        "slash" => 0x2f,
        "bracketleft" => 0x5b,
        "bracketright" => 0x5d,
        _ => return None,
    })
}

/// Every item of `bar` that has a chord, with its action: the window's keys for its menus.
pub fn chords(bar: &MenuBar) -> Vec<(Chord, MenuAction)> {
    fn walk(menu: &Menu, out: &mut Vec<(Chord, MenuAction)>) {
        for entry in menu.sections.iter().flatten() {
            match entry {
                Entry::Item(item) => {
                    let action = match &item.command {
                        cupertino_menu_model::Command::Action(name) => MenuAction::from_name(name),
                        cupertino_menu_model::Command::System(_) => None,
                    };
                    if let (Some(chord), Some(action)) = (&item.chord, action) {
                        out.push((chord.clone(), action));
                    }
                }
                Entry::Submenu(menu) => walk(menu, out),
            }
        }
    }
    let mut out = Vec::new();
    walk(&bar.app, &mut out);
    for menu in &bar.menus {
        walk(menu, &mut out);
    }
    out
}

/// Whether each item that depends on the selection or the history can be chosen in `state`.
pub fn enabled(state: &MenuState) -> Vec<(MenuAction, bool)> {
    use MenuAction::*;
    let selected = state.count > 0;
    let writable = !state.read_only;
    vec![
        (NewFolder, writable),
        (
            NewFolderWithSelection,
            selected && !state.in_trash && writable,
        ),
        (Open, selected),
        (OpenInNewWindow, selected && state.all_folders),
        (MoveToTrash, selected && !state.in_trash && writable),
        (PutBack, selected && state.in_trash),
        (Rename, state.count == 1 && !state.in_trash && writable),
        (Duplicate, selected && !state.in_trash && writable),
        (MakeAlias, selected && !state.in_trash && writable),
        (ShowOriginal, state.single_link),
        (Copy, selected),
        (Paste, state.can_paste && !state.in_trash && writable),
        (MoveHere, state.can_move && !state.in_trash && writable),
        (Back, state.can_go_back),
        (Forward, state.can_go_forward),
        (EnclosingFolder, state.has_parent),
        (ShowProgress, state.jobs),
        (Peek, selected),
        (CloseTab, state.tabs > 1),
        (NextTab, state.tabs > 1),
        (PreviousTab, state.tabs > 1),
    ]
}
