//! The Kit window's context menus, run headless: what a secondary click on an item, on a selection and on empty space
//! selects and offers, the Menu key, and rows that act.

#![cfg(feature = "app")]

use crate::common;

use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use common::{place, Run};
use cupertino_files::kit::Observer;
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::{Key, Role, WindowKey};
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
    // the view modes are a submenu, not rows of their own
    assert!(enabled(&rows, "Show As"), "{rows:?}");
    for view in ["Icons", "List", "Columns", "Gallery"] {
        assert!(!labels(&rows).contains(&view), "{view}: {rows:?}");
    }
    assert!(!labels(&rows).contains(&"Move to Trash"));
    run.close_menu();
}

#[test]
fn the_keyboard_opens_show_as_and_chooses_a_view_in_it() {
    let mut run = run();
    let main = run.window;
    let at = empty_space(&run);
    run.press_at(at, MouseButton::Right);
    let menu = level(&run, 0).expect("open");
    run.window = menu;
    run.driver
        .window_system_mut()
        .configure(menu, None, WindowStates::default())
        .unwrap();
    run.settle();
    run.frames(1100, 100);
    // "sh" reaches Show As before Show View Options
    for c in "sh".chars() {
        run.key(c as u32, Some(c));
    }
    assert_eq!(focused_row(&run, 0).as_deref(), Some("Show As"));
    run.key(RIGHT, None);
    let submenu = level(&run, 1).expect("the views' submenu");
    run.window = submenu;
    run.driver
        .window_system_mut()
        .configure(submenu, None, WindowStates::default())
        .unwrap();
    run.settle();
    let views = rows_of(&run, 1);
    assert_eq!(
        views,
        [
            ("Icons".to_owned(), true),
            ("List".to_owned(), true),
            ("Columns".to_owned(), true),
            ("Gallery".to_owned(), true)
        ]
    );
    assert_eq!(
        focused_row(&run, 1).as_deref(),
        Some("Icons"),
        "Right focuses the first row"
    );
    run.key('c' as u32, Some('c'));
    assert_eq!(focused_row(&run, 1).as_deref(), Some("Columns"));
    // the typed letters are forgotten after a pause
    run.frames(1100, 100);
    run.key('i' as u32, Some('i'));
    assert_eq!(focused_row(&run, 1).as_deref(), Some("Icons"));
    run.frames(1100, 100);
    run.key('g' as u32, Some('g'));
    assert_eq!(focused_row(&run, 1).as_deref(), Some("Gallery"));
    press(&mut run, RETURN);
    run.window = main;
    assert!(level(&run, 0).is_none(), "choosing closed the menu");
    // the strip of every item, the first selected and shown large
    run.wait_until(|run| run.icons().len() == 3);
    assert_eq!(run.selected_icons().len(), 1);
    assert!(run.named(Role::Image).contains(&run.selected_icons()[0]));
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
    // the paste runs on a worker and ends refused: its failure is listed in the progress window, which opens
    let progress = WindowKey(Key::from("progress".to_owned()));
    run.wait_until(|run| {
        let windows = run.driver.runtime().windows();
        windows.into_iter().any(|(key, _)| key == progress)
    });
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
    run.driver.look(run.window, "files-paste-progress").unwrap();
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

// ---- the pointer and keys as every menu takes them --------------------------------------------

use cupertino_kit::geometry::Rect;
use cupertino_kit::platform::{PointerEvent, PopupEdge, WindowEvent, WindowId, WindowStates};
use cupertino_menu_model::tracking::{
    SUBMENU_AIM_PAUSE, SUBMENU_CLOSE_DELAY, SUBMENU_OPEN_DELAY, TRACKING_INTERVAL,
};

const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;
const RIGHT: u32 = 0xff53;

/// Only the press of a key that closes the window it goes to: no release reaches it.
fn press(run: &mut Run, keysym: u32) {
    run.send(WindowEvent::Key(
        cupertino_kit::platform::KeyEvent {
            code: 0,
            keysym: cupertino_kit::platform::Keysym(keysym),
            text: None,
            pressed: true,
            repeat: false,
            mods: Mods::NONE,
        },
        cupertino_kit::platform::Timestamp(0),
    ));
}

fn ms(seconds: f64) -> u64 {
    (seconds * 1000.0).round() as u64
}

/// The pointer as the compositor routes it: to the window under it, Leave and Enter as it changes windows; while a
/// menu holds it, a release goes to the menu window under it.
struct Pointer {
    main: WindowId,
    over: Option<WindowId>,
    configured: Vec<WindowId>,
    at: Point,
}

impl Pointer {
    fn new(run: &Run) -> Self {
        Pointer {
            main: run.window,
            over: None,
            configured: vec![run.window],
            at: Point::new(0.0, 0.0),
        }
    }

    /// Maps new popups at the size they were placed at and lets them lay out.
    fn settle(&mut self, run: &mut Run) {
        run.window = self.main;
        run.settle();
        for id in run.windows() {
            if self.configured.contains(&id) {
                continue;
            }
            let size = run
                .driver
                .runtime()
                .window_system()
                .placement(id)
                .unwrap()
                .map(|p| p.size);
            run.driver
                .window_system_mut()
                .configure(id, size, WindowStates::default())
                .unwrap();
            self.configured.push(id);
        }
        run.settle();
    }

    fn wait(&mut self, run: &mut Run, ms: u64) {
        run.frames(ms, 16.min(ms.max(1)));
        self.settle(run);
    }

    fn locate(&self, run: &Run, p: Point) -> (WindowId, Point) {
        for id in run.windows().into_iter().rev() {
            if id == self.main {
                continue;
            }
            let r = rect(run, id);
            if r.contains(p) {
                return (id, Point::new(p.x - r.min_x(), p.y - r.min_y()));
            }
        }
        (self.main, p)
    }

    fn pointer(&self, run: &mut Run, window: WindowId, event: PointerEvent) {
        run.window = window;
        run.send(WindowEvent::Pointer(
            event,
            cupertino_kit::platform::Timestamp(0),
        ));
    }

    fn move_to(&mut self, run: &mut Run, p: Point) {
        self.at = p;
        let (id, local) = self.locate(run, p);
        if self.over != Some(id) {
            if let Some(old) = self.over.filter(|old| run.windows().contains(old)) {
                self.pointer(run, old, PointerEvent::Leave);
            }
            self.pointer(run, id, PointerEvent::Enter { position: local });
            self.over = Some(id);
        }
        self.pointer(run, id, PointerEvent::Motion { position: local });
        self.settle(run);
    }

    /// From where the pointer is to `to` in `steps` moves `ms` apart.
    fn glide(&mut self, run: &mut Run, to: Point, steps: u32, ms: u64) {
        let from = self.at;
        for step in 1..=steps {
            let k = f64::from(step) / f64::from(steps);
            self.wait(run, ms);
            self.move_to(
                run,
                Point::new(from.x + (to.x - from.x) * k, from.y + (to.y - from.y) * k),
            );
        }
    }

    fn button(&mut self, run: &mut Run, button: MouseButton, pressed: bool) {
        let (id, position) = self.locate(run, self.at);
        self.pointer(
            run,
            id,
            PointerEvent::Button {
                position,
                button,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            },
        );
        self.settle(run);
    }
}

/// The open window `id` in the main window's coordinates, from where it asked to be placed.
fn rect(run: &Run, id: WindowId) -> Rect {
    let system = run.driver.runtime().window_system();
    let Some(parent) = system.parent(id).unwrap() else {
        return Rect::new(0.0, 0.0, 1.0e6, 1.0e6);
    };
    let spec = system.placement(id).unwrap().unwrap().clone();
    let base = match system.parent(parent).unwrap() {
        None => Point::new(0.0, 0.0),
        Some(_) => rect(run, parent).origin,
    };
    let a = spec.anchor_rect;
    let ax = match spec.anchor {
        PopupEdge::Left | PopupEdge::TopLeft | PopupEdge::BottomLeft => a.min_x(),
        PopupEdge::Right | PopupEdge::TopRight | PopupEdge::BottomRight => a.max_x(),
        _ => a.min_x() + a.width() / 2.0,
    };
    let ay = match spec.anchor {
        PopupEdge::Top | PopupEdge::TopLeft | PopupEdge::TopRight => a.min_y(),
        PopupEdge::Bottom | PopupEdge::BottomLeft | PopupEdge::BottomRight => a.max_y(),
        _ => a.min_y() + a.height() / 2.0,
    };
    let (w, h) = (spec.size.width, spec.size.height);
    let x = match spec.gravity {
        PopupEdge::Left | PopupEdge::TopLeft | PopupEdge::BottomLeft => -w,
        PopupEdge::Right | PopupEdge::TopRight | PopupEdge::BottomRight => 0.0,
        _ => -w / 2.0,
    };
    let y = match spec.gravity {
        PopupEdge::Top | PopupEdge::TopLeft | PopupEdge::TopRight => -h,
        PopupEdge::Bottom | PopupEdge::BottomLeft | PopupEdge::BottomRight => 0.0,
        _ => -h / 2.0,
    };
    Rect::new(
        base.x + ax + spec.offset.x + x,
        base.y + ay + spec.offset.y + y,
        w,
        h,
    )
}

/// The menu window of `depth` (0 the menu, 1 its submenu), if open.
fn level(run: &Run, depth: usize) -> Option<WindowId> {
    let suffix = if depth == 0 {
        "context-menu\"".to_owned()
    } else {
        format!("context-menu/{depth}\"")
    };
    run.driver
        .runtime()
        .windows()
        .into_iter()
        .find(|(key, _)| format!("{key:?}").contains(&suffix))
        .map(|(_, id)| id)
}

/// The row `label` of the menu window `depth`, in the main window's coordinates.
fn row(run: &mut Run, depth: usize, label: &str) -> Rect {
    let id = level(run, depth).expect("the menu is open");
    let main = run.window;
    run.window = id;
    let r = run.bounds(Role::MenuItem, label);
    run.window = main;
    let origin = rect(run, id).origin;
    Rect::new(
        origin.x + r.min_x(),
        origin.y + r.min_y(),
        r.width(),
        r.height(),
    )
}

fn middle(r: Rect, along: f64) -> Point {
    Point::new(r.min_x() + r.width() * along, r.min_y() + r.height() / 2.0)
}

/// The rows of the menu window `depth`, each with whether it is enabled.
fn rows_of(run: &Run, depth: usize) -> Vec<(String, bool)> {
    let id = level(run, depth).expect("the menu is open");
    let snapshot = run.driver.runtime().snapshot(id).unwrap();
    snapshot
        .accessibility
        .iter()
        .filter(|(_, node)| node.role == Role::MenuItem)
        .filter_map(|(_, node)| Some((node.name.clone()?, !node.states.disabled)))
        .collect()
}

/// The focused row of the menu window `depth`.
fn focused_row(run: &Run, depth: usize) -> Option<String> {
    let id = level(run, depth)?;
    let snapshot = run.driver.runtime().snapshot(id).unwrap();
    let focus = snapshot.focus?;
    snapshot
        .accessibility
        .iter()
        .find(|(node, _)| *node == focus)
        .and_then(|(_, node)| node.name.clone())
}

#[test]
fn a_secondary_press_dragged_onto_a_row_chooses_it_once_and_a_quick_click_leaves_the_menu_open() {
    let mut run = run();
    let mut ptr = Pointer::new(&run);
    let (_, a, _) = run
        .listed()
        .into_iter()
        .find(|(name, _, _)| name == "a.txt")
        .unwrap();
    ptr.move_to(&mut run, a.center());
    ptr.button(&mut run, MouseButton::Right, true);
    let duplicate = row(&mut run, 0, "Duplicate");
    ptr.glide(&mut run, middle(duplicate, 0.5), 8, 16);
    ptr.button(&mut run, MouseButton::Right, false);
    assert!(level(&run, 0).is_none(), "the release chose and closed");
    run.wait_until(|run| run.rows().len() == 4);
    run.frames(200, 50);
    assert_eq!(run.rows().len(), 4, "one copy: {:?}", run.rows());

    // a quick secondary click, and a long one that reaches no row, leave the menu open
    let at = empty_space(&run);
    ptr.move_to(&mut run, at);
    ptr.button(&mut run, MouseButton::Right, true);
    ptr.wait(&mut run, 100);
    ptr.button(&mut run, MouseButton::Right, false);
    assert!(level(&run, 0).is_some());
    let new_folder = row(&mut run, 0, "New Folder");
    ptr.glide(&mut run, middle(new_folder, 0.5), 4, 16);
    ptr.button(&mut run, MouseButton::Left, true);
    ptr.button(&mut run, MouseButton::Left, false);
    run.wait_until(|run| run.home().join("Documents/untitled folder").is_dir());
}

/// The folder menu open on empty space with Sort By's submenu, opened by resting on its row.
fn sort_by_open(run: &mut Run, ptr: &mut Pointer) {
    let at = empty_space(run);
    ptr.move_to(run, at);
    ptr.button(run, MouseButton::Right, true);
    ptr.button(run, MouseButton::Right, false);
    let sort_by = row(run, 0, "Sort By");
    ptr.glide(run, middle(sort_by, 0.5), 4, 16);
    assert!(level(run, 1).is_none(), "not before the delay");
    ptr.wait(run, ms(SUBMENU_OPEN_DELAY) + 20);
    assert!(level(run, 1).is_some(), "Sort By's submenu");
}

#[test]
fn a_diagonal_into_sort_by_keeps_its_submenu_and_a_rest_or_a_slow_path_gives_it_up() {
    let mut run = run();
    let mut ptr = Pointer::new(&run);
    sort_by_open(&mut run, &mut ptr);
    let size = middle(row(&mut run, 1, "Size"), 0.3);
    ptr.glide(&mut run, size, 12, 16);
    assert!(level(&run, 1).is_some());
    assert_eq!(focused_row(&run, 0).as_deref(), Some("Sort By"));
    assert_eq!(focused_row(&run, 1).as_deref(), Some("Size"));
    ptr.button(&mut run, MouseButton::Left, true);
    ptr.button(&mut run, MouseButton::Left, false);
    assert!(level(&run, 0).is_none(), "Size chosen");

    // a rest on the way, over the rows below Sort By
    sort_by_open(&mut run, &mut ptr);
    let below = row(&mut run, 0, "Show View Options");
    let from = ptr.at;
    ptr.glide(&mut run, Point::new(from.x + 20.0, below.center().y), 3, 16);
    ptr.wait(&mut run, 16);
    assert!(level(&run, 1).is_some(), "heading into the submenu");
    ptr.wait(&mut run, ms(SUBMENU_AIM_PAUSE + SUBMENU_CLOSE_DELAY) + 50);
    assert!(level(&run, 1).is_none(), "resting gave it up");
    run.close_menu();

    // the same way, crawling
    sort_by_open(&mut run, &mut ptr);
    let size = middle(row(&mut run, 1, "Size"), 0.3);
    let from = ptr.at;
    let mut closed = false;
    for step in 1..=12 {
        let k = f64::from(step) / 12.0;
        ptr.wait(&mut run, 400);
        ptr.move_to(
            &mut run,
            Point::new(
                from.x + (size.x - from.x) * k,
                from.y + (size.y - from.y) * k,
            ),
        );
        if level(&run, 1).is_none() {
            closed = true;
            break;
        }
    }
    assert!(closed, "crawling over the rows below gave the submenu up");
}

#[test]
fn typed_letters_find_a_row_return_chooses_it_and_escape_closes_one_level_then_the_menu() {
    let mut run = run();
    run.click_row("a.txt");
    let main = run.window;
    let before = run.driver.runtime().snapshot(main).unwrap().focus;
    run.key(MENU, None);
    let menu = level(&run, 0).expect("open");
    run.window = menu;
    run.driver
        .window_system_mut()
        .configure(menu, None, WindowStates::default())
        .unwrap();
    run.settle();
    for c in "dup".chars() {
        run.key(c as u32, Some(c));
    }
    assert_eq!(focused_row(&run, 0).as_deref(), Some("Duplicate"));
    press(&mut run, RETURN);
    run.window = main;
    assert!(level(&run, 0).is_none());
    run.wait_until(|run| run.rows().len() == 4);

    // the folder menu: Sort By's submenu by the keyboard, then Escape twice
    run.click_row("b.txt");
    let before_folder = run.driver.runtime().snapshot(main).unwrap().focus;
    let at = empty_space(&run);
    run.press_at(at, MouseButton::Right);
    let menu = level(&run, 0).expect("open");
    run.window = menu;
    run.driver
        .window_system_mut()
        .configure(menu, None, WindowStates::default())
        .unwrap();
    run.settle();
    run.frames(1100, 100);
    for c in "sor".chars() {
        run.key(c as u32, Some(c));
    }
    assert_eq!(focused_row(&run, 0).as_deref(), Some("Sort By"));
    run.key(RIGHT, None);
    let submenu = level(&run, 1).expect("the submenu");
    run.window = submenu;
    run.driver
        .window_system_mut()
        .configure(submenu, None, WindowStates::default())
        .unwrap();
    run.settle();
    press(&mut run, ESCAPE);
    run.window = menu;
    assert!(level(&run, 1).is_none() && level(&run, 0).is_some());
    assert_eq!(
        focused_row(&run, 0).as_deref(),
        Some("Sort By"),
        "back on its row"
    );
    press(&mut run, ESCAPE);
    run.window = main;
    assert!(level(&run, 0).is_none());
    let after = run.driver.runtime().snapshot(main).unwrap().focus;
    assert_eq!(after, before_folder, "the focus is back where it was");
    assert!(before.is_some());
}

/// A secondary press on `a.txt` whose menu the compositor slid to fit, leaving Duplicate under the pointer, which
/// never moves but for a twitch; the release of the press `held_ms` later.
fn release_on_a_menu_slid_under_the_pointer(run: &mut Run, held_ms: u64) {
    let mut ptr = Pointer::new(run);
    let (_, a, _) = run
        .listed()
        .into_iter()
        .find(|(name, _, _)| name == "a.txt")
        .unwrap();
    ptr.move_to(run, a.center());
    ptr.button(run, MouseButton::Right, true);
    let menu = level(run, 0).expect("open on the press");
    let duplicate = row(run, 0, "Duplicate");
    let origin = rect(run, menu).origin;
    let under = Point::new(
        duplicate.center().x - origin.x,
        duplicate.center().y - origin.y,
    );
    let twitch = Point::new(under.x + 1.0, under.y);
    ptr.pointer(run, ptr.main, PointerEvent::Leave);
    ptr.pointer(run, menu, PointerEvent::Enter { position: under });
    ptr.pointer(run, menu, PointerEvent::Motion { position: twitch });
    ptr.wait(run, held_ms);
    ptr.pointer(
        run,
        menu,
        PointerEvent::Button {
            position: twitch,
            button: MouseButton::Right,
            pressed: false,
            serial: 1,
            mods: Mods::NONE,
        },
    );
    ptr.settle(run);
}

#[test]
fn a_secondary_click_whose_menu_opens_with_a_row_under_the_pointer_does_not_choose_it() {
    let mut run = run();
    release_on_a_menu_slid_under_the_pointer(&mut run, 30);
    assert!(
        level(&run, 0).is_some(),
        "a quick click leaves the menu open and chooses nothing"
    );
    run.frames(200, 50);
    assert_eq!(run.rows().len(), 3, "nothing duplicated: {:?}", run.rows());
    run.close_menu();
    assert!(run.context_menu().is_none());

    release_on_a_menu_slid_under_the_pointer(&mut run, ms(TRACKING_INTERVAL) + 10);
    assert!(
        level(&run, 0).is_none(),
        "held, the release chose and closed"
    );
    run.wait_until(|run| run.rows().len() == 4);
}
