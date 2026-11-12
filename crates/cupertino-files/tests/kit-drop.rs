//! Files dropped on the browser window on CupertinoKit from another application, run headless: on a folder of the list
//! or the grid, on a place of the sidebar, on the Trash and on the folder shown; moved within a device unless Option
//! copies, a name the folder has given a free one, the place highlighted while the drag is over it, and nothing taken
//! where a drop has nowhere to go. The window's own drags show their action as they move. With
//! `CUPERTINO_DROP_CAPTURES` naming a directory, a highlighted folder row is written there as PNGs, light and dark at
//! 1x and 2x, for a look at it.

#![cfg(feature = "app")]

use crate::common;

use std::path::{Path, PathBuf};

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::platform::{
    Capabilities, ClipboardData, DragProgress, DropAction, DropToken, Mods, MouseButton,
    PlatformEvent, PointerEvent, Timestamp, WindowEvent,
};
use cupertino_kit::theme::{Appearance, ThemePreferences};
use cupertino_ui_test::IncomingDrag;

const URI_LIST: &str = "text/uri-list";

/// A window on `home/Documents` (the folder `Notes`, the files `a.txt` and `b.txt`) in the list view, with Documents,
/// Archive and the Trash in the sidebar, on a headless system that takes drops; `home/Elsewhere` holds `x.txt` and
/// `y.txt`, which another application drags.
fn window(appearance: Appearance) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    let archive = home.path().join("Archive");
    let elsewhere = home.path().join("Elsewhere");
    let trash = home.path().join("Trash/files");
    for dir in [
        documents.join("Notes"),
        archive.clone(),
        elsewhere.clone(),
        trash.clone(),
    ] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    std::fs::write(documents.join("b.txt"), "bb").unwrap();
    std::fs::write(elsewhere.join("x.txt"), "x").unwrap();
    std::fs::write(elsewhere.join("y.txt"), "yy").unwrap();
    let places = vec![
        place("Documents", &documents, PlaceKind::Documents),
        place("Archive", &archive, PlaceKind::Documents),
        place("Trash", &trash, PlaceKind::Trash),
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
    Run::start_on(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        capabilities,
    )
}

fn uri_list(paths: &[PathBuf]) -> Vec<ClipboardData> {
    let uris: String = paths
        .iter()
        .map(|p| format!("file://{}\r\n", p.display()))
        .collect();
    vec![ClipboardData {
        mime: URI_LIST.into(),
        bytes: uris.into_bytes(),
    }]
}

fn elsewhere(run: &Run, names: &[&str]) -> Vec<PathBuf> {
    let dir = run.home().join("Elsewhere");
    names.iter().map(|n| dir.join(n)).collect()
}

/// Another application starts dragging `names` of `home/Elsewhere` (copy or move) and moves it to `at` with `mods`
/// held.
fn drag_in(run: &mut Run, names: &[&str], at: Point, mods: Mods) -> DropToken {
    let paths = elsewhere(run, names);
    let window = run.window;
    let both = [DropAction::Copy, DropAction::Move].into();
    let system = run.driver.window_system_mut();
    let token = system
        .drag_enter(
            window,
            Point::new(1.0, 1.0),
            uri_list(&paths),
            both,
            Mods::NONE,
        )
        .unwrap();
    system.drag_move(window, at, mods).unwrap();
    run.settle();
    token
}

/// Drop the drag over the window, and wait until the file operation it started, if any, has ended: its row leaves.
fn release(run: &mut Run) -> bool {
    let window = run.window;
    let dropped = run.driver.window_system_mut().drag_drop(window).unwrap();
    run.settle();
    run.wait_until(|run| run.named(Role::ProgressIndicator).is_empty());
    dropped
}

fn incoming(run: &Run, token: DropToken) -> &IncomingDrag {
    run.driver
        .runtime()
        .window_system()
        .incoming_drag(token)
        .unwrap()
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The colour of the latest frame at `at`, in points at `scale`.
fn pixel(run: &mut Run, at: Point, scale: u32) -> [u8; 4] {
    let window = run.window;
    let frames = run.driver.window_system_mut().frames(window).unwrap();
    let frame = frames.last().unwrap();
    let x = (at.x * f64::from(scale)) as usize;
    let y = (at.y * f64::from(scale)) as usize;
    let i = (y * frame.width as usize + x) * 4;
    frame.pixels[i..i + 4].try_into().unwrap()
}

/// Inside `rect`, clear of its text: its leading edge, half way down.
fn inside(rect: Rect) -> Point {
    Point::new(rect.origin.x + 4.0, rect.origin.y + rect.size.height / 2.0)
}

#[test]
fn files_dropped_on_a_folder_row_move_into_it_and_the_row_is_highlighted_while_over_it() {
    let mut run = window(Appearance::Light);
    let notes = run.row_bounds("Notes");
    let a = run.row_bounds("a.txt");
    let (notes_before, a_before) = (
        pixel(&mut run, inside(notes), 1),
        pixel(&mut run, inside(a), 1),
    );
    let paths = ["x.txt", "y.txt"];
    let token = drag_in(&mut run, &paths, notes.center(), Mods::NONE);
    let drag = incoming(&run, token);
    let answer = drag.answers.last().unwrap();
    assert_eq!(answer.mime.as_deref(), Some(URI_LIST));
    assert_eq!(answer.preferred, Some(DropAction::Move));
    assert_eq!(drag.chosen, Some(DropAction::Move));
    assert_ne!(
        pixel(&mut run, inside(notes), 1),
        notes_before,
        "the folder under the drag is highlighted"
    );
    assert_eq!(
        pixel(&mut run, inside(a), 1),
        a_before,
        "the others are not"
    );
    assert!(release(&mut run));
    let drag = incoming(&run, token);
    assert_eq!(drag.reads, [URI_LIST]);
    assert_eq!(drag.finished, Some(Some(DropAction::Move)));
    let documents = run.home().join("Documents");
    assert_eq!(names(&documents.join("Notes")), ["x.txt", "y.txt"]);
    assert!(
        names(&run.home().join("Elsewhere")).is_empty(),
        "moved, not copied"
    );
    assert_eq!(
        pixel(&mut run, inside(notes), 1),
        notes_before,
        "the highlight goes with the drop"
    );
}

#[test]
fn option_copies_and_a_name_the_folder_has_is_given_a_free_one() {
    let mut run = window(Appearance::Light);
    std::fs::write(run.home().join("Elsewhere/a.txt"), "other a").unwrap();
    let paths = ["a.txt"];
    // over a file the drop lands on the folder shown
    let a = run.row_bounds("b.txt");
    let token = drag_in(&mut run, &paths, a.center(), Mods::ALT);
    assert_eq!(incoming(&run, token).chosen, Some(DropAction::Copy));
    assert!(release(&mut run));
    assert_eq!(incoming(&run, token).finished, Some(Some(DropAction::Copy)));
    let documents = run.home().join("Documents");
    assert_eq!(names(&documents), ["Notes", "a copy.txt", "a.txt", "b.txt"]);
    assert_eq!(
        std::fs::read_to_string(documents.join("a copy.txt")).unwrap(),
        "other a"
    );
    assert_eq!(
        names(&run.home().join("Elsewhere")),
        ["a.txt", "x.txt", "y.txt"]
    );
    run.wait_until(|run| run.selected_rows() == ["a copy.txt"]);
}

#[test]
fn files_dropped_on_a_sidebar_place_go_into_its_folder_and_on_the_trash_into_the_trash() {
    let mut run = window(Appearance::Light);
    let archive = run.bounds(Role::ListItem, "Archive");
    let before = pixel(&mut run, inside(archive), 1);
    let paths = ["x.txt"];
    let token = drag_in(&mut run, &paths, archive.center(), Mods::NONE);
    assert_ne!(
        pixel(&mut run, inside(archive), 1),
        before,
        "the place is highlighted"
    );
    assert!(release(&mut run));
    assert_eq!(incoming(&run, token).finished, Some(Some(DropAction::Move)));
    assert_eq!(names(&run.home().join("Archive")), ["x.txt"]);

    let trash = run.bounds(Role::ListItem, "Trash");
    let paths = ["y.txt"];
    let token = drag_in(&mut run, &paths, trash.center(), Mods::ALT);
    // the trash takes items away, so it shows a move whatever is held
    assert_eq!(incoming(&run, token).chosen, Some(DropAction::Move));
    assert!(release(&mut run));
    assert_eq!(incoming(&run, token).finished, Some(Some(DropAction::Move)));
    assert_eq!(names(&run.home().join("Trash/files")), ["y.txt"]);
    assert!(names(&run.home().join("Elsewhere")).is_empty());
}

#[test]
fn files_dropped_on_an_icon_folder_or_the_grids_background_land_there() {
    let mut run = window(Appearance::Light);
    run.menu("view-icons");
    let cells = run.icon_cells();
    let notes = cells.iter().find(|(n, _, _)| n == "Notes").unwrap().1;
    let token = drag_in(&mut run, &["x.txt"], notes.center(), Mods::NONE);
    assert!(release(&mut run));
    assert_eq!(incoming(&run, token).finished, Some(Some(DropAction::Move)));
    assert_eq!(names(&run.home().join("Documents/Notes")), ["x.txt"]);
    // below the last row of icons, the folder shown
    let low = cells
        .iter()
        .map(|(_, r, _)| r.origin.y + r.size.height)
        .fold(0.0, f64::max);
    let token = drag_in(
        &mut run,
        &["y.txt"],
        Point::new(notes.center().x, low + 30.0),
        Mods::NONE,
    );
    assert!(release(&mut run));
    assert_eq!(incoming(&run, token).finished, Some(Some(DropAction::Move)));
    assert!(run.home().join("Documents/y.txt").exists());
}

#[test]
fn a_drag_of_another_type_or_onto_the_toolbar_is_refused_and_a_drop_with_no_files_changes_nothing()
{
    let mut run = window(Appearance::Light);
    let window = run.window;
    let notes = run.row_bounds("Notes");
    let text = vec![ClipboardData {
        mime: "text/plain".into(),
        bytes: b"words".to_vec(),
    }];
    let system = run.driver.window_system_mut();
    let token = system
        .drag_enter(
            window,
            notes.center(),
            text,
            [DropAction::Copy].into(),
            Mods::NONE,
        )
        .unwrap();
    run.settle();
    assert_eq!(incoming(&run, token).chosen, None);
    assert!(
        !release(&mut run),
        "a drag nothing takes leaves at the release"
    );
    assert_eq!(incoming(&run, token).finished, Some(None));

    let search = run.bounds(Role::TextInput, "Search");
    let token = drag_in(&mut run, &["x.txt"], search.center(), Mods::NONE);
    assert_eq!(
        incoming(&run, token).chosen,
        None,
        "the toolbar is no folder"
    );
    assert!(!release(&mut run));

    let empty = vec![ClipboardData {
        mime: URI_LIST.into(),
        bytes: b"https://example.org/page\r\n".to_vec(),
    }];
    let system = run.driver.window_system_mut();
    let token = system
        .drag_enter(
            window,
            notes.center(),
            empty,
            [DropAction::Copy, DropAction::Move].into(),
            Mods::NONE,
        )
        .unwrap();
    run.settle();
    assert!(release(&mut run));
    assert_eq!(
        incoming(&run, token).finished,
        Some(None),
        "nothing taken: the source keeps its data"
    );
    assert!(names(&run.home().join("Documents/Notes")).is_empty());
}

/// Another application drags `paths` with copy and move allowed over `at` with `mods`, and drops; how the window
/// finished the drop.
fn drop_list(run: &mut Run, list: Vec<ClipboardData>, at: Point, mods: Mods) -> Option<DropAction> {
    let window = run.window;
    let both = [DropAction::Copy, DropAction::Move].into();
    let system = run.driver.window_system_mut();
    let token = system.drag_enter(window, at, list, both, mods).unwrap();
    run.settle();
    assert!(release(run));
    incoming(run, token).finished.expect("finished")
}

#[test]
fn a_drop_that_fails_in_part_or_moves_nothing_is_not_told_done_so_the_source_keeps_its_items() {
    let mut run = window(Appearance::Light);
    let notes = run.row_bounds("Notes").center();
    let gone = run.home().join("Elsewhere/gone.txt");
    // with Option held the items copy: x.txt does, gone.txt cannot
    let paths = vec![run.home().join("Elsewhere/x.txt"), gone.clone()];
    assert_eq!(
        drop_list(&mut run, uri_list(&paths), notes, Mods::ALT),
        None
    );
    assert_eq!(names(&run.home().join("Documents/Notes")), ["x.txt"]);
    // with the primary modifier they move: y.txt does, gone.txt cannot
    let paths = vec![run.home().join("Elsewhere/y.txt"), gone];
    assert_eq!(
        drop_list(&mut run, uri_list(&paths), notes, Mods::PRIMARY),
        None
    );
    assert_eq!(
        names(&run.home().join("Documents/Notes")),
        ["x.txt", "y.txt"]
    );
    assert_eq!(
        names(&run.home().join("Elsewhere")),
        ["x.txt"],
        "the copied item stays, for the source to keep"
    );

    // another application's item dropped on the folder that holds it stays there, and is not said moved
    let b = run.row_bounds("b.txt").center();
    let a = vec![run.home().join("Documents/a.txt")];
    assert_eq!(drop_list(&mut run, uri_list(&a), b, Mods::NONE), None);
    assert!(run.home().join("Documents/a.txt").exists());
}

#[test]
fn a_list_with_a_remote_relative_or_climbing_uri_is_refused_whole() {
    let mut run = window(Appearance::Light);
    let notes = run.row_bounds("Notes").center();
    let x = run.home().join("Elsewhere/x.txt");
    for odd in [
        "file://elsewhere/tmp/y".to_string(),
        "file:y.txt".to_string(),
        format!(
            "file://{}/../Elsewhere/y.txt",
            run.home().join("Documents").display()
        ),
    ] {
        let list = vec![ClipboardData {
            mime: URI_LIST.into(),
            bytes: format!("file://{}\r\n{odd}\r\n", x.display()).into_bytes(),
        }];
        assert_eq!(drop_list(&mut run, list, notes, Mods::NONE), None, "{odd}");
    }
    assert!(names(&run.home().join("Documents/Notes")).is_empty());
    assert_eq!(names(&run.home().join("Elsewhere")), ["x.txt", "y.txt"]);
}

#[test]
fn the_windows_own_drag_shows_its_action_and_has_nowhere_to_go_in_the_folder_holding_it() {
    let mut run = window(Appearance::Light);
    run.menu("view-icons");
    let window = run.window;
    let cells = run.icon_cells();
    let cell = |name: &str| cells.iter().find(|(n, _, _)| n == name).unwrap().1;
    let a = cell("a.txt");
    let pointer = |run: &mut Run, event| run.send(WindowEvent::Pointer(event, Timestamp(0)));
    pointer(
        &mut run,
        PointerEvent::Motion {
            position: a.center(),
        },
    );
    pointer(
        &mut run,
        PointerEvent::Button {
            position: a.center(),
            button: MouseButton::Left,
            pressed: true,
            serial: 1,
            mods: Mods::NONE,
        },
    );
    let moved = Point::new(a.center().x + 12.0, a.center().y + 6.0);
    pointer(&mut run, PointerEvent::Motion { position: moved });
    let started = run
        .driver
        .runtime()
        .window_system()
        .drags()
        .last()
        .cloned()
        .expect("a drag");
    let own = started.request.data.clone();
    let both = [DropAction::Copy, DropAction::Move].into();
    // the compositor hands the window its own drag back
    let system = run.driver.window_system_mut();
    let token = system
        .drag_enter(window, a.center(), own, both, Mods::NONE)
        .unwrap();
    run.settle();
    assert_eq!(
        incoming(&run, token).chosen,
        None,
        "a.txt's own folder takes nothing"
    );
    run.driver
        .window_system_mut()
        .drag_move(window, cell("Notes").center(), Mods::NONE)
        .unwrap();
    run.settle();
    assert_eq!(
        incoming(&run, token).chosen,
        Some(DropAction::Move),
        "within the device"
    );
    run.driver
        .window_system_mut()
        .drag_move(window, cell("Notes").center(), Mods::ALT)
        .unwrap();
    run.settle();
    assert_eq!(
        incoming(&run, token).chosen,
        Some(DropAction::Copy),
        "Option copies"
    );
    assert!(release(&mut run));
    run.driver
        .window_system_mut()
        .enqueue(PlatformEvent::Drag {
            token: started.token,
            progress: DragProgress::Finished,
        })
        .unwrap();
    run.settle();
    assert_eq!(names(&run.home().join("Documents/Notes")), ["a.txt"]);
    assert!(run.home().join("Documents/a.txt").exists(), "copied");
}

#[test]
fn a_highlighted_folder_row_in_light_and_dark_at_1x_and_2x() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        for scale in [1u32, 2] {
            let mut run = window(appearance);
            run.send(WindowEvent::Scale(f64::from(scale)));
            let notes = run.row_bounds("Notes");
            let before = pixel(&mut run, inside(notes), scale);
            drag_in(&mut run, &["x.txt"], notes.center(), Mods::NONE);
            let lit = pixel(&mut run, inside(notes), scale);
            assert_ne!(
                lit, before,
                "{appearance:?} at {scale}x: the row is highlighted"
            );
            capture(&mut run, appearance, scale);
        }
    }
}

/// Writes the latest frame as `<CUPERTINO_DROP_CAPTURES>/files-drop-row-<appearance>@<scale>x.png` when that variable
/// is set.
fn capture(run: &mut Run, appearance: Appearance, scale: u32) {
    let Some(dir) = std::env::var_os("CUPERTINO_DROP_CAPTURES") else {
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
    let path = Path::new(&dir).join(format!("files-drop-row-{name}@{scale}x.png"));
    pixmap.save_png(&path).unwrap();
    eprintln!("wrote {}", path.display());
}
