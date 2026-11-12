//! The preview window, run headless: Space or File › Quick Preview opens it on the selection, the arrows go through a
//! selection of several items or move a single one, and Space or Escape closes it after its motion, which Reduce
//! Motion turns into a fade.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::platform::{Mods, WindowStates};
use cupertino_kit::theme::{Appearance, ThemePreferences};

const SPACE: u32 = 0x20;
const ESCAPE: u32 = 0xff1b;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;

/// A window on `home/Documents` (`a.txt`, `b.txt`, `c.bin`), in the list.
fn run(reduce_motion: bool) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::write(documents.join("a.txt"), "alpha line\nsecond line\n").unwrap();
    std::fs::write(documents.join("b.txt"), "bravo\n").unwrap();
    std::fs::write(documents.join("c.bin"), [0u8, 1, 2, 3]).unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
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

/// The preview window, when one is open besides the browser window.
fn preview(run: &Run) -> Option<cupertino_kit::platform::WindowId> {
    let windows = run.driver.runtime().windows();
    windows
        .into_iter()
        .find(|(key, _)| format!("{key:?}").contains("preview/"))
        .map(|(_, id)| id)
}

/// The preview window was opened: the compositor shows it, as it does a new window, and its motion runs.
fn shown(run: &mut Run) {
    let window = preview(run).expect("a preview window");
    run.driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    run.frames(500, 16);
}

/// The texts the preview window shows.
fn preview_texts(run: &Run) -> Vec<String> {
    let window = preview(run).expect("a preview window");
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::StaticText)
        .filter_map(|(_, n)| n.name.clone())
        .collect()
}

#[test]
fn space_opens_the_preview_of_the_selected_text_file_and_space_closes_it() {
    let mut run = run(false);
    run.click_row("a.txt");
    run.key(SPACE, Some(' '));
    assert!(preview(&run).is_some(), "Space opened the preview");
    shown(&mut run);
    // the text is read away from the window's thread, and shows once read
    run.wait_until(|run| {
        preview_texts(run)
            .iter()
            .any(|t| t.starts_with("alpha line"))
    });
    let texts = preview_texts(&run);
    assert!(texts.iter().any(|t| t == "a.txt"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.starts_with("alpha line")),
        "the text file's start: {texts:?}"
    );
    // the preview window can be read
    let window = preview(&run).unwrap();
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    let gaps = cupertino_ui_test::semantic_gaps(&cupertino_ui_test::whole_tree(
        window,
        snapshot.accessibility.clone(),
        snapshot.focus,
    ));
    // Kit names no window root
    let gaps: Vec<String> = gaps
        .into_iter()
        .filter(|gap| !gap.starts_with("Window #0: no name"))
        .collect();
    assert!(gaps.is_empty(), "{gaps:?}");

    // Space again closes it once it has shrunk away
    run.key(SPACE, Some(' '));
    assert!(preview(&run).is_some(), "it shrinks away first");
    run.frames(500, 16);
    assert!(preview(&run).is_none());
}

#[test]
fn the_arrows_go_through_a_selection_of_several_and_move_a_single_one() {
    let mut run = run(false);
    run.click_row("a.txt");
    run.click_row_with("b.txt", Mods::SHIFT);
    run.menu("peek");
    shown(&mut run);
    assert!(preview_texts(&run).iter().any(|t| t == "a.txt"));
    assert!(preview_texts(&run).iter().any(|t| t == "1 of 2"));
    let window = run.window;
    run.window = preview(&run).unwrap();
    run.key(RIGHT, None);
    run.window = window;
    assert!(preview_texts(&run).iter().any(|t| t == "b.txt"));
    assert_eq!(
        run.selected_rows(),
        ["a.txt", "b.txt"],
        "the selection stays"
    );

    // one item: the arrows move the window's selection, and the preview follows it
    run.click_row("a.txt");
    let main = run.window;
    run.window = preview(&run).unwrap();
    run.key(DOWN, None);
    run.window = main;
    assert_eq!(run.selected_rows(), ["b.txt"]);
    assert!(preview_texts(&run).iter().any(|t| t == "b.txt"));
    // a file that is not text shows its kind and size
    run.click_row("c.bin");
    assert!(preview_texts(&run).iter().any(|t| t == "c.bin"));
    assert!(
        preview_texts(&run).iter().any(|t| t.contains("4 bytes")),
        "{:?}",
        preview_texts(&run)
    );
}

#[test]
fn escape_closes_the_preview_and_reduce_motion_only_fades_it() {
    let mut run = run(true);
    run.click_row("b.txt");
    run.key(SPACE, Some(' '));
    shown(&mut run);
    let window = run.window;
    run.window = preview(&run).unwrap();
    run.key(ESCAPE, None);
    run.window = window;
    assert!(preview(&run).is_some(), "it fades out first");
    run.frames(500, 16);
    let windows = run.driver.runtime().windows();
    assert!(preview(&run).is_none(), "{windows:?}");
}
