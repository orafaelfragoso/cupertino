//! The window's tabs, run headless: Primary+T opens one on the folder shown and the tab bar with it, each tab keeps
//! its own folder, history and view, a tab dragged along the bar moves, and Primary+W closes the tab shown before the
//! window.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::Mods;
use cupertino_kit::theme::{Appearance, ThemePreferences};

const T: u32 = 0x74;
const W: u32 = 0x77;

/// A window on `home/Documents` (`a.txt`, `b.txt`, `Notes/inner.txt`), in the list.
fn run() -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    std::fs::write(documents.join("b.txt"), "bb").unwrap();
    std::fs::write(documents.join("Notes/inner.txt"), "inner").unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
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

/// The tabs, by name, and the one shown.
fn tabs(run: &Run) -> (Vec<String>, Option<String>) {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    // in the order the bar shows them
    let mut tabs: Vec<_> = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::Tab)
        .map(|(_, n)| {
            (
                n.bounds.min_x(),
                n.name.clone().unwrap_or_default(),
                n.states.selected,
            )
        })
        .collect();
    tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tabs: Vec<_> = tabs.into_iter().map(|(_, n, s)| (n, s)).collect();
    let shown = tabs.iter().find(|(_, s)| *s).map(|(n, _)| n.clone());
    (tabs.into_iter().map(|(n, _)| n).collect(), shown)
}

fn names(run: &Run) -> Vec<String> {
    tabs(run).0
}

/// Opens `Notes` in the tab shown, by its row.
fn open_notes(run: &mut Run) {
    run.click_row("Notes");
    run.click_row("Notes");
    run.wait_until(|run| run.title() == "Notes");
}

#[test]
fn primary_t_opens_a_tab_on_the_folder_shown_and_each_tab_keeps_its_own_history() {
    let mut run = run();
    assert!(names(&run).is_empty(), "no tab bar for one tab");
    run.key_with(T, Some('t'), Mods::PRIMARY);
    assert_eq!(
        tabs(&run),
        (
            vec!["Documents".to_owned(), "Documents".to_owned()],
            Some("Documents".to_owned())
        )
    );
    assert!(run.named(Role::TabList).iter().any(|n| n == "Tabs"));

    // the new tab goes into Notes and shows it as icons; the first tab stays where it was
    open_notes(&mut run);
    run.menu("view-icons");
    assert_eq!(names(&run), ["Documents", "Notes"]);
    run.click_named(Role::Tab, "Documents");
    run.wait_until(|run| run.title() == "Documents");
    assert_eq!(
        run.rows(),
        ["a.txt", "b.txt", "Notes"],
        "the first tab's list"
    );
    assert!(
        run.disabled().contains(&common::action("go-back")),
        "the first tab went nowhere"
    );

    // the second tab comes back as it was left: in Notes, as icons, with Back
    run.click_named(Role::Tab, "Notes");
    run.wait_until(|run| run.title() == "Notes");
    run.wait_until(|run| run.icons().len() == 1);
    assert!(!run.disabled().contains(&common::action("go-back")));
    run.menu("go-back");
    run.wait_until(|run| run.title() == "Documents");
    assert_eq!(names(&run), ["Documents", "Documents"]);

    // the menus go through the tabs too
    run.menu("previous-tab");
    assert_eq!(tabs(&run).1.as_deref(), Some("Documents"));
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"]);
}

#[test]
fn a_tab_dragged_along_the_bar_moves() {
    let mut run = run();
    run.menu("new-tab");
    open_notes(&mut run);
    assert_eq!(names(&run), ["Documents", "Notes"]);
    let first = run.bounds(Role::Tab, "Documents");
    let second = run.bounds(Role::Tab, "Notes");
    let to = Point::new(first.min_x() + 4.0, first.center().y);
    run.drag(second.center(), to, Mods::NONE);
    assert_eq!(names(&run), ["Notes", "Documents"]);
    assert_eq!(
        tabs(&run).1.as_deref(),
        Some("Notes"),
        "the tab dragged is shown"
    );
    assert_eq!(run.title(), "Notes");
}

#[test]
fn primary_w_closes_the_tab_shown_and_then_the_window() {
    let mut run = run();
    run.key_with(T, Some('t'), Mods::PRIMARY);
    open_notes(&mut run);
    assert_eq!(run.menu_label("close-tab"), "Close Tab");
    run.key_with(W, Some('w'), Mods::PRIMARY);
    assert!(names(&run).is_empty(), "one tab left: the bar goes");
    assert_eq!(run.title(), "Documents", "the tab before it is shown");
    assert_eq!(run.windows().len(), 1);

    // the tab bar can be shown for one tab, from the View menu
    run.menu("toggle-tab-bar");
    assert_eq!(names(&run), ["Documents"]);
    run.click_named(Role::Button, "New Tab");
    assert_eq!(names(&run), ["Documents", "Documents"]);
    run.menu("close-tab");
    run.key_with(W, Some('w'), Mods::PRIMARY);
    assert!(
        run.driver.runtime().exited(),
        "the last tab closes the window, and with it Files"
    );
}
