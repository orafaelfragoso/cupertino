//! The Kit window's context menus, run headless: what a secondary click on an item, on a selection and on empty space
//! selects and offers, the Menu key, and rows that act.

#![cfg(feature = "app")]

mod common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::{Mods, MouseButton};

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
    let table = run.bounds(Role::Table, "Documents");
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
