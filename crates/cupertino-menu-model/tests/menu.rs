//! Actions, roles and the menu tree.

use cupertino_keyboard::modifiers::Modifier;
use cupertino_menu_model::{
    ActionName, Chord, Command, Entry, Item, Menu, MenuBar, Role, Scope, SystemCommand,
};

fn action(full: &str) -> ActionName {
    ActionName::parse(full).expect(full)
}

fn item(label: &str, full: &str) -> Entry {
    Entry::Item(Item::new(label, action(full), None))
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

fn labels(menu: &Menu) -> Vec<Vec<String>> {
    menu.sections
        .iter()
        .map(|section| {
            section
                .iter()
                .map(|entry| match entry {
                    Entry::Item(i) => i.label.clone(),
                    Entry::Submenu(m) => format!("> {}", m.title),
                })
                .collect()
        })
        .collect()
}

#[test]
fn an_action_name_is_a_scope_and_letters_digits_and_inner_hyphens() {
    for (scope, name, full) in [
        (Scope::App, "quit", "app.quit"),
        (Scope::Win, "save-as", "win.save-as"),
        (Scope::Win, "find-next", "win.find-next"),
        (Scope::App, "a1", "app.a1"),
        (Scope::Win, "x", "win.x"),
    ] {
        let a = ActionName::new(scope, name).expect(name);
        assert_eq!(
            (a.scope(), a.name(), a.full()),
            (scope, name, full.to_string())
        );
        assert_eq!(ActionName::parse(full), Some(a), "{full}");
    }
    for name in ["", "-a", "a-", "-", "a_b", "a.b", "a b", "é"] {
        assert!(ActionName::new(Scope::App, name).is_none(), "{name:?}");
    }
    for full in [
        "quit", "foo.bar", "app.", ".quit", "app.a.b", "App.quit", "app.-x",
    ] {
        assert_eq!(ActionName::parse(full), None, "{full:?}");
    }
}

#[test]
fn a_role_is_found_from_its_action_in_the_right_scope() {
    for (role, full) in [
        (Role::About, "app.about"),
        (Role::Preferences, "app.preferences"),
        (Role::Quit, "app.quit"),
        (Role::Close, "win.close"),
        (Role::FullScreen, "win.fullscreen"),
    ] {
        assert_eq!(role.action().full(), full);
        assert_eq!(Role::of(&action(full)), Some(role), "{full}");
    }
    for (full, role) in [
        ("app.settings", Some(Role::Preferences)),
        ("win.settings", None),
        ("win.quit", None),
        ("app.close", None),
        ("app.save", None),
    ] {
        assert_eq!(Role::of(&action(full)), role, "{full}");
    }

    let quit = Item::new("Quit", action("app.quit"), None);
    assert_eq!(quit.role(), Some(Role::Quit));
    let hide = Item {
        command: Command::System(SystemCommand::Hide),
        ..quit
    };
    assert_eq!(hide.role(), None, "a system command has no role");
}

#[test]
fn the_accessible_name_defaults_to_the_label() {
    let mut i = Item::new("Close", action("win.close"), None);
    assert_eq!(i.accessible_name(), "Close");
    i.accessible_name = Some("Close window".to_string());
    assert_eq!(i.accessible_name(), "Close window");
}

#[test]
fn an_icon_is_added_without_changing_the_rest() {
    let plain = Item::new("Delete", action("win.delete"), Chord::new(&[], "Delete"));
    let i = plain.clone().with_icon("symbol.trash");
    assert_eq!(i.icon.as_deref(), Some("symbol.trash"));
    assert_eq!(Item { icon: None, ..i }, plain);
}

#[test]
fn exporting_drops_system_commands_and_whatever_they_leave_empty() {
    let only_system = Menu::new("Hidden", vec![vec![system("Hide", SystemCommand::Hide)]]);
    let kept = Menu::new("Find", vec![vec![item("Find…", "win.find")]]);
    let bar = MenuBar {
        app: Menu::new(
            "Files",
            vec![
                vec![item("About", "app.about")],
                vec![
                    system("Hide Files", SystemCommand::Hide),
                    system("Hide Others", SystemCommand::HideOthers),
                    system("Show All", SystemCommand::ShowAll),
                ],
                vec![item("Quit Files", "app.quit")],
            ],
        ),
        menus: vec![
            Menu::new("File", vec![vec![item("Open", "app.open")]]),
            Menu::new(
                "Window",
                vec![vec![system("Show All", SystemCommand::ShowAll)]],
            ),
            Menu::new(
                "Edit",
                vec![
                    vec![item("Undo", "win.undo")],
                    vec![Entry::Submenu(only_system), Entry::Submenu(kept.clone())],
                ],
            ),
        ],
    };
    let out = bar.exported();
    assert_eq!(out.app.title, "Files");
    assert_eq!(
        labels(&out.app),
        vec![vec!["About".to_string()], vec!["Quit Files".to_string()]]
    );
    let titles: Vec<&str> = out.menus.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["File", "Edit"],
        "an emptied menu goes, order kept"
    );
    assert_eq!(
        labels(&out.menus[1]),
        vec![vec!["Undo".to_string()], vec!["> Find".to_string()]]
    );
    assert_eq!(out.menus[1].sections[1][0], Entry::Submenu(kept));

    let emptied = MenuBar {
        app: Menu::new("App", vec![vec![system("Hide", SystemCommand::Hide)]]),
        menus: vec![],
    }
    .exported();
    assert_eq!(emptied.app.title, "App", "the application menu stays");
    assert!(emptied.app.sections.is_empty());
}

#[test]
fn exporting_is_idempotent_and_keeps_item_details() {
    let chord = Chord::new(&[Modifier::Primary], "q");
    let quit = Item::new("Quit", action("app.quit"), chord).with_icon("symbol.power");
    let bar = MenuBar {
        app: Menu::new("App", vec![vec![Entry::Item(quit.clone())]]),
        menus: vec![],
    };
    let once = bar.exported();
    assert_eq!(once.app.sections[0][0], Entry::Item(quit));
    assert_eq!(once.exported(), once);
}
