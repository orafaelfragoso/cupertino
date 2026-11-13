//! What the window repaints for its common interactions, and how long each frame takes at 3840×2160.
//!
//! The tests pin the damage, never the wall time: a switch and a slider repaint only themselves, and switching panes
//! repaints in the sidebar only the rows the selection left and reached.
//!
//! `repaint_budget_report` is `#[ignore]`d: it measures wall time, which depends on the machine, and is run by hand in
//! the release profile (see the crate's README). At each backing scale of [`SCALES`], with the window filling a
//! 3840×2160 display, it times every frame of each interaction with [`FrameMeter`]. Tables go to standard output and,
//! when `CUPERTINO_REPAINT_DIR` names a folder, to `settings-<scale>x.md` and `settings-<scale>x.csv` there;
//! `CUPERTINO_REPAINT_SCALES` (comma-separated) measures other scales.

use cupertino_kit::geometry::Rect;
use cupertino_kit::platform::ScrollSource;
use cupertino_ui_test::repaint::{csv, FrameMeter, FrameSample, Summary};
use cupertino_ui_test::HeadlessWindowSystem;

use super::*;

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

/// The panes the sidebar switches between, in order.
const PANES: [&str; 8] = [
    "keyboard",
    "mouse",
    "appearance",
    "accessibility",
    "sound",
    "displays",
    "privacy",
    "desktop-dock",
];

/// The test runtime with the application process's renderer and node limits.
fn options() -> RuntimeOptions {
    let mut options = runtime_options();
    let glyphs = Arc::clone(&options.text);
    options.node_limit = NODE_LIMIT;
    options.renderers = Box::new(move || {
        Box::new(CpuRenderer::new(
            RENDERER,
            GlyphRaster::new(Arc::clone(&glyphs)),
        ))
    });
    options
}

/// The window on `pane` at `scale`, filling the display when `fill` and at its default size otherwise, presenting
/// frames without keeping their pixels.
fn start(dir: &Path, pane: PaneId, scale: f64, fill: bool) -> (TestDriver<SettingsApp>, WindowId) {
    let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
    let (wake, woken) = async_channel::bounded(1);
    let (notify, background) = background_channel(move || {
        let _ = wake.try_send(());
    });
    let state = State::new(paths(dir), services, pane, 1080.0, notify).unwrap();
    launch(Start {
        state,
        background,
        woken,
        activation: String::new(),
    });
    let mut driver = TestDriver::new(
        Capabilities::default(),
        options(),
        ManualClock::new(Timestamp(0)),
    );
    driver.run_until_idle(budget()).unwrap();
    let window = driver.runtime().windows()[0].1;
    let size = fill.then(|| Size::new(DISPLAY.0 / scale, DISPLAY.1 / scale));
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    let system = driver.window_system_mut();
    system.set_keep_pixels(false);
    system
        .enqueue_window(window, WindowEvent::Scale(scale))
        .unwrap();
    system.configure(window, size, active).unwrap();
    settle(&mut driver, window);
    (driver, window)
}

fn pointer(system: &mut HeadlessWindowSystem, window: WindowId, event: PointerEvent) {
    let event = WindowEvent::Pointer(event, Timestamp(0));
    system.enqueue_window(window, event).unwrap();
}

fn button(system: &mut HeadlessWindowSystem, window: WindowId, position: Point, pressed: bool) {
    let event = PointerEvent::Button {
        position,
        button: MouseButton::Left,
        pressed,
        serial: 1,
        mods: Mods::NONE,
    };
    pointer(system, window, event);
}

fn press_and_release(system: &mut HeadlessWindowSystem, window: WindowId, position: Point) {
    pointer(system, window, PointerEvent::Motion { position });
    button(system, window, position, true);
    button(system, window, position, false);
}

fn type_key(system: &mut HeadlessWindowSystem, window: WindowId, c: char) {
    for pressed in [true, false] {
        let event = KeyEvent {
            code: 0,
            keysym: Keysym(c as u32),
            text: pressed.then_some(c),
            pressed,
            repeat: false,
            mods: Mods::NONE,
        };
        let event = WindowEvent::Key(event, Timestamp(0));
        system.enqueue_window(window, event).unwrap();
    }
}

/// The bounds of the node with `key`.
fn rect_of(driver: &TestDriver<SettingsApp>, window: WindowId, key: &str) -> Rect {
    let runtime = driver.runtime();
    let id = runtime
        .find(window, &Key::from(key.to_owned()))
        .unwrap_or_else(|| panic!("no node {key}"));
    let snapshot = runtime.snapshot(window).unwrap();
    snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect
}

/// Steps until the window has presented nothing for `quiet` steps in a row, at most `limit` steps.
fn until_quiet(
    driver: &mut TestDriver<SettingsApp>,
    meter: &mut FrameMeter,
    quiet: usize,
    limit: usize,
) {
    let mut still = 0;
    for _ in 0..limit {
        let presented = meter.step(driver, |_, _| {}).is_some();
        still = if presented { 0 } else { still + 1 };
        if still >= quiet {
            return;
        }
    }
}

/// Each interaction's frames, in the order measured.
fn interactions(scale: f64) -> Vec<(&'static str, Vec<FrameSample>)> {
    let dir = tempfile::tempdir().unwrap();
    let (mut driver, window) = start(dir.path(), PaneId::DesktopDock, scale, true);
    let mut meter = FrameMeter::new(window, Timestamp(0));
    let mut out = Vec::new();

    // switch panes from the sidebar, waiting for each to settle
    for pane in PANES {
        let at = rect_of(&driver, window, pane).center();
        meter.step(&mut driver, |s, _| press_and_release(s, window, at));
        until_quiet(&mut driver, &mut meter, 10, 200);
    }
    out.push(("switch panes", meter.take()));

    // toggle a switch, letting its knob slide each time
    let switch = format!("{:?}-control", ControlId::AutoHide);
    for _ in 0..6 {
        let at = rect_of(&driver, window, &switch).center();
        meter.step(&mut driver, |s, _| press_and_release(s, window, at));
        until_quiet(&mut driver, &mut meter, 10, 200);
    }
    out.push(("toggle switch", meter.take()));

    // drag a slider's knob across its track and back
    let slider = rect_of(
        &driver,
        window,
        &format!("{:?}-control", ControlId::DockSize),
    );
    let y = slider.center().y;
    let (from, to) = (slider.min_x() + 4.0, slider.max_x() - 4.0);
    meter.step(&mut driver, |s, _| {
        let at = Point::new(from, y);
        pointer(s, window, PointerEvent::Motion { position: at });
        button(s, window, at, true);
    });
    for i in 1..=60 {
        let t = f64::from(if i <= 30 { i } else { 60 - i }) / 30.0;
        let position = Point::new(from + (to - from) * t, y);
        meter.step(&mut driver, |s, _| {
            pointer(s, window, PointerEvent::Motion { position })
        });
    }
    meter.step(&mut driver, |s, _| {
        button(s, window, Point::new(from, y), false)
    });
    out.push(("drag slider", meter.take()));
    until_quiet(&mut driver, &mut meter, 10, 200);
    meter.take();

    // a long pane fits a window that fills the display, so it is scrolled at the window's default size
    let (mut small, small_window) = start(dir.path(), PaneId::Accessibility, scale, false);
    let mut small_meter = FrameMeter::new(small_window, Timestamp(0));
    let size = default_size(WIDTH_FACTOR);
    let content = Point::new(SIDEBAR + (size.width - SIDEBAR) / 2.0, size.height / 2.0);
    for i in 0..60 {
        let delta = if i < 30 { 40.0 } else { -40.0 };
        let event = PointerEvent::Scroll {
            position: content,
            delta: Point::new(0.0, delta),
            value120: None,
            source: ScrollSource::Wheel,
            stop: false,
            inverted: (false, false),
        };
        small_meter.step(&mut small, |s, _| pointer(s, small_window, event));
    }
    out.push(("scroll pane (default size)", small_meter.take()));
    drop(small);

    // the caret blinks in the search field, and typing searches
    let search = rect_of(&driver, window, "search").center();
    meter.step(&mut driver, |s, _| press_and_release(s, window, search));
    until_quiet(&mut driver, &mut meter, 10, 200);
    meter.take();
    for _ in 0..180 {
        meter.step(&mut driver, |_, _| {});
    }
    out.push(("caret blink", meter.take()));
    for c in "sound".chars() {
        meter.step(&mut driver, |s, _| type_key(s, window, c));
        until_quiet(&mut driver, &mut meter, 3, 100);
    }
    out.push(("search typing", meter.take()));
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
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::var_os("CUPERTINO_REPAINT_DIR").map(std::path::PathBuf::from);
    for scale in scales() {
        let runs = interactions(scale);
        let mut table = format!(
            "Settings, window {}x{} pt at scale {scale} ({}x{} px)\n\n{}\n",
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
            std::fs::write(dir.join(format!("settings-{scale}x.md")), &table).unwrap();
            let rows = runs.iter().map(|(n, s)| (*n, s.as_slice()));
            std::fs::write(dir.join(format!("settings-{scale}x.csv")), csv(rows)).unwrap();
        }
    }
}

/// The damage of every frame `window` presented since frame `from`, and the frames there are now.
fn damage_since(
    driver: &TestDriver<SettingsApp>,
    window: WindowId,
    from: usize,
) -> (Vec<cupertino_kit::render::PixelRect>, usize) {
    let frames = driver.runtime().window_system().frames(window).unwrap();
    let damage = frames[from..]
        .iter()
        .flat_map(|f| f.damage.clone())
        .collect();
    (damage, frames.len())
}

/// Whether `rect`, in pixels, lies inside `bounds`, in points at scale 1, give or take a pixel.
fn inside(rect: &cupertino_kit::render::PixelRect, bounds: Rect) -> bool {
    f64::from(rect.x) >= bounds.min_x().floor() - 1.0
        && f64::from(rect.y) >= bounds.min_y().floor() - 1.0
        && f64::from(rect.x + rect.width) <= bounds.max_x().ceil() + 1.0
        && f64::from(rect.y + rect.height) <= bounds.max_y().ceil() + 1.0
}

/// How far past its bounds a control may paint: its focus ring and its shadow.
const CONTROL_REACH: f64 = 8.0;
/// How far past its bounds a held slider may paint: its knob swells into a lens, with a shadow.
const HELD_REACH: f64 = 16.0;

#[test]
fn a_switch_repaints_only_itself_while_its_knob_slides() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut driver, window) = start(dir.path(), PaneId::DesktopDock, 1.0, false);
    let mut meter = FrameMeter::new(window, Timestamp(0));
    // the window's opening transitions end first
    until_quiet(&mut driver, &mut meter, 10, 200);
    let switch = rect_of(
        &driver,
        window,
        &format!("{:?}-control", ControlId::AutoHide),
    );
    let (_, before) = damage_since(&driver, window, 0);
    meter.step(&mut driver, |s, _| {
        press_and_release(s, window, switch.center())
    });
    until_quiet(&mut driver, &mut meter, 10, 200);
    let (damage, after) = damage_since(&driver, window, before);
    assert!(after > before + 1, "the knob slid over frames");
    let reach = switch.outset(CONTROL_REACH);
    assert!(
        damage.iter().all(|r| inside(r, reach)),
        "only the switch: {damage:?} outside {reach:?}"
    );
}

#[test]
fn a_slider_repaints_only_itself_while_its_knob_is_dragged() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut driver, window) = start(dir.path(), PaneId::DesktopDock, 1.0, false);
    let mut meter = FrameMeter::new(window, Timestamp(0));
    // the window's opening transitions end first
    until_quiet(&mut driver, &mut meter, 10, 200);
    let slider = rect_of(
        &driver,
        window,
        &format!("{:?}-control", ControlId::DockSize),
    );
    let y = slider.center().y;
    let (_, before) = damage_since(&driver, window, 0);
    meter.step(&mut driver, |s, _| {
        let at = Point::new(slider.min_x() + 4.0, y);
        pointer(s, window, PointerEvent::Motion { position: at });
        button(s, window, at, true);
    });
    for i in 1..=10 {
        let x = slider.min_x() + 4.0 + (slider.width() - 8.0) * f64::from(i) / 10.0;
        meter.step(&mut driver, |s, _| {
            pointer(
                s,
                window,
                PointerEvent::Motion {
                    position: Point::new(x, y),
                },
            )
        });
    }
    meter.step(&mut driver, |s, _| {
        button(s, window, Point::new(slider.max_x() - 4.0, y), false)
    });
    until_quiet(&mut driver, &mut meter, 10, 200);
    let (damage, after) = damage_since(&driver, window, before);
    assert!(after > before + 5, "the knob moved with the pointer");
    let reach = slider.outset(HELD_REACH);
    assert!(
        damage.iter().all(|r| inside(r, reach)),
        "only the slider: {damage:?} outside {reach:?}"
    );
}

#[test]
fn switching_panes_repaints_in_the_sidebar_only_the_rows_the_selection_left_and_reached() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // rows a window of the default height shows
    let (mut driver, window) = start(dir.path(), PaneId::Notifications, 1.0, false);
    let mut meter = FrameMeter::new(window, Timestamp(0));
    // the window's opening transitions end first
    until_quiet(&mut driver, &mut meter, 10, 200);
    let mut from = rect_of(&driver, window, "notifications");
    for pane in ["sound", "focus", "appearance"] {
        let to = rect_of(&driver, window, pane);
        let (_, before) = damage_since(&driver, window, 0);
        meter.step(&mut driver, |s, _| {
            press_and_release(s, window, to.center())
        });
        until_quiet(&mut driver, &mut meter, 10, 200);
        let (damage, after) = damage_since(&driver, window, before);
        assert!(after > before, "the pane changed");
        let (a, b) = (from.outset(CONTROL_REACH), to.outset(CONTROL_REACH));
        let in_sidebar: Vec<_> = damage
            .iter()
            .filter(|r| f64::from(r.x) < SIDEBAR - CONTROL_REACH)
            .collect();
        // two rows next to each other repaint as one rectangle
        let span = a.union(&b);
        let area: f64 = in_sidebar
            .iter()
            .map(|r| f64::from(r.width) * f64::from(r.height))
            .sum();
        assert!(
            in_sidebar.iter().all(|r| inside(r, span))
                && area <= a.width() * a.height() + b.width() * b.height(),
            "only the two rows of the sidebar: {in_sidebar:?} outside {a:?} and {b:?}"
        );
        from = to;
    }
}

#[test]
fn a_turn_that_changes_no_reading_presents_nothing_on_a_pane_of_readings() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // the Account pane's rows are all readings of one control
    let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
    let (wake, woken) = async_channel::bounded(1);
    let waker = wake.clone();
    let (notify, background) = background_channel(move || {
        let _ = wake.try_send(());
    });
    let state = State::new(paths(dir.path()), services, PaneId::Account, 1080.0, notify).unwrap();
    launch(Start {
        state,
        background,
        woken,
        activation: String::new(),
    });
    let mut driver = TestDriver::new(
        Capabilities::default(),
        options(),
        ManualClock::new(Timestamp(0)),
    );
    driver.run_until_idle(budget()).unwrap();
    let window = driver.runtime().windows()[0].1;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    driver
        .window_system_mut()
        .configure(window, None, active)
        .unwrap();
    settle(&mut driver, window);
    let mut meter = FrameMeter::new(window, Timestamp(0));
    until_quiet(&mut driver, &mut meter, 10, 200);
    assert!(
        driver
            .runtime()
            .find(window, &Key::from("Reading-1"))
            .is_some(),
        "the pane shows its readings"
    );
    let (_, before) = damage_since(&driver, window, 0);
    // a wake with nothing waiting: the window updates and views again, and every reading is as it was
    waker.try_send(()).unwrap();
    until_quiet(&mut driver, &mut meter, 10, 200);
    let (damage, after) = damage_since(&driver, window, before);
    assert_eq!(after, before, "no frame: {damage:?}");
}
