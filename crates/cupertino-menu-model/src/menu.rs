//! The menu tree, its actions and the roles the menu bar rebinds.

use crate::chord::Chord;

/// Where an action lives: the application, or the window whose menus are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    App,
    Win,
}

impl Scope {
    pub fn prefix(self) -> &'static str {
        match self {
            Scope::App => "app",
            Scope::Win => "win",
        }
    }
}

/// An action by scope and name, written `app.quit` or `win.close`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActionName {
    scope: Scope,
    name: String,
}

impl ActionName {
    /// `None` unless `name` is ASCII letters, digits and inner hyphens.
    pub fn new(scope: Scope, name: &str) -> Option<ActionName> {
        let valid = !name.is_empty()
            && !name.starts_with('-')
            && !name.ends_with('-')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        valid.then(|| ActionName {
            scope,
            name: name.to_string(),
        })
    }

    /// Parse `app.<name>` or `win.<name>`.
    pub fn parse(full: &str) -> Option<ActionName> {
        let (prefix, name) = full.split_once('.')?;
        let scope = [Scope::App, Scope::Win]
            .into_iter()
            .find(|s| s.prefix() == prefix)?;
        ActionName::new(scope, name)
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn full(&self) -> String {
        format!("{}.{}", self.scope.prefix(), self.name)
    }
}

/// An item the menu bar finds by its action and rebinds to its own item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    About,
    Preferences,
    Quit,
    Close,
    FullScreen,
}

impl Role {
    pub const ALL: [Role; 5] = [
        Role::About,
        Role::Preferences,
        Role::Quit,
        Role::Close,
        Role::FullScreen,
    ];

    /// The action an exporter names the role with.
    pub fn action(self) -> ActionName {
        let (scope, name) = match self {
            Role::About => (Scope::App, "about"),
            Role::Preferences => (Scope::App, "preferences"),
            Role::Quit => (Scope::App, "quit"),
            Role::Close => (Scope::Win, "close"),
            Role::FullScreen => (Scope::Win, "fullscreen"),
        };
        ActionName {
            scope,
            name: name.to_string(),
        }
    }

    /// The role an action names, including `app.settings` for the preferences.
    pub fn of(action: &ActionName) -> Option<Role> {
        if action.scope == Scope::App && action.name == "settings" {
            return Some(Role::Preferences);
        }
        Role::ALL.into_iter().find(|r| r.action() == *action)
    }
}

/// A command the desktop runs for every application; the menu bar builds its item and no application exports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemCommand {
    Hide,
    HideOthers,
    ShowAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Action(ActionName),
    System(SystemCommand),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub command: Command,
    pub chord: Option<Chord>,
    /// The name an assistive technology reads when it differs from the label.
    pub accessible_name: Option<String>,
    /// The picture drawn before the label, as a template symbol of the system's pictures (`symbol.trash`); none for a
    /// row that has no picture.
    pub icon: Option<String>,
}

impl Item {
    pub fn new(label: &str, action: ActionName, chord: Option<Chord>) -> Item {
        Item {
            label: label.to_string(),
            command: Command::Action(action),
            chord,
            accessible_name: None,
            icon: None,
        }
    }

    /// The same item with the picture `symbol` before its label.
    pub fn with_icon(mut self, symbol: &str) -> Item {
        self.icon = Some(symbol.to_string());
        self
    }

    pub fn accessible_name(&self) -> &str {
        self.accessible_name.as_deref().unwrap_or(&self.label)
    }

    pub fn role(&self) -> Option<Role> {
        match &self.command {
            Command::Action(action) => Role::of(action),
            Command::System(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Item(Item),
    Submenu(Menu),
}

/// A menu: groups of entries, with a separator between groups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    pub sections: Vec<Vec<Entry>>,
}

impl Menu {
    pub fn new(title: &str, sections: Vec<Vec<Entry>>) -> Menu {
        Menu {
            title: title.to_string(),
            sections,
        }
    }

    /// The menu without system commands, empty groups and empty submenus; `None` when nothing is left.
    fn exported(&self) -> Option<Menu> {
        let sections: Vec<Vec<Entry>> = self
            .sections
            .iter()
            .map(|section| {
                section
                    .iter()
                    .filter_map(|entry| match entry {
                        Entry::Item(item) => {
                            matches!(item.command, Command::Action(_)).then(|| entry.clone())
                        }
                        Entry::Submenu(menu) => menu.exported().map(Entry::Submenu),
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|section| !section.is_empty())
            .collect();
        (!sections.is_empty()).then(|| Menu::new(&self.title, sections))
    }
}

/// An application's menus: the application menu, then the menus in the order the bar shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuBar {
    pub app: Menu,
    pub menus: Vec<Menu>,
}

impl MenuBar {
    /// What an application publishes to the menu bar: everything but the system commands, which the bar adds itself.
    pub fn exported(&self) -> MenuBar {
        MenuBar {
            app: self
                .app
                .exported()
                .unwrap_or_else(|| Menu::new(&self.app.title, Vec::new())),
            menus: self.menus.iter().filter_map(Menu::exported).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str, action: &str) -> Entry {
        Entry::Item(Item::new(label, ActionName::parse(action).unwrap(), None))
    }

    fn system(label: &str, command: SystemCommand) -> Entry {
        Entry::Item(Item {
            label: label.to_string(),
            command: Command::System(command),
            chord: None,
            accessible_name: None,
            icon: None,
        })
    }

    #[test]
    fn action_names_have_a_scope_and_a_plain_name() {
        let a = ActionName::parse("win.save-as").unwrap();
        assert_eq!(
            (a.scope(), a.name(), a.full().as_str()),
            (Scope::Win, "save-as", "win.save-as")
        );
        for bad in [
            "save", "doc.save", "app.", "app.-x", "app.x-", "app.a.b", "app.a b",
        ] {
            assert_eq!(ActionName::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn roles_are_found_by_action() {
        let roles: Vec<String> = Role::ALL.iter().map(|r| r.action().full()).collect();
        assert_eq!(
            roles,
            [
                "app.about",
                "app.preferences",
                "app.quit",
                "win.close",
                "win.fullscreen"
            ]
        );
        for r in Role::ALL {
            assert_eq!(Role::of(&r.action()), Some(r));
        }
        assert_eq!(
            Role::of(&ActionName::parse("app.settings").unwrap()),
            Some(Role::Preferences)
        );
        assert_eq!(Role::of(&ActionName::parse("win.quit").unwrap()), None);
        assert_eq!(Role::of(&ActionName::parse("app.new").unwrap()), None);
    }

    #[test]
    fn the_export_leaves_out_system_commands_and_what_they_leave_empty() {
        let bar = MenuBar {
            app: Menu::new(
                "Notes",
                vec![
                    vec![item("About Notes", "app.about")],
                    vec![
                        system("Hide Notes", SystemCommand::Hide),
                        system("Show All", SystemCommand::ShowAll),
                    ],
                    vec![item("Quit Notes", "app.quit")],
                ],
            ),
            menus: vec![
                Menu::new("Arrange", vec![vec![system("Hide", SystemCommand::Hide)]]),
                Menu::new(
                    "Format",
                    vec![vec![
                        Entry::Submenu(Menu::new(
                            "Empty",
                            vec![vec![system("x", SystemCommand::HideOthers)]],
                        )),
                        item("Bold", "win.bold"),
                    ]],
                ),
            ],
        };
        assert_eq!(
            bar.exported(),
            MenuBar {
                app: Menu::new(
                    "Notes",
                    vec![
                        vec![item("About Notes", "app.about")],
                        vec![item("Quit Notes", "app.quit")]
                    ],
                ),
                menus: vec![Menu::new("Format", vec![vec![item("Bold", "win.bold")]])],
            }
        );
    }
}
