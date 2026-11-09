//! Pointer constraints against a headless compositor and clients on its socket: a lock keeps the pointer in place and
//! sends only relative motion, a confine keeps it in its region, a constraint is active only on the window with the
//! keyboard focus and a client that answers its pings, and the release chord gives the pointer back until the person
//! focuses or presses on the window again. Tests skip (pass) when no EGL device is available.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::{Client, Constraint};
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::frame::CursorChoice;
use cupertino_server::Cupertino;
use cupertino_wm::WindowId;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::Keycode;
use smithay::input::pointer::{CursorIcon, CursorImageStatus, RelativeMotionEvent};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Logical, Point};

/// evdev codes
const KEY_ESC: u32 = 1;
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

    fn advance(&mut self, seconds: f64) {
        self.now += seconds;
        self.settle();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

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

    fn release_chord(&mut self) {
        self.key(KEY_SUPER, true);
        self.key(KEY_ESC, true);
        self.key(KEY_ESC, false);
        self.key(KEY_SUPER, false);
    }

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

    /// The point `(x, y)` of `window`'s content, in global coordinates.
    fn in_window(&self, window: WindowId, x: f64, y: f64) -> Point<f64, Logical> {
        let frame = self.state.wm.window(window).unwrap().geometry();
        (frame.x + x, frame.y + y).into()
    }

    /// A motion of the person's pointer to `to`, by the delta from where it is.
    fn move_to(&mut self, to: Point<f64, Logical>) {
        let delta = to - self.state.pointer.current_location();
        let rel = RelativeMotionEvent {
            delta,
            delta_unaccel: delta,
            utime: (self.now * 1e6) as u64,
        };
        self.state
            .pointer_moved_to(to, (self.now * 1000.0) as u32, Some(rel));
        self.settle();
    }

    fn press(&mut self) {
        let time = (self.now * 1000.0) as u32;
        let left = cupertino_server::events::BTN_LEFT;
        self.state.pointer_button(left, ButtonState::Pressed, time);
        self.state.pointer_button(left, ButtonState::Released, time);
        self.settle();
    }
}

#[test]
fn a_lock_keeps_the_pointer_in_place_and_sends_only_relative_motion_until_the_release_chord() {
    let Some(mut h) = Harness::new() else { return };
    let game = Client::constraining(h.socket_path(), "org.example.game", Constraint::Lock, false);
    let window = h.mapped(1, "org.example.game");
    let centre = h.in_window(window, 200.0, 150.0);
    h.move_to(centre);
    h.wait_until("the lock", |_| game.constraint_events() == [true]);
    let notice = h
        .state
        .overlays
        .shortcut_notice
        .clone()
        .expect("the notice");
    assert!(
        notice
            .text
            .starts_with("Press Super+Escape to get your pointer back."),
        "{}",
        notice.text
    );

    let motions = game.motions().len();
    h.move_to(centre + Point::from((50.0, 30.0)));
    assert_eq!(
        h.state.pointer.current_location(),
        centre,
        "the pointer stays"
    );
    assert_eq!(
        game.motions().len(),
        motions,
        "no motion reaches the client"
    );
    assert_eq!(game.relative_motions().last(), Some(&(50.0, 30.0)));

    h.release_chord();
    h.wait_until("the unlock", |_| game.constraint_events() == [true, false]);
    assert!(
        !game.keys().iter().any(|(key, _)| *key == KEY_ESC),
        "Escape of the release chord stays with the compositor: {:?}",
        game.keys()
    );
    assert!(h.state.overlays.shortcut_notice.is_none());
    let moved = centre + Point::from((50.0, 30.0));
    h.move_to(moved);
    assert_eq!(
        h.state.pointer.current_location(),
        moved,
        "the pointer moves again"
    );
}

#[test]
fn the_authentication_prompt_ends_the_lock_and_the_inhibitor_while_it_is_shown() {
    let Some(mut h) = Harness::new() else { return };
    let game = Client::constraining(h.socket_path(), "org.example.game", Constraint::Lock, true);
    let window = h.mapped(1, "org.example.game");
    // the prompt is in the top left corner of the output, away from the pointer and the window
    h.move_to(h.in_window(window, 200.0, 150.0));
    h.wait_until("the lock and the inhibitor", |_| {
        game.constraint_events() == [true] && game.inhibitor_events() == [true]
    });

    let prompt = Client::prompt(common::prompt_agent(&h.state));
    h.wait_until("the prompt to be shown", |s| s.prompt_shown());
    h.wait_until("the lock and the inhibitor to end", |_| {
        game.constraint_events() == [true, false] && game.inhibitor_events() == [true, false]
    });

    prompt.unlock();
    h.wait_until("the prompt to go", |s| !s.prompt_shown());
    h.wait_until("the lock and the inhibitor to be taken again", |_| {
        game.constraint_events() == [true, false, true]
            && game.inhibitor_events() == [true, false, true]
    });
}

#[test]
fn a_confine_keeps_the_pointer_inside_its_region_and_lets_it_slide_along_the_edge() {
    let Some(mut h) = Harness::new() else { return };
    let region = Constraint::Confine {
        x: 100,
        y: 100,
        w: 100,
        h: 50,
    };
    let game = Client::constraining(h.socket_path(), "org.example.game", region, false);
    let window = h.mapped(1, "org.example.game");
    h.move_to(h.in_window(window, 20.0, 20.0));
    assert_eq!(
        game.constraint_events(),
        Vec::<bool>::new(),
        "outside the region: not yet"
    );

    h.move_to(h.in_window(window, 150.0, 125.0));
    h.wait_until("the confine", |_| game.constraint_events() == [true]);
    h.move_to(h.in_window(window, 380.0, 130.0));
    assert_eq!(
        h.state.pointer.current_location(),
        h.in_window(window, 199.0, 130.0)
    );
    h.move_to(h.in_window(window, 120.0, -40.0));
    assert_eq!(
        h.state.pointer.current_location(),
        h.in_window(window, 120.0, 100.0)
    );
}

#[test]
fn a_constraint_is_active_only_on_the_window_with_the_keyboard_focus() {
    let Some(mut h) = Harness::new() else { return };
    let game = Client::constraining(h.socket_path(), "org.example.game", Constraint::Lock, false);
    let window = h.mapped(1, "org.example.game");
    // a corner of the window, away from the layer surface in the middle of the output
    h.move_to(h.in_window(window, 5.0, 5.0));
    h.wait_until("the lock", |_| game.constraint_events() == [true]);

    // a layer surface takes the keyboard; the pointer stays over the game
    let _layer = Client::inhibiting_layer(h.socket_path());
    h.wait_until("the layer to take the keyboard", |s| {
        s.exclusive_layer.is_some()
    });
    h.advance(0.5);
    assert_eq!(
        h.state.pointer.current_location(),
        h.in_window(window, 5.0, 5.0)
    );
    h.wait_until("the unlock", |_| game.constraint_events() == [true, false]);
}

#[test]
fn a_released_window_shows_the_cursor_and_is_armed_again_by_a_press_while_it_has_the_focus() {
    let Some(mut h) = Harness::new() else { return };
    let vm = Client::constraining(h.socket_path(), "org.example.vm", Constraint::Lock, true);
    let window = h.mapped(1, "org.example.vm");
    h.move_to(h.in_window(window, 200.0, 150.0));
    h.wait_until("both held", |_| {
        vm.constraint_events() == [true] && vm.inhibitor_events().last() == Some(&true)
    });
    let notice = h
        .state
        .overlays
        .shortcut_notice
        .clone()
        .expect("the notice");
    assert!(
        notice
            .text
            .starts_with("Press Super+Escape to get your input back."),
        "{}",
        notice.text
    );

    h.release_chord();
    h.wait_until("both released", |_| {
        vm.constraint_events() == [true, false] && vm.inhibitor_events().last() == Some(&false)
    });
    h.state.cursor_status = CursorImageStatus::Hidden;
    assert!(
        matches!(
            h.state.cursor_choice(),
            CursorChoice::Icon(CursorIcon::Default)
        ),
        "a released window cannot hide the cursor"
    );
    // still the keyboard's window: a press on it is the person's way back in
    assert_eq!(h.state.wm.key_window(), Some(window));
    h.press();
    h.wait_until("both held again", |_| {
        vm.constraint_events() == [true, false, true] && vm.inhibitor_events().last() == Some(&true)
    });
    assert!(matches!(h.state.cursor_choice(), CursorChoice::Hidden));
}

#[test]
fn a_client_that_does_not_answer_its_pings_holds_neither_the_keys_nor_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    let vm = Client::constraining(h.socket_path(), "org.example.vm", Constraint::Lock, true);
    let window = h.mapped(1, "org.example.vm");
    h.move_to(h.in_window(window, 200.0, 150.0));
    h.wait_until("both held", |_| {
        vm.constraint_events() == [true] && vm.inhibitor_events().last() == Some(&true)
    });

    vm.mute(true);
    for dt in [1.1, 2.5] {
        h.now += dt;
        h.state.fixed_time = Some(h.now);
        h.state.tick();
    }
    assert!(h.state.window_is_unresponsive(window));
    h.wait_until("both let go", |_| {
        vm.constraint_events() == [true, false] && vm.inhibitor_events().last() == Some(&false)
    });

    vm.mute(false);
    h.advance(1.1);
    h.wait_until("held again once it answers", |s| {
        !s.window_is_unresponsive(window)
            && vm.constraint_events() == [true, false, true]
            && vm.inhibitor_events().last() == Some(&true)
    });
}

#[test]
fn a_lock_holds_while_the_pointer_is_over_a_subsurface_of_its_surface() {
    let Some(mut h) = Harness::new() else { return };
    let game = Client::constraining_under_subsurface(h.socket_path(), "org.example.game");
    let window = h.mapped(1, "org.example.game");
    let centre = h.in_window(window, 200.0, 150.0);
    h.move_to(centre);
    h.wait_until("the lock", |_| game.constraint_events() == [true]);

    h.move_to(centre + Point::from((50.0, 30.0)));
    assert_eq!(
        h.state.pointer.current_location(),
        centre,
        "the pointer stays"
    );
    assert_eq!(game.relative_motions().last(), Some(&(50.0, 30.0)));
}

#[test]
fn the_notice_goes_with_the_hold_however_it_ends() {
    let Some(mut h) = Harness::new() else { return };
    let game = Client::constraining(h.socket_path(), "org.example.game", Constraint::Lock, false);
    let window = h.mapped(1, "org.example.game");
    h.move_to(h.in_window(window, 200.0, 150.0));
    h.wait_until("the lock", |_| game.constraint_events() == [true]);
    assert!(h.state.overlays.shortcut_notice.is_some());

    // the client ends the lock itself: no unlock comes from the compositor, and the notice goes well before its time
    game.unlock();
    h.wait_until("the notice to go", |s| {
        !s.pointer_held() && s.overlays.shortcut_notice.is_none()
    });
}

#[test]
fn the_release_chord_releases_the_application_not_only_its_window() {
    let Some(mut h) = Harness::new() else { return };
    let _vm = Client::inhibiting_twice(h.socket_path(), "org.example.vm");
    h.wait_until("both windows", |s| s.wm.windows().count() == 2);
    h.advance(1.0);
    let windows: Vec<WindowId> = h.state.wm.windows().map(|w| w.id).collect();
    h.state.focus_window_by_input(windows[0]);
    h.advance(0.5);
    h.wait_until("held", |s| s.shortcut_inhibit.holding());

    h.release_chord();
    assert!(!h.state.shortcut_inhibit.holding());
    // the application moves the focus to its other window itself: still released
    h.state
        .command(cupertino_wm::Command::FocusWindow { window: windows[1] });
    h.advance(0.5);
    assert_eq!(h.state.wm.key_window(), Some(windows[1]));
    assert!(
        !h.state.shortcut_inhibit.holding(),
        "a window of a released application holds nothing"
    );

    // the person focuses a window of the application: held again
    h.state.focus_window_by_input(windows[0]);
    h.advance(0.5);
    h.wait_until("held again", |s| s.shortcut_inhibit.holding());
}
