//! The real Settings window, a CupertinoKit app in the full-height-sidebar style, whose window control buttons the
//! server draws at the top of the sidebar, against the headless server: a drag on its toolbar where no control lies
//! moves the window through `xdg_toplevel.move`, a drag on the sidebar's search field or on the toolbar's Back button
//! does not, and a double click on the toolbar zooms the window and another restores it.
//! Needs an EGL device and `dbus-daemon` for the app's private bus.

use crate::common;

use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use common::spawn;
use cupertino_server::{backend::headless, config::Config, Cupertino};
use cupertino_test_bus::PrivateBus;
use cupertino_wm::LifecycleState;
use smithay::backend::input::ButtonState;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;

/// The window's content height at its default size; a title bar the server drew would sit above it in the frame.
const CONTENT_HEIGHT: f64 = 748.0;
/// Content points: in the toolbar row past the back and forward buttons, where no item lies; on the search field at
/// the top of the sidebar, below the window control buttons; and on the Back button.
const TOOLBAR_GAP: (f64, f64) = (520.0, 26.0);
const SEARCH_FIELD: (f64, f64) = (116.0, 75.0);
const BACK_BUTTON: (f64, f64) = (259.0, 26.0);
/// The Accessibility pane's Display row, which opens a page and so gives Back a page to go back to.
const DISPLAY_ROW: (f64, f64) = (486.0, 171.0);
/// How far a drag takes the pointer, in steps the client sees one by one; little enough down that the tall window stays
/// on the output.
const DRAG: (f64, f64) = (120.0, 30.0);
const DRAG_STEPS: u32 = 8;
/// Evdev code of the left button.
const BTN_LEFT: u32 = 0x110;

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

struct Harness {
    state: Cupertino,
    event_loop: smithay::reexports::calloop::EventLoop<'static, Cupertino>,
    app: Process,
    bus: PrivateBus,
    _home: tempfile::TempDir,
    /// The height of the server's title bar over the content.
    title_bar: f64,
    /// The input clock in milliseconds, which the client reads double clicks with.
    time: u32,
}

impl Harness {
    /// `None` without an EGL device.
    fn new() -> Option<Self> {
        let binary = common::fixtures::artifact("cupertino-settings", "bin", "cupertino-settings");
        let home = tempfile::tempdir().unwrap();
        let bus = PrivateBus::start().expect("this test needs dbus-daemon");
        let event_loop = smithay::reexports::calloop::EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            smithay::reexports::wayland_server::Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, (1280, 800), 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(state.socket_name.as_ref().unwrap());
        let app = Process(
            spawn::cleared(&binary, home.path())
                .args(["--pane", "accessibility"])
                .env("XDG_CONFIG_HOME", home.path().join("config"))
                .env("XDG_DATA_HOME", home.path().join("data"))
                .env("XDG_DATA_DIRS", home.path().join("share"))
                .env("XDG_RUNTIME_DIR", home.path())
                .env("WAYLAND_DISPLAY", socket)
                .env("CUPERTINO_ASSET_SEARCH_PATH", "")
                .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
                .env("DBUS_SYSTEM_BUS_ADDRESS", &bus.address)
                .spawn()
                .unwrap(),
        );
        let mut h = Self {
            state,
            event_loop,
            app,
            bus,
            _home: home,
            title_bar: 0.0,
            time: 1_000,
        };
        h.wait_until("the Settings window", |s| title(s) == "Accessibility");
        h.settle();
        h.title_bar = frame(&h.state).3 - CONTENT_HEIGHT;
        Some(h)
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
        assert!(
            self.app.0.try_wait().unwrap().is_none(),
            "Settings exited unexpectedly"
        );
        assert!(!self.bus.has_exited(), "private bus exited unexpectedly");
    }

    fn wait_until(&mut self, what: &str, condition: impl Fn(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if condition(&self.state) {
                return;
            }
        }
        panic!(
            "timed out waiting for {what}; frame={:?}",
            frame(&self.state)
        );
    }

    /// Steps until `condition` holds or `limit` passes; whether it held.
    fn wait_for(&mut self, limit: Duration, condition: impl Fn(&Cupertino) -> bool) -> bool {
        let end = Instant::now() + limit;
        while Instant::now() < end {
            self.step();
            if condition(&self.state) {
                return true;
            }
        }
        false
    }

    /// [`common::ping_barrier`] on Settings: it has handled what it was sent, and the server has what it did with it.
    fn settle(&mut self) {
        let client = self
            .state
            .windows
            .iter()
            .find(|w| w.mapped)
            .unwrap()
            .toplevel
            .client();
        let (event_loop, app, bus) = (&mut self.event_loop, &mut self.app, &mut self.bus);
        common::ping_barrier(&mut self.state, &client, |state| {
            headless::step(state, event_loop, Duration::from_millis(5)).unwrap();
            assert!(
                app.0.try_wait().unwrap().is_none(),
                "Settings exited unexpectedly"
            );
            assert!(!bus.has_exited(), "private bus exited unexpectedly");
        });
    }

    /// The screen point of a point in the window's content, at any size.
    fn screen(&self, content: (f64, f64)) -> (f64, f64) {
        let (x, y, _, _) = frame(&self.state);
        (x + content.0, y + self.title_bar + content.1)
    }

    fn move_to(&mut self, point: (f64, f64)) {
        self.time += 10;
        self.state.pointer_moved_to(point.into(), self.time, None);
        self.step();
    }

    fn button(&mut self, pressed: bool) {
        self.time += 10;
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        self.state.pointer_button(BTN_LEFT, state, self.time);
        self.step();
    }

    /// Presses at `content`, travels [`DRAG`] in steps with the button held, the client answering each step before
    /// the next, and releases; returns once the server has what the client did with the release.
    fn drag(&mut self, content: (f64, f64)) {
        let start = self.screen(content);
        self.move_to(start);
        self.button(true);
        self.settle();
        for i in 1..=DRAG_STEPS {
            let f = f64::from(i) / f64::from(DRAG_STEPS);
            self.move_to((start.0 + DRAG.0 * f, start.1 + DRAG.1 * f));
            self.settle();
        }
        self.button(false);
        self.settle();
    }

    fn click(&mut self, content: (f64, f64)) {
        let point = self.screen(content);
        self.move_to(point);
        self.button(true);
        self.button(false);
    }

    /// Two presses at `content` well within a double click.
    fn double_click(&mut self, content: (f64, f64)) {
        let point = self.screen(content);
        self.move_to(point);
        for _ in 0..2 {
            self.button(true);
            self.button(false);
        }
    }

    fn state(&self) -> LifecycleState {
        let id = self.state.windows.iter().find(|w| w.mapped).unwrap().id;
        self.state.wm.window(id).unwrap().state()
    }
}

/// The window's frame on screen: x, y, width and height, title bar included.
fn frame(state: &Cupertino) -> (f64, f64, f64, f64) {
    let window = state.windows.iter().find(|w| w.mapped).unwrap();
    let f = state.wm.window(window.id).unwrap().geometry();
    (f.x, f.y, f.w, f.h)
}

fn title(state: &Cupertino) -> String {
    state
        .windows
        .iter()
        .find(|w| w.mapped)
        .and_then(|w| {
            with_states(w.surface(), |s| {
                s.data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .title
                    .clone()
            })
        })
        .unwrap_or_default()
}

#[test]
fn a_drag_on_the_toolbar_moves_the_kit_window_and_one_on_the_search_field_or_the_back_button_does_not(
) {
    let Some(mut h) = Harness::new() else {
        return;
    };
    assert!(
        h.state
            .windows
            .iter()
            .find(|w| w.mapped)
            .unwrap()
            .server_side,
        "the server draws the window control buttons"
    );
    let before = frame(&h.state);
    h.drag(SEARCH_FIELD);
    assert_eq!(frame(&h.state), before, "the search field took the drag");
    // Back is enabled once a page has been left
    h.click(DISPLAY_ROW);
    h.wait_until("the Display page", |s| title(s) == "Display");
    h.settle();
    let before = frame(&h.state);
    h.drag(BACK_BUTTON);
    assert_eq!(frame(&h.state), before, "the Back button took the drag");
    assert_eq!(
        title(&h.state),
        "Display",
        "released away from it, Back does not act"
    );

    h.drag(TOOLBAR_GAP);
    h.wait_until("the window to follow the drag", |s| {
        frame(s).0 > before.0 + DRAG.0 / 2.0 && frame(s).1 > before.1 + DRAG.1 / 2.0
    });
    let after = frame(&h.state);
    assert_eq!(
        (after.2, after.3),
        (before.2, before.3),
        "moved, not resized"
    );
    assert_eq!(h.state(), LifecycleState::Normal);
}

#[test]
fn a_double_click_on_the_toolbar_zooms_the_kit_window_and_another_restores_it() {
    let Some(mut h) = Harness::new() else {
        return;
    };
    let before = frame(&h.state);
    h.double_click(TOOLBAR_GAP);
    h.wait_until("the window to zoom", |s| {
        let id = s.windows.iter().find(|w| w.mapped).unwrap().id;
        s.wm.window(id).unwrap().state() == LifecycleState::Zoomed
    });
    // the client hears it is maximized before the next double click asks for the other state
    h.settle();
    assert_eq!(h.state(), LifecycleState::Zoomed);
    // a double click that lands while the client still lays out the zoomed window may meet a control there, which
    // takes the press; the restore is asked again a little later, past the double click interval
    for _ in 0..5 {
        h.double_click(TOOLBAR_GAP);
        if h.wait_for(Duration::from_secs(2), |s| frame(s) == before) {
            return;
        }
        h.time += 1_000;
        h.settle();
    }
    panic!("the window was not restored; frame={:?}", frame(&h.state));
}
