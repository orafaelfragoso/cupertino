//! The examples' applications driven by input on the headless window system, checked against Inspector dumps in
//! `tests/goldens/`. `CUPERTINO_UPDATE_GOLDENS=1` rewrites the dumps.

#[path = "../examples/apps/counter.rs"]
mod counter;
#[path = "../examples/apps/forms.rs"]
mod forms;
#[path = "../examples/apps/virtual-list.rs"]
mod virtual_list;

mod support;

use cupertino_kit::core::{Key, Role};
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::Mods;
use support::{Run, RETURN, SPACE, TAB};
use virtual_list::{VirtualList, ROW_HEIGHT};

const DOWN: u32 = 0xff54;
const PAGE_UP: u32 = 0xff55;
const END: u32 = 0xff57;

#[test]
fn the_counter_counts_clicks_and_keys() {
    let mut run = Run::<counter::Counter>::new();
    run.click("more");
    run.click("more");
    run.key(TAB, None);
    run.key(SPACE, Some(' '));
    assert_eq!(*run.model(), 1);
    run.golden("counter");
}

#[test]
fn the_form_takes_a_name_a_password_and_a_choice() {
    let mut run = Run::<forms::Forms>::new();
    run.key(TAB, None);
    run.type_text("Ada");
    run.key(TAB, None);
    run.type_text("hunter2");
    run.key(RETURN, None);
    run.key(TAB, None);
    run.key(SPACE, Some(' '));
    let account = run.model();
    assert_eq!(
        (account.name.as_str(), account.signed_in, account.remember),
        ("Ada", Some(7), true)
    );
    let dump = run.golden("forms");
    assert!(!dump.contains("hunter2"));
}

fn selected<const N: usize>(run: &Run<VirtualList<N>>) -> Vec<(usize, usize)> {
    let selection = &run.model().selection;
    selection
        .ranges()
        .iter()
        .map(|r| (r.start, r.end))
        .collect()
}

#[test]
fn the_virtual_list_moves_selects_scrolls_and_reverses_50_000_rows() {
    let mut run = Run::<VirtualList<50_000>>::new();
    let viewport = run.model().scroll.viewport.height;
    run.key(TAB, None);
    for _ in 0..3 {
        run.key(DOWN, None);
    }
    run.chord(DOWN, None, Mods::SHIFT);
    assert_eq!(selected(&run), [(2, 4)]);
    run.key(END, None);
    assert_eq!(selected(&run), [(49_999, 50_000)]);
    assert_eq!(
        run.model().scroll.offset.y,
        50_000.0 * ROW_HEIGHT - viewport
    );
    run.key(PAGE_UP, None);
    let page = (viewport / ROW_HEIGHT).floor() as usize;
    assert_eq!(selected(&run), [(49_999 - page, 50_000 - page)]);
    let offset = run.model().scroll.offset.y;
    run.wheel(Point::new(240.0, 200.0), -100.0);
    let offset = offset - 100.0;
    assert_eq!(run.model().scroll.offset.y, offset);
    let row = (offset / ROW_HEIGHT).ceil() as usize;
    run.click(Key::Int(row as u64));
    assert_eq!(selected(&run), [(row, row + 1)]);
    run.click("reverse");
    assert_eq!(selected(&run), [(49_999 - row, 50_000 - row)]);
    assert!(run.model().descending);
    run.golden("virtual-list");
}

/// The rows built, the nodes the last layout sized and the nodes painted: first, then after End.
fn work<const N: usize>() -> ([(usize, usize, usize); 2], f64) {
    let mut run = Run::<VirtualList<N>>::new();
    let viewport = run.model().scroll.viewport.height;
    let measure = |run: &Run<VirtualList<N>>| {
        let snapshot = run.snapshot();
        let rows = snapshot
            .accessibility
            .iter()
            .filter(|(_, n)| n.role == Role::ListItem)
            .count();
        (
            rows,
            snapshot.layout.solves,
            snapshot.display_list.nodes.len(),
        )
    };
    let first = measure(&run);
    run.key(TAB, None);
    run.key(END, None);
    ([first, measure(&run)], viewport)
}

#[test]
fn a_list_builds_lays_out_and_paints_as_much_for_50_000_rows_as_for_1_000() {
    let (small, viewport) = work::<1_000>();
    let bound = (2.0 * viewport / ROW_HEIGHT).ceil() as usize + 1 + 2;
    for (rows, _, _) in small {
        assert!(rows <= bound, "{rows} rows built, bound {bound}");
    }
    assert_eq!(work::<10_000>().0, small);
    assert_eq!(work::<50_000>().0, small);
}
