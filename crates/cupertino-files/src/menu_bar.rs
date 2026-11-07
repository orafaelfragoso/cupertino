//! The menus Files publishes to the desktop's menu bar (menu bar spec §8): only what the window can do, and no Window
//! or Help menu, which the bar adds. Each item's chord is the window's own key for it. Files exports no Quit: each
//! window is a process of its own, so the bar's Quit, which closes every window of the application, quits Files.

use cupertino_menu_model::standard::{self, Features};
use cupertino_menu_model::{ActionName, Chord, Entry, Item, Menu, MenuBar, Scope};
use cupertino_modifiers::Modifier;

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
    FullScreen,
    Back,
    Forward,
    EnclosingFolder,
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
            SortBy(key, _) => (Scope::Win, sort_name(key)),
            TogglePathBar => (Scope::Win, "toggle-path-bar"),
            ToggleStatusBar => (Scope::Win, "toggle-status-bar"),
            FullScreen => (Scope::Win, "fullscreen"),
            Back => (Scope::Win, "go-back"),
            Forward => (Scope::Win, "go-forward"),
            EnclosingFolder => (Scope::Win, "go-enclosing-folder"),
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
            TogglePathBar,
            ToggleStatusBar,
            FullScreen,
            Back,
            Forward,
            EnclosingFolder,
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

fn item(label: &str, action: MenuAction, chord: Option<Chord>) -> Entry {
    Entry::Item(Item::new(label, action.name(), chord))
}

/// The menus for `state`. Put Back is there only in the trash, Eject only on a removable file system, and the toggles
/// name what they will do.
pub fn menu_bar(state: &MenuState) -> MenuBar {
    use MenuAction::*;
    use Modifier::{Alt, Ctrl, Primary, Shift};
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
                item("New Window", NewWindow, chord(&[Primary], "n")),
                item("New Folder", NewFolder, chord(&[Shift, Primary], "n")),
                item(
                    "New Folder with Selection",
                    NewFolderWithSelection,
                    chord(&[Ctrl, Primary], "n"),
                ),
            ],
            vec![
                item("Open", Open, chord(&[Primary], "o")),
                item("Open in New Window", OpenInNewWindow, None),
                item("Close Window", CloseWindow, chord(&[Primary], "w")),
            ],
            [
                Some(item(
                    "Move to Trash",
                    MoveToTrash,
                    chord(&[Primary], "BackSpace"),
                )),
                state.in_trash.then(|| item("Put Back", PutBack, None)),
                Some(item("Rename", Rename, None)),
                Some(item("Duplicate", Duplicate, chord(&[Primary], "d"))),
                Some(item("Make Alias", MakeAlias, chord(&[Ctrl, Primary], "a"))),
                Some(item("Show Original", ShowOriginal, chord(&[Primary], "r"))),
            ]
            .into_iter()
            .flatten()
            .collect(),
            state
                .eject
                .iter()
                .map(|name| item(&format!("Eject “{name}”"), Eject, chord(&[Primary], "e")))
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
                item("Copy", Copy, chord(&[Primary], "c")),
                item("Paste Item", Paste, chord(&[Primary], "v")),
                item("Move Item Here", MoveHere, chord(&[Alt, Primary], "v")),
            ],
            vec![item("Select All", SelectAll, chord(&[Primary], "a"))],
        ],
    );
    let sort_by = Menu::new(
        "Sort By",
        vec![sorts()
            .map(|(title, key, ascending)| item(title, SortBy(key, ascending), None))
            .collect()],
    );
    let view = Menu::new(
        "View",
        vec![
            vec![
                item("as Icons", View(ViewMode::Icons), chord(&[Primary], "1")),
                item("as List", View(ViewMode::List), chord(&[Primary], "2")),
            ],
            vec![Entry::Submenu(sort_by)],
            vec![
                item(
                    if state.path_bar {
                        "Hide Path Bar"
                    } else {
                        "Show Path Bar"
                    },
                    TogglePathBar,
                    chord(&[Alt, Primary], "p"),
                ),
                item(
                    if state.status_bar {
                        "Hide Status Bar"
                    } else {
                        "Show Status Bar"
                    },
                    ToggleStatusBar,
                    chord(&[Primary], "slash"),
                ),
            ],
            vec![full_screen],
        ],
    );
    let go = Menu::new(
        "Go",
        vec![vec![
            item("Back", Back, chord(&[Primary], "bracketleft")),
            item("Forward", Forward, chord(&[Primary], "bracketright")),
            item("Enclosing Folder", EnclosingFolder, chord(&[Primary], "Up")),
        ]],
    );
    MenuBar {
        app: Menu::new(&standard.app.title, Vec::new()),
        menus: vec![file, edit, view, go],
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
        (MoveHere, state.can_paste && !state.in_trash && writable),
        (Back, state.can_go_back),
        (Forward, state.can_go_forward),
        (EnclosingFolder, state.has_parent),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each menu as the bar reads it: titles, then `label action chord` per item, `---` between groups.
    fn outline(bar: &MenuBar) -> Vec<String> {
        let mut out = Vec::new();
        for menu in &bar.menus {
            out.push(format!("{}:", menu.title));
            for (i, section) in menu.sections.iter().enumerate() {
                if i > 0 {
                    out.push("  ---".into());
                }
                for entry in section {
                    match entry {
                        Entry::Item(item) => out.push(format!(
                            "  {} {}",
                            item.label,
                            match &item.command {
                                cupertino_menu_model::Command::Action(a) => a.full(),
                                cupertino_menu_model::Command::System(_) => "system".into(),
                            }
                        )),
                        Entry::Submenu(menu) => out.push(format!("  {} ▸", menu.title)),
                    }
                }
            }
        }
        out
    }

    #[test]
    fn the_menus_are_the_confirmed_ones_in_order() {
        let bar = menu_bar(&MenuState::default()).exported();
        assert_eq!(
            outline(&bar),
            [
                "File:",
                "  New Window app.new",
                "  New Folder win.new-folder",
                "  New Folder with Selection win.new-folder-with-selection",
                "  ---",
                "  Open win.open",
                "  Open in New Window win.open-in-new-window",
                "  Close Window win.close",
                "  ---",
                "  Move to Trash win.move-to-trash",
                "  Rename win.rename",
                "  Duplicate win.duplicate",
                "  Make Alias win.make-alias",
                "  Show Original win.show-original",
                "Edit:",
                "  Copy win.copy",
                "  Paste Item win.paste",
                "  Move Item Here win.move-here",
                "  ---",
                "  Select All win.select-all",
                "View:",
                "  as Icons win.view-icons",
                "  as List win.view-list",
                "  ---",
                "  Sort By ▸",
                "  ---",
                "  Show Path Bar win.toggle-path-bar",
                "  Show Status Bar win.toggle-status-bar",
                "  ---",
                "  Enter Full Screen win.fullscreen",
                "Go:",
                "  Back win.go-back",
                "  Forward win.go-forward",
                "  Enclosing Folder win.go-enclosing-folder",
            ]
        );
        // the App menu exports nothing: About and Settings… have no handler, and the bar's Quit closes every window
        assert!(bar.app.sections.is_empty());
        assert_eq!(
            MenuAction::from_name(&ActionName::parse("app.quit").unwrap()),
            None
        );
    }

    #[test]
    fn put_back_is_there_only_in_the_trash_and_the_toggles_name_what_they_do() {
        let state = MenuState {
            in_trash: true,
            path_bar: true,
            status_bar: true,
            full_screen: true,
            ..MenuState::default()
        };
        let lines = outline(&menu_bar(&state).exported());
        for line in [
            "  Put Back win.put-back",
            "  Hide Path Bar win.toggle-path-bar",
            "  Hide Status Bar win.toggle-status-bar",
            "  Exit Full Screen win.fullscreen",
        ] {
            assert!(lines.contains(&line.to_string()), "{line} in {lines:?}");
        }
    }

    #[test]
    fn eject_names_the_removable_file_system_shown_and_is_there_only_on_one() {
        let on_stick = MenuState {
            eject: Some("KINGSTON".into()),
            ..MenuState::default()
        };
        let lines = outline(&menu_bar(&on_stick).exported());
        let at = lines
            .iter()
            .position(|l| l == "  Eject “KINGSTON” win.eject")
            .expect("the Eject item");
        assert_eq!(
            lines[at - 2..at],
            ["  Show Original win.show-original", "  ---"]
        );
        let elsewhere = outline(&menu_bar(&MenuState::default()).exported());
        assert!(elsewhere.iter().all(|l| !l.contains("win.eject")));
    }

    #[test]
    fn every_published_action_maps_back_to_its_item() {
        let bar = menu_bar(&MenuState {
            in_trash: true,
            eject: Some("KINGSTON".into()),
            ..MenuState::default()
        });
        fn actions(menu: &Menu, out: &mut Vec<ActionName>) {
            for entry in menu.sections.iter().flatten() {
                match entry {
                    Entry::Item(item) => {
                        if let cupertino_menu_model::Command::Action(a) = &item.command {
                            out.push(a.clone());
                        }
                    }
                    Entry::Submenu(menu) => actions(menu, out),
                }
            }
        }
        let mut names = Vec::new();
        actions(&bar.app, &mut names);
        for menu in &bar.menus {
            actions(menu, &mut names);
        }
        for name in names {
            let action = MenuAction::from_name(&name).unwrap_or_else(|| panic!("{}", name.full()));
            assert_eq!(action.name(), name);
        }
    }

    #[test]
    fn every_chord_names_a_key_the_window_can_bind() {
        let bar = menu_bar(&MenuState {
            in_trash: true,
            eject: Some("KINGSTON".into()),
            ..MenuState::default()
        });
        let chords = chords(&bar);
        assert!(chords.len() >= 15, "{chords:?}");
        for (chord, action) in &chords {
            assert!(keysym(chord.key.name()).is_some(), "{action:?}: {chord:?}");
        }
        assert_eq!(keysym("n"), Some(0x6e));
        assert_eq!(keysym("1"), Some(0x31));
        assert_eq!(keysym("BackSpace"), Some(0xff08));
        assert_eq!(keysym("F13"), None);
        let trash = chords
            .iter()
            .find(|(_, action)| *action == MenuAction::MoveToTrash)
            .unwrap();
        assert_eq!(trash.0.key.name(), "BackSpace");
    }

    #[test]
    fn items_that_need_a_selection_are_disabled_without_one() {
        let none = enabled(&MenuState::default());
        assert!(
            none.iter()
                .all(|(action, on)| !on || *action == MenuAction::NewFolder),
            "{none:?}"
        );
        let one_folder = enabled(&MenuState {
            count: 1,
            all_folders: true,
            has_parent: true,
            ..MenuState::default()
        });
        for action in [
            MenuAction::Open,
            MenuAction::OpenInNewWindow,
            MenuAction::MoveToTrash,
            MenuAction::Rename,
            MenuAction::EnclosingFolder,
        ] {
            assert!(one_folder.contains(&(action, true)), "{action:?}");
        }
        assert!(one_folder.contains(&(MenuAction::PutBack, false)));
    }

    #[test]
    fn read_only_media_take_nothing_new_and_lose_nothing() {
        let state = MenuState {
            count: 1,
            has_parent: true,
            read_only: true,
            ..MenuState::default()
        };
        let on = enabled(&state);
        for action in [
            MenuAction::NewFolder,
            MenuAction::NewFolderWithSelection,
            MenuAction::MoveToTrash,
            MenuAction::Rename,
            MenuAction::Duplicate,
            MenuAction::MakeAlias,
        ] {
            assert!(on.contains(&(action, false)), "{action:?}");
        }
        assert!(on.contains(&(MenuAction::Open, true)));
    }
}
