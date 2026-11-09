//! What the browser window repaints for its common interactions, and how long each frame takes at 3840×2160.
//!
//! `repaint_budget_report` is `#[ignore]`d: it measures wall time, which depends on the machine, and is run by hand in
//! the release profile (see the crate's README). It opens a generated folder of [`ITEMS`] files in a fixture home and,
//! at each backing scale of [`SCALES`] on a 3840×2160 display, times every frame of each interaction with
//! [`FrameMeter`]. Tables go to standard output and, when `CUPERTINO_REPAINT_DIR` names a folder, to
//! `files-<scale>x.md` and `files-<scale>x.csv` there; `CUPERTINO_REPAINT_SCALES` (comma-separated) measures other scales.

#![cfg(feature = "app")]

mod common;

use std::time::{Duration, Instant};

use common::{place, Host, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::Key;
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::platform::{
    KeyEvent, Keysym, Mods, MouseButton, PointerEvent, ScrollSource, Timestamp, WindowEvent,
    WindowId, WindowStates,
};
use cupertino_kit::render::CpuConfig;
use cupertino_kit::theme::{Appearance, ThemePreferences};
use cupertino_ui_test::repaint::{csv, FrameMeter, FrameSample, Summary};
use cupertino_ui_test::HeadlessWindowSystem;

/// The files of the generated folder.
const ITEMS: usize = 1500;

/// The display's physical size.
const DISPLAY: (f64, f64) = (3840.0, 2160.0);

/// The backing scales measured, each with the window filling the display.
const SCALES: [f64; 3] = [2.0, 1.5, 1.0];

/// The renderer of the application process.
const RENDERER: CpuConfig = CpuConfig {
    max_pixels: 1 << 26,
    glyph_cache_bytes: 8 << 20,
    image_cache_bytes: 32 << 20,
    font_smoothing: true,
};

/// The most nodes a window of the application process may have.
const NODE_LIMIT: usize = 100_000;

const DOWN: u32 = 0xff54;
const ESCAPE: u32 = 0xff1b;

/// A window on `home/Documents` with the folder `Big` of [`ITEMS`] files next to it in the sidebar, sized to fill the
/// display at `scale`, presenting frames without keeping their pixels.
fn start(scale: f64) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    let big = home.path().join("Big");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::create_dir_all(&big).unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    for i in 0..ITEMS {
        let ext = ["txt", "png", "pdf", "rs", "md"][i % 5];
        std::fs::write(big.join(format!("file {i:04}.{ext}")), vec![b'x'; i % 977]).unwrap();
    }
    let places = vec![
        place("Documents", &documents, PlaceKind::Documents),
        place("Big", &big, PlaceKind::Downloads),
    ];
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let host = Host {
        renderer: Some(RENDERER),
        node_limit: Some(NODE_LIMIT),
        ..Host::default()
    };
    let mut run = Run::start_hosted(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        |start| start,
        host,
    );
    run.wait_until(|run| run.rows().len() == 2);
    let window = run.window;
    let size = Size::new(DISPLAY.0 / scale, DISPLAY.1 / scale);
    let system = run.driver.window_system_mut();
    system.set_keep_pixels(false);
    system
        .enqueue_window(window, WindowEvent::Scale(scale))
        .unwrap();
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    system.configure(window, Some(size), active).unwrap();
    run.settle();
    run
}

fn pointer(system: &mut HeadlessWindowSystem, window: WindowId, event: PointerEvent) {
    let event = WindowEvent::Pointer(event, Timestamp(0));
    system.enqueue_window(window, event).unwrap();
}

fn click(system: &mut HeadlessWindowSystem, window: WindowId, position: Point) {
    pointer(system, window, PointerEvent::Motion { position });
    for pressed in [true, false] {
        let button = PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        };
        pointer(system, window, button);
    }
}

fn key(system: &mut HeadlessWindowSystem, window: WindowId, keysym: u32, text: Option<char>) {
    for pressed in [true, false] {
        let event = KeyEvent {
            code: 0,
            keysym: Keysym(keysym),
            text: text.filter(|_| pressed),
            pressed,
            repeat: false,
            mods: Mods::NONE,
        };
        let event = WindowEvent::Key(event, Timestamp(0));
        system.enqueue_window(window, event).unwrap();
    }
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

/// Steps until the window has presented nothing for `quiet` steps in a row, at most `limit` steps.
fn until_quiet(run: &mut Run, meter: &mut FrameMeter, quiet: usize, limit: usize) {
    let mut still = 0;
    for _ in 0..limit {
        let presented = meter.step(&mut run.driver, |_, _| {}).is_some();
        still = if presented { 0 } else { still + 1 };
        if still >= quiet {
            return;
        }
    }
}

/// Steps until `done` holds, waiting for a thread (the folder's reader) to wake the runtime whenever the window asks
/// for no frame; the twenty seconds only guard against a hang.
fn step_until(run: &mut Run, meter: &mut FrameMeter, done: impl Fn(&Run) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(run) {
        assert!(
            run.wait_for_progress(deadline),
            "the window never caught up"
        );
        meter.step(&mut run.driver, |_, _| {});
    }
}

/// Each interaction's frames, in the order measured.
fn interactions(scale: f64) -> Vec<(&'static str, Vec<FrameSample>)> {
    let mut run = start(scale);
    let window = run.window;
    let mut meter = FrameMeter::new(window, Timestamp(0));
    let mut out = Vec::new();

    // open the folder of many items from the sidebar
    let big = rect_of(&run, "place-Big");
    meter.step(&mut run.driver, |s, _| click(s, window, big.center()));
    step_until(&mut run, &mut meter, |run| run.rows().len() > 20);
    until_quiet(&mut run, &mut meter, 30, 2000);
    out.push(("open folder", meter.take()));

    // select the first row, then move the selection down with the arrow key among the rows in view
    let first = run.listed()[0].1;
    meter.step(&mut run.driver, |s, _| click(s, window, first.center()));
    until_quiet(&mut run, &mut meter, 10, 100);
    meter.take();
    let list = rect_of(&run, "list");
    let in_view = run
        .listed()
        .iter()
        .filter(|(_, r, _)| r.max_y() <= list.max_y())
        .count();
    let measured = in_view.saturating_sub(2).min(30);
    for _ in 0..measured {
        meter.step(&mut run.driver, |s, _| key(s, window, DOWN, None));
    }
    until_quiet(&mut run, &mut meter, 10, 100);
    out.push(("arrow selection", meter.take()));

    // past the last row in view, each step scrolls the listing by a row
    for _ in measured..in_view {
        meter.step(&mut run.driver, |s, _| key(s, window, DOWN, None));
    }
    until_quiet(&mut run, &mut meter, 10, 100);
    meter.take();
    for _ in 0..30 {
        meter.step(&mut run.driver, |s, _| key(s, window, DOWN, None));
    }
    until_quiet(&mut run, &mut meter, 10, 100);
    out.push(("arrow selection, scrolling", meter.take()));

    // hover from the sidebar across the toolbar and down the rows
    let size = Size::new(DISPLAY.0 / scale, DISPLAY.1 / scale);
    for i in 0..90 {
        let t = f64::from(i) / 90.0;
        let position = Point::new(
            size.width * (0.02 + 0.9 * t),
            size.height * (0.01 + 0.9 * t),
        );
        meter.step(&mut run.driver, |s, _| {
            pointer(s, window, PointerEvent::Motion { position })
        });
    }
    out.push(("hover", meter.take()));

    // scroll the listing with a wheel, then let the scroller fade
    let at = list.center();
    until_quiet(&mut run, &mut meter, 10, 100);
    meter.take();
    for _ in 0..90 {
        let event = PointerEvent::Scroll {
            position: at,
            delta: Point::new(0.0, 48.0),
            value120: None,
            source: ScrollSource::Wheel,
            stop: false,
            inverted: (false, false),
        };
        meter.step(&mut run.driver, |s, _| pointer(s, window, event));
    }
    out.push(("scroll listing", meter.take()));
    until_quiet(&mut run, &mut meter, 30, 400);
    meter.take();

    // the search field opens when it takes the keyboard
    let search = rect_of(&run, "search");
    meter.step(&mut run.driver, |s, _| click(s, window, search.center()));
    until_quiet(&mut run, &mut meter, 10, 100);
    out.push(("search field opens", meter.take()));

    // the caret blinks while the field waits
    for _ in 0..180 {
        meter.step(&mut run.driver, |_, _| {});
    }
    out.push(("caret blink", meter.take()));

    // typing a search narrows the listing at each key
    for c in "file 1".chars() {
        meter.step(&mut run.driver, |s, _| key(s, window, c as u32, Some(c)));
        until_quiet(&mut run, &mut meter, 3, 200);
    }
    out.push(("search typing", meter.take()));

    // cleared and left: the field closes
    meter.step(&mut run.driver, |s, _| key(s, window, ESCAPE, None));
    until_quiet(&mut run, &mut meter, 10, 400);
    meter.take();
    let list = rect_of(&run, "list");
    let rows = run.listed();
    let row = rows
        .iter()
        .map(|r| r.1)
        .find(|r| r.min_y() > list.min_y() + 40.0)
        .unwrap();
    meter.step(&mut run.driver, |s, _| click(s, window, row.center()));
    until_quiet(&mut run, &mut meter, 10, 100);
    out.push(("search field closes", meter.take()));

    // rename the selected item: the caret moves with each typed key
    run.menu("rename");
    run.wait_until(|run| {
        let runtime = run.driver.runtime();
        runtime
            .find(run.window, &Key::from("name-field/0"))
            .is_some()
    });
    until_quiet(&mut run, &mut meter, 10, 100);
    meter.take();
    for c in "renamed item".chars() {
        meter.step(&mut run.driver, |s, _| key(s, window, c as u32, Some(c)));
    }
    out.push(("rename typing", meter.take()));
    out
}

/// [`SCALES`], or the comma-separated scales of `CUPERTINO_REPAINT_SCALES`.
fn scales() -> Vec<f64> {
    match std::env::var("CUPERTINO_REPAINT_SCALES") {
        Ok(list) => list.split(',').map(|s| s.trim().parse().unwrap()).collect(),
        Err(_) => SCALES.to_vec(),
    }
}

#[test]
#[ignore = "wall time: run by hand in the release profile"]
fn repaint_budget_report() {
    let dir = std::env::var_os("CUPERTINO_REPAINT_DIR").map(std::path::PathBuf::from);
    for scale in scales() {
        let runs = interactions(scale);
        let mut table = format!(
            "Files, {ITEMS} items, window {}x{} pt at scale {scale} ({}x{} px)\n\n{}\n",
            DISPLAY.0 / scale,
            DISPLAY.1 / scale,
            DISPLAY.0,
            DISPLAY.1,
            Summary::markdown_header()
        );
        for (name, samples) in &runs {
            table.push_str(&Summary::of(samples).markdown_row(name));
            table.push('\n');
        }
        println!("{table}");
        if let Some(dir) = &dir {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("files-{scale}x.md")), &table).unwrap();
            let rows = runs.iter().map(|(n, s)| (*n, s.as_slice()));
            std::fs::write(dir.join(format!("files-{scale}x.csv")), csv(rows)).unwrap();
        }
    }
}
