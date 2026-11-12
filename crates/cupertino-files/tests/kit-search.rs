//! The Kit window's search, run headless: "No Results" after a search that matches nothing and only then, and the
//! search field's width while it rests, has the keyboard, holds text, or both, animated or at once with reduce motion.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::search_field::{open_width, REST};
use cupertino_kit::core::Role;
use cupertino_kit::theme::{Appearance, ThemePreferences};

const ESCAPE: u32 = 0xff1b;

/// A window on `home/Documents` (`Notes`, `a.txt`, `b.txt`) with an empty `Downloads` in the sidebar.
fn run(reduce_motion: bool) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    let downloads = home.path().join("Downloads");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(documents.join("b.txt"), "bb").unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    let places = vec![
        place("Documents", &documents, PlaceKind::Documents),
        place("Downloads", &downloads, PlaceKind::Downloads),
    ];
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion,
    };
    let mut run = Run::start_with(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
    );
    run.wait_until(|run| run.rows().len() == 3);
    run
}

fn says_no_results(run: &Run) -> bool {
    run.named(Role::StaticText)
        .iter()
        .any(|text| text == "No Results")
}

fn field_width(run: &Run) -> f64 {
    run.bounds(Role::TextInput, "Search").size.width
}

#[test]
fn no_results_shows_only_after_a_search_that_matches_nothing() {
    let mut run = run(false);
    assert!(!says_no_results(&run), "a folder with items");
    run.click("search");
    run.type_text("zzz");
    run.wait_until(|run| run.rows().is_empty());
    assert!(says_no_results(&run), "a search that matched nothing");
    run.driver
        .look(run.window, "files-search-no-results")
        .unwrap();

    // a match comes back: the message goes
    for _ in 0..3 {
        run.key(0xff08, None);
    }
    run.type_text("a.t");
    run.wait_until(|run| run.rows() == ["a.txt"]);
    assert!(!says_no_results(&run));

    // a file that matches is made while the search shows nothing
    run.type_text("q");
    run.wait_until(|run| run.rows().is_empty());
    assert!(says_no_results(&run));
    std::fs::write(run.home().join("Documents/a.tq"), "").unwrap();
    run.wait_until(|run| run.rows() == ["a.tq"]);
    assert!(!says_no_results(&run));

    // the field is cleared: the folder is back and the message gone
    run.type_text("zz");
    run.wait_until(|run| run.rows().is_empty());
    run.key(ESCAPE, None);
    run.wait_until(|run| run.rows().len() == 4);
    assert!(!says_no_results(&run));
    assert_eq!(run.named(Role::TextInput), ["Search"]);
}

#[test]
fn an_empty_folder_without_a_search_says_nothing() {
    let mut run = run(false);
    run.click("place-Downloads");
    assert_eq!(run.title(), "Downloads");
    assert_eq!(run.rows(), Vec::<String>::new());
    assert!(!says_no_results(&run));
}

#[test]
fn no_results_reads_the_same_in_the_icon_view() {
    let mut run = run(false);
    run.menu("view-icons");
    run.click("search");
    run.type_text("zzz");
    run.wait_until(|run| run.icons().is_empty() && says_no_results(run));
    run.key(ESCAPE, None);
    run.wait_until(|run| run.icons().len() == 3);
    assert!(!says_no_results(&run));
}

#[test]
fn the_field_grows_while_it_has_the_keyboard_or_text_and_never_over_its_neighbours() {
    let mut run = run(false);
    let open = open_width(run.bounds(Role::Table, "Documents").size.width);
    assert!(
        open > REST * 2.0,
        "the default window leaves the field room: {open}"
    );
    assert_eq!(field_width(&run), REST, "resting");

    // focused and empty: it eases out over 0.2 s
    run.click("search");
    run.frames(100, 16);
    let midway = field_width(&run);
    assert!(midway > REST && midway < open, "half way: {midway}");
    assert!(
        midway > REST + (open - REST) / 2.0,
        "an ease-out is past half way at half the time: {midway}"
    );
    run.frames(150, 16);
    assert_eq!(field_width(&run), open, "focused and empty");

    // focused with text, a query longer than the field that one item matches
    const LONG: &str = "a long query that runs past the field";
    std::fs::write(run.home().join(format!("Documents/{LONG}.txt")), "").unwrap();
    run.type_text(LONG);
    run.wait_until(|run| run.rows() == [format!("{LONG}.txt")]);
    run.frames(250, 16);
    assert_eq!(field_width(&run), open, "focused with text");
    run.driver
        .look(run.window, "files-search-long-query")
        .unwrap();

    // unfocused with text: the list takes the keyboard
    run.click_row(&format!("{LONG}.txt"));
    run.frames(250, 16);
    assert_eq!(field_width(&run), open, "unfocused with text");

    // never over the forward button or the title
    let field = run.bounds(Role::TextInput, "Search");
    let forward = run.bounds(Role::Button, "Forward");
    let title = run.bounds(Role::StaticText, "Documents");
    assert!(field.min_x() > forward.max_x(), "{field:?} {forward:?}");
    assert!(field.min_x() >= title.min_x(), "{field:?} {title:?}");

    // cleared and unfocused: back to rest
    run.click("search");
    run.key(ESCAPE, None);
    run.click_row("a.txt");
    run.frames(250, 16);
    assert_eq!(field_width(&run), REST, "resting again");
}

#[test]
fn with_reduce_motion_the_field_changes_width_at_once() {
    let mut run = run(true);
    run.click("search");
    run.frames(16, 16);
    let open = open_width(run.bounds(Role::Table, "Documents").size.width);
    assert_eq!(field_width(&run), open);
    run.click_row("a.txt");
    run.frames(16, 16);
    assert_eq!(field_width(&run), REST);
}
