//! The open panel of the panes (`chooser::Chooser`) on real folders: what each mode lists and chooses, what the
//! panel's buttons, its places, its rows and Return and Escape do, and how the folders it asks for come back.
#![allow(
    clippy::disallowed_methods,
    reason = "a test makes its folders and reads the panel's listings on its own thread"
)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cupertino_kit::controls::Selection;
use cupertino_kit::filepicker::Mode;
use cupertino_kit::files::panel::PanelEvent;
use cupertino_kit::files::places::{Place, PlaceGroup, PlaceKind};
use cupertino_kit::files::ViewEvent;
use cupertino_settings::chooser::{Chooser, Event, Outcome};

/// A folder of the test's own: `Pictures` (two pictures, a text file and the folder `Trips`), `bin` (a program and a
/// file that is not one) and the file `notes.txt`.
fn folders(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("chooser-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("Pictures/Trips")).unwrap();
    fs::create_dir_all(root.join("bin")).unwrap();
    for file in [
        "Pictures/a.png",
        "Pictures/b.jpg",
        "Pictures/c.txt",
        "bin/readme",
        "notes.txt",
    ] {
        fs::write(root.join(file), b"").unwrap();
    }
    fs::write(root.join("bin/tool"), b"\x7fELF").unwrap();
    fs::set_permissions(root.join("bin/tool"), fs::Permissions::from_mode(0o755)).unwrap();
    root
}

/// Read every folder the panel asks for, as the window's tasks do.
fn settle(chooser: &mut Chooser) {
    while let Some(request) = chooser.take_request() {
        chooser.apply(request.run());
    }
}

/// The row named `name`.
fn row(chooser: &Chooser, name: &str) -> usize {
    (chooser.entries().iter())
        .position(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
}

fn names(chooser: &Chooser) -> Vec<String> {
    let mut names: Vec<String> = chooser.entries().iter().map(|e| e.name.clone()).collect();
    names.sort();
    names
}

/// A case: the mode, where it opens, the steps and what each decided, the folder it shows at the end.
type Case = (Mode, &'static str, Vec<(Step, Outcome)>, &'static str);

/// What a step of a case does.
enum Step {
    /// The person selects the row of this name.
    Select(&'static str),
    /// The person double-clicks the row of this name.
    Open(&'static str),
    Event(Event),
}

fn take(chooser: &mut Chooser, step: Step) -> Outcome {
    let event = match step {
        Step::Select(name) => {
            let i = row(chooser, name);
            Event::List(ViewEvent::Select(Selection::new(
                std::iter::once(i..i + 1),
                Some(i),
                Some(i),
            )))
        }
        Step::Open(name) => Event::List(ViewEvent::Open(row(chooser, name))),
        Step::Event(event) => event,
    };
    let outcome = chooser.update(event);
    settle(chooser);
    outcome
}

#[test]
fn each_mode_lists_its_items_and_chooses_its_kind() {
    let root = folders("modes");
    // (mode, where it opens, the items it lists, what Choose gives with nothing selected)
    let cases: [(Mode, &str, &[&str], Option<&str>); 3] = [
        (Mode::Folder, "", &["Pictures", "bin"], Some("")),
        (Mode::Photo, "Pictures", &["Trips", "a.png", "b.jpg"], None),
        (Mode::Program, "bin", &["tool"], None),
    ];
    for (mode, start, listed, nothing) in cases {
        let mut chooser = Chooser::new(&root.join(start), mode, Vec::new());
        assert!(
            chooser.entries().is_empty(),
            "{mode:?} lists nothing before its folder is read"
        );
        settle(&mut chooser);
        assert_eq!(chooser.dir(), root.join(start), "{mode:?}");
        assert_eq!(names(&chooser), listed.to_vec(), "{mode:?}");
        assert_eq!(
            chooser.choice(),
            nothing.map(|n| root.join(start).join(n).components().collect()),
            "{mode:?}"
        );
    }
}

#[test]
fn the_rows_buttons_and_keys_choose_open_and_cancel() {
    let root = folders("steps");
    let chosen = |path: &str| Outcome::Chosen(root.join(path));
    // (mode, where it opens, the steps and what each decided, the folder it shows at the end)
    let cases: Vec<Case> = vec![
        // a folder chooser chooses the folder it shows, or the selected one, by button or Return
        (
            Mode::Folder,
            "",
            vec![(Step::Event(Event::Panel(PanelEvent::Accept)), chosen(""))],
            "",
        ),
        (
            Mode::Folder,
            "",
            vec![
                (Step::Select("Pictures"), Outcome::Stay),
                (Step::Event(Event::Return), chosen("Pictures")),
            ],
            "",
        ),
        // a double click opens a folder, in every mode
        (
            Mode::Folder,
            "",
            vec![
                (Step::Open("Pictures"), Outcome::Stay),
                (
                    Step::Event(Event::Panel(PanelEvent::Accept)),
                    chosen("Pictures"),
                ),
            ],
            "Pictures",
        ),
        // a picture chooser chooses nothing without a picture, opens a folder on Return and chooses a picture
        (
            Mode::Photo,
            "Pictures",
            vec![
                (Step::Event(Event::Panel(PanelEvent::Accept)), Outcome::Stay),
                (Step::Event(Event::Return), Outcome::Stay),
                (Step::Select("b.jpg"), Outcome::Stay),
                (Step::Event(Event::Return), chosen("Pictures/b.jpg")),
            ],
            "Pictures",
        ),
        (
            Mode::Photo,
            "Pictures",
            vec![
                (Step::Select("Trips"), Outcome::Stay),
                (Step::Event(Event::Return), Outcome::Stay),
            ],
            "Pictures/Trips",
        ),
        (
            Mode::Photo,
            "Pictures",
            vec![(Step::Open("a.png"), chosen("Pictures/a.png"))],
            "Pictures",
        ),
        // a program chooser chooses a program
        (
            Mode::Program,
            "bin",
            vec![
                (Step::Select("tool"), Outcome::Stay),
                (
                    Step::Event(Event::Panel(PanelEvent::Accept)),
                    chosen("bin/tool"),
                ),
            ],
            "bin",
        ),
        // Cancel and Escape end it, whatever is selected
        (
            Mode::Photo,
            "Pictures",
            vec![
                (Step::Select("a.png"), Outcome::Stay),
                (
                    Step::Event(Event::Panel(PanelEvent::Cancel)),
                    Outcome::Cancel,
                ),
            ],
            "Pictures",
        ),
        (
            Mode::Folder,
            "",
            vec![(Step::Event(Event::Cancel), Outcome::Cancel)],
            "",
        ),
    ];
    for (i, (mode, start, steps, end)) in cases.into_iter().enumerate() {
        let mut chooser = Chooser::new(&root.join(start), mode, Vec::new());
        settle(&mut chooser);
        for (n, (step, decided)) in steps.into_iter().enumerate() {
            assert_eq!(take(&mut chooser, step), decided, "case {i}, step {n}");
        }
        assert_eq!(chooser.dir(), root.join(end), "case {i}");
    }
}

#[test]
fn places_and_the_enclosing_folder_move_it_and_a_stale_listing_is_dropped() {
    let root = folders("moves");
    let places = vec![
        Place {
            name: "Home".into(),
            path: root.clone(),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Home,
        },
        Place {
            name: "Pictures".into(),
            path: root.join("Pictures"),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Pictures,
        },
    ];
    let mut chooser = Chooser::new(&root.join("Pictures/Trips"), Mode::Folder, places);
    settle(&mut chooser);
    assert_eq!(chooser.place(), None);

    // the enclosing folder opens with the folder that was left selected, which Choose then gives
    take(&mut chooser, Step::Event(Event::Panel(PanelEvent::Parent)));
    assert_eq!(
        (chooser.dir(), chooser.place()),
        (root.join("Pictures").as_path(), Some(1))
    );
    assert_eq!(chooser.choice(), Some(root.join("Pictures/Trips")));

    // a place opens its folder, nothing selected
    take(
        &mut chooser,
        Step::Event(Event::Panel(PanelEvent::Place(0))),
    );
    assert_eq!((chooser.dir(), chooser.place()), (root.as_path(), Some(0)));
    assert_eq!(chooser.choice(), Some(root.clone()));

    // a folder asked for and then left before it was read keeps the one asked for last
    chooser.update(Event::Panel(PanelEvent::Place(1)));
    let stale = chooser.take_request().unwrap();
    chooser.update(Event::Panel(PanelEvent::Parent));
    let before = chooser.listings();
    chooser.apply(stale.run());
    assert_eq!(
        (chooser.dir(), chooser.listings()),
        (root.as_path(), before)
    );
    settle(&mut chooser);
    assert_eq!(chooser.dir(), root.parent().unwrap());
}

#[test]
fn a_folder_that_cannot_be_read_leaves_the_open_one_and_says_so() {
    let root = folders("unreadable");
    // where it opens is gone: the nearest folder that exists opens
    let mut chooser = Chooser::new(&root.join("Pictures/gone/deeper"), Mode::Photo, Vec::new());
    settle(&mut chooser);
    assert_eq!(
        (chooser.dir(), chooser.unreadable()),
        (root.join("Pictures").as_path(), false)
    );

    let places = vec![Place {
        name: "Notes".into(),
        path: root.join("notes.txt"),
        group: PlaceGroup::Favorites,
        kind: PlaceKind::Documents,
    }];
    let mut chooser = Chooser::new(&root, Mode::Folder, places);
    settle(&mut chooser);
    take(
        &mut chooser,
        Step::Event(Event::Panel(PanelEvent::Place(0))),
    );
    assert_eq!(
        (chooser.dir(), chooser.unreadable()),
        (root.as_path(), true)
    );
    assert_eq!(names(&chooser), vec!["Pictures", "bin"]);
}
