//! Selecting by typing and with the rubber band, in the browser window on CupertinoKit run headless, through the
//! keyboard and the pointer: a name typed in the list or the icon grid selects the item it starts and scrolls it into
//! view; a band held at the top or the bottom edge of the icon grid or the list scrolls the items under it, selects
//! those it passes and stops when it is let go. In the list a band starts on a row's blank part, as under the rows.

#![cfg(feature = "app")]

use crate::common;

use std::collections::HashMap;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Role;
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::platform::{Mods, MouseButton, PointerEvent, Timestamp, WindowEvent};

/// A window on `home/Documents`, holding `item 00.txt` to `item 59.txt` and `zeta.txt`, which sorts last.
fn window() -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    for i in 0..60 {
        std::fs::write(documents.join(format!("item {i:02}.txt")), "x").unwrap();
    }
    std::fs::write(documents.join("zeta.txt"), "z").unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let mut run = Run::start(home, documents, places, Notices::start(None), None);
    run.wait_until(|run| run.rows().iter().any(|r| r == "item 00.txt"));
    run
}

/// Whether `inner` lies wholly inside `outer`.
fn within(inner: Rect, outer: Rect) -> bool {
    inner.min_y() >= outer.min_y() - 0.5 && inner.max_y() <= outer.max_y() + 0.5
}

#[test]
fn a_name_typed_in_the_list_selects_its_item_and_scrolls_it_into_view() {
    let mut run = window();
    let table = run.bounds(Role::Table, "Documents");
    assert!(
        !run.rows().contains(&"zeta.txt".to_owned()),
        "out of view at first"
    );
    run.click_row("item 00.txt");
    run.type_text("zet");
    assert_eq!(run.selected_rows(), ["zeta.txt"]);
    let zeta = run.row_bounds("zeta.txt");
    assert!(within(zeta, table), "{zeta:?} in {table:?}");

    // a new name after the pause selects again, back up the list
    run.frames(3_000, 500);
    run.type_text("item 04");
    assert_eq!(run.selected_rows(), ["item 04.txt"]);
    assert!(within(run.row_bounds("item 04.txt"), table));
}

/// The icon grid's items in the window, by name.
fn cells(run: &Run) -> HashMap<String, Rect> {
    run.icon_cells()
        .into_iter()
        .map(|(name, rect, _)| (name, rect))
        .collect()
}

#[test]
fn a_name_typed_in_the_icon_grid_selects_its_item_and_scrolls_it_into_view() {
    let mut run = window();
    run.menu("view-icons");
    run.wait_until(|run| cells(run).contains_key("item 00.txt"));
    let grid = run.bounds(Role::List, "Documents");
    let zeta = |run: &Run| cells(run).get("zeta.txt").copied();
    assert!(
        zeta(&run).is_none_or(|z| !within(z, grid)),
        "out of view at first"
    );
    run.click_icon("item 00.txt");
    run.type_text("zeta");
    assert_eq!(run.selected_icons(), ["zeta.txt"]);
    let shown = zeta(&run).expect("the item is shown");
    assert!(within(shown, grid), "{shown:?} in {grid:?}");
}

/// The End key.
const END: u32 = 0xff57;

fn pointer(run: &mut Run, event: PointerEvent) {
    run.send(WindowEvent::Pointer(event, Timestamp(0)));
}

fn button(run: &mut Run, position: Point, pressed: bool) {
    pointer(
        run,
        PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        },
    );
}

#[test]
fn a_band_held_at_the_top_edge_scrolls_the_grid_back_selecting_the_items_it_passes_and_stops_when_let_go(
) {
    let mut run = window();
    run.menu("view-icons");
    run.wait_until(|run| cells(run).contains_key("item 00.txt"));
    let grid = run.bounds(Role::List, "Documents");
    // to the end of the folder
    run.click_icon("item 00.txt");
    run.key(END, None);
    assert_eq!(run.selected_icons(), ["zeta.txt"]);
    let last = cells(&run)["zeta.txt"];
    assert!(within(last, grid));
    // in the gap before the last item, up to the top edge, in its column
    let from = Point::new(last.min_x() - 1.0, last.min_y() + 2.0);
    let edge = Point::new(last.center().x, grid.min_y() + 4.0);
    pointer(&mut run, PointerEvent::Motion { position: from });
    button(&mut run, from, true);
    pointer(&mut run, PointerEvent::Motion { position: edge });
    let before = run.selected_icons();
    let at = cells(&run)["zeta.txt"].min_y();
    // the pointer stays still at the edge: the content goes on scrolling down under it
    run.frames(600, 50);
    assert!(
        cells(&run).get("zeta.txt").is_none_or(|z| z.min_y() > at),
        "the items moved down under the band"
    );
    let after = run.selected_icons();
    assert!(
        after.len() > before.len(),
        "the band holds the items it passed: {before:?} then {after:?}"
    );
    // the last item's column, every row the band passed in view, with no gap
    let after = indices(&after);
    let step = after.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>();
    assert!(step.iter().all(|&s| s == step[0]), "{after:?}");
    button(&mut run, edge, false);
    let held = cells(&run);
    let selected = run.selected_icons();
    run.frames(400, 50);
    assert_eq!(cells(&run), held, "a release stops the scroll");
    assert_eq!(run.selected_icons(), selected);
}

/// The index in `item NN.txt` of each name, in order.
fn indices(names: &[String]) -> Vec<usize> {
    let mut indices: Vec<usize> = names
        .iter()
        .filter_map(|n| n.strip_prefix("item ")?.strip_suffix(".txt")?.parse().ok())
        .collect();
    indices.sort_unstable();
    indices
}

#[test]
fn a_band_held_at_the_bottom_edge_scrolls_the_grid_on_selecting_the_rows_it_passes_and_stops_when_let_go(
) {
    let mut run = window();
    run.menu("view-icons");
    run.wait_until(|run| cells(run).contains_key("item 00.txt"));
    let grid = run.bounds(Role::List, "Documents");
    let first = cells(&run)["item 00.txt"];
    let columns = cells(&run)
        .values()
        .filter(|r| (r.min_y() - first.min_y()).abs() < 0.5)
        .count();
    // in the gap after the first item, down to the bottom edge, in its column
    let from = Point::new(first.max_x() + 1.0, first.min_y() + 2.0);
    let edge = Point::new(first.center().x, grid.max_y() - 4.0);
    pointer(&mut run, PointerEvent::Motion { position: from });
    button(&mut run, from, true);
    pointer(&mut run, PointerEvent::Motion { position: edge });
    let before = indices(&run.selected_icons());
    run.frames(800, 50);
    let after = indices(&run.selected_icons());
    assert!(after.len() > before.len(), "{before:?} then {after:?}");
    // the first column, every row the band passed, with no gap
    // (the rows scrolled out of view above are not read)
    let expected: Vec<usize> = (0..after.len())
        .map(|row| after[0] + row * columns)
        .collect();
    assert_eq!(after, expected, "{columns} columns");
    assert_eq!(after[0] % columns, 0);
    button(&mut run, edge, false);
    let held = cells(&run);
    run.frames(400, 50);
    assert_eq!(cells(&run), held, "a release stops the scroll");
    assert_eq!(indices(&run.selected_icons()), after);
}

/// The numbers of the `item NN.txt` rows selected in the list, in order.
fn selected_items(run: &Run) -> Vec<usize> {
    run.selected_rows()
        .iter()
        .filter_map(|name| {
            name.strip_prefix("item ")?
                .strip_suffix(".txt")?
                .parse()
                .ok()
        })
        .collect()
}

#[test]
fn a_band_from_a_rows_blank_part_held_at_the_bottom_edge_scrolls_the_list_on_selecting_the_rows_it_passes(
) {
    let mut run = window();
    let table = run.bounds(Role::Table, "Documents");
    // a click on a row's blank part selects the row, as a click on its name does
    let blank = |run: &Run, name: &str| {
        let row = run.row_bounds(name);
        Point::new(row.max_x() - 60.0, row.center().y)
    };
    let at = blank(&run, "item 02.txt");
    pointer(&mut run, PointerEvent::Motion { position: at });
    button(&mut run, at, true);
    button(&mut run, at, false);
    run.frames(100, 50);
    assert_eq!(run.selected_rows(), ["item 02.txt"]);

    // a press there that moves draws a band, not a drag, and the edge scrolls the rows on under it
    let from = blank(&run, "item 03.txt");
    let edge = Point::new(from.x, table.max_y() - 4.0);
    pointer(&mut run, PointerEvent::Motion { position: from });
    button(&mut run, from, true);
    pointer(
        &mut run,
        PointerEvent::Motion {
            position: Point::new(from.x - 2.0, from.y + 20.0),
        },
    );
    pointer(&mut run, PointerEvent::Motion { position: edge });
    let before = selected_items(&run);
    assert_eq!(before.first(), Some(&3), "{before:?}");
    run.frames(800, 50);
    let after = selected_items(&run);
    assert!(after.len() > before.len(), "{before:?} then {after:?}");
    assert!(
        after.last() > before.last(),
        "rows past the first view are selected: {before:?} then {after:?}"
    );
    // every row the band passed that is still in view, with no gap
    let expected: Vec<usize> = (after[0]..after[0] + after.len()).collect();
    assert_eq!(after, expected);
    button(&mut run, edge, false);
    let held = run.rows();
    run.frames(400, 50);
    assert_eq!(run.rows(), held, "a release stops the scroll");
    assert_eq!(selected_items(&run), after);
}

/// The bounds of the name cell (the first) of the list's row that shows `name`.
fn name_cell(run: &Run, name: &str) -> Rect {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let nodes: HashMap<_, _> = snapshot
        .accessibility
        .iter()
        .map(|(id, n)| (*id, n))
        .collect();
    let row = run.row_bounds(name);
    snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.role == Role::Row && n.bounds == row)
        .and_then(|(_, n)| n.children.first())
        .map(|cell| nodes[cell].bounds)
        .unwrap_or_else(|| panic!("no name cell for {name:?}"))
}

#[test]
fn in_the_name_cell_a_band_starts_past_the_name_and_a_press_on_the_name_does_not_draw_one() {
    let mut run = window();
    let row_height = run.row_bounds("item 04.txt").min_y() - run.row_bounds("item 03.txt").min_y();

    // past the end of a short name, the name cell is the row's blank part: a move draws a band over the rows below
    let cell = name_cell(&run, "item 03.txt");
    let from = Point::new(cell.max_x() - 10.0, cell.center().y);
    let to = Point::new(from.x, from.y + 2.0 * row_height);
    pointer(&mut run, PointerEvent::Motion { position: from });
    button(&mut run, from, true);
    pointer(
        &mut run,
        PointerEvent::Motion {
            position: Point::new(from.x, from.y + row_height),
        },
    );
    pointer(&mut run, PointerEvent::Motion { position: to });
    button(&mut run, to, false);
    run.frames(100, 50);
    assert_eq!(selected_items(&run), [3, 4, 5]);

    // on the name's text the press grabs the row: a move draws no band
    let cell = name_cell(&run, "item 10.txt");
    let from = Point::new(cell.min_x() + 26.0, cell.center().y);
    let to = Point::new(from.x, from.y + 2.0 * row_height);
    pointer(&mut run, PointerEvent::Motion { position: from });
    button(&mut run, from, true);
    pointer(
        &mut run,
        PointerEvent::Motion {
            position: Point::new(from.x, from.y + row_height),
        },
    );
    pointer(&mut run, PointerEvent::Motion { position: to });
    button(&mut run, to, false);
    run.frames(100, 50);
    assert_eq!(selected_items(&run), [10]);
}
