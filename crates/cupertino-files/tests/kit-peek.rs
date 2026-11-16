//! The preview window, run headless: Space or File › Quick Preview opens it on the selection, as a panel of the browser
//! window, which keeps the keyboard and its menus; the arrows go through a selection of several items or move a single
//! one; Space or Escape closes it. It opens once its item is ready and only then grows, out of the item it shows, and
//! shrinks back into the item as it closes; Reduce Motion shows and takes it away at once.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Host, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::window::ViewMode;
use cupertino_kit::core::{Key, Role};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::platform::{
    Capabilities, Mods, PointerEvent, PopupEdge, PresentChange, PresentProperty, ScrollSource,
    Timestamp, WindowEvent, WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::theme::{Appearance, ThemePreferences};

const SPACE: u32 = 0x20;
const ESCAPE: u32 = 0xff1b;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;

fn preferences(reduce_motion: bool) -> ThemePreferences {
    ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion,
    }
}

/// A window on `home/Documents` (`a.txt`, `b.txt`, `c.bin`), in the list.
fn run(reduce_motion: bool) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::write(documents.join("a.txt"), "alpha line\nsecond line\n").unwrap();
    std::fs::write(documents.join("b.txt"), "bravo\n").unwrap();
    std::fs::write(documents.join("c.bin"), [0u8, 1, 2, 3]).unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let mut run = Run::start_with(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences(reduce_motion),
    );
    run.wait_until(|run| run.rows().len() == 3);
    run
}

/// A window on a folder of 60 files that are not text (`item-00.bin` …), in `view`, on a window system that hands
/// popups' transitions to the compositor.
fn long_run(view: ViewMode) -> Run {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("Documents");
    std::fs::create_dir_all(&folder).unwrap();
    for i in 0..60 {
        std::fs::write(folder.join(format!("item-{i:02}.bin")), [0u8, 1]).unwrap();
    }
    let places = vec![place("Documents", &folder, PlaceKind::Documents)];
    let host = Host {
        capabilities: Capabilities {
            present_popups: true,
            ..Capabilities::default()
        },
        ..Host::default()
    };
    let mut run = Run::start_hosted(
        home,
        folder,
        places,
        Notices::start(None),
        None,
        preferences(false),
        |start| start,
        host,
    );
    run.wait_until(|run| !run.rows().is_empty());
    match view {
        ViewMode::Icons => {
            run.menu("view-icons");
            run.wait_until(|run| !run.icons().is_empty());
        }
        ViewMode::Columns => {
            run.menu("view-columns");
            run.wait_until(|run| column_row(run, "item-03.bin").is_some());
        }
        _ => {}
    }
    run
}

/// The row of the column view that shows the item `name` of the window's folder, once laid out.
fn column_row(run: &Run, name: &str) -> Option<Rect> {
    let path = run.home().join("Documents").join(name);
    let key = Key::from(format!("row/0/{}", path.display()));
    let runtime = run.driver.runtime();
    let id = runtime.find(run.window, &key)?;
    let snapshot = runtime.snapshot(run.window).unwrap();
    Some(snapshot.nodes.iter().find(|n| n.id == id)?.rect)
}

/// The rectangle of the node keyed `key` in the browser window.
fn node_rect(run: &Run, key: &'static str) -> Rect {
    let runtime = run.driver.runtime();
    let id = runtime
        .find(run.window, &Key::from(key))
        .unwrap_or_else(|| panic!("no node {key}"));
    let snapshot = runtime.snapshot(run.window).unwrap();
    snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect
}

/// The preview window, when one is open besides the browser window (not one closing).
fn preview(run: &Run) -> Option<WindowId> {
    let windows = run.driver.runtime().windows();
    windows
        .into_iter()
        .find(|(key, _)| format!("{key:?}").contains("\"preview/"))
        .map(|(_, id)| id)
}

/// A preview window that is shrinking away.
fn closing(run: &Run) -> bool {
    // the view no longer declares it, and the window system still shows it
    let declared = run.driver.runtime().windows().len();
    run.driver.runtime().window_system().windows().len() > declared
}

/// The preview window was opened: the compositor places it at `placed` in the browser window, configures it, and its
/// motion runs.
fn shown_at(run: &mut Run, placed: Option<Rect>) {
    let window = preview(run).expect("a preview window");
    let system = run.driver.window_system_mut();
    if let Some(placed) = placed {
        system
            .enqueue_window(window, WindowEvent::PopupPlaced(placed))
            .unwrap();
    }
    system
        .configure(window, None, WindowStates::default())
        .unwrap();
    run.frames(500, 16);
}

fn shown(run: &mut Run) {
    shown_at(run, None);
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
    // the text is read away from the window's thread; the preview opens with it
    run.wait_until(|run| preview(run).is_some());
    shown(&mut run);
    let texts = preview_texts(&run);
    assert!(texts.iter().any(|t| t == "a.txt"), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.starts_with("alpha line")),
        "the text file's start, with the window's first frame: {texts:?}"
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
    assert!(
        preview(&run).is_none() && closing(&run),
        "it shrinks away first: {:?}",
        run.driver.runtime().windows()
    );
    run.frames(500, 16);
    assert!(!closing(&run));
}

/// The bar of the menus window `window` publishes.
fn menus(run: &Run, window: WindowId) -> Option<cupertino_kit::platform::MenuDescription> {
    let system = run.driver.runtime().window_system();
    system.menus(window).unwrap().cloned()
}

#[test]
fn the_preview_leaves_the_menu_bar_and_the_keyboard_with_its_window() {
    let mut run = run(false);
    run.click_row("a.txt");
    let before = menus(&run, run.window).expect("the browser window's menus");
    run.key(SPACE, Some(' '));
    run.wait_until(|run| preview(run).is_some());
    shown(&mut run);
    let window = preview(&run).unwrap();
    // a panel of the browser window, which stays the window the menu bar follows: it publishes no menus of its own
    let system = run.driver.runtime().window_system();
    assert_eq!(system.parent(window).unwrap(), Some(run.window));
    let placement = system.placement(window).unwrap().expect("a popup");
    assert!(!placement.grab, "it takes neither the keyboard nor a grab");
    assert_eq!(menus(&run, window), None);
    let during = menus(&run, run.window).expect("the browser window keeps its menus");
    assert_eq!(during.bar, before.bar);
    assert!(
        !during.disabled.contains(&common::action("peek")),
        "File › Quick Preview closes it"
    );

    // the keyboard stays on the browser window: Escape there closes the preview, and the menus are as they were
    run.key(ESCAPE, None);
    assert!(preview(&run).is_none());
    run.frames(500, 16);
    let after = menus(&run, run.window).expect("the browser window's menus");
    assert_eq!(after, before);
    assert!(run.windows().len() == 1, "{:?}", run.windows());

    // in full screen the preview is a window of its own, which shows the browser window's menus
    run.key(SPACE, Some(' '));
    run.wait_until(|run| preview(run).is_some());
    shown(&mut run);
    let browser = run.window;
    run.window = preview(&run).unwrap();
    run.click("full-screen");
    run.window = browser;
    run.settle();
    let full = run
        .driver
        .runtime()
        .windows()
        .into_iter()
        .find(|(key, _)| format!("{key:?}").contains("preview-full-screen/"))
        .map(|(_, id)| id)
        .expect("the full screen preview");
    let system = run.driver.runtime().window_system();
    assert!(system
        .requests(full)
        .unwrap()
        .contains(&WindowRequest::Fullscreen(true)));
    assert_eq!(system.parent(full).unwrap(), None);
    assert_eq!(
        menus(&run, full).map(|m| m.bar),
        Some(before.bar),
        "the full screen preview shows the browser window's menus"
    );
}

#[test]
fn the_arrows_go_through_a_selection_of_several_and_move_a_single_one() {
    let mut run = run(false);
    run.click_row("a.txt");
    run.click_row_with("b.txt", Mods::SHIFT);
    run.menu("peek");
    run.wait_until(|run| preview(run).is_some());
    shown(&mut run);
    assert!(preview_texts(&run).iter().any(|t| t == "a.txt"));
    assert!(preview_texts(&run).iter().any(|t| t == "1 of 2"));
    assert!(preview_texts(&run).iter().any(|t| t.starts_with("alpha")));
    // the arrows on the browser window step the preview; what it shows next is never the item before
    run.key(RIGHT, None);
    let texts = preview_texts(&run);
    assert!(texts.iter().any(|t| t == "b.txt"), "{texts:?}");
    assert!(
        !texts.iter().any(|t| t.starts_with("alpha")),
        "no text of the item before: {texts:?}"
    );
    assert_eq!(
        run.selected_rows(),
        ["a.txt", "b.txt"],
        "the selection stays"
    );
    run.wait_until(|run| preview_texts(run).iter().any(|t| t.starts_with("bravo")));

    // one item: the arrows move the window's selection, and the preview follows it
    run.click_row("a.txt");
    run.key(DOWN, None);
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
fn a_preview_the_screen_has_no_room_for_shows_whole_at_the_smaller_size_it_is_given() {
    let mut run = run(false);
    run.click_row("c.bin");
    run.key(SPACE, Some(' '));
    let window = preview(&run).expect("an item shown by its icon is ready at once");
    // the compositor shrinks the panel to the room left between the menu bar and the dock
    let room = cupertino_kit::geometry::Size::new(500.0, 400.0);
    run.driver
        .window_system_mut()
        .configure(window, Some(room), WindowStates::default())
        .unwrap();
    run.frames(500, 16);
    let runtime = run.driver.runtime();
    let snapshot = runtime.snapshot(window).unwrap();
    let sheet = runtime.find(window, &Key::from("sheet")).unwrap();
    let rect = snapshot.nodes.iter().find(|n| n.id == sheet).unwrap().rect;
    assert_eq!((rect.width(), rect.height()), (500.0, 400.0));
    // its toolbar and its item's name are still there, inside it
    let close = run_bounds(&run, window, Role::Button, "Close");
    assert!(close.max_y() <= 400.0 && close.min_x() >= 0.0, "{close:?}");
    assert!(preview_texts(&run).iter().any(|t| t == "c.bin"));
}

/// The bounds of the accessible node of `window` with `role` and `name`.
fn run_bounds(run: &Run, window: WindowId, role: Role, name: &str) -> Rect {
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
        .map(|(_, n)| n.bounds)
        .unwrap_or_else(|| panic!("no {role:?} {name:?}"))
}

#[test]
fn reduce_motion_shows_and_takes_the_preview_away_at_once() {
    let mut run = run(true);
    run.click_row("c.bin");
    run.key(SPACE, Some(' '));
    assert!(
        preview(&run).is_some(),
        "an item shown by its icon is ready at once"
    );
    shown(&mut run);
    run.key(ESCAPE, None);
    assert!(preview(&run).is_none());
    assert!(!closing(&run), "{:?}", run.driver.runtime().windows());
}

/// The point of the browser window a popup placed at `placed` grows out of (`grow`) or shrinks into (`close`), read
/// off the offsets the window system was asked to move it by at the start of the one and the end of the other.
fn moved_from(changes: &[PresentChange], placed: Rect, size: (f64, f64), scale: f64) -> Point {
    let offset = |property: PresentProperty| {
        changes
            .iter()
            .find(|c| c.property == property)
            .expect("an offset")
            .to
    };
    // the window scaled about its centre to `scale`, then moved: the point that stays is the origin
    let (x, y) = (
        offset(PresentProperty::OffsetX),
        offset(PresentProperty::OffsetY),
    );
    let (w, h) = size;
    Point::new(
        placed.min_x() + w / 2.0 + x / (1.0 - scale),
        placed.min_y() + h / 2.0 + y / (1.0 - scale),
    )
}

/// The presentation changes the preview window `window` was asked for, each request apart.
fn presents(run: &Run, window: WindowId) -> Vec<Vec<PresentChange>> {
    let system = run.driver.runtime().window_system();
    let requests = system.requests(window).unwrap();
    requests
        .iter()
        .filter_map(|r| match r {
            WindowRequest::Present(changes) => Some(changes.clone()),
            _ => None,
        })
        .collect()
}

/// Open the preview of the item at `item` (its bounds in the browser window), with the compositor placing it where
/// the window asks; check that it lies over the folder, no higher than the window's top, grows out of the item and
/// shrinks back into it.
fn grows_out_of_and_back_into(run: &mut Run, item: Rect) {
    run.key(SPACE, Some(' '));
    let window = preview(run).expect("a preview of an item that is not text opens at once");
    let system = run.driver.runtime().window_system();
    let spec = system.placement(window).unwrap().unwrap().clone();
    assert_eq!(
        (spec.anchor, spec.gravity),
        (PopupEdge::Top, PopupEdge::Bottom)
    );
    assert_eq!(
        spec.anchor_rect,
        node_rect(run, "folder"),
        "over the folder"
    );
    let top = spec.anchor_rect.min_y() + spec.offset.y;
    assert!(top >= 0.0, "never above the window's top edge: {spec:?}");
    assert!(
        spec.constraint.slide && spec.constraint.resize && !spec.constraint.flip,
        "it slides, and shrinks where sliding is not enough, to fit the room the screen has: {spec:?}"
    );
    // as the compositor places it, centred on the middle of the folder's top edge
    let placed = Rect::new(
        spec.anchor_rect.min_x() + spec.anchor_rect.width() / 2.0 - spec.size.width / 2.0,
        top,
        spec.size.width,
        spec.size.height,
    );
    // no frame of it yet: nothing asked of its presentation before its first buffer
    assert!(presents(run, window).is_empty());
    run.frames(400, 100);
    assert!(
        presents(run, window).is_empty(),
        "the motion waits for the first configure"
    );
    shown_at(run, Some(placed));
    let size = (spec.size.width, spec.size.height);
    let grow = presents(run, window);
    assert_eq!(grow.len(), 1, "one motion, from its first frame: {grow:?}");
    assert_eq!(grow[0][0].property, PresentProperty::Scale);
    assert!(grow[0][0].to < 0.2, "it starts small: {:?}", grow[0][0]);
    let from = moved_from(&grow[0][..3], placed, size, grow[0][0].to);
    let middle = Point::new(
        item.min_x() + item.width() / 2.0,
        item.min_y() + item.height() / 2.0,
    );
    assert!(
        (from.x - middle.x).abs() < 1.0 && (from.y - middle.y).abs() < 1.0,
        "it grows out of the item at {middle:?}, not {from:?}"
    );
    run.key(SPACE, Some(' '));
    let close = presents(run, window);
    assert_eq!(close.len(), 2, "{close:?}");
    let into = moved_from(&close[1], placed, size, 0.0);
    assert!(
        (into.x - middle.x).abs() < 1.0 && (into.y - middle.y).abs() < 1.0,
        "it shrinks back into the item at {middle:?}, not {into:?}"
    );
    run.frames(500, 16);
}

#[test]
fn the_preview_grows_out_of_an_item_near_the_bottom_of_a_scrolled_list() {
    let mut run = long_run(ViewMode::List);
    // the list scrolled down far enough to bring its last rows into view: the preview grows out of the row clicked
    let list = node_rect(&run, "list");
    for _ in 0..30 {
        run.send(WindowEvent::Pointer(
            PointerEvent::Scroll {
                position: list.center(),
                delta: Point::new(0.0, 120.0),
                value120: None,
                source: ScrollSource::Wheel,
                stop: false,
                inverted: (false, false),
            },
            Timestamp(0),
        ));
    }
    run.frames(1500, 100);
    let rows = run.listed();
    let (name, _, _) = rows
        .iter()
        .rev()
        .find(|(_, rect, _)| rect.min_y() > 100.0)
        .cloned()
        .unwrap();
    assert!(name.starts_with("item-5"), "scrolled near the end: {name}");
    run.click_row(&name);
    let item = run.row_bounds(&name);
    assert!(item.min_y() > list.center().y, "near the bottom: {item:?}");
    grows_out_of_and_back_into(&mut run, item);
}

#[test]
fn the_preview_grows_out_of_an_icon_and_out_of_a_column_s_row() {
    let mut run = long_run(ViewMode::Icons);
    let (name, _, _) = run.icon_cells().into_iter().nth(5).unwrap();
    run.click_icon(&name);
    let (_, rect, _) = run.icon_cells().into_iter().nth(5).unwrap();
    grows_out_of_and_back_into(&mut run, rect);

    let mut run = long_run(ViewMode::Columns);
    let row = column_row(&run, "item-03.bin").unwrap();
    run.press_at(row.center(), cupertino_kit::platform::MouseButton::Left);
    run.settle();
    let item = column_row(&run, "item-03.bin").unwrap();
    grows_out_of_and_back_into(&mut run, item);
}
