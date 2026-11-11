//! Folders that spring open under a held drag, in the browser window on CupertinoKit run headless: a folder row, an icon,
//! a sidebar place and a path bar folder open after the delay and not before, flashing just before; the drag goes on in
//! the folder that opened; the drag leaving the window or being cancelled takes the window back to its folder, history,
//! expanded folders, search, selection, view and scroll position, and a drop keeps the folder the drag opened, even when the source hears a cancel
//! after it. With `CUPERTINO_SPRING_CAPTURES` naming a directory, a flashing folder row is written there as PNGs, light
//! and dark at 1x and 2x, for a look at it.

#![cfg(feature = "app")]

mod common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::spring::{SPRING_DELAY, SPRING_FLASH};
use cupertino_kit::core::{AccessibleValue, Role};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::platform::{
    Capabilities, ClipboardData, DragProgress, DropAction, DropToken, Mods, PlatformEvent,
    PointerEvent, ScrollSource, Timestamp, WindowEvent,
};
use cupertino_kit::theme::{Appearance, ThemePreferences};

const URI_LIST: &str = "text/uri-list";
/// Milliseconds of the delay before a folder opens, and of its flash.
const DELAY_MS: u64 = (SPRING_DELAY * 1000.0) as u64;
const FLASH_MS: u64 = (SPRING_FLASH * 1000.0) as u64;

/// A window on `home/Documents` in the list view: the folders `Notes` (holding `deep.txt` and the folder `Inner`, which
/// holds `core.txt`) and `Projects` (holding `plan.txt`), and the files `w00.txt` to `w39.txt`, with Documents and Archive in the sidebar, on a headless system
/// that drags and takes drops; `home/Elsewhere` holds `x.txt`, which another application drags.
fn window() -> Run {
    window_in(Appearance::Light)
}

/// [`window`] in `appearance`.
fn window_in(appearance: Appearance) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    let archive = home.path().join("Archive");
    let elsewhere = home.path().join("Elsewhere");
    for dir in [
        documents.join("Notes/Inner"),
        documents.join("Projects"),
        archive.clone(),
        elsewhere.clone(),
        home.path().join("Trash/files"),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(documents.join("Notes/deep.txt"), "d").unwrap();
    std::fs::write(documents.join("Notes/Inner/core.txt"), "c").unwrap();
    std::fs::write(documents.join("Projects/plan.txt"), "p").unwrap();
    std::fs::write(archive.join("old.txt"), "o").unwrap();
    for i in 0..40 {
        std::fs::write(documents.join(format!("w{i:02}.txt")), "f").unwrap();
    }
    std::fs::write(elsewhere.join("x.txt"), "x").unwrap();
    let places = vec![
        place("Documents", &documents, PlaceKind::Documents),
        place("Archive", &archive, PlaceKind::Documents),
    ];
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let capabilities = Capabilities {
        drag_and_drop: true,
        ..Capabilities::default()
    };
    let mut run = Run::start_on(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        capabilities,
    );
    run.wait_until(|run| run.rows().iter().any(|r| r == "Notes"));
    run
}

/// Another application starts dragging `home/Elsewhere/x.txt` (copy or move) over the window and moves it to `at`.
fn drag_in(run: &mut Run, at: Point) -> DropToken {
    let path = run.home().join("Elsewhere/x.txt");
    let data = vec![ClipboardData {
        mime: URI_LIST.into(),
        bytes: format!("file://{}\r\n", path.display()).into_bytes(),
    }];
    let window = run.window;
    let both = [DropAction::Copy, DropAction::Move].into();
    let system = run.driver.window_system_mut();
    let token = system
        .drag_enter(window, Point::new(1.0, 1.0), data, both, Mods::NONE)
        .unwrap();
    system.drag_move(window, at, Mods::NONE).unwrap();
    run.settle();
    token
}

/// [`drag_in`] onto the row of `name`.
fn drag_in_row(run: &mut Run, name: &str) -> DropToken {
    let at = inside(run.row_bounds(name));
    drag_in(run, at)
}

/// [`drag_to`] the row of `name`.
fn drag_to_row(run: &mut Run, name: &str) {
    let at = inside(run.row_bounds(name));
    drag_to(run, at);
}

fn drag_to(run: &mut Run, at: Point) {
    let window = run.window;
    let system = run.driver.window_system_mut();
    system.drag_move(window, at, Mods::NONE).unwrap();
    run.settle();
}

/// Inside `rect`, clear of its text: its leading edge, half way down.
fn inside(rect: Rect) -> Point {
    Point::new(rect.origin.x + 4.0, rect.origin.y + rect.size.height / 2.0)
}

/// Holds the drag still for `ms` milliseconds.
fn hold(run: &mut Run, ms: u64) {
    run.frames(ms, 20);
    run.settle();
}

/// The folder the window shows, by its title.
fn shown(run: &Run) -> String {
    run.title()
}

/// Holds still over a folder for the delay, and waits for the folder it opened to be listed.
fn spring(run: &mut Run, folder: &str, holds: &str) {
    hold(run, DELAY_MS + 20);
    assert_eq!(shown(run), folder);
    run.wait_until(|run| run.rows().iter().any(|r| r == holds));
}

#[test]
fn a_folder_row_held_under_a_drag_flashes_and_opens_after_the_delay_and_not_before() {
    let mut run = window();
    drag_in_row(&mut run, "Notes");
    hold(&mut run, DELAY_MS - FLASH_MS - 40);
    assert_eq!(shown(&run), "Documents");
    assert!(
        !run.selected_rows().contains(&"Notes".to_owned()),
        "no flash yet"
    );
    hold(&mut run, FLASH_MS);
    assert_eq!(shown(&run), "Documents", "not before the delay");
    assert_eq!(
        run.selected_rows(),
        ["Notes"],
        "the folder flashes just before it opens"
    );
    spring(&mut run, "Notes", "Inner");
    assert!(run.selected_rows().is_empty(), "the flash is over");

    // the drag goes on in the folder that opened
    drag_to_row(&mut run, "Inner");
    hold(&mut run, DELAY_MS - 60);
    assert_eq!(shown(&run), "Notes");
    hold(&mut run, 80);
    assert_eq!(shown(&run), "Inner");
}

#[test]
fn moving_to_another_folder_or_off_the_folders_starts_the_wait_again() {
    let mut run = window();
    drag_in_row(&mut run, "Notes");
    hold(&mut run, DELAY_MS / 2);
    drag_to_row(&mut run, "Projects");
    hold(&mut run, DELAY_MS / 2 + 40);
    assert_eq!(shown(&run), "Documents", "the wait started again");
    drag_to_row(&mut run, "w00.txt");
    hold(&mut run, DELAY_MS * 2);
    assert_eq!(shown(&run), "Documents", "a file opens nothing");
    drag_to_row(&mut run, "Projects");
    spring(&mut run, "Projects", "plan.txt");
}

#[test]
fn an_icon_a_sidebar_place_and_a_path_bar_folder_spring_open_too() {
    let mut run = window();
    run.menu("view-icons");
    let notes = run
        .icon_cells()
        .into_iter()
        .find(|(name, _, _)| name == "Notes")
        .unwrap()
        .1;
    let token = drag_in(&mut run, notes.center());
    hold(&mut run, DELAY_MS + 20);
    assert_eq!(shown(&run), "Notes");
    run.wait_until(|run| run.icons().iter().any(|(name, _)| name == "Inner"));
    // the icon of the folder that opened under the pointer springs as well
    let inner = run
        .icon_cells()
        .into_iter()
        .find(|(name, _, _)| name == "Inner")
        .unwrap()
        .1;
    drag_to(&mut run, inner.center());
    hold(&mut run, DELAY_MS + 20);
    assert_eq!(shown(&run), "Inner");

    let archive = run.bounds(Role::ListItem, "Archive");
    drag_to(&mut run, archive.center());
    hold(&mut run, DELAY_MS + 20);
    assert_eq!(shown(&run), "Archive");
    run.driver
        .window_system_mut()
        .drag_leave(run.window)
        .unwrap();
    run.settle();
    assert_eq!(shown(&run), "Documents");
    assert!(run
        .driver
        .runtime()
        .window_system()
        .incoming_drag(token)
        .is_some());

    // from inside Notes, the path bar's Documents opens the folder above
    run.menu("view-list");
    run.click_row("Notes");
    run.menu("open");
    assert_eq!(shown(&run), "Notes");
    run.menu("toggle-path-bar");
    let documents = run.bounds(Role::Button, "Documents");
    drag_in(&mut run, documents.center());
    spring(&mut run, "Documents", "Projects");
}

/// Scrolls the list down by `dy` points with the wheel.
fn wheel(run: &mut Run, at: Point, dy: f64) {
    run.send(WindowEvent::Pointer(
        PointerEvent::Scroll {
            position: at,
            delta: Point::new(0.0, dy),
            value120: None,
            source: ScrollSource::Wheel,
            stop: false,
            inverted: (false, false),
        },
        Timestamp(0),
    ));
}

#[test]
fn the_drag_leaving_the_window_takes_it_back_to_its_folder_history_selection_view_and_scroll() {
    let mut run = window();
    // a history to keep: Projects, then back to Documents
    run.click_row("Projects");
    run.menu("open");
    assert_eq!(shown(&run), "Projects");
    run.menu("go-back");
    run.wait_until(|run| run.rows().iter().any(|r| r == "w05.txt"));
    // scrolled down a little, with a file selected, so the folder rows stay in view
    let table = run.bounds(Role::Table, "Documents");
    let top = run.row_bounds("Notes").min_y();
    wheel(&mut run, table.center(), 10.0);
    run.click_row("w05.txt");
    let before = run.listed();
    let notes = run.row_bounds("Notes");
    assert!(notes.min_y() < top, "scrolled: {before:?}");

    // the part of the folder's row still in view
    drag_in(
        &mut run,
        Point::new(notes.min_x() + 4.0, notes.max_y() - 3.0),
    );
    spring(&mut run, "Notes", "Inner");
    drag_to_row(&mut run, "Inner");
    spring(&mut run, "Inner", "core.txt");
    run.driver
        .window_system_mut()
        .drag_leave(run.window)
        .unwrap();
    run.settle();
    assert_eq!(shown(&run), "Documents");
    run.wait_until(|run| run.listed() == before);
    assert_eq!(run.selected_rows(), ["w05.txt"]);
    // the history is the one from before the drag: back goes to Projects, as it did
    run.menu("go-forward");
    assert_eq!(shown(&run), "Projects");
}

/// The drag leaves the window.
fn leave(run: &mut Run) {
    run.driver
        .window_system_mut()
        .drag_leave(run.window)
        .unwrap();
    run.settle();
}

#[test]
fn the_expanded_folders_come_back_with_the_selection_inside_them() {
    const RIGHT: u32 = 0xff53;
    let mut run = window();
    run.click_row("Notes");
    run.key(RIGHT, None);
    run.wait_until(|run| run.rows().iter().any(|r| r == "Inner"));
    run.click_row("Inner");
    run.key(RIGHT, None);
    run.wait_until(|run| run.rows().iter().any(|r| r == "core.txt"));
    run.click_row("deep.txt");
    let before = run.rows();

    drag_in_row(&mut run, "Projects");
    spring(&mut run, "Projects", "plan.txt");
    leave(&mut run);
    assert_eq!(shown(&run), "Documents");
    run.wait_until(|run| run.rows() == before);
    run.wait_until(|run| run.selected_rows() == ["deep.txt"]);
}

#[test]
fn the_search_comes_back_with_its_text() {
    let mut run = window();
    run.click("search");
    run.type_text("o");
    run.wait_until(|run| run.rows() == ["Notes", "Projects"]);

    drag_in_row(&mut run, "Projects");
    spring(&mut run, "Projects", "plan.txt");
    assert!(
        run.rows().iter().any(|r| r == "plan.txt"),
        "the folder opened whole"
    );
    leave(&mut run);
    assert_eq!(shown(&run), "Documents");
    run.wait_until(|run| run.rows() == ["Notes", "Projects"]);
    // the field shows the text again
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let texts: Vec<String> = snapshot
        .accessibility
        .iter()
        .filter_map(|(_, node)| match &node.value {
            Some(AccessibleValue::Text {
                value,
                editing: Some(_),
            }) => Some(value.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["o"]);
}

#[test]
fn the_icon_view_comes_back_too() {
    let mut run = window();
    run.menu("view-icons");
    let notes = run
        .icon_cells()
        .into_iter()
        .find(|(name, _, _)| name == "Notes")
        .unwrap()
        .1;
    drag_in(&mut run, notes.center());
    hold(&mut run, DELAY_MS + 20);
    assert_eq!(shown(&run), "Notes");
    run.menu("view-list");
    run.driver
        .window_system_mut()
        .drag_leave(run.window)
        .unwrap();
    run.settle();
    assert_eq!(shown(&run), "Documents");
    assert!(run.rows().is_empty(), "the icon view again");
    run.wait_until(|run| run.icons().iter().any(|(name, _)| name == "Notes"));
}

/// After springing through two folders, a drop lands in the deeper one and the window stays there.
#[test]
fn a_drop_keeps_the_folder_the_drag_opened() {
    let mut run = window();
    let token = drag_in_row(&mut run, "Notes");
    spring(&mut run, "Notes", "Inner");
    drag_to_row(&mut run, "Inner");
    spring(&mut run, "Inner", "core.txt");
    // on the folder shown, under its rows
    let table = run.bounds(Role::Table, "Inner");
    drag_to(&mut run, Point::new(table.center().x, table.max_y() - 20.0));
    let window = run.window;
    assert!(run.driver.window_system_mut().drag_drop(window).unwrap());
    // the move runs on a worker thread, which reports the drop finished once it is done
    run.wait_until(|run| {
        run.driver
            .runtime()
            .window_system()
            .incoming_drag(token)
            .is_some_and(|drag| drag.finished.is_some())
    });
    let incoming = run.driver.runtime().window_system().incoming_drag(token);
    assert_eq!(
        incoming.unwrap().finished,
        Some(Some(DropAction::Move)),
        "dropped in the folder that opened"
    );
    assert!(run.home().join("Documents/Notes/Inner/x.txt").exists());
    // the source's leave after the drop changes nothing
    run.driver.window_system_mut().drag_leave(window).unwrap();
    run.settle();
    hold(&mut run, DELAY_MS * 2);
    assert_eq!(shown(&run), "Inner");
    // the history keeps the folders the drag went through
    run.menu("go-back");
    assert_eq!(shown(&run), "Notes");
}

#[test]
fn a_drop_where_nothing_takes_it_is_a_leave() {
    let mut run = window();
    drag_in_row(&mut run, "Notes");
    spring(&mut run, "Notes", "deep.txt");
    // over the toolbar, which takes nothing
    let toolbar = Point::new(run.bounds(Role::Table, "Notes").center().x, 20.0);
    drag_to(&mut run, toolbar);
    hold(&mut run, DELAY_MS * 2);
    assert_eq!(shown(&run), "Notes", "still over the window");
    let window = run.window;
    assert!(!run.driver.window_system_mut().drag_drop(window).unwrap());
    run.settle();
    assert_eq!(shown(&run), "Documents");
}

/// The window's own drag of `name`'s row, held over its own window at `at` as the compositor shows it there.
fn own_drag(run: &mut Run, name: &str) -> cupertino_ui_test::StartedDrag {
    let row = run.row_bounds(name);
    // on the row's name: a press on its blank part draws a band
    let from = Point::new(row.min_x() + 48.0, row.center().y);
    run.hold(from, Point::new(from.x + 30.0, from.y + 10.0), Mods::NONE);
    let started = run.driver.runtime().window_system().drags()[0].clone();
    let window = run.window;
    let both = [DropAction::Copy, DropAction::Move].into();
    let system = run.driver.window_system_mut();
    system
        .drag_enter(window, from, started.request.data.clone(), both, Mods::NONE)
        .unwrap();
    run.settle();
    started
}

#[test]
fn a_cancelled_drag_of_the_windows_own_items_takes_it_back() {
    let mut run = window();
    let started = own_drag(&mut run, "w03.txt");
    drag_to_row(&mut run, "Projects");
    spring(&mut run, "Projects", "plan.txt");
    // Escape: the compositor cancels the drag
    run.driver
        .window_system_mut()
        .enqueue(PlatformEvent::Drag {
            token: started.token,
            progress: DragProgress::Cancelled,
        })
        .unwrap();
    run.settle();
    assert_eq!(shown(&run), "Documents");
    run.wait_until(|run| run.selected_rows() == ["w03.txt"]);
    // the compositor ends the drag over the window too: nothing opens after it
    run.driver
        .window_system_mut()
        .drag_leave(run.window)
        .unwrap();
    run.settle();
    hold(&mut run, DELAY_MS * 2);
    assert_eq!(shown(&run), "Documents");
}

/// The window's own items dropped in a folder the drag opened: the drop keeps that folder, also when the source hears
/// the drag cancelled after the drop, as it does when a drop held open while it is worked on is let go early.
#[test]
fn a_cancel_after_a_drop_keeps_the_folder_the_drag_opened() {
    let mut run = window();
    let started = own_drag(&mut run, "w03.txt");
    drag_to_row(&mut run, "Projects");
    spring(&mut run, "Projects", "plan.txt");
    // on the folder shown, under its rows
    let table = run.bounds(Role::Table, "Projects");
    drag_to(&mut run, Point::new(table.center().x, table.max_y() - 20.0));
    let window = run.window;
    assert!(run.driver.window_system_mut().drag_drop(window).unwrap());
    // the source hears the drop was made, then, once Files has it, that the drag was cancelled
    for progress in [DragProgress::Dropped, DragProgress::Cancelled] {
        run.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Drag {
                token: started.token,
                progress,
            })
            .unwrap();
        run.settle();
    }
    let moved = run.home().join("Documents/Projects/w03.txt");
    run.wait_until(|_| moved.exists());
    hold(&mut run, DELAY_MS * 2);
    assert_eq!(shown(&run), "Projects");
}

#[test]
fn a_flashing_folder_row_in_light_and_dark_at_1x_and_2x() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        for scale in [1u32, 2] {
            let mut run = window_in(appearance);
            run.send(WindowEvent::Scale(f64::from(scale)));
            drag_in_row(&mut run, "Notes");
            hold(&mut run, DELAY_MS - FLASH_MS / 2);
            assert_eq!(run.selected_rows(), ["Notes"], "{appearance:?} at {scale}x");
            capture(&mut run, appearance, scale);
        }
    }
}

/// Writes the latest frame as `<CUPERTINO_SPRING_CAPTURES>/files-spring-flash-<appearance>@<scale>x.png` when that
/// variable is set.
fn capture(run: &mut Run, appearance: Appearance, scale: u32) {
    let Some(dir) = std::env::var_os("CUPERTINO_SPRING_CAPTURES") else {
        return;
    };
    let window = run.window;
    let frames = run.driver.window_system_mut().frames(window).unwrap();
    let frame = frames.last().unwrap();
    let mut pixmap = tiny_skia::Pixmap::new(frame.width, frame.height).unwrap();
    pixmap.data_mut().copy_from_slice(&frame.pixels);
    let name = match appearance {
        Appearance::Light => "light",
        _ => "dark",
    };
    let path = std::path::Path::new(&dir).join(format!("files-spring-flash-{name}@{scale}x.png"));
    pixmap.save_png(&path).unwrap();
    eprintln!("wrote {}", path.display());
}
