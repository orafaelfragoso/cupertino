//! The standard App, File, Edit and View menus.

use cupertino_keyboard::modifiers::Modifier;
use cupertino_menu_model::standard::{menu_bar, Features};
use cupertino_menu_model::{Chord, Command, Entry, Item, Menu, MenuBar, Role, SystemCommand};
use Modifier::{Alt, Ctrl, Primary, Shift};

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
    std::iter::once(&bar.app)
        .chain(&bar.menus)
        .flat_map(items)
        .collect()
}

fn labels(menu: &Menu) -> Vec<String> {
    items(menu).iter().map(|i| i.label.clone()).collect()
}

/// An item: its label, its system command or else its action, and its chord modifiers and key ("" for none).
type ItemRow<'a> = (
    &'a str,
    Option<&'a Command>,
    &'a str,
    &'a [Modifier],
    &'a str,
);

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

#[test]
fn every_standard_item_has_its_command_and_chord() {
    let b = menu_bar("Files", &full());
    let on = menu_bar(
        "Files",
        &Features {
            full_screen: true,
            ..full()
        },
    );
    let mut every = all_items(&b);
    every.extend(items(&on.menus[2]));
    let hide = Command::System(SystemCommand::Hide);
    let hide_others = Command::System(SystemCommand::HideOthers);
    let show_all = Command::System(SystemCommand::ShowAll);
    let rows: [ItemRow; 26] = [
        ("About Files", None, "app.about", &[], ""),
        ("Settings…", None, "app.preferences", &[Primary], "comma"),
        ("Hide Files", Some(&hide), "", &[Primary], "h"),
        ("Hide Others", Some(&hide_others), "", &[Primary, Alt], "h"),
        ("Show All", Some(&show_all), "", &[], ""),
        ("Quit Files", None, "app.quit", &[Primary], "q"),
        ("New Window", None, "app.new", &[Primary], "n"),
        ("Open…", None, "app.open", &[Primary], "o"),
        ("Close Window", None, "win.close", &[Primary], "w"),
        ("Save", None, "win.save", &[Primary], "s"),
        ("Save As…", None, "win.save-as", &[Shift, Primary], "s"),
        ("Undo", None, "win.undo", &[Primary], "z"),
        ("Redo", None, "win.redo", &[Shift, Primary], "z"),
        ("Cut", None, "win.cut", &[Primary], "x"),
        ("Copy", None, "win.copy", &[Primary], "c"),
        ("Paste", None, "win.paste", &[Primary], "v"),
        ("Delete", None, "win.delete", &[], ""),
        ("Select All", None, "win.select-all", &[Primary], "a"),
        ("Find…", None, "win.find", &[Primary], "f"),
        ("Find Next", None, "win.find-next", &[Primary], "g"),
        (
            "Find Previous",
            None,
            "win.find-previous",
            &[Shift, Primary],
            "g",
        ),
        (
            "Use Selection for Find",
            None,
            "win.use-selection-for-find",
            &[Primary],
            "e",
        ),
        (
            "Hide Toolbar",
            None,
            "win.toggle-toolbar",
            &[Primary, Alt],
            "t",
        ),
        (
            "Show Sidebar",
            None,
            "win.toggle-sidebar",
            &[Primary, Alt],
            "s",
        ),
        (
            "Enter Full Screen",
            None,
            "win.fullscreen",
            &[Primary, Ctrl],
            "f",
        ),
        (
            "Exit Full Screen",
            None,
            "win.fullscreen",
            &[Primary, Ctrl],
            "f",
        ),
    ];
    for (label, system, action, modifiers, key) in rows {
        let item = every
            .iter()
            .find(|i| i.label == label)
            .unwrap_or_else(|| panic!("no item {label:?}"));
        match system {
            Some(command) => assert_eq!(&item.command, command, "{label}"),
            None => match &item.command {
                Command::Action(a) => assert_eq!(a.full(), action, "{label}"),
                Command::System(_) => panic!("{label} is a system command"),
            },
        }
        let chord = (!key.is_empty()).then(|| Chord::new(modifiers, key).unwrap());
        assert_eq!(item.chord, chord, "{label}");
    }
    let find = b.menus[1]
        .sections
        .iter()
        .flatten()
        .find_map(|e| match e {
            Entry::Submenu(m) => Some(m),
            Entry::Item(_) => None,
        })
        .expect("a Find submenu");
    assert_eq!(find.title, "Find");
}

#[test]
fn features_add_their_items_and_unhandled_ones_are_omitted() {
    let rows: [(&str, &str, Features, [&[&str]; 4]); 6] = [
        (
            "every application",
            "Files",
            Features::default(),
            [
                &["Hide Files", "Hide Others", "Show All", "Quit Files"],
                &["Close Window"],
                &[
                    "Undo",
                    "Redo",
                    "Cut",
                    "Copy",
                    "Paste",
                    "Delete",
                    "Select All",
                ],
                &["Enter Full Screen"],
            ],
        ),
        (
            "about and settings, named for the application",
            "Notes",
            Features {
                about: true,
                settings: true,
                ..Features::default()
            },
            [
                &[
                    "About Notes",
                    "Settings…",
                    "Hide Notes",
                    "Hide Others",
                    "Show All",
                    "Quit Notes",
                ],
                &["Close Window"],
                &[
                    "Undo",
                    "Redo",
                    "Cut",
                    "Copy",
                    "Paste",
                    "Delete",
                    "Select All",
                ],
                &["Enter Full Screen"],
            ],
        ),
        (
            "new labelled by the application",
            "Files",
            Features {
                new: Some("New Folder".to_string()),
                ..Features::default()
            },
            [
                &["Hide Files", "Hide Others", "Show All", "Quit Files"],
                &["New Folder", "Close Window"],
                &[
                    "Undo",
                    "Redo",
                    "Cut",
                    "Copy",
                    "Paste",
                    "Delete",
                    "Select All",
                ],
                &["Enter Full Screen"],
            ],
        ),
        (
            "toolbar and sidebar name what the next click does",
            "Files",
            Features {
                toolbar: Some(false),
                sidebar: Some(true),
                ..Features::default()
            },
            [
                &["Hide Files", "Hide Others", "Show All", "Quit Files"],
                &["Close Window"],
                &[
                    "Undo",
                    "Redo",
                    "Cut",
                    "Copy",
                    "Paste",
                    "Delete",
                    "Select All",
                ],
                &["Show Toolbar", "Hide Sidebar", "Enter Full Screen"],
            ],
        ),
        (
            "full screen window",
            "Files",
            Features {
                full_screen: true,
                ..Features::default()
            },
            [
                &["Hide Files", "Hide Others", "Show All", "Quit Files"],
                &["Close Window"],
                &[
                    "Undo",
                    "Redo",
                    "Cut",
                    "Copy",
                    "Paste",
                    "Delete",
                    "Select All",
                ],
                &["Exit Full Screen"],
            ],
        ),
        (
            "everything",
            "Files",
            full(),
            [
                &[
                    "About Files",
                    "Settings…",
                    "Hide Files",
                    "Hide Others",
                    "Show All",
                    "Quit Files",
                ],
                &["New Window", "Open…", "Close Window", "Save", "Save As…"],
                &[
                    "Undo",
                    "Redo",
                    "Cut",
                    "Copy",
                    "Paste",
                    "Delete",
                    "Select All",
                    "Find…",
                    "Find Next",
                    "Find Previous",
                    "Use Selection for Find",
                ],
                &["Hide Toolbar", "Show Sidebar", "Enter Full Screen"],
            ],
        ),
    ];
    for (name, app, features, expected) in rows {
        let b = menu_bar(app, &features);
        assert_eq!(b.app.title, app, "{name}");
        let titles: Vec<&str> = b.menus.iter().map(|m| m.title.as_str()).collect();
        assert_eq!(titles, ["File", "Edit", "View"], "{name}");
        let got: Vec<Vec<String>> = std::iter::once(&b.app)
            .chain(&b.menus)
            .map(labels)
            .collect();
        assert_eq!(got, expected.map(|m| m.to_vec()), "{name}");
    }
}

#[test]
fn the_standard_menus_hold_no_empty_groups() {
    for features in [Features::default(), full()] {
        let b = menu_bar("Files", &features);
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
    let b = menu_bar("Files", &full());
    let out = b.exported();
    assert_eq!(
        labels(&out.app),
        vec!["About Files", "Settings…", "Quit Files"]
    );
    assert_eq!(out.menus, b.menus);
}

#[test]
fn the_standard_items_carry_every_role_and_no_two_share_a_chord() {
    let b = menu_bar("Files", &full());
    let mut roles: Vec<Role> = all_items(&b).iter().filter_map(|i| i.role()).collect();
    roles.sort_by_key(|r| Role::ALL.iter().position(|x| x == r));
    assert_eq!(roles, Role::ALL.to_vec());

    let mut chords: Vec<&Chord> = all_items(&b)
        .iter()
        .filter_map(|i| i.chord.as_ref())
        .collect();
    let total = chords.len();
    chords.sort_by_key(|c| (c.modifiers, c.key.name().to_string()));
    chords.dedup();
    assert_eq!(chords.len(), total);
}
