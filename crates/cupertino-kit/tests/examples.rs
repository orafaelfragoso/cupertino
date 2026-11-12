//! The examples' applications driven by input on the headless window system, checked against their traces and
//! Inspector dumps in `tests/goldens/`. `CUPERTINO_UPDATE_GOLDENS=1` rewrites the goldens.

#[path = "../examples/apps/counter.rs"]
mod counter;
#[path = "../examples/apps/editor.rs"]
mod editor;
#[path = "../examples/apps/forms.rs"]
mod forms;
#[path = "../examples/apps/virtual-list.rs"]
pub(crate) mod virtual_list;

use crate::support;

use cupertino_kit::core::{Key, Role};
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::Mods;
use cupertino_kit::theme::ThemeTokens;
use support::{Run, Traced, DOWN, END, LEFT, PAGE_UP, RETURN, SPACE, TAB};
use virtual_list::{VirtualList, ROW_HEIGHT};

#[test]
fn the_counter_counts_clicks_and_keys() {
    let mut run = Run::<Traced<counter::Counter>>::new();
    run.click("more");
    run.click("more");
    run.key(TAB, None);
    run.key(SPACE, Some(' '));
    assert_eq!(*run.model(), 1);
    run.golden("counter");
}

/// Points `CUPERTINO_SPEC_DIR` at an empty directory; no other test here reads the installed theme data.
#[test]
fn the_counter_starts_on_the_neutral_theme_without_theme_data_and_matches_its_golden() {
    let empty = tempfile::tempdir().unwrap();
    std::env::set_var(cupertino_theme::SPEC_DIR_ENV, empty.path());
    assert!(cupertino_theme::Theme::load().is_err());

    let options = cupertino_kit::app::options().unwrap();
    let neutral = ThemeTokens::neutral(cupertino_kit::app::start_preferences(None)).unwrap();
    assert_eq!(*options.theme, neutral);

    let mut run = Run::<Traced<counter::Counter>>::with_theme(neutral);
    run.click("more");
    run.click("more");
    run.key(TAB, None);
    run.key(SPACE, Some(' '));
    assert_eq!(*run.model(), 1);
    run.golden("counter-neutral");
}

#[test]
fn the_counter_golden_fails_when_two_inputs_swap_places() {
    let mut run = Run::<Traced<counter::Counter>>::new();
    run.key(TAB, None);
    run.key(SPACE, Some(' '));
    run.click("more");
    run.click("more");
    assert_eq!(*run.model(), 1);
    assert_eq!(
        run.compare("counter").unwrap_err(),
        "line 2:\n  expected: > click Str(\"more\")\n  actual:   > key Tab"
    );
}

/// Points `CUPERTINO_SPEC_DIR` at an empty directory, as the neutral Counter test does.
#[test]
fn the_editor_types_selects_and_undoes() {
    let empty = tempfile::tempdir().unwrap();
    std::env::set_var(cupertino_theme::SPEC_DIR_ENV, empty.path());
    let neutral = ThemeTokens::neutral(cupertino_kit::app::start_preferences(None)).unwrap();

    let mut run = Run::<Traced<editor::Editor>>::with_theme(neutral);
    run.click("text");
    run.type_text("Hello");
    run.key(RETURN, None);
    run.type_text("world");
    for _ in 0.."world".len() {
        run.chord(LEFT, None, Mods::SHIFT);
    }
    run.type_text("there");
    assert_eq!(run.model().text, "Hello\nthere");
    run.chord(u32::from('z'), Some('z'), Mods::PRIMARY);
    assert_eq!(run.model().text, "Hello\nworld");
    run.golden("editor");
}

#[test]
fn the_form_takes_a_name_a_password_and_a_choice() {
    let mut run = Run::<Traced<forms::Forms>>::new();
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
    // The password's seven keys, the Return that submits it and the Tab that leaves its field.
    assert_eq!(dump.matches("> key, protected\n").count(), 9);
}

fn selected<const N: usize>(run: &Run<Traced<VirtualList<N>>>) -> Vec<(usize, usize)> {
    let selection = &run.model().selection;
    selection
        .ranges()
        .iter()
        .map(|r| (r.start, r.end))
        .collect()
}

#[test]
fn the_virtual_list_moves_selects_scrolls_and_reverses_50_000_rows() {
    let mut run = Run::<Traced<VirtualList<50_000>>>::new();
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

/// The median of `times` in µs.
fn median_us(mut times: Vec<std::time::Duration>) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1e6
}

/// Time `step` on `run` `count` times: the median in µs, and the frames presented and the pixels they damaged over all
/// steps.
fn timed<const N: usize>(
    run: &mut Run<VirtualList<N>>,
    count: usize,
    mut step: impl FnMut(&mut Run<VirtualList<N>>),
) -> (f64, usize, u64) {
    let window = run.window;
    let before = run.driver.window_system_mut().frames(window).unwrap().len();
    let mut times = Vec::new();
    for _ in 0..count {
        let start = std::time::Instant::now();
        step(run);
        times.push(start.elapsed());
    }
    let presented = &run.driver.window_system_mut().frames(window).unwrap()[before..];
    let damaged = presented
        .iter()
        .flat_map(|f| &f.damage)
        .map(|r| u64::from(r.width) * u64::from(r.height))
        .sum();
    (median_us(times), presented.len(), damaged)
}

/// One row of the Virtual List matrix at `scale`: open, 30 Down keys, 30 wheel detents and End, through the runtime,
/// layout, paint and the CPU renderer onto the headless window system.
fn virtual_list_row<const N: usize>(scale: f64) {
    let start = std::time::Instant::now();
    let mut run = Run::<VirtualList<N>>::new();
    run.send(cupertino_kit::platform::WindowEvent::Scale(scale));
    let open = start.elapsed().as_secs_f64() * 1e3;
    run.key(TAB, None);
    let centre = Point::new(240.0, 300.0);
    let (down, down_frames, down_px) = timed(&mut run, 30, |run| run.key(DOWN, None));
    let (wheel, wheel_frames, wheel_px) = timed(&mut run, 30, |run| run.wheel(centre, 40.0));
    let (end, _, end_px) = timed(&mut run, 1, |run| run.key(END, None));
    let snapshot = run.snapshot();
    let rows = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::ListItem)
        .count();
    println!(
        "{N} | @{scale} | {open:.1} | {down:.0} ({down_frames} frames, {} px/frame) | {wheel:.0} ({wheel_frames} frames, {} px/frame) | {end:.0} ({end_px} px) | {rows} rows, {} solves, {} nodes",
        down_px / down_frames.max(1) as u64,
        wheel_px / wheel_frames.max(1) as u64,
        snapshot.layout.solves,
        snapshot.display_list.nodes.len(),
    );
}

/// Open, select and scroll the Virtual List at 1,000, 10,000 and 50,000 rows, @1 and @2: open in ms, steps as the
/// median µs per key or wheel event, each with the frames it presented and the pixels they damaged.
#[test]
#[ignore = "timing matrix; run with --release --ignored --nocapture"]
fn virtual_list_matrix() {
    println!("rows | scale | open ms | Down µs | wheel µs | End µs | after End");
    for scale in [1.0, 2.0] {
        virtual_list_row::<1_000>(scale);
        virtual_list_row::<10_000>(scale);
        virtual_list_row::<50_000>(scale);
    }
}
