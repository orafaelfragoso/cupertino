//! Tags, run headless: the toolbar's Tags button edits the selection's tags in a popover, a colour by its row and a
//! tag of any name typed in its field, kept in the items' extended attributes; the sidebar lists the tags in use, one
//! chosen shows the folder's items that carry it, and the search finds an item by a tag's name.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::tags::{read_tags, write_tags};
use cupertino_kit::core::Role;
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, Timestamp, WindowEvent, WindowId};
use cupertino_kit::theme::{Appearance, ThemePreferences};

const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;

/// A window on `home/Documents` (`a.txt`, `b.txt`, `Notes`), with `b.txt` tagged `Project`; none on a file system
/// without extended attributes.
fn run() -> Option<Run> {
    // the build's own directory: a temporary file system may keep no extended attributes
    let home = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    std::fs::write(documents.join("b.txt"), "bb").unwrap();
    if write_tags(&documents.join("b.txt"), &["Project".to_owned()]).is_err() {
        eprintln!("SKIPPED: no extended attributes here");
        return None;
    }
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
    Some(run)
}

/// The tags popover, when open.
fn popover(run: &Run) -> Option<WindowId> {
    let windows = run.driver.runtime().windows();
    windows
        .into_iter()
        .find(|(key, _)| format!("{key:?}").contains("tags/"))
        .map(|(_, id)| id)
}

/// Runs `act` on the popover, then goes back to the browser window.
fn in_popover(run: &mut Run, act: impl FnOnce(&mut Run)) {
    let browser = run.window;
    run.window = popover(run).expect("the tags popover");
    act(run);
    run.window = browser;
}

/// The tag rows of the popover, by name, and whether each is checked.
fn tag_rows(run: &Run) -> Vec<(String, bool)> {
    let window = popover(run).expect("the tags popover");
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    let mut rows: Vec<_> = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::CheckBox)
        .map(|(_, n)| {
            let on = format!("{:?}", n.value).contains("Toggle(True)");
            (n.bounds.min_y(), n.name.clone().unwrap_or_default(), on)
        })
        .collect();
    rows.sort_by(|a, b| a.0.total_cmp(&b.0));
    rows.into_iter().map(|(_, n, on)| (n, on)).collect()
}

#[test]
fn the_popover_puts_a_colour_and_a_typed_tag_on_the_selection() {
    let Some(mut run) = run() else { return };
    let a = run.home().join("Documents/a.txt");
    run.click_row("a.txt");
    run.click_named(Role::PopUpButton, "Tags");
    assert!(popover(&run).is_some(), "Tags opens its popover");
    let rows = tag_rows(&run);
    let names: Vec<&str> = rows.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray", "Project"],
        "the colours, then the tags in use"
    );
    assert!(rows.iter().all(|(_, on)| !on), "{rows:?}");

    // a colour's row puts it on, and the row shows it
    in_popover(&mut run, |run| run.click_named(Role::CheckBox, "Red"));
    assert_eq!(read_tags(&a).unwrap(), ["Red"]);
    run.wait_until(|run| tag_rows(run)[0] == ("Red".to_owned(), true));

    // a name typed in the field and Return puts it on too
    in_popover(&mut run, |run| {
        run.type_text("Errands");
        run.key(RETURN, None);
    });
    assert_eq!(read_tags(&a).unwrap(), ["Red", "Errands"]);
    // a comma parts two tags, as the attribute keeps them
    in_popover(&mut run, |run| {
        run.type_text("Trip, Taxes");
        run.key(RETURN, None);
    });
    assert_eq!(read_tags(&a).unwrap(), ["Red", "Errands", "Trip", "Taxes"]);

    // the row again takes it off; Escape closes the popover
    in_popover(&mut run, |run| run.click_named(Role::CheckBox, "Red"));
    assert_eq!(read_tags(&a).unwrap(), ["Errands", "Trip", "Taxes"]);
    // the press closes it, so its release goes to no window
    let escape = KeyEvent {
        code: 0,
        keysym: Keysym(ESCAPE),
        text: None,
        pressed: true,
        repeat: false,
        mods: Mods::NONE,
    };
    in_popover(&mut run, |run| {
        run.send(WindowEvent::Key(escape, Timestamp(0)))
    });
    assert!(popover(&run).is_none());
}

#[test]
fn the_sidebar_lists_the_tags_in_use_and_the_search_finds_items_by_tag() {
    let Some(mut run) = run() else { return };
    run.wait_until(|run| run.named(Role::ListItem).iter().any(|n| n == "Project"));
    assert!(run.named(Role::Group).iter().any(|g| g == "Tags"));

    // a tag of the sidebar shows the items that carry it
    run.click_named(Role::ListItem, "Project");
    run.wait_until(|run| run.rows() == ["b.txt"]);

    // the search finds an item by its tag's whole name, in any case, besides the names that hold the text
    run.click("search");
    run.key(ESCAPE, None);
    run.wait_until(|run| run.rows().len() == 3);
    run.click("search");
    run.type_text("project");
    run.wait_until(|run| run.rows() == ["b.txt"]);
}
