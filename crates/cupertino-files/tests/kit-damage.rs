//! What the browser window repaints for its common interactions: the damage of each frame, never its wall time. A
//! selection repaints the rows it left and reached, the caret's blink repaints the search field, the search field's
//! animation repaints the toolbar, a scroll step repaints the listing, and a window kept up to date from damage alone
//! is the window drawn whole.

#![cfg(feature = "app")]

use crate::common;

use common::{place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Key;
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::platform::{PointerEvent, ScrollSource, Timestamp, WindowEvent, WindowStates};
use cupertino_kit::render::PixelRect;

const DOWN: u32 = 0xff54;

/// How far past its box a focused control draws its focus ring.
const FOCUS_RING: f64 = 8.0;

/// A window on a folder of `items` files, at `scale`.
fn start(items: usize, scale: f64) -> Run {
    let home = tempfile::tempdir().unwrap();
    let folder = home.path().join("Documents");
    std::fs::create_dir_all(&folder).unwrap();
    for i in 0..items {
        std::fs::write(folder.join(format!("file {i:03}.txt")), "x").unwrap();
    }
    let places = vec![place("Documents", &folder, PlaceKind::Documents)];
    let mut run = Run::start(home, folder, places, Notices::start(None), None);
    run.wait_until(|run| run.rows().len() >= items.min(10));
    if scale != 1.0 {
        run.send(WindowEvent::Scale(scale));
    }
    run.frames(1000, 16);
    run
}

/// The frames presented so far.
fn presented(run: &Run) -> usize {
    let frames = run.driver.runtime().window_system().frames(run.window);
    frames.map_or(0, <[_]>::len)
}

/// The damage of every frame presented since frame `from`.
fn damage_since(run: &Run, from: usize) -> Vec<PixelRect> {
    let frames = run.driver.runtime().window_system().frames(run.window);
    let frames = frames.unwrap();
    frames[from..]
        .iter()
        .flat_map(|f| f.damage.clone())
        .collect()
}

/// The bounds of the node with `key`.
fn rect_of(run: &Run, key: &str) -> Rect {
    let runtime = run.driver.runtime();
    let id = runtime
        .find(run.window, &Key::from(key.to_string()))
        .unwrap_or_else(|| panic!("no node {key}"));
    let snapshot = runtime.snapshot(run.window).unwrap();
    snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect
}

/// Whether `rect`, in pixels, lies inside `bounds`, in points at scale 1, give or take the pixel a rounded edge takes.
fn inside(rect: &PixelRect, bounds: Rect) -> bool {
    f64::from(rect.x) >= bounds.min_x().floor() - 1.0
        && f64::from(rect.y) >= bounds.min_y().floor() - 1.0
        && f64::from(rect.x + rect.width) <= bounds.max_x().ceil() + 1.0
        && f64::from(rect.y + rect.height) <= bounds.max_y().ceil() + 1.0
}

fn area(rects: &[PixelRect]) -> u64 {
    rects
        .iter()
        .map(|r| u64::from(r.width) * u64::from(r.height))
        .sum()
}

#[test]
fn an_arrow_key_repaints_only_the_row_the_selection_left_and_the_row_it_reached() {
    let mut run = start(40, 1.0);
    let rows = run.listed();
    run.click_row(&rows[2].0);
    run.frames(500, 16);
    for _ in 0..3 {
        let rows = run.listed();
        let from = rows.iter().position(|r| r.2).expect("a row is selected");
        let (left, reached) = (rows[from].1, rows[from + 1].1);
        let before = presented(&run);
        run.key(DOWN, None);
        run.frames(200, 16);
        let damage = damage_since(&run, before);
        assert!(!damage.is_empty(), "the selection moved on screen");
        // the text of a row reaches as far as its font may draw, which is past the row's edges
        let row = left.height();
        let band = left.union(&reached);
        let reach = Rect::new(
            band.min_x(),
            band.min_y() - row,
            band.width(),
            band.height() + 2.0 * row,
        );
        assert!(
            damage.iter().all(|r| inside(r, reach)),
            "only the two rows: {damage:?} outside {reach:?}"
        );
        let rows_area = (band.width() * band.height()) as u64;
        assert!(
            area(&damage) <= 2 * rows_area,
            "about two rows' pixels, not more: {damage:?}"
        );
    }
}

#[test]
fn the_caret_blinks_inside_the_search_field_and_repaints_nothing_else() {
    let mut run = start(10, 1.0);
    run.click("search");
    run.frames(1000, 16);
    let field = rect_of(&run, "search");
    let before = presented(&run);
    run.frames(3000, 16);
    let damage = damage_since(&run, before);
    assert!(presented(&run) > before, "the caret blinked");
    // the field draws its focus ring around itself
    let reach = field.outset(FOCUS_RING);
    assert!(
        damage.iter().all(|r| inside(r, reach)),
        "only the field: {damage:?} outside {reach:?}"
    );
}

/// How long the caret shows, then hides, while it blinks: the theme's phase.
const CARET_PHASE_MS: u64 = 500;

/// How long after focus the caret stops blinking and stays on.
const BLINK_LIMIT_MS: u64 = 10_000;

#[test]
fn an_idle_window_commits_no_frame_and_a_blinking_caret_one_per_edge() {
    let mut run = start(40, 1.0);
    // the scroller's fade and anything else that moves comes to rest
    run.frames(3000, 16);
    let before = presented(&run);
    run.frames(5000, 16);
    assert_eq!(presented(&run), before, "an idle window commits nothing");
    run.click("search");
    run.frames(1000, 16);
    let before = presented(&run);
    let span = 3000;
    run.frames(span, 16);
    let edges = span / CARET_PHASE_MS;
    let committed = (presented(&run) - before) as u64;
    assert!(
        (edges - 1..=edges + 1).contains(&committed),
        "{committed} frames for {edges} edges of the caret"
    );
    // past the blink's limit the caret stays on, and nothing is committed
    run.frames(BLINK_LIMIT_MS, 16);
    let before = presented(&run);
    run.frames(5000, 16);
    assert_eq!(presented(&run), before, "a steady caret commits nothing");
}

#[test]
fn the_search_field_s_animation_repaints_only_the_toolbar() {
    let mut run = start(10, 1.0);
    // the listing fills the pane under the toolbar row
    let listing = rect_of(&run, "list");
    let row = listing.min_y() + cupertino_kit::controls::TOOLBAR_ROW;
    let before = presented(&run);
    run.click("search");
    run.frames(1000, 16);
    let damage = damage_since(&run, before);
    assert!(presented(&run) > before + 1, "the field animated open");
    // the soft shade under the toolbar's glass in the light appearance reaches a few points past the row
    const SHADE: f64 = 6.0;
    assert!(
        damage
            .iter()
            .all(|r| f64::from(r.y + r.height) <= (row + SHADE).ceil()),
        "in the toolbar row: {damage:?} reaches below {row}"
    );
    assert!(
        damage.iter().all(|r| f64::from(r.y) < row),
        "nothing of the listing below the row: {damage:?}"
    );
}

#[test]
fn moving_the_pointer_over_the_rows_repaints_no_row() {
    let mut run = start(40, 1.0);
    let rows = run.listed();
    let before = presented(&run);
    for (_, row, _) in rows.iter().take(10) {
        run.send(WindowEvent::Pointer(
            PointerEvent::Motion {
                position: row.center(),
            },
            Timestamp(0),
        ));
        run.frames(32, 16);
    }
    let damage = damage_since(&run, before);
    let listing = rect_of(&run, "list");
    let rows_area = rows[0].1.width() * rows[0].1.height();
    assert!(
        damage
            .iter()
            .all(|r| !inside(r, listing) || (area(std::slice::from_ref(r)) as f64) < rows_area),
        "a hover repaints no whole row: {damage:?}"
    );
}

#[test]
fn a_wheel_step_repaints_the_listing_and_nothing_outside_it() {
    let mut run = start(200, 1.0);
    let listing = rect_of(&run, "list");
    for _ in 0..5 {
        let before = presented(&run);
        run.send(WindowEvent::Pointer(
            PointerEvent::Scroll {
                position: listing.center(),
                delta: Point::new(0.0, 40.0),
                value120: None,
                source: ScrollSource::Wheel,
                stop: false,
                inverted: (false, false),
            },
            Timestamp(0),
        ));
        run.frames(16, 16);
        let damage = damage_since(&run, before);
        assert!(!damage.is_empty(), "the listing scrolled");
        assert!(
            damage.iter().all(|r| inside(r, listing)),
            "only the listing: {damage:?} outside {listing:?}"
        );
    }
}

#[test]
fn a_window_kept_from_damage_alone_is_the_window_drawn_whole() {
    // a fractional scale, where a point's edge falls inside a pixel
    let mut run = start(120, 1.5);
    let rows = run.listed();
    run.click_row(&rows[1].0);
    for _ in 0..4 {
        run.key(DOWN, None);
        run.frames(100, 16);
    }
    let listing = rect_of(&run, "list");
    for _ in 0..3 {
        run.send(WindowEvent::Pointer(
            PointerEvent::Scroll {
                position: listing.center(),
                delta: Point::new(0.0, 37.0),
                value120: None,
                source: ScrollSource::Wheel,
                stop: false,
                inverted: (false, false),
            },
            Timestamp(0),
        ));
        run.frames(32, 16);
    }
    assert_drawn_whole(&mut run, "after selecting and scrolling");
    run.frames(2000, 16);
    assert_drawn_whole(&mut run, "after the scroller faded");
    run.click("search");
    run.frames(1000, 16);
    assert_drawn_whole(&mut run, "after the search field opened");
    run.type_text("file 1");
    assert_drawn_whole(&mut run, "after typing");
    // the caret on and off
    for _ in 0..4 {
        run.frames(300, 16);
        assert_drawn_whole(&mut run, "while the caret blinks");
    }
    run.key(0xff1b, None);
    run.frames(1000, 16);
    assert_drawn_whole(&mut run, "after the search was left");
}

/// How far apart a pixel drawn again inside a damaged rectangle may be from the same pixel of a whole frame: the CPU
/// renderer samples a curved or fractional anti-aliased edge cut by a damaged rectangle's border on a slightly different
/// sub-pixel grid, which moves its coverage a few levels (six at most here, at 1.5x). A pixel left stale (a caret, a
/// selection, text that went) is much further.
const EDGE: u8 = 8;

/// The last frame, which was not drawn whole, has the pixels of the window drawn whole, as a configure draws it.
#[track_caller]
fn assert_drawn_whole(run: &mut Run, when: &str) {
    let frames = run.driver.runtime().window_system().frames(run.window);
    let last = frames.unwrap().last().unwrap();
    let kept = last.pixels.clone();
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    let before = presented(run);
    run.driver
        .window_system_mut()
        .configure(run.window, None, active)
        .unwrap();
    run.settle();
    let frames = run.driver.runtime().window_system().frames(run.window);
    let whole = frames.unwrap()[before..]
        .iter()
        .find(|f| area(&f.damage) == u64::from(f.width) * u64::from(f.height))
        .expect("the configure drew the window whole");
    let stale = kept
        .chunks(4)
        .zip(whole.pixels.chunks(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > EDGE))
        .count();
    assert_eq!(stale, 0, "pixels the damage left stale {when}");
}
