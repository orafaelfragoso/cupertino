//! The gallery view, run headless: its toolbar button, its menu item and its keys show it; the first item is shown
//! large over a strip of every item, which the arrows, Home, End and a click move along; the item shown is renamed
//! under its picture; the view reads as a list of the items beside the picture and its preview.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::platform::Mods;
use cupertino_ui_test::{semantic_gaps, whole_tree};

const LEFT: u32 = 0xff51;
const UP: u32 = 0xff52;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;
const RETURN: u32 = 0xff0d;

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

/// The name of the item shown large: the picture read by its name.
fn shown(run: &Run) -> Vec<String> {
    run.named(Role::Image)
}

/// Whether the window reads a preview of `name`.
fn previewed(run: &Run, name: &str) -> bool {
    run.named(Role::Group)
        .iter()
        .any(|g| g == &format!("Preview of {name}"))
}

#[test]
fn the_toolbar_button_shows_the_first_item_large_over_a_strip_of_every_item() {
    let mut run = run();
    run.click_named(Role::RadioButton, "Gallery");
    run.wait_until(|run| run.icons().len() == 3);
    assert_eq!(run.rows(), Vec::<String>::new(), "the table is gone");
    assert_eq!(
        run.icons(),
        [
            ("a.txt".to_owned(), true),
            ("b.txt".to_owned(), false),
            ("Notes".to_owned(), false)
        ],
        "the first item is selected"
    );
    assert_eq!(shown(&run), ["a.txt"]);
    assert!(previewed(&run, "a.txt"), "the preview pane beside it");
    // the pictures of the strip are in one row, under the large one
    let cells = run.icon_cells();
    assert!(cells
        .iter()
        .all(|(_, r, _)| r.min_y() == cells[0].1.min_y()));
    assert!(cells.windows(2).all(|w| w[0].1.max_x() < w[1].1.min_x()));
    let picture = run.bounds(Role::Image, "a.txt");
    assert!(picture.max_y() <= cells[0].1.min_y());
    assert!(picture.height() > 2.0 * cells[0].1.height());
    // the button reads as the view chosen, and every node of the view reads with a role and a name
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    assert!(snapshot.accessibility.iter().any(|(_, n)| {
        n.role == Role::RadioButton && n.name.as_deref() == Some("Gallery") && n.states.selected
    }));
    let mut gaps = semantic_gaps(&whole_tree(
        run.window,
        snapshot.accessibility,
        snapshot.focus,
    ));
    gaps.retain(|gap| {
        !matches!(
            gap.as_str(),
            "Window #0: no name" | "Window #0 > Toolbar #1: no name"
        )
    });
    assert_eq!(gaps, Vec::<String>::new());
}

#[test]
fn primary_4_shows_the_gallery_and_its_menu_item_says_so() {
    let mut run = run();
    assert_eq!(run.menu_label("view-gallery"), "as Gallery");
    run.key_with('4' as u32, Some('4'), Mods::PRIMARY);
    run.wait_until(|run| run.icons().len() == 3);
    assert_eq!(shown(&run), ["a.txt"]);
    // and back
    run.key_with('2' as u32, Some('2'), Mods::PRIMARY);
    run.wait_until(|run| run.rows().len() == 3);
    assert!(shown(&run).is_empty());
}

#[test]
fn the_arrows_home_and_end_move_along_the_strip_and_the_large_picture_follows() {
    let mut run = run();
    run.menu("view-gallery");
    run.wait_until(|run| run.icons().len() == 3);
    run.click_icon("a.txt");
    run.key(RIGHT, None);
    assert_eq!(run.selected_icons(), ["b.txt"]);
    run.wait_until(|run| shown(run) == ["b.txt"]);
    assert!(previewed(&run, "b.txt"));
    // the strip is one row: Up and Down stay
    run.key(UP, None);
    run.key(DOWN, None);
    assert_eq!(run.selected_icons(), ["b.txt"]);
    run.key(END, None);
    assert_eq!(run.selected_icons(), ["Notes"]);
    assert_eq!(shown(&run), ["Notes"]);
    run.key(RIGHT, None);
    assert_eq!(run.selected_icons(), ["Notes"], "past the end it stays");
    run.key(HOME, None);
    assert_eq!(run.selected_icons(), ["a.txt"]);
    run.key(LEFT, None);
    assert_eq!(run.selected_icons(), ["a.txt"]);
    // Shift extends; the large picture shows the item the keyboard is on
    run.key_with(RIGHT, None, Mods::SHIFT);
    assert_eq!(run.selected_icons(), ["a.txt", "b.txt"]);
    assert_eq!(shown(&run), ["b.txt"]);
    // Return opens a folder, which the gallery shows too
    run.key(END, None);
    run.key(RETURN, None);
    run.wait_until(|run| run.title() == "Notes");
    assert_eq!(run.icons(), Vec::<(String, bool)>::new(), "an empty folder");
    assert!(shown(&run).is_empty());
}

#[test]
fn a_click_on_a_picture_of_the_strip_shows_it_and_typing_selects_by_name() {
    let mut run = run();
    run.menu("view-gallery");
    run.wait_until(|run| run.icons().len() == 3);
    run.click_icon("Notes");
    assert_eq!(run.selected_icons(), ["Notes"]);
    assert_eq!(shown(&run), ["Notes"]);
    run.frames(1100, 100);
    run.key('b' as u32, Some('b'));
    assert_eq!(run.selected_icons(), ["b.txt"]);
    assert_eq!(shown(&run), ["b.txt"]);
}

#[test]
fn the_item_shown_is_renamed_under_its_picture() {
    let mut run = run();
    run.menu("view-gallery");
    run.wait_until(|run| run.icons().len() == 3);
    run.click_icon("b.txt");
    run.menu("rename");
    run.wait_until(|run| run.named(Role::TextInput).contains(&"Name".to_owned()));
    let field = run.bounds(Role::TextInput, "Name");
    let picture = run.bounds(Role::Image, "b.txt");
    assert!(
        field.min_y() >= picture.max_y(),
        "{field:?} under {picture:?}"
    );
    // the field opens with the name before the extension selected
    run.type_text("c");
    run.key(RETURN, None);
    run.wait_until(|run| run.home().join("Documents/c.txt").exists());
    run.wait_until(|run| run.selected_icons() == ["c.txt"]);
}
