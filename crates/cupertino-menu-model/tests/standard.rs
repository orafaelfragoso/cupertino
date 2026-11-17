//! The standard App, File, Edit and View menus.

use cupertino_keyboard::modifiers::{Modifier, Profile};
use cupertino_menu_model::standard::{menu_bar, Features};
use cupertino_menu_model::{Chord, Command, Entry, Item, Menu, MenuBar, Role, SystemCommand};

fn items(menu: &Menu) -> Vec<&Item> {
    let mut out = Vec::new();
    for entry in menu.sections.iter().flatten() {
        match entry {
            Entry::Item(i) => out.push(i),
            Entry::Submenu(m) => out.extend(items(m)),
        }
    }
    out
}

fn all_items(bar: &MenuBar) -> Vec<&Item> {
    let mut out = items(&bar.app);
    for m in &bar.menus {
        out.extend(items(m));
    }
    out
}

fn labels(menu: &Menu) -> Vec<String> {
    items(menu).iter().map(|i| i.label.clone()).collect()
}

fn find<'a>(menu: &'a Menu, label: &str) -> &'a Item {
    items(menu)
        .into_iter()
        .find(|i| i.label == label)
        .unwrap_or_else(|| panic!("no item {label:?} in {:?}", labels(menu)))
}

fn action_of(item: &Item) -> String {
    match &item.command {
        Command::Action(a) => a.full(),
        Command::System(_) => panic!("a system command"),
    }
}

fn chord(modifiers: &[Modifier], key: &str) -> Option<Chord> {
    Chord::new(modifiers, key)
}

fn full() -> Features {
    Features {
        about: true,
        settings: true,
        new: Some("New Window".to_string()),
        open: true,
        save: true,
        save_as: true,
        find: true,
        toolbar: Some(true),
        sidebar: Some(false),
        full_screen: false,
    }
}

fn bar(features: &Features) -> MenuBar {
    menu_bar("Files", features)
}

#[test]
fn the_bar_has_the_app_menu_then_file_edit_and_view() {
    let b = bar(&full());
    assert_eq!(b.app.title, "Files");
    let titles: Vec<&str> = b.menus.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(titles, vec!["File", "Edit", "View"]);
}

#[test]
fn the_app_menu_is_named_for_the_application() {
    let b = menu_bar("Notes", &full());
    assert_eq!(b.app.title, "Notes");
    assert!(labels(&b.app).contains(&"Hide Notes".to_string()));
    assert!(labels(&b.app).contains(&"Quit Notes".to_string()));
    assert!(labels(&b.app).contains(&"About Notes".to_string()));
}

#[test]
fn every_application_has_hide_hide_others_show_all_and_quit() {
    let b = bar(&Features::default());
    assert_eq!(
        labels(&b.app),
        vec!["Hide Files", "Hide Others", "Show All", "Quit Files"]
    );
    let hide = find(&b.app, "Hide Files");
    assert_eq!(hide.command, Command::System(SystemCommand::Hide));
    assert_eq!(hide.chord, chord(&[Modifier::Primary], "h"));
    let others = find(&b.app, "Hide Others");
    assert_eq!(others.command, Command::System(SystemCommand::HideOthers));
    assert_eq!(
        others.chord,
        chord(&[Modifier::Primary, Modifier::Alt], "h")
    );
    let all = find(&b.app, "Show All");
    assert_eq!(all.command, Command::System(SystemCommand::ShowAll));
    assert_eq!(all.chord, None);
    let quit = find(&b.app, "Quit Files");
    assert_eq!(action_of(quit), "app.quit");
    assert_eq!(quit.chord, chord(&[Modifier::Primary], "q"));
}

#[test]
fn about_and_settings_appear_only_when_handled() {
    let none = bar(&Features::default());
    assert!(items(&none.app)
        .iter()
        .all(|i| i.label != "About Files" && i.label != "Settings…"));
    let b = bar(&Features {
        about: true,
        settings: true,
        ..Features::default()
    });
    assert_eq!(action_of(find(&b.app, "About Files")), "app.about");
    assert_eq!(find(&b.app, "About Files").chord, None);
    let settings = find(&b.app, "Settings…");
    assert_eq!(action_of(settings), "app.preferences");
    assert_eq!(settings.chord, chord(&[Modifier::Primary], "comma"));
    assert_eq!(settings.role(), Some(Role::Preferences));
}

#[test]
fn the_file_menu_always_closes_the_window() {
    let b = bar(&Features::default());
    assert_eq!(labels(&b.menus[0]), vec!["Close Window"]);
    let close = find(&b.menus[0], "Close Window");
    assert_eq!(action_of(close), "win.close");
    assert_eq!(close.chord, chord(&[Modifier::Primary], "w"));
    assert_eq!(close.role(), Some(Role::Close));
}

#[test]
fn the_file_menu_has_new_open_save_and_save_as_when_handled() {
    let b = bar(&full());
    assert_eq!(
        labels(&b.menus[0]),
        vec!["New Window", "Open…", "Close Window", "Save", "Save As…"]
    );
    let new = find(&b.menus[0], "New Window");
    assert_eq!(action_of(new), "app.new");
    assert_eq!(new.chord, chord(&[Modifier::Primary], "n"));
    assert_eq!(action_of(find(&b.menus[0], "Open…")), "app.open");
    assert_eq!(action_of(find(&b.menus[0], "Save")), "win.save");
    let save_as = find(&b.menus[0], "Save As…");
    assert_eq!(action_of(save_as), "win.save-as");
    assert_eq!(
        save_as.chord,
        chord(&[Modifier::Shift, Modifier::Primary], "s")
    );
}

#[test]
fn the_new_item_takes_its_label_from_the_application() {
    let b = bar(&Features {
        new: Some("New Folder".to_string()),
        ..Features::default()
    });
    assert_eq!(action_of(find(&b.menus[0], "New Folder")), "app.new");
}

#[test]
fn the_edit_menu_always_has_the_editing_commands() {
    let b = bar(&Features::default());
    assert_eq!(
        labels(&b.menus[1]),
        vec![
            "Undo",
            "Redo",
            "Cut",
            "Copy",
            "Paste",
            "Delete",
            "Select All"
        ]
    );
    let redo = find(&b.menus[1], "Redo");
    assert_eq!(action_of(redo), "win.redo");
    assert_eq!(
        redo.chord,
        chord(&[Modifier::Shift, Modifier::Primary], "z")
    );
    assert_eq!(
        find(&b.menus[1], "Copy").chord,
        chord(&[Modifier::Primary], "c")
    );
    assert_eq!(find(&b.menus[1], "Delete").chord, None);
}

#[test]
fn the_find_submenu_appears_only_when_handled() {
    let without = bar(&Features::default());
    assert!(!without.menus[1]
        .sections
        .iter()
        .flatten()
        .any(|e| matches!(e, Entry::Submenu(_))));
    let with = bar(&Features {
        find: true,
        ..Features::default()
    });
    let submenu = with.menus[1]
        .sections
        .iter()
        .flatten()
        .find_map(|e| match e {
            Entry::Submenu(m) => Some(m),
            Entry::Item(_) => None,
        })
        .expect("a Find submenu");
    assert_eq!(submenu.title, "Find");
    assert_eq!(
        labels(submenu),
        vec![
            "Find…",
            "Find Next",
            "Find Previous",
            "Use Selection for Find"
        ]
    );
    assert_eq!(
        find(submenu, "Find…").chord,
        chord(&[Modifier::Primary], "f")
    );
    assert_eq!(
        find(submenu, "Find Previous").chord,
        chord(&[Modifier::Shift, Modifier::Primary], "g")
    );
}

#[test]
fn the_view_menu_always_has_full_screen_labelled_by_the_window_state() {
    let off = bar(&Features::default());
    assert_eq!(labels(&off.menus[2]), vec!["Enter Full Screen"]);
    let on = bar(&Features {
        full_screen: true,
        ..Features::default()
    });
    let exit = find(&on.menus[2], "Exit Full Screen");
    assert_eq!(action_of(exit), "win.fullscreen");
    assert_eq!(exit.chord, chord(&[Modifier::Primary, Modifier::Ctrl], "f"));
    assert_eq!(exit.role(), Some(Role::FullScreen));
}

#[test]
fn the_toolbar_and_sidebar_items_name_what_the_next_click_does() {
    let b = bar(&Features {
        toolbar: Some(true),
        sidebar: Some(false),
        ..Features::default()
    });
    let view = &b.menus[2];
    let toolbar = find(view, "Hide Toolbar");
    assert_eq!(action_of(toolbar), "win.toggle-toolbar");
    assert_eq!(
        toolbar.chord,
        chord(&[Modifier::Primary, Modifier::Alt], "t")
    );
    let sidebar = find(view, "Show Sidebar");
    assert_eq!(action_of(sidebar), "win.toggle-sidebar");
    assert_eq!(
        sidebar.chord,
        chord(&[Modifier::Primary, Modifier::Alt], "s")
    );
    assert_eq!(
        labels(view),
        vec!["Hide Toolbar", "Show Sidebar", "Enter Full Screen"]
    );
}

#[test]
fn a_window_without_a_toolbar_or_sidebar_has_no_such_items() {
    let b = bar(&Features::default());
    assert!(items(&b.menus[2])
        .iter()
        .all(|i| !i.label.contains("Toolbar") && !i.label.contains("Sidebar")));
}

#[test]
fn unhandled_features_are_omitted_not_disabled() {
    let small = all_items(&bar(&Features::default())).len();
    let big = all_items(&bar(&full())).len();
    assert_eq!(small, 4 + 1 + 7 + 1);
    assert_eq!(big, 6 + 5 + 11 + 3);
}

#[test]
fn the_standard_menus_hold_no_empty_groups() {
    for features in [Features::default(), full()] {
        let b = bar(&features);
        for menu in std::iter::once(&b.app).chain(&b.menus) {
            assert!(
                menu.sections.iter().all(|s| !s.is_empty()),
                "{} has an empty group",
                menu.title
            );
        }
    }
}

#[test]
fn exporting_the_standard_bar_keeps_everything_but_the_desktop_commands() {
    let b = bar(&full());
    let out = b.exported();
    assert_eq!(
        labels(&out.app),
        vec!["About Files", "Settings…", "Quit Files"]
    );
    assert_eq!(out.menus.len(), 3);
    assert_eq!(out.menus[1], b.menus[1]);
}

#[test]
fn the_standard_actions_that_have_roles_carry_them() {
    let b = bar(&full());
    let mut roles: Vec<Role> = all_items(&b).iter().filter_map(|i| i.role()).collect();
    roles.sort_by_key(|r| Role::ALL.iter().position(|x| x == r));
    assert_eq!(roles, Role::ALL.to_vec());
}

#[test]
fn no_two_standard_items_share_a_chord() {
    let b = bar(&full());
    let mut chords: Vec<&Chord> = all_items(&b)
        .iter()
        .filter_map(|i| i.chord.as_ref())
        .collect();
    let total = chords.len();
    chords.sort_by_key(|c| (c.modifiers, c.key.name().to_string()));
    chords.dedup();
    assert_eq!(chords.len(), total);
}

#[test]
fn standard_chords_display_through_the_active_profile() {
    let b = bar(&full());
    let quit = find(&b.app, "Quit Files").chord.clone().unwrap();
    assert_eq!(quit.display(&Profile::cupertino()).unwrap(), "Super+Q");
    assert_eq!(quit.display(&Profile::classic_pc()).unwrap(), "Ctrl+Q");
    let full_screen = find(&b.menus[2], "Enter Full Screen")
        .chord
        .clone()
        .unwrap();
    assert_eq!(
        full_screen.display(&Profile::cupertino()).unwrap(),
        "Super+Ctrl+F"
    );
}
