//! The menus Settings publishes to the desktop's menu bar (menu bar spec §8): Quit by role, the editing items of the
//! search field and Find, and full screen. No About or Settings… item (the bar's About stands), no pane list, and no
//! Window or Help menu, which the bar adds.

use cupertino_menu_model::standard::{self, Features};
use cupertino_menu_model::{ActionName, Chord, Command, Entry, Item, Menu, MenuBar, Scope};
use cupertino_modifiers::Modifier;

/// What a menu item asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FullScreen,
}

const ACTIONS: [MenuAction; 9] = [
    MenuAction::Quit,
    MenuAction::Undo,
    MenuAction::Redo,
    MenuAction::Cut,
    MenuAction::Copy,
    MenuAction::Paste,
    MenuAction::SelectAll,
    MenuAction::Find,
    MenuAction::FullScreen,
];

/// What the menus depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MenuState {
    /// The search field has the keyboard.
    pub search_focused: bool,
    /// Text is selected in the search field.
    pub has_selection: bool,
    pub full_screen: bool,
}

impl MenuAction {
    /// The action the item of `self` is published with: the standard names, so the bar knows the roles.
    pub fn name(self) -> ActionName {
        use MenuAction::*;
        let (scope, name) = match self {
            Quit => (Scope::App, "quit"),
            Undo => (Scope::Win, "undo"),
            Redo => (Scope::Win, "redo"),
            Cut => (Scope::Win, "cut"),
            Copy => (Scope::Win, "copy"),
            Paste => (Scope::Win, "paste"),
            SelectAll => (Scope::Win, "select-all"),
            Find => (Scope::Win, "find"),
            FullScreen => (Scope::Win, "fullscreen"),
        };
        ActionName::new(scope, name).expect("the menu's action names are valid")
    }

    /// The action of a chosen item; `None` for a name the menus do not publish.
    pub fn from_name(name: &ActionName) -> Option<MenuAction> {
        ACTIONS.into_iter().find(|action| action.name() == *name)
    }
}

fn chord(modifiers: &[Modifier], key: &str) -> Option<Chord> {
    Some(Chord::new(modifiers, key).expect("the menu's chords name keysyms"))
}

fn item(label: &str, action: MenuAction, chord: Option<Chord>) -> Entry {
    Entry::Item(Item::new(label, action.name(), chord))
}

/// The menus for `state`; the full screen item names what it will do.
pub fn menu_bar(state: &MenuState) -> MenuBar {
    use MenuAction::*;
    use Modifier::{Primary, Shift};
    let standard = standard::menu_bar(
        "Settings",
        &Features {
            full_screen: state.full_screen,
            ..Features::default()
        },
    );
    let full_screen = standard
        .menus
        .iter()
        .flat_map(|menu| menu.sections.iter().flatten())
        .find(|entry| {
            matches!(entry, Entry::Item(item) if item.command == Command::Action(FullScreen.name()))
        })
        .cloned()
        .expect("the standard View menu has full screen");
    let edit = Menu::new(
        "Edit",
        vec![
            vec![
                item("Undo", Undo, chord(&[Primary], "z")),
                item("Redo", Redo, chord(&[Shift, Primary], "z")),
            ],
            vec![
                item("Cut", Cut, chord(&[Primary], "x")),
                item("Copy", Copy, chord(&[Primary], "c")),
                item("Paste", Paste, chord(&[Primary], "v")),
                item("Select All", SelectAll, chord(&[Primary], "a")),
            ],
            vec![item("Find", Find, chord(&[Primary], "f"))],
        ],
    );
    let view = Menu::new("View", vec![vec![full_screen]]);
    MenuBar {
        app: standard.app,
        menus: vec![edit, view],
    }
}

/// Whether each editing item can be chosen in `state`: they act on the search field while it has the keyboard.
pub fn enabled(state: &MenuState) -> Vec<(MenuAction, bool)> {
    use MenuAction::*;
    let field = state.search_focused;
    vec![
        (Undo, field),
        (Redo, field),
        (Cut, field && state.has_selection),
        (Copy, field && state.has_selection),
        (Paste, field),
        (SelectAll, field),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(menu: &Menu) -> Vec<String> {
        menu.sections
            .iter()
            .enumerate()
            .flat_map(|(i, section)| {
                (i > 0)
                    .then(|| "---".to_string())
                    .into_iter()
                    .chain(section.iter().map(|entry| match entry {
                        Entry::Item(item) => item.label.clone(),
                        Entry::Submenu(menu) => format!("{} ▸", menu.title),
                    }))
            })
            .collect()
    }

    #[test]
    fn the_menus_are_the_confirmed_ones() {
        let bar = menu_bar(&MenuState::default()).exported();
        assert_eq!(labels(&bar.app), ["Quit Settings"]);
        assert_eq!(
            bar.menus
                .iter()
                .map(|m| (m.title.clone(), labels(m)))
                .collect::<Vec<_>>(),
            [
                (
                    "Edit".to_string(),
                    [
                        "Undo",
                        "Redo",
                        "---",
                        "Cut",
                        "Copy",
                        "Paste",
                        "Select All",
                        "---",
                        "Find"
                    ]
                    .map(String::from)
                    .to_vec()
                ),
                ("View".to_string(), vec!["Enter Full Screen".to_string()]),
            ]
        );
    }

    #[test]
    fn every_action_maps_back_to_its_item() {
        for action in ACTIONS {
            assert_eq!(MenuAction::from_name(&action.name()), Some(action));
        }
    }

    #[test]
    fn the_editing_items_follow_the_search_field() {
        assert!(enabled(&MenuState::default()).iter().all(|(_, on)| !on));
        let typing = enabled(&MenuState {
            search_focused: true,
            ..MenuState::default()
        });
        assert!(typing.contains(&(MenuAction::Paste, true)));
        assert!(typing.contains(&(MenuAction::Copy, false)));
        let selected = enabled(&MenuState {
            search_focused: true,
            has_selection: true,
            ..MenuState::default()
        });
        assert!(selected.contains(&(MenuAction::Cut, true)));
    }
}
