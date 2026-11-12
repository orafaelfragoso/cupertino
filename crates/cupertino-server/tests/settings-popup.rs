//! Issue #5: the real Settings choice menu must not survive dismissal or navigation.
//! Run through `python3 ci/build-test-fixtures.py --locked -- cargo test --locked --workspace --all-features -p cupertino-server --test suite settings_popup:: -- --test-threads=1`.

use crate::common;

use std::path::PathBuf;
use std::process::Child;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use common::spawn;
use cupertino_server::{backend::headless, config::Config, Cupertino};
use cupertino_test_bus::PrivateBus;
use cupertino_theme::Appearance;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::{PopupKind, PopupManager};
use smithay::input::keyboard::Keycode;
use smithay::input::pointer::MotionEvent;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::SERIAL_COUNTER;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shm::with_buffer_contents;

/// The window's content height; a title bar the server drew would sit above it in the frame.
const CONTENT_HEIGHT: f64 = 748.0;
// Centres in the 740 × 748 content: the Accessibility pane's Display row and the Colour filter button on the page it
// opens, and the Appearance sidebar row.
const DISPLAY_ROW: (f64, f64) = (486.0, 171.0);
const COLOUR_FILTER: (f64, f64) = (613.0, 314.0);
const APPEARANCE_ROW: (f64, f64) = (116.0, 271.0);
// The search field at the top of the sidebar, and the Top Left corner's button on the Hot Corners page, which the
// search opens (its row lies below the Desktop & Dock pane's first screen).
const SEARCH_FIELD: (f64, f64) = (116.0, 75.0);
const TOP_LEFT_CORNER: (f64, f64) = (636.0, 82.0);

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().unwrap().is_none() {
            self.0.kill().unwrap();
        }
        self.0.wait().unwrap();
    }
}

fn settings_binary() -> &'static PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        common::fixtures::artifact("cupertino-settings", "bin", "cupertino-settings")
    })
}

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    app: Process,
    bus: PrivateBus,
    address: String,
    home: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self::with_menu(
            "accessibility",
            "Accessibility",
            (Some(DISPLAY_ROW), "Display"),
            COLOUR_FILTER,
        )
    }

    /// Settings on `pane`, the page its `row` opens (or a search for the page's title, with no row), and the menu of
    /// `button` on that page open.
    fn with_menu(
        pane: &str,
        pane_title: &str,
        (row, page_title): (Option<(f64, f64)>, &str),
        button: (f64, f64),
    ) -> Self {
        let binary = settings_binary();
        let home = tempfile::tempdir().unwrap();
        let bus = PrivateBus::start().expect("this regression needs dbus-daemon");
        let address = bus.address.clone();
        let event_loop = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        headless::init(&mut state, (1280, 800), 1.0).expect("EGL is required for this regression");
        state.set_appearance(Appearance::Light);
        let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"))
            .join(state.socket_name.as_ref().unwrap());
        let app = Process(
            spawn::cleared(binary, home.path())
                .args(["--pane", pane])
                .env("XDG_CONFIG_HOME", home.path().join("config"))
                .env("XDG_DATA_HOME", home.path().join("data"))
                .env("XDG_DATA_DIRS", home.path().join("share"))
                .env("XDG_RUNTIME_DIR", home.path())
                .env("WAYLAND_DISPLAY", socket)
                .env("CUPERTINO_ASSET_SEARCH_PATH", "")
                .env("DBUS_SESSION_BUS_ADDRESS", &address)
                .env("DBUS_SYSTEM_BUS_ADDRESS", &address)
                .spawn()
                .unwrap(),
        );
        let mut h = Self {
            state,
            event_loop,
            app,
            bus,
            address,
            home,
        };
        h.wait_until("the pane's window", |s| title(s) == pane_title);
        h.settle();
        match row {
            Some(row) => h.click(row),
            None => h.search(&page_title.to_lowercase()),
        }
        h.wait_until("the row's page", |s| title(s) == page_title);
        h.settle();
        h.click(button);
        h.wait_until("one drawn choice popup", |s| {
            popup_count(s) == 1 && popup_drawn(s)
        });
        assert_eq!(title(&h.state), page_title);
        h
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

    fn wait_until(&mut self, what: &str, mut condition: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if condition(&self.state) {
                return;
            }
        }
        panic!(
            "timed out waiting for {what}; title={:?}, popups={}",
            title(&self.state),
            popup_count(&self.state)
        );
    }

    /// [`common::ping_barrier`] on Settings: it has handled what it was sent, and the server has what it did with it,
    /// the frames it drew included.
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

    /// Steps until the open menu is gone: the client closes it on its own time, which a loaded machine stretches.
    fn wait_for_menu_to_close(&mut self) {
        self.wait_until("the menu to close", |s| popup_count(s) == 0);
    }

    /// Steps until `dock.conf` holds the line `expected`.
    fn wait_for_dock_setting(&mut self, expected: &str) {
        let path = self.home.path().join("config/cupertino/dock.conf");
        self.wait_until("the choice to be stored", |_| {
            std::fs::read_to_string(&path)
                .is_ok_and(|stored| stored.lines().any(|l| l.trim() == expected))
        });
    }

    fn click(&mut self, point: (f64, f64)) {
        let window = self.state.windows.iter().find(|w| w.mapped).unwrap();
        let frame = self.state.wm.window(window.id).unwrap().geometry();
        let top = frame.y + frame.h - CONTENT_HEIGHT;
        self.state
            .pointer_moved_to((frame.x + point.0, top + point.1).into(), 0, None);
        self.step();
        self.state.pointer_button(0x110, ButtonState::Pressed, 0);
        self.step();
        self.state.pointer_button(0x110, ButtonState::Released, 0);
    }

    /// Type `query` into the search field and press Return, which opens the page of the first result.
    fn search(&mut self, query: &str) {
        self.click(SEARCH_FIELD);
        self.settle();
        for c in query.chars() {
            let code = match c {
                ' ' => 57,
                'c' => 46,
                'e' => 18,
                'h' => 35,
                'n' => 49,
                'o' => 24,
                'r' => 19,
                's' => 31,
                't' => 20,
                _ => panic!("no key for {c:?}"),
            };
            self.tap(code);
        }
        self.settle();
        self.tap(KEY_ENTER);
    }

    fn tap(&mut self, evdev: u32) {
        let code = Keycode::new(evdev + 8);
        self.state.keyboard_event_now(code, KeyState::Pressed, 0);
        self.step();
        self.state.keyboard_event_now(code, KeyState::Released, 0);
        self.step();
    }

    // Use the public ShowPane handoff so the navigation assertion is independent of pointer dismissal.
    fn show_appearance(&mut self) {
        let mut handoff = Process(
            spawn::cleared(settings_binary(), self.home.path())
                .args(["--pane", "appearance"])
                .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
                .spawn()
                .unwrap(),
        );
        self.wait_until("Appearance pane", |s| title(s) == "Appearance");
        self.wait_until("pane redraw", |s| !window_pixels(s).is_empty());
        self.settle();
        assert!(
            handoff.0.wait().unwrap().success(),
            "ShowPane handoff failed"
        );
    }
}

fn title(state: &Cupertino) -> String {
    state
        .windows
        .iter()
        .find(|w| w.mapped)
        .map(|w| {
            with_states(w.surface(), |s| {
                s.data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .title
                    .clone()
                    .unwrap()
            })
        })
        .unwrap_or_default()
}

fn popup_count(state: &Cupertino) -> usize {
    state
        .windows
        .iter()
        .filter(|w| w.mapped)
        .flat_map(|w| PopupManager::popups_for_surface(w.surface()))
        .count()
}

/// Every popup has committed a buffer: until then it cannot be hit, so a press at its place is a press outside it.
fn popup_drawn(state: &Cupertino) -> bool {
    let window = state.windows.iter().find(|w| w.mapped).unwrap();
    PopupManager::popups_for_surface(window.surface()).all(|(popup, _)| {
        with_renderer_surface_state(popup.wl_surface(), |s| s.buffer().is_some()).unwrap_or(false)
    })
}

fn window_pixels(state: &Cupertino) -> Vec<u8> {
    let window = state.windows.iter().find(|w| w.mapped).unwrap();
    surface_pixels(window.surface())
}

/// The open menu's pixels; empty with no menu open.
fn popup_pixels(state: &Cupertino) -> Vec<u8> {
    let window = state.windows.iter().find(|w| w.mapped).unwrap();
    PopupManager::popups_for_surface(window.surface())
        .next()
        .map_or_else(Vec::new, |(popup, _)| surface_pixels(popup.wl_surface()))
}

fn surface_pixels(
    surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
) -> Vec<u8> {
    let buffer = with_renderer_surface_state(surface, |s| s.buffer().cloned())
        .unwrap()
        .unwrap();
    with_buffer_contents(&buffer, |ptr, len, data| {
        let start = data.offset as usize;
        let count = (data.stride * data.height) as usize;
        assert!(start + count <= len);
        // SAFETY: the buffer is valid for len bytes during the callback; the range is checked above.
        unsafe { std::slice::from_raw_parts(ptr.add(start), count) }.to_vec()
    })
    .unwrap()
}

#[test]
fn outside_click_dismisses_without_activating_the_underlying_pane() {
    let mut h = Harness::new();
    h.click(APPEARANCE_ROW);
    h.wait_for_menu_to_close();
    // a press that was forwarded to the pane would switch it on the client's time, after the menu is gone
    h.settle();
    assert_eq!(
        (popup_count(&h.state), title(&h.state)),
        (0, "Display".into()),
        "outside click must dismiss the popup and consume the pane activation"
    );
    assert!(
        h.state.popup_grab.is_none()
            && !h.state.pointer.is_grabbed()
            && !h.state.seat.get_keyboard().unwrap().is_grabbed(),
        "the server releases the pointer and keyboard grabs with the press"
    );
}

#[test]
fn changing_settings_pane_destroys_the_choice_popup() {
    let mut h = Harness::new();
    h.show_appearance();
    // the client closes the menu as it leaves the page, on its own time
    h.wait_until("Appearance to drop Accessibility's menu", |s| {
        popup_count(s) == 0
    });
}

#[test]
fn appearance_change_does_not_leave_an_orphaned_choice_popup() {
    let mut h = Harness::new();
    let light = window_pixels(&h.state);
    let light_menu = popup_pixels(&h.state);
    h.state.set_appearance(Appearance::Dark);
    h.wait_until("Settings to redraw for Dark appearance", |s| {
        window_pixels(s) != light
    });
    h.wait_until("the menu to close or redraw for Dark appearance", |s| {
        popup_count(s) == 0 || popup_pixels(s) != light_menu
    });
    assert!(popup_count(&h.state) <= 1, "one menu at most");
}

#[test]
fn pressing_a_row_of_the_open_menu_selects_it() {
    let mut h = Harness::with_menu(
        "desktop-dock",
        "Desktop & Dock",
        (None, "Hot Corners"),
        TOP_LEFT_CORNER,
    );
    let window = h.state.windows.iter().find(|w| w.mapped).unwrap();
    let (popup, _) = PopupManager::popups_for_surface(window.surface())
        .next()
        .unwrap();
    let PopupKind::Xdg(popup) = popup else {
        panic!("Settings' menu is an xdg popup")
    };
    let location = popup.with_pending_state(|state| state.geometry.loc);
    // the second row ("Overview"): menu padding 5 plus one 22-point row and half a row
    h.click((
        f64::from(location.x) + 100.0,
        f64::from(location.y) + 5.0 + 22.0 + 11.0,
    ));
    h.wait_for_menu_to_close();
    h.wait_for_dock_setting("corner_top_left = overview");
}

#[test]
fn a_popup_under_a_stationary_pointer_takes_the_pointer_focus() {
    let mut h = Harness::with_menu(
        "desktop-dock",
        "Desktop & Dock",
        (None, "Hot Corners"),
        TOP_LEFT_CORNER,
    );
    let window = h.state.windows.iter().find(|w| w.mapped).unwrap();
    let parent = window.surface().clone();
    let frame = h.state.wm.window(window.id).unwrap().geometry();
    let (popup, _) = PopupManager::popups_for_surface(&parent).next().unwrap();
    let PopupKind::Xdg(popup) = popup else {
        panic!("Settings' menu is an xdg popup")
    };
    let location = popup.with_pending_state(|state| state.geometry.loc);
    let at = (
        frame.x + f64::from(location.x) + 100.0,
        frame.y + f64::from(location.y) + 30.0,
    )
        .into();
    // the pointer rests on the menu's place with the focus still on the window, as when the menu appears under it
    let pointer = h.state.pointer.clone();
    pointer.motion(
        &mut h.state,
        Some((parent.clone(), (frame.x, frame.y).into())),
        &MotionEvent {
            location: at,
            serial: SERIAL_COUNTER.next_serial(),
            time: 0,
        },
    );
    assert_eq!(pointer.current_focus(), Some(parent));
    h.state.refocus_pointer();
    assert_eq!(
        pointer.current_focus().as_ref(),
        Some(popup.wl_surface()),
        "the menu takes the pointer without any motion"
    );
}

const KEY_ESC: u32 = 1;
const KEY_ENTER: u32 = 28;
const KEY_DOWN: u32 = 108;
const KEY_TAB: u32 = 15;
/// How far a channel may differ between two runs and still count as the same look. An anti-aliased edge, as a group's
/// rounded corner beside the pop-up button, can come out one level apart depending on which frames the client's
/// repaints were split into; a focus ring that moved changes its pixels far more.
const SAME_LOOK: u8 = 2;

/// Whether two windows' pixels show the same look, every channel within [`SAME_LOOK`].
fn same_look(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= SAME_LOOK)
}

/// A pane with its menu open, the `keys` tapped (the last closes the menu), and its pixels once the client has had its
/// time: a key that reached the pane moves its focus ring. The pixels are compared between two runs, so a pane that draws
/// live data (a clock, a battery in the sidebar or the Accessibility pane) would need that region masked.
fn pane_after(keys: &[u32]) -> Vec<u8> {
    let mut h = Harness::new();
    for key in keys {
        h.tap(*key);
    }
    h.wait_for_menu_to_close();
    // a key that reached the pane would act on the client's time, after the menu is gone
    h.settle();
    assert_eq!(
        (popup_count(&h.state), title(&h.state)),
        (0, "Display".into()),
        "Escape closes the menu and leaves the pane as it was"
    );
    window_pixels(&h.state)
}

#[test]
fn escape_closes_the_open_menu_and_tab_does_not_reach_the_pane() {
    let mut h = Harness::new();
    h.tap(KEY_TAB);
    assert_eq!(popup_count(&h.state), 1, "Tab does not close the menu");
    drop(h);
    // Tab only moves the pane's focus, so the title cannot show it: the pane must look as if only Escape was pressed
    assert!(
        same_look(&pane_after(&[KEY_TAB, KEY_ESC]), &pane_after(&[KEY_ESC])),
        "Tab reached the pane behind the menu and moved its focus"
    );
}

#[test]
fn down_and_return_choose_the_next_item_of_the_open_menu() {
    let mut h = Harness::with_menu(
        "desktop-dock",
        "Desktop & Dock",
        (None, "Hot Corners"),
        TOP_LEFT_CORNER,
    );
    h.tap(KEY_DOWN);
    assert_eq!(popup_count(&h.state), 1, "Down only moves the highlight");
    h.tap(KEY_ENTER);
    h.wait_for_menu_to_close();
    h.wait_for_dock_setting("corner_top_left = overview");
}
