//! Issue #5: the real Settings choice menu must not survive dismissal or navigation.
//! Run with `cargo test -p cupertino-server --test settings-popup -- --test-threads=1`.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use cupertino_server::{backend::headless, config::Config, Cupertino};
use cupertino_theme::Appearance;
use smithay::backend::input::ButtonState;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::desktop::{PopupKind, PopupManager};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::wayland::compositor::with_states;
use smithay::wayland::shell::xdg::XdgToplevelSurfaceData;
use smithay::wayland::shm::with_buffer_contents;

const TIMEOUT: Duration = Duration::from_secs(10);
// Centres of the Colour filter button and Appearance sidebar row in the 715 × 625 window.
const COLOUR_FILTER: (f64, f64) = (600.0, 348.0);
// Centre of the Top Left hot corner's button on the Desktop & Dock pane.
const TOP_LEFT_CORNER: (f64, f64) = (600.0, 386.0);
const APPEARANCE_ROW: (f64, f64) = (100.0, 256.0);

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
        let exe = std::env::current_exe().unwrap();
        let profile = exe.parent().unwrap().parent().unwrap();
        let mut build = Command::new(std::env::var_os("CARGO").expect("run with cargo test"));
        build.args([
            "build",
            "--locked",
            "-p",
            "cupertino-settings",
            "--features",
            "app",
            "--target-dir",
        ]);
        build.arg(profile.parent().unwrap());
        if profile.file_name().unwrap() == "release" {
            build.arg("--release");
        }
        assert!(
            build.status().unwrap().success(),
            "building the real Settings process"
        );
        profile.join("cupertino-settings")
    })
}

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    app: Process,
    bus: Process,
    address: String,
    home: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self::with_menu("accessibility", "Accessibility", COLOUR_FILTER)
    }

    fn with_menu(pane: &str, pane_title: &str, button: (f64, f64)) -> Self {
        let binary = settings_binary();
        let home = tempfile::tempdir().unwrap();
        let mut bus = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(bus.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "the private bus published its address");
        let bus = Process(bus);
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
            Command::new(binary)
                .args(["--pane", pane])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", home.path())
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
        h.run_for(Duration::from_millis(250));
        h.click(button);
        h.wait_until("one mapped choice popup", |s| popup_count(s) == 1);
        assert_eq!(title(&h.state), pane_title);
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
        assert!(
            self.bus.0.try_wait().unwrap().is_none(),
            "private bus exited unexpectedly"
        );
    }

    fn wait_until(&mut self, what: &str, condition: impl Fn(&Cupertino) -> bool) {
        let end = Instant::now() + TIMEOUT;
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

    fn run_for(&mut self, duration: Duration) {
        let end = Instant::now() + duration;
        while Instant::now() < end {
            self.step();
        }
    }

    fn click(&mut self, point: (f64, f64)) {
        let window = self.state.windows.iter().find(|w| w.mapped).unwrap();
        let frame = self.state.wm.window(window.id).unwrap().geometry();
        self.state
            .pointer_moved_to((frame.x + point.0, frame.y + point.1).into(), 0, None);
        self.step();
        self.state.pointer_button(0x110, ButtonState::Pressed, 0);
        self.step();
        self.state.pointer_button(0x110, ButtonState::Released, 0);
    }

    // Use the public ShowPane handoff so the navigation assertion is independent of pointer dismissal.
    fn show_appearance(&mut self) {
        let mut handoff = Process(
            Command::new(settings_binary())
                .args(["--pane", "appearance"])
                .env_clear()
                .env("HOME", self.home.path())
                .env("DBUS_SESSION_BUS_ADDRESS", &self.address)
                .spawn()
                .unwrap(),
        );
        self.wait_until("Appearance pane", |s| title(s) == "Appearance");
        self.wait_until("pane redraw", |s| !window_pixels(s).is_empty());
        self.run_for(Duration::from_millis(250));
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

fn window_pixels(state: &Cupertino) -> Vec<u8> {
    let window = state.windows.iter().find(|w| w.mapped).unwrap();
    let buffer = with_renderer_surface_state(window.surface(), |s| s.buffer().cloned())
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
    h.run_for(Duration::from_millis(500));
    assert_eq!(
        (popup_count(&h.state), title(&h.state)),
        (0, "Accessibility".into()),
        "outside click must dismiss the popup and consume the pane activation"
    );
}

#[test]
fn changing_settings_pane_destroys_the_choice_popup() {
    let mut h = Harness::new();
    h.show_appearance();
    assert_eq!(
        popup_count(&h.state),
        0,
        "Appearance must not retain Accessibility's menu"
    );
}

#[test]
fn appearance_change_does_not_leave_an_orphaned_choice_popup() {
    let mut h = Harness::new();
    let light = window_pixels(&h.state);
    h.state.set_appearance(Appearance::Dark);
    h.wait_until("Settings to redraw for Dark appearance", |s| {
        window_pixels(s) != light
    });
    h.run_for(Duration::from_millis(250));
    assert_eq!(
        popup_count(&h.state),
        0,
        "Dark Appearance must not retain Accessibility's menu"
    );
}

#[test]
#[ignore = "#5: the server delivers a press on a menu row to the parent window"]
fn pressing_a_row_of_the_open_menu_selects_it() {
    let mut h = Harness::with_menu("desktop-dock", "Desktop & Dock", TOP_LEFT_CORNER);
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
    h.run_for(Duration::from_millis(500));
    assert_eq!(popup_count(&h.state), 0, "the menu closes on the choice");
    let stored = std::fs::read_to_string(h.home.path().join("config/cupertino/dock.conf")).unwrap();
    assert!(
        stored
            .lines()
            .any(|l| l.trim() == "corner_top_left = overview"),
        "the chosen row is committed: {stored:?}"
    );
}
