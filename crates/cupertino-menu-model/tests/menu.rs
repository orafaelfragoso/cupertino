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
fn scopes_have_their_prefixes() {
    assert_eq!(Scope::App.prefix(), "app");
    assert_eq!(Scope::Win.prefix(), "win");
}

#[test]
fn an_action_name_is_letters_digits_and_inner_hyphens() {
    for name in ["quit", "save-as", "find-next", "a1", "x"] {
        let a = ActionName::new(Scope::Win, name).expect(name);
        assert_eq!(a.name(), name);
        assert_eq!(a.scope(), Scope::Win);
    }
}

#[test]
fn malformed_action_names_are_rejected() {
    for name in ["", "-a", "a-", "-", "a_b", "a.b", "a b", "é"] {
        assert!(ActionName::new(Scope::App, name).is_none(), "{name:?}");
    }
}

#[test]
fn an_action_is_written_with_its_scope() {
    let a = ActionName::new(Scope::App, "quit").unwrap();
    assert_eq!(a.full(), "app.quit");
    assert_eq!(
        ActionName::new(Scope::Win, "save-as").unwrap().full(),
        "win.save-as"
    );
}

#[test]
fn parsing_reads_both_scopes_and_round_trips() {
    for full in ["app.quit", "win.close", "win.save-as"] {
        assert_eq!(action(full).full(), full);
    }
    assert_eq!(action("app.quit").scope(), Scope::App);
    assert_eq!(action("win.close").scope(), Scope::Win);
}

#[test]
fn parsing_rejects_unknown_scopes_and_malformed_names() {
    for full in [
        "quit", "foo.bar", "app.", ".quit", "app.a.b", "App.quit", "app.-x",
    ] {
        assert_eq!(ActionName::parse(full), None, "{full:?}");
    }
}

#[test]
fn roles_name_their_actions() {
    let table = [
        (Role::About, "app.about"),
        (Role::Preferences, "app.preferences"),
        (Role::Quit, "app.quit"),
        (Role::Close, "win.close"),
        (Role::FullScreen, "win.fullscreen"),
    ];
    for (role, full) in table {
        assert_eq!(role.action().full(), full);
        assert_eq!(Role::of(&action(full)), Some(role));
    }
    assert_eq!(Role::ALL.len(), 5);
}

#[test]
fn settings_is_an_alias_of_the_preferences_role() {
    assert_eq!(Role::of(&action("app.settings")), Some(Role::Preferences));
    assert_eq!(Role::of(&action("win.settings")), None);
}

#[test]
fn a_role_needs_the_right_scope() {
    assert_eq!(Role::of(&action("win.quit")), None);
    assert_eq!(Role::of(&action("app.close")), None);
    assert_eq!(Role::of(&action("app.save")), None);
}

#[test]
fn an_item_reports_its_role_from_its_action_only() {
    let quit = Item::new("Quit", action("app.quit"), None);
    assert_eq!(quit.role(), Some(Role::Quit));
    assert_eq!(Item::new("Save", action("win.save"), None).role(), None);
    let hide = Item {
        command: Command::System(SystemCommand::Hide),
        ..quit
    };
    assert_eq!(hide.role(), None);
}

#[test]
fn an_item_keeps_its_label_action_and_chord() {
    let chord = Chord::new(&[Modifier::Primary], "q");
    let i = Item::new("Quit", action("app.quit"), chord.clone());
    assert_eq!(i.label, "Quit");
    assert_eq!(i.command, Command::Action(action("app.quit")));
    assert_eq!(i.chord, chord);
    assert_eq!(i.icon, None);
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
    let i = Item::new("Delete", action("win.delete"), None).with_icon("symbol.trash");
    assert_eq!(i.icon.as_deref(), Some("symbol.trash"));
    assert_eq!(i.label, "Delete");
}

#[test]
fn a_menu_keeps_its_title_and_sections() {
    let m = Menu::new("File", vec![vec![item("Open", "app.open")], vec![]]);
    assert_eq!(m.title, "File");
    assert_eq!(m.sections.len(), 2);
}

#[test]
fn exporting_drops_system_commands() {
    let bar = MenuBar {
        app: Menu::new(
            "Files",
            vec![
                vec![
                    system("Hide Files", SystemCommand::Hide),
                    system("Hide Others", SystemCommand::HideOthers),
                    system("Show All", SystemCommand::ShowAll),
                ],
                vec![item("Quit Files", "app.quit")],
            ],
        ),
        menus: vec![],
    };
    let out = bar.exported();
    assert_eq!(out.app.title, "Files");
    assert_eq!(labels(&out.app), vec![vec!["Quit Files".to_string()]]);
}

#[test]
fn exporting_drops_groups_a_system_command_left_empty() {
    let menu = Menu::new(
        "Files",
        vec![
            vec![item("About", "app.about")],
            vec![system("Hide", SystemCommand::Hide)],
            vec![item("Quit", "app.quit")],
        ],
    );
    let out = MenuBar {
        app: menu,
        menus: vec![],
    }
    .exported();
    assert_eq!(out.app.sections.len(), 2);
}

#[test]
fn exporting_drops_a_submenu_with_nothing_left_and_keeps_one_with_items() {
    let only_system = Menu::new("Hidden", vec![vec![system("Hide", SystemCommand::Hide)]]);
    let kept = Menu::new("Find", vec![vec![item("Find…", "win.find")]]);
    let edit = Menu::new(
        "Edit",
        vec![
            vec![item("Undo", "win.undo")],
            vec![Entry::Submenu(only_system), Entry::Submenu(kept.clone())],
        ],
    );
    let out = MenuBar {
        app: Menu::new("App", vec![]),
        menus: vec![edit],
    }
    .exported();
    assert_eq!(
        labels(&out.menus[0]),
        vec![vec!["Undo".to_string()], vec!["> Find".to_string()]]
    );
    assert_eq!(out.menus[0].sections[1][0], Entry::Submenu(kept));
}

#[test]
fn exporting_drops_menus_that_end_up_empty_and_keeps_the_order() {
    let bar = MenuBar {
        app: Menu::new("App", vec![]),
        menus: vec![
            Menu::new("File", vec![vec![item("Open", "app.open")]]),
            Menu::new(
                "Window",
                vec![vec![system("Show All", SystemCommand::ShowAll)]],
            ),
            Menu::new("View", vec![vec![item("Zoom", "win.zoom")]]),
        ],
    };
    let titles: Vec<String> = bar.exported().menus.into_iter().map(|m| m.title).collect();
    assert_eq!(titles, vec!["File", "View"]);
}

#[test]
fn an_application_menu_with_nothing_left_stays_as_an_empty_titled_menu() {
    let bar = MenuBar {
        app: Menu::new("App", vec![vec![system("Hide", SystemCommand::Hide)]]),
        menus: vec![],
    };
    let out = bar.exported();
    assert_eq!(out.app.title, "App");
    assert!(out.app.sections.is_empty());
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
