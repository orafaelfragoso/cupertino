//! The standard menus every application has: App, File, Edit and View.
//!
//! The Window and Help menus are the desktop's own and are not part of an application's menus.

use crate::chord::Chord;
use crate::menu::{ActionName, Command, Entry, Item, Menu, MenuBar, Scope, SystemCommand};
use cupertino_modifiers::Modifier;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Role;
    use cupertino_modifiers::Profile;

    /// Each menu as its groups of `label [action] chord` lines, submenus indented.
    fn outline(menu: &Menu) -> Vec<Vec<String>> {
        menu.sections
            .iter()
            .map(|section| {
                section
                    .iter()
                    .flat_map(|entry| match entry {
                        Entry::Item(item) => {
                            let command = match &item.command {
                                Command::Action(a) => a.full(),
                                Command::System(s) => format!("{s:?}"),
                            };
                            let chord = item
                                .chord
                                .as_ref()
                                .and_then(|c| c.display(&Profile::cupertino()))
                                .unwrap_or_default();
                            vec![format!("{} [{command}] {chord}", item.label)
                                .trim_end()
                                .to_string()]
                        }
                        Entry::Submenu(menu) => std::iter::once(format!("{} ▸", menu.title))
                            .chain(
                                outline(menu)
                                    .into_iter()
                                    .flatten()
                                    .map(|l| format!("  {l}")),
                            )
                            .collect(),
                    })
                    .collect()
            })
            .collect()
    }

    fn all() -> Features {
        Features {
            about: true,
            settings: true,
            new: Some("New Note".to_string()),
            open: true,
            save: true,
            save_as: true,
            find: true,
            toolbar: Some(true),
            sidebar: Some(false),
            full_screen: false,
        }
    }

    #[test]
    fn an_application_with_every_feature_has_the_full_standard_menus() {
        let bar = menu_bar("Notes", &all());
        let titles: Vec<&str> = bar.menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["File", "Edit", "View"]);
        assert_eq!(
            outline(&bar.app),
            [
                vec!["About Notes [app.about]"],
                vec!["Settings… [app.preferences] Super+,"],
                vec![
                    "Hide Notes [Hide] Super+H",
                    "Hide Others [HideOthers] Super+Alt+H",
                    "Show All [ShowAll]"
                ],
                vec!["Quit Notes [app.quit] Super+Q"],
            ]
        );
        assert_eq!(
            outline(&bar.menus[0]),
            [
                vec!["New Note [app.new] Super+N", "Open… [app.open] Super+O"],
                vec![
                    "Close Window [win.close] Super+W",
                    "Save [win.save] Super+S",
                    "Save As… [win.save-as] Shift+Super+S",
                ],
            ]
        );
        assert_eq!(
            outline(&bar.menus[1]),
            [
                vec!["Undo [win.undo] Super+Z", "Redo [win.redo] Shift+Super+Z"],
                vec![
                    "Cut [win.cut] Super+X",
                    "Copy [win.copy] Super+C",
                    "Paste [win.paste] Super+V",
                    "Delete [win.delete]",
                    "Select All [win.select-all] Super+A",
                ],
                vec![
                    "Find ▸",
                    "  Find… [win.find] Super+F",
                    "  Find Next [win.find-next] Super+G",
                    "  Find Previous [win.find-previous] Shift+Super+G",
                    "  Use Selection for Find [win.use-selection-for-find] Super+E",
                ],
            ]
        );
        assert_eq!(
            outline(&bar.menus[2]),
            [
                vec![
                    "Hide Toolbar [win.toggle-toolbar] Super+Alt+T",
                    "Show Sidebar [win.toggle-sidebar] Super+Alt+S"
                ],
                vec!["Enter Full Screen [win.fullscreen] Super+Ctrl+F"],
            ]
        );
    }

    #[test]
    fn items_the_application_does_not_handle_are_omitted() {
        let bar = menu_bar(
            "Clock",
            &Features {
                full_screen: true,
                ..Features::default()
            },
        );
        assert_eq!(
            outline(&bar.app),
            [
                vec![],
                vec![],
                vec![
                    "Hide Clock [Hide] Super+H",
                    "Hide Others [HideOthers] Super+Alt+H",
                    "Show All [ShowAll]"
                ],
                vec!["Quit Clock [app.quit] Super+Q"],
            ] as [Vec<&str>; 4]
        );
        assert_eq!(
            outline(&bar.menus[0]),
            [vec![], vec!["Close Window [win.close] Super+W"]] as [Vec<&str>; 2]
        );
        assert_eq!(bar.menus[1].sections.last(), Some(&Vec::new()));
        assert_eq!(
            outline(&bar.menus[2]),
            [
                vec![],
                vec!["Exit Full Screen [win.fullscreen] Super+Ctrl+F"]
            ] as [Vec<&str>; 2]
        );
    }

    #[test]
    fn the_export_keeps_what_the_application_owns() {
        let exported = menu_bar("Notes", &all()).exported();
        assert_eq!(
            outline(&exported.app),
            [
                vec!["About Notes [app.about]"],
                vec!["Settings… [app.preferences] Super+,"],
                vec!["Quit Notes [app.quit] Super+Q"],
            ]
        );
        let roles: Vec<Role> = exported
            .app
            .sections
            .iter()
            .flatten()
            .filter_map(|e| match e {
                Entry::Item(item) => item.role(),
                Entry::Submenu(_) => None,
            })
            .collect();
        assert_eq!(roles, [Role::About, Role::Preferences, Role::Quit]);
        assert_eq!(exported.menus, menu_bar("Notes", &all()).menus);

        let bare = menu_bar("Clock", &Features::default()).exported();
        assert_eq!(outline(&bare.app), [vec!["Quit Clock [app.quit] Super+Q"]]);
        assert_eq!(
            outline(&bare.menus[0]),
            [vec!["Close Window [win.close] Super+W"]]
        );
        assert_eq!(bare.menus[1].sections.len(), 2);
    }
}
