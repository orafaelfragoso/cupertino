//! Keyboard shortcuts inhibit against a headless compositor and clients on its socket: an application that holds the
//! shortcuts gets them (Super+Tab reaches it and opens no switcher), the release chord gives them back and the client
//! cannot take them again by moving the focus itself, and a notice names the application. Tests skip (pass) when no EGL
//! device is available.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::Client;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use cupertino_wm::{Command, WindowId};
use smithay::backend::input::KeyState;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

/// evdev codes
const KEY_ESC: u32 = 1;
const KEY_TAB: u32 = 15;
const KEY_SUPER: u32 = 125;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    now: f64,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .ok()?;
        match headless::init(&mut state, (1000, 700), 1.0) {
            Ok(()) => Some(Harness {
                state,
                event_loop,
                now: 100.0,
            }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.state.fixed_time = Some(self.now);
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Let the real time the clients need pass at the model time `now`.
    fn settle(&mut self) {
        let end = Instant::now() + Duration::from_millis(150);
        self.wait_until("the time to pass", |_| Instant::now() >= end);
    }

    /// Move the model clock on by `seconds`, a few steps so that every tick sees it.
    fn advance(&mut self, seconds: f64) {
        self.now += seconds;
        self.settle();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }
}

impl Harness {
    fn key(&mut self, evdev: u32, pressed: bool) {
        self.state.key_from_device(
            Keycode::new(evdev + 8),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            (self.now * 1000.0) as u32,
        );
        self.settle();
    }

    fn chord(&mut self, modifier: u32, key: u32) {
        self.key(modifier, true);
        self.key(key, true);
        self.key(key, false);
        self.key(modifier, false);
    }

    /// The window of the application `identity`.
    fn window_of(&self, identity: &str) -> WindowId {
        self.state
            .wm
            .windows()
            .find(|w| {
                self.state
                    .wm
                    .app(w.app)
                    .is_some_and(|a| a.identity == identity)
            })
            .map(|w| w.id)
            .expect("a window of the application")
    }

    /// Wait for `count` windows, let their opening finish and give `identity`'s window the focus, as the person does.
    fn mapped(&mut self, count: usize, identity: &str) -> WindowId {
        self.wait_until("the windows to map", |s| s.wm.windows().count() == count);
        self.advance(1.0);
        let window = self.window_of(identity);
        self.state.focus_window_by_input(window);
        self.advance(1.0);
        window
    }
}

#[test]
fn the_application_that_holds_the_shortcuts_gets_super_tab_and_no_switcher_opens() {
    let Some(mut h) = Harness::new() else { return };
    let vm = Client::inhibiting(h.socket_path(), "org.example.vm");
    h.wait_until("the inhibitor to be active", |_| {
        vm.inhibitor_events() == [true]
    });
    let notice = h
        .state
        .overlays
        .shortcut_notice
        .clone()
        .expect("the notice");
    assert!(notice.text.contains("org.example.vm"), "{}", notice.text);
    assert!(notice.text.contains("Super+Escape"), "{}", notice.text);

    h.chord(KEY_SUPER, KEY_TAB);
    assert!(h.state.wm.switcher().is_none(), "the switcher stayed shut");
    assert!(
        vm.keys().contains(&(KEY_TAB, true)),
        "the client got Tab: {:?}",
        vm.keys()
    );

    h.advance(5.0);
    assert!(
        h.state.overlays.shortcut_notice.is_none(),
        "the notice goes"
    );
}

#[test]
fn the_release_chord_gives_the_shortcuts_back_and_is_not_forwarded() {
    let Some(mut h) = Harness::new() else { return };
    let vm = Client::inhibiting(h.socket_path(), "org.example.vm");
    // the switcher needs a second application to switch to
    let _other = Client::toplevel(h.socket_path());
    h.mapped(2, "org.example.vm");
    h.wait_until("the inhibitor to be active", |_| {
        vm.inhibitor_events().last() == Some(&true)
    });

    h.chord(KEY_SUPER, KEY_ESC);
    h.wait_until("the inhibitor to be inactive", |_| {
        vm.inhibitor_events().last() == Some(&false)
    });
    assert!(
        !vm.keys().iter().any(|(key, _)| *key == KEY_ESC),
        "Escape of the release chord stays with the compositor: {:?}",
        vm.keys()
    );
    assert!(h.state.overlays.shortcut_notice.is_none());

    // the modifier is held: the switcher is open until it is let go
    h.key(KEY_SUPER, true);
    h.key(KEY_TAB, true);
    assert!(
        h.state.wm.switcher().is_some(),
        "Super+Tab is the system's again"
    );
    h.key(KEY_TAB, false);
    h.key(KEY_SUPER, false);
}

#[test]
fn a_released_surface_is_armed_again_only_by_focus_from_the_persons_input() {
    let Some(mut h) = Harness::new() else { return };
    let vm = Client::inhibiting(h.socket_path(), "org.example.vm");
    h.wait_until("the inhibitor to be active", |_| {
        vm.inhibitor_events() == [true]
    });
    let _other = Client::toplevel(h.socket_path());
    let a = h.mapped(2, "org.example.vm");
    let b = h.window_of("org.example.locked-out");

    // another window has the keyboard: inactive; the person comes back: active; the person releases it
    h.state.focus_window_by_input(b);
    h.advance(1.0);
    h.wait_until("inactive", |_| vm.inhibitor_events() == [true, false]);
    h.state.focus_window_by_input(a);
    h.advance(1.0);
    h.wait_until("armed by input", |_| {
        vm.inhibitor_events() == [true, false, true]
    });
    h.chord(KEY_SUPER, KEY_ESC);
    h.wait_until("released", |_| {
        vm.inhibitor_events() == [true, false, true, false]
    });

    // the focus goes elsewhere and a request of a client brings it back: still released
    h.state.focus_window_by_input(b);
    h.advance(1.0);
    h.state.command(Command::FocusWindow { window: a });
    h.advance(1.0);
    assert_eq!(h.state.wm.key_window(), Some(a));
    assert_eq!(
        vm.inhibitor_events(),
        [true, false, true, false],
        "a client request does not arm the shortcuts again"
    );

    // the person focuses it again: armed
    h.state.focus_window_by_input(b);
    h.advance(1.0);
    h.state.focus_window_by_input(a);
    h.advance(1.0);
    h.wait_until("armed by input again", |_| {
        vm.inhibitor_events() == [true, false, true, false, true]
    });
}

#[test]
fn a_release_chord_the_keymap_cannot_type_gets_the_default_beside_it() {
    use cupertino_server::input::{parse_chord, Action, KeyBindings};

    let Some(mut h) = Harness::new() else { return };
    let super_escape = parse_chord("Super+Escape").unwrap();
    let mut bindings = KeyBindings::default();
    // XF86Launch... is not a key of the US layout the tests run with: a chord on it cannot be pressed
    let untypeable = parse_chord("Super+0x1008ff40").unwrap();
    bindings.bind(untypeable, Action::ReleaseShortcuts);
    h.state.key_router.bindings = bindings;
    h.state.keep_release_chord_typeable();
    assert_eq!(
        h.state
            .key_router
            .bindings
            .lookup(super_escape.sym, super_escape.mods),
        Some(Action::ReleaseShortcuts),
        "the default is bound beside it"
    );

    let typeable = parse_chord("Super+x").unwrap();
    let mut bindings = KeyBindings::default();
    bindings.bind(typeable, Action::ReleaseShortcuts);
    h.state.key_router.bindings = bindings;
    h.state.keep_release_chord_typeable();
    assert_eq!(
        h.state
            .key_router
            .bindings
            .lookup(super_escape.sym, super_escape.mods),
        None,
        "a chord the keymap can type is left alone"
    );
}

#[test]
fn the_notice_keeps_the_way_out_whatever_the_application_is_called() {
    // 2000 characters, with a control character and characters that turn the text around
    let long: &'static str =
        Box::leak(format!("org.example.\u{202e}\u{7}{}", "x".repeat(2000)).into_boxed_str());
    let Some(mut h) = Harness::new() else { return };
    let vm = Client::inhibiting(h.socket_path(), long);
    h.wait_until("the inhibitor to be active", |_| {
        vm.inhibitor_events() == [true]
    });
    let notice = h
        .state
        .overlays
        .shortcut_notice
        .clone()
        .expect("the notice");
    assert!(
        notice.text.starts_with("Press Super+Escape"),
        "{}",
        notice.text
    );
    assert!(
        notice.text.chars().count() < 120,
        "{}",
        notice.text.chars().count()
    );
    assert!(
        !notice
            .text
            .chars()
            .any(|c| c.is_control() || c == '\u{202e}'),
        "no control or bidi characters"
    );
}

#[test]
fn a_layer_surface_never_holds_the_shortcuts() {
    let Some(mut h) = Harness::new() else { return };
    let layer = Client::inhibiting_layer(h.socket_path());
    // the layer surface maps and takes the keyboard (exclusive); an inhibitor on it must stay inactive
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        h.settle();
    }
    assert!(
        h.state.exclusive_layer.is_some(),
        "the layer surface has the keyboard"
    );
    assert_eq!(layer.inhibitor_events(), Vec::<bool>::new());
    assert!(h.state.overlays.shortcut_notice.is_none());
}

#[test]
fn the_notice_of_the_window_that_has_the_focus_survives_the_deactivation_of_another() {
    let Some(mut h) = Harness::new() else { return };
    let a = Client::inhibiting(h.socket_path(), "org.example.first");
    h.wait_until("the first inhibitor to be active", |_| {
        a.inhibitor_events().last() == Some(&true)
    });
    let b = Client::inhibiting(h.socket_path(), "org.example.second");
    h.wait_until("both windows", |s| s.wm.windows().count() == 2);
    h.advance(1.0);
    let (first, second) = (
        h.window_of("org.example.first"),
        h.window_of("org.example.second"),
    );

    // the inhibitors were created in the order first, second; focus the second, then the first: the second's
    // deactivation comes after the first's activation in the same pass
    h.state.focus_window_by_input(second);
    h.advance(0.1);
    h.wait_until("the second is active", |_| {
        b.inhibitor_events().last() == Some(&true)
    });
    h.state.focus_window_by_input(first);
    h.wait_until("the first is active again", |_| {
        a.inhibitor_events().last() == Some(&true)
    });
    let notice = h
        .state
        .overlays
        .shortcut_notice
        .clone()
        .expect("the notice of the first");
    assert!(notice.text.contains("org.example.first"), "{}", notice.text);
}

#[test]
fn the_notice_ends_at_its_time_and_not_before() {
    let Some(mut h) = Harness::new() else { return };
    let vm = Client::inhibiting(h.socket_path(), "org.example.vm");
    h.wait_until("the inhibitor to be active", |_| {
        vm.inhibitor_events() == [true]
    });
    let until = h
        .state
        .overlays
        .shortcut_notice
        .clone()
        .expect("the notice")
        .until;
    let focus = h.state.seat.get_keyboard().and_then(|k| k.current_focus());
    h.state.fixed_time = Some(until - 0.001);
    h.state.refresh_shortcut_inhibitors(focus.as_ref());
    assert!(
        h.state.overlays.shortcut_notice.is_some(),
        "still shown just before"
    );
    h.state.fixed_time = Some(until);
    h.state.refresh_shortcut_inhibitors(focus.as_ref());
    assert!(
        h.state.overlays.shortcut_notice.is_none(),
        "gone at its time"
    );
}
