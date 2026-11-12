//! The column view, run headless: the chain of folders with Right and Left, a row of another column choosing its
//! folder, typing to select in the column shown, the preview of a selected file at the end of the chain, and a
//! column's divider resizing it.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::window::ViewMode;
use cupertino_files::window_prefs::WindowPrefs;
use cupertino_kit::core::Role;
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::platform::{Mods, WindowStates};
use cupertino_kit::theme::{Appearance, ThemePreferences};

const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;

/// A window in the column view on `home/Documents` (`Notes` holding `inner.txt`, `a.txt`, `b.txt`).
fn run() -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::write(documents.join("Notes/inner.txt"), "inside\n").unwrap();
    std::fs::write(documents.join("a.txt"), "alpha\n").unwrap();
    std::fs::write(documents.join("b.txt"), "bravo\n").unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let prefs = prefs_file(home.path());
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let mut run = Run::start_configured(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        move |start| start.with_prefs_file(prefs).with_view(ViewMode::Columns),
    );
    // wide enough for two columns and the preview
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    run.driver
        .window_system_mut()
        .configure(run.window, Some(Size::new(1200.0, 600.0)), active)
        .unwrap();
    run.wait_until(|run| texts(run).iter().any(|t| t == "b.txt"));
    run
}

/// Where the window keeps what new windows start with, under `home`.
fn prefs_file(home: &std::path::Path) -> std::path::PathBuf {
    home.join("config/cupertino/files/window.json")
}

/// The texts the window shows, in tree order.
fn texts(run: &Run) -> Vec<String> {
    run.named(Role::StaticText)
}

/// The names of the lists the window shows: one a column, named after its folder.
fn columns(run: &Run) -> Vec<String> {
    let mut lists = run.named(Role::List);
    lists.retain(|name| name != "Sidebar");
    lists
}

/// Click the text `name` furthest along the chain.
fn click_text(run: &mut Run, name: &str) {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let at = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::StaticText && n.name.as_deref() == Some(name))
        .map(|(_, n)| n.bounds.center())
        .max_by(|a, b| a.x.total_cmp(&b.x))
        .unwrap_or_else(|| panic!("no text {name:?}"));
    run.press_at(at, cupertino_kit::platform::MouseButton::Left);
}

#[test]
fn right_goes_into_the_selected_folder_and_left_comes_back() {
    let mut run = run();
    assert_eq!(columns(&run), ["Documents"]);
    click_text(&mut run, "Notes");
    run.wait_until(|run| texts(run).iter().any(|t| t == "inner.txt"));
    assert_eq!(
        columns(&run),
        ["Documents", "Notes"],
        "the folder's items follow"
    );

    run.key(RIGHT, None);
    run.wait_until(|run| run.title() == "Notes");
    assert_eq!(columns(&run), ["Documents", "Notes"]);
    assert!(
        run.named(Role::Group)
            .iter()
            .any(|g| g == "Preview of inner.txt"),
        "the first item is selected and previewed"
    );

    run.key(LEFT, None);
    let previewed = |run: &Run| {
        run.named(Role::Group)
            .iter()
            .any(|g| g.starts_with("Preview of"))
    };
    run.wait_until(|run| !previewed(run));
    assert_eq!(
        columns(&run),
        ["Documents", "Notes"],
        "the folder left stays selected"
    );
    assert_eq!(run.title(), "Notes", "the title is the folder selected");
    // no Left past the folder the chain starts at
    run.key(LEFT, None);
    assert_eq!(columns(&run), ["Documents", "Notes"]);
    assert_eq!(run.title(), "Notes");
}

#[test]
fn the_title_follows_the_folder_selected_in_a_column() {
    let mut run = run();
    assert_eq!(run.title(), "Documents");
    click_text(&mut run, "Notes");
    run.wait_until(|run| run.title() == "Notes");
    // the toolbar's title, in its row, follows the window's
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    assert!(snapshot.accessibility.iter().any(|(_, n)| {
        n.role == Role::StaticText && n.name.as_deref() == Some("Notes") && n.bounds.max_y() <= 52.0
    }));

    // a file selected in the first column: its folder is the title again
    click_text(&mut run, "b.txt");
    run.wait_until(|run| run.title() == "Documents");
    assert_eq!(columns(&run), ["Documents"]);
}

#[test]
fn a_row_of_another_column_shows_its_folder_and_typing_selects_in_the_column_shown() {
    let mut run = run();
    click_text(&mut run, "Notes");
    run.wait_until(|run| texts(run).iter().any(|t| t == "inner.txt"));
    click_text(&mut run, "inner.txt");
    run.wait_until(|run| run.title() == "Notes");
    assert!(run
        .named(Role::Group)
        .iter()
        .any(|g| g == "Preview of inner.txt"));

    // back in the first column, by its row
    click_text(&mut run, "b.txt");
    run.wait_until(|run| run.title() == "Documents");
    assert!(
        run.named(Role::Group)
            .iter()
            .any(|g| g == "Preview of b.txt"),
        "{:?} {:?}",
        run.named(Role::Group),
        columns(&run)
    );
    assert_eq!(columns(&run), ["Documents"]);

    run.type_text("a");
    assert!(run
        .named(Role::Group)
        .iter()
        .any(|g| g == "Preview of a.txt"));
    // the preview reads the text file's start, away from the window's thread
    run.wait_until(|run| texts(run).iter().any(|t| t.starts_with("alpha")));
}

#[test]
fn a_columns_divider_resizes_it() {
    let mut run = run();
    click_text(&mut run, "Notes");
    run.wait_until(|run| columns(run).len() == 2);
    let list = run.bounds(Role::List, "Documents");
    assert!((list.size.width - 244.0).abs() < 1.0, "{list:?}");
    let edge = list.max_x() + 0.5;
    let y = list.min_y() + 60.0;
    let prefs = prefs_file(&run.home());
    let kept = || WindowPrefs::load(&prefs).column_width;
    let before = kept();
    run.hold(Point::new(edge, y), Point::new(edge + 40.0, y), Mods::NONE);
    let wider = run.bounds(Role::List, "Documents");
    assert!((wider.size.width - 284.0).abs() < 1.0, "{wider:?}");
    assert_eq!(kept(), before, "nothing is written while the divider moves");
    run.release(Point::new(edge + 40.0, y), Mods::NONE);
    assert!(
        (kept() - (before + 40.0)).abs() < 1.0,
        "the width the divider left is kept once let go"
    );
}
