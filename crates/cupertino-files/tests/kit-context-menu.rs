//! The Kit window's context menus, run headless: what a secondary click on an item, on a selection and on empty space
//! selects and offers, the Menu key, and rows that act.

#![cfg(feature = "app")]

mod common;

use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use common::{place, Run};
use cupertino_files::kit::Observer;
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::{Mods, MouseButton};
use cupertino_kit::theme::{Appearance, ThemePreferences};

const MENU: u32 = 0xff67;

/// A window on `home/Documents` (`Notes`, `a.txt`, `b.txt`), in the list view.
fn run() -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::write(documents.join("b.txt"), "bb").unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let mut run = Run::start(home, documents, places, Notices::start(None), None);
    run.wait_until(|run| run.rows().len() == 3);
    run
}

fn labels(rows: &[(String, bool)]) -> Vec<&str> {
    rows.iter().map(|(label, _)| label.as_str()).collect()
}

fn enabled(rows: &[(String, bool)], label: &str) -> bool {
    rows.iter()
        .find(|(l, _)| l == label)
        .unwrap_or_else(|| panic!("no row {label:?} in {rows:?}"))
        .1
}

/// A point of the list below its rows.
fn empty_space(run: &Run) -> Point {
    let table = run.bounds(Role::Table, &run.title());
    Point::new(table.center().x, table.max_y() - 20.0)
}

#[test]
fn a_secondary_click_on_an_item_selects_it_and_offers_what_it_can_do() {
    let mut run = run();
    run.click_row("b.txt");
    run.right_click_row("a.txt");
    assert_eq!(run.selected_rows(), ["a.txt"]);
    let rows = run.menu_rows();
    for label in ["Open", "Move to Trash", "Rename", "Duplicate", "Make Alias"] {
        assert!(enabled(&rows, label), "{label}: {rows:?}");
    }
    assert!(labels(&rows).contains(&"New Folder with Selection (1 Item)"));
    assert!(!enabled(&rows, "Get Info"), "not offered yet");

    run.choose("Move to Trash");
    assert!(run.context_menu().is_none(), "a chosen row closes the menu");
    run.wait_until(|run| run.rows() == ["b.txt", "Notes"]);
}

#[test]
fn a_secondary_click_in_a_selection_keeps_it_and_names_its_count() {
    let mut run = run();
    run.click_row("a.txt");
    run.click_row_with("b.txt", Mods::SHIFT);
    run.right_click_row("b.txt");
    assert_eq!(run.selected_rows(), ["a.txt", "b.txt"]);
    let rows = run.menu_rows();
    assert!(labels(&rows).contains(&"New Folder with Selection (2 Items)"));
    assert!(!enabled(&rows, "Rename"), "one item at a time");

    run.choose("Duplicate");
    run.wait_until(|run| run.rows().len() == 5);
}

#[test]
fn a_secondary_click_on_empty_space_clears_the_selection_and_offers_the_folder() {
    let mut run = run();
    run.click_row("a.txt");
    let at = empty_space(&run);
    run.press_at(at, MouseButton::Right);
    assert!(run.selected_rows().is_empty());
    let rows = run.menu_rows();
    assert!(enabled(&rows, "New Folder"), "{rows:?}");
    assert!(enabled(&rows, "as Icons"));
    assert!(!enabled(&rows, "as Columns"));
    assert!(!labels(&rows).contains(&"Move to Trash"));

    run.choose("as Icons");
    run.wait_until(|run| run.icons().len() == 3);
}

#[test]
fn new_folder_from_the_folder_menu_makes_one_and_edits_its_name() {
    let mut run = run();
    let at = empty_space(&run);
    run.press_at(at, MouseButton::Right);
    run.choose("New Folder");
    run.wait_until(|run| run.named(Role::TextInput).contains(&"Name".to_owned()));
    assert!(run.home().join("Documents/untitled folder").is_dir());
}

#[test]
fn the_menu_key_opens_the_menu_of_the_selection_or_of_the_folder() {
    let mut run = run();
    run.click_row("a.txt");
    run.key(MENU, None);
    assert!(labels(&run.menu_rows()).contains(&"Move to Trash"));
    run.choose("Open");
    assert!(run.context_menu().is_none());
}

#[test]
fn a_secondary_click_on_an_icon_selects_it() {
    let mut run = run();
    run.menu("view-icons");
    run.wait_until(|run| run.icons().len() == 3);
    let (_, rect, _) = run
        .icon_cells()
        .into_iter()
        .find(|(name, _, _)| name == "Notes")
        .unwrap();
    run.press_at(rect.center(), MouseButton::Right);
    assert_eq!(run.selected_icons(), ["Notes"]);
    let rows = run.menu_rows();
    assert!(enabled(&rows, "Open"));
    assert!(labels(&rows).contains(&"Move to Trash"));
}

/// Into the folder `name` of the list.
fn enter(run: &mut Run, name: &str) {
    run.click_row(name);
    run.key_with(0xff54, None, Mods::PRIMARY);
    run.wait_until(|run| run.title() == name);
}

/// The rows of the folder menu, opened on the list's empty space.
fn folder_menu(run: &mut Run) -> Vec<(String, bool)> {
    let at = empty_space(run);
    run.press_at(at, MouseButton::Right);
    run.menu_rows()
}

#[test]
fn copy_then_paste_item_copies_into_another_folder_under_a_free_name() {
    let mut run = run();
    let documents = run.home().join("Documents");
    assert!(
        !enabled(&folder_menu(&mut run), "Paste Item"),
        "nothing copied yet"
    );
    run.close_menu();
    assert!(run.context_menu().is_none());
    run.right_click_row("a.txt");
    run.choose("Copy “a.txt”");

    enter(&mut run, "Notes");
    assert!(enabled(&folder_menu(&mut run), "Paste Item"));
    run.choose("Paste Item");
    run.wait_until(|run| run.rows() == ["a.txt"]);
    assert_eq!(run.selected_rows(), ["a.txt"], "what arrived is selected");
    assert!(
        documents.join("a.txt").is_file(),
        "a copy leaves the original"
    );

    // again: the folder has the name, so the copy takes a free one
    folder_menu(&mut run);
    run.choose("Paste Item");
    run.wait_until(|run| run.rows().len() == 2);
    let names = std::fs::read_dir(documents.join("Notes")).unwrap().count();
    assert_eq!(names, 2);
}

#[test]
fn move_item_here_moves_and_empties_the_pasteboard() {
    let mut run = run();
    let documents = run.home().join("Documents");
    run.click_row("b.txt");
    run.menu("copy");
    enter(&mut run, "Notes");
    let rows = folder_menu(&mut run);
    assert!(enabled(&rows, "Move Item Here"), "{rows:?}");
    run.choose("Move Item Here");
    run.wait_until(|run| run.rows() == ["b.txt"]);
    assert!(!documents.join("b.txt").exists());
    assert!(run.disabled().contains(&common::action("paste")));
}

#[test]
fn a_folder_is_not_pasted_into_itself() {
    let mut run = run();
    let notes = run.home().join("Documents/Notes");
    run.click_row("Notes");
    // the window's shortcuts: Primary+C, then Primary+V inside the folder
    run.key_with(u32::from('c'), Some('c'), Mods::PRIMARY);
    enter(&mut run, "Notes");
    run.key_with(u32::from('v'), Some('v'), Mods::PRIMARY);
    std::thread::sleep(std::time::Duration::from_millis(100));
    run.settle();
    assert_eq!(
        std::fs::read_dir(&notes).unwrap().count(),
        0,
        "nothing made"
    );
    assert!(run.rows().is_empty());
}

/// A gate a transfer waits at once it copied part of a file, until the test opens it; `reached` hears when it waits.
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn observer(gate: &Arc<Gate>, reached: mpsc::Sender<()>) -> Observer {
        let gate = Arc::clone(gate);
        let reached = Mutex::new(Some(reached));
        Arc::new(move |progress| {
            if progress.bytes == 0 || progress.bytes == progress.total_bytes {
                return;
            }
            if let Some(reached) = reached.lock().unwrap().take() {
                let _ = reached.send(());
            }
            let open = gate.open.lock().unwrap();
            let _open = gate
                .opened
                .wait_timeout_while(open, Duration::from_secs(10), |open| !*open)
                .unwrap();
        })
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

#[test]
fn a_large_paste_runs_beside_the_window_shows_its_progress_and_stops_without_a_half_copy() {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    // four chunks of the copy, so it can stop between two of them
    let big = vec![5u8; 4 << 20];
    std::fs::write(documents.join("big.bin"), &big).unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let gate = Arc::new(Gate {
        open: Mutex::new(false),
        opened: Condvar::new(),
    });
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let mut run = Run::start_configured(
        home,
        documents.clone(),
        places,
        Notices::start(None),
        None,
        preferences,
        |start| start.with_transfer_observer(observer),
    );
    run.wait_until(|run| run.rows().len() == 2);
    run.click_row("big.bin");
    run.menu("copy");
    enter(&mut run, "Notes");
    run.menu("paste");
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");

    // the copy waits at the gate on its own thread: the window still answers and shows what it does
    run.wait_until(|run| !run.named(Role::ProgressIndicator).is_empty());
    assert_eq!(
        run.named(Role::ProgressIndicator),
        ["Copying “big.bin” to “Notes”"]
    );
    assert!(run.named(Role::Button).contains(&"Stop".to_owned()));
    run.menu("toggle-status-bar");
    assert!(
        run.read_aloud().iter().any(|t| t.contains("item")),
        "the window answers"
    );

    run.click_named(Role::Button, "Stop");
    gate.release();
    run.wait_until(|run| run.named(Role::ProgressIndicator).is_empty());
    let left: Vec<_> = std::fs::read_dir(documents.join("Notes"))
        .unwrap()
        .collect();
    assert!(
        left.is_empty(),
        "a stopped copy leaves nothing half written: {left:?}"
    );
    assert_eq!(std::fs::read(documents.join("big.bin")).unwrap(), big);
    assert!(run.rows().is_empty());
}

#[test]
fn a_paste_that_runs_to_its_end_takes_its_row_away_and_selects_what_arrived() {
    let mut run = run();
    run.click_row("b.txt");
    run.menu("copy");
    enter(&mut run, "Notes");
    run.menu("paste");
    run.wait_until(|run| run.selected_rows() == ["b.txt"]);
    run.wait_until(|run| run.named(Role::ProgressIndicator).is_empty());
}
