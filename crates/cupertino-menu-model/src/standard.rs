//! The standard menus every application has: App, File, Edit and View.
//!
//! The Window and Help menus are the desktop's own and are not part of an application's menus.

use crate::chord::Chord;
use crate::menu::{ActionName, Command, Entry, Item, Menu, MenuBar, Scope, SystemCommand};
use cupertino_keyboard::modifiers::Modifier;
use Modifier::{Alt, Ctrl, Primary, Shift};

/// What the application handles beyond the commands every application has. Items for anything left out are omitted,
/// not disabled. Every application has Hide, Hide Others, Show All, Quit, Close Window, the editing commands of the
/// focused text control and Full Screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub about: bool,
    /// Settings…, sent to the application as its open-settings message.
    pub settings: bool,
    /// The label of the New item, naming what it makes ("New Window", "New Folder").
    pub new: Option<String>,
    pub open: bool,
    pub save: bool,
    pub save_as: bool,
    /// Find, Find Next, Find Previous and Use Selection for Find.
    pub find: bool,
    /// Whether the window's toolbar is shown, when it has one.
    pub toolbar: Option<bool>,
    /// Whether the window's sidebar is shown, when it has one.
    pub sidebar: Option<bool>,
    /// Whether the window is full screen.
    pub full_screen: bool,
}

fn chord(modifiers: &[Modifier], key: &str) -> Option<Chord> {
    Some(Chord::new(modifiers, key).expect("a standard chord names a keysym"))
}

fn item(label: &str, scope: Scope, name: &str, chord: Option<Chord>) -> Entry {
    let action = ActionName::new(scope, name).expect("a standard action name is valid");
    Entry::Item(Item::new(label, action, chord))
}

fn system(label: &str, command: SystemCommand, chord: Option<Chord>) -> Entry {
    Entry::Item(Item {
        label: label.to_string(),
        command: Command::System(command),
        chord,
        accessible_name: None,
        icon: None,
    })
}

/// The standard menus of an application named `app_name`.
pub fn menu_bar(app_name: &str, features: &Features) -> MenuBar {
    use Scope::{App, Win};
    let app = Menu::new(
        app_name,
        vec![
            features
                .about
                .then(|| item(&format!("About {app_name}"), App, "about", None))
                .into_iter()
                .collect(),
            features
                .settings
                .then(|| item("Settings…", App, "preferences", chord(&[Primary], "comma")))
                .into_iter()
                .collect(),
            vec![
                system(
                    &format!("Hide {app_name}"),
                    SystemCommand::Hide,
                    chord(&[Primary], "h"),
                ),
                system(
                    "Hide Others",
                    SystemCommand::HideOthers,
                    chord(&[Primary, Alt], "h"),
                ),
                system("Show All", SystemCommand::ShowAll, None),
            ],
            vec![item(
                &format!("Quit {app_name}"),
                App,
                "quit",
                chord(&[Primary], "q"),
            )],
        ],
    );
    let file = Menu::new(
        "File",
        vec![
            [
                features
                    .new
                    .as_deref()
                    .map(|label| item(label, App, "new", chord(&[Primary], "n"))),
                features
                    .open
                    .then(|| item("Open…", App, "open", chord(&[Primary], "o"))),
            ]
            .into_iter()
            .flatten()
            .collect(),
            [
                Some(item("Close Window", Win, "close", chord(&[Primary], "w"))),
                features
                    .save
                    .then(|| item("Save", Win, "save", chord(&[Primary], "s"))),
                features
                    .save_as
                    .then(|| item("Save As…", Win, "save-as", chord(&[Shift, Primary], "s"))),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ],
    );
    let find = Menu::new(
        "Find",
        vec![vec![
            item("Find…", Win, "find", chord(&[Primary], "f")),
            item("Find Next", Win, "find-next", chord(&[Primary], "g")),
            item(
                "Find Previous",
                Win,
                "find-previous",
                chord(&[Shift, Primary], "g"),
            ),
            item(
                "Use Selection for Find",
                Win,
                "use-selection-for-find",
                chord(&[Primary], "e"),
            ),
        ]],
    );
    let edit = Menu::new(
        "Edit",
        vec![
            vec![
                item("Undo", Win, "undo", chord(&[Primary], "z")),
                item("Redo", Win, "redo", chord(&[Shift, Primary], "z")),
            ],
            vec![
                item("Cut", Win, "cut", chord(&[Primary], "x")),
                item("Copy", Win, "copy", chord(&[Primary], "c")),
                item("Paste", Win, "paste", chord(&[Primary], "v")),
                item("Delete", Win, "delete", None),
                item("Select All", Win, "select-all", chord(&[Primary], "a")),
            ],
            features
                .find
                .then_some(Entry::Submenu(find))
                .into_iter()
                .collect(),
        ],
    );
    let toggle = |shown: Option<bool>, what: &str, name: &str, key: &str| {
        shown.map(|shown| {
            let label = format!("{} {what}", if shown { "Hide" } else { "Show" });
            item(&label, Win, name, chord(&[Primary, Alt], key))
        })
    };
    let view = Menu::new(
        "View",
        vec![
            [
                toggle(features.toolbar, "Toolbar", "toggle-toolbar", "t"),
                toggle(features.sidebar, "Sidebar", "toggle-sidebar", "s"),
            ]
            .into_iter()
            .flatten()
            .collect(),
            vec![item(
                if features.full_screen {
                    "Exit Full Screen"
                } else {
                    "Enter Full Screen"
                },
                Win,
                "fullscreen",
                chord(&[Primary, Ctrl], "f"),
            )],
        ],
    );
    MenuBar {
        app,
        menus: vec![file, edit, view],
    }
}
