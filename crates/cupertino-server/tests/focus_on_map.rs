//! The focus policy for windows that map, against a headless compositor and clients on its socket: the first window of a
//! program the person just started comes to the front (a command typed in a terminal), unless the person has given input to
//! another client since it connected, in which case it opens behind and a valid activation token it presents before it maps
//! still brings it forward. Tests skip (pass) when no EGL device is available.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::Client;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::events::BTN_LEFT;
use cupertino_server::Cupertino;
use cupertino_wm::WindowId;
use smithay::backend::input::ButtonState;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

fn client_count(state: &Cupertino) -> usize {
    let mut count = 0;
    state
        .display_handle
        .backend_handle()
        .with_all_clients(|_| count += 1);
    count
}

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
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
        common::each_connection_a_process(&mut state);
        match headless::init(&mut state, (1000, 700), 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
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

    fn run_for(&mut self, duration: Duration) {
        let end = Instant::now() + duration;
        self.wait_until("the time to pass", |_| Instant::now() >= end);
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn clients(&self) -> usize {
        client_count(&self.state)
    }

    /// The window that is mapped and is not `known`.
    fn newest(&self, known: &[WindowId]) -> Option<WindowId> {
        self.state
            .windows
            .iter()
            .filter(|e| e.mapped)
            .map(|e| e.id)
            .find(|id| !known.contains(id))
    }

    /// A press on the window `id`: input that goes to its client.
    fn click_on(&mut self, id: WindowId) {
        let frame = self.state.wm.window(id).unwrap().geometry();
        self.state
            .pointer_moved_to((frame.x + 60.0, frame.y + 120.0).into(), 0, None);
        self.run_for(Duration::from_millis(60));
        self.state.pointer_button(BTN_LEFT, ButtonState::Pressed, 0);
        self.run_for(Duration::from_millis(30));
        self.state
            .pointer_button(BTN_LEFT, ButtonState::Released, 0);
        self.run_for(Duration::from_millis(30));
    }
}

/// Move the frozen model clock on.
fn advance(h: &mut Harness, seconds: f64) {
    h.state.fixed_time = h.state.fixed_time.map(|t| t + seconds);
}

/// An application `a` that is mapped and has the focus; the harness has stepped until it settled.
fn with_front_window(h: &mut Harness) -> (Client, WindowId) {
    let a = Client::toplevel_after(
        h.socket_path(),
        "org.example.front",
        Arc::new(AtomicBool::new(true)),
        Arc::default(),
    );
    h.wait_until("the first window to map", |s| {
        s.windows.iter().any(|e| e.mapped)
    });
    h.state.fixed_time = Some(h.state.now() + 5.0);
    h.state.tick();
    let id = h.newest(&[]).expect("a mapped window");
    assert_eq!(h.state.wm.key_window(), Some(id));
    h.run_for(Duration::from_millis(100));
    (a, id)
}

#[test]
fn the_first_window_of_a_program_the_person_just_started_comes_to_the_front() {
    let Some(mut h) = Harness::new() else { return };
    let (_a, front) = with_front_window(&mut h);
    let _typed = Client::toplevel_after(
        h.socket_path(),
        "org.example.typed",
        Arc::new(AtomicBool::new(true)),
        Arc::default(),
    );
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    let new = h.newest(&[front]).unwrap();
    assert_eq!(
        h.state.wm.key_window(),
        Some(new),
        "no input went elsewhere since it connected"
    );
}

#[test]
fn a_program_that_connects_and_maps_while_the_person_types_opens_behind_the_key_window() {
    let Some(mut h) = Harness::new() else { return };
    let (_a, front) = with_front_window(&mut h);
    advance(&mut h, 1.0);
    h.click_on(front);
    let _intruder = Client::toplevel_after(
        h.socket_path(),
        "org.example.intruder",
        Arc::new(AtomicBool::new(true)),
        Arc::default(),
    );
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    let new = h.newest(&[front]).unwrap();
    assert_eq!(h.state.wm.key_window(), Some(front), "the keystrokes stay");
    let order = h.state.wm.stacking_order();
    let at = |w: WindowId| order.iter().position(|x| *x == w).unwrap();
    assert!(at(new) < at(front), "behind the key window: {order:?}");
}

#[test]
fn a_process_that_reconnects_gets_the_first_window_grant_once() {
    let Some(mut h) = Harness::new() else { return };
    h.state.process_of = Box::new(cupertino_server::process::peer_process);
    let (_a, front) = with_front_window(&mut h);
    // the test's clients are all this process: its first window was the front one
    let _again = Client::toplevel_after(
        h.socket_path(),
        "org.example.again",
        Arc::new(AtomicBool::new(true)),
        Arc::default(),
    );
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    assert_eq!(
        h.state.wm.key_window(),
        Some(front),
        "no input went elsewhere, but the process had its grant"
    );
}

#[test]
fn a_program_the_person_left_before_its_window_mapped_opens_behind_the_key_window() {
    let Some(mut h) = Harness::new() else { return };
    let (_a, front) = with_front_window(&mut h);
    let before = h.clients();
    let gate = Arc::new(AtomicBool::new(false));
    let _slow = Client::toplevel_after(
        h.socket_path(),
        "org.example.slow",
        gate.clone(),
        Arc::default(),
    );
    h.wait_until("the program to connect", |s| client_count(s) > before);
    h.run_for(Duration::from_millis(50));
    // the person goes back to the front application before the slow program has a window
    h.click_on(front);
    gate.store(true, Ordering::SeqCst);
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    let new = h.newest(&[front]).unwrap();
    assert_eq!(h.state.wm.key_window(), Some(front), "the keyboard stays");
    let order = h.state.wm.stacking_order();
    let at = |w: WindowId| order.iter().position(|x| *x == w).unwrap();
    assert!(at(new) < at(front), "behind the key window: {order:?}");
}

#[test]
fn a_valid_token_presented_before_the_window_maps_brings_it_forward() {
    let Some(mut h) = Harness::new() else { return };
    let (_a, front) = with_front_window(&mut h);
    let before = h.clients();
    let gate = Arc::new(AtomicBool::new(false));
    let token = Arc::new(Mutex::new(None));
    let _launched = Client::toplevel_after(
        h.socket_path(),
        "org.example.launched",
        gate.clone(),
        token.clone(),
    );
    h.wait_until("the program to connect", |s| client_count(s) > before);
    h.run_for(Duration::from_millis(50));
    // the person goes back to the front application, then starts something: the token is made after that input
    h.click_on(front);
    h.run_for(Duration::from_millis(150));
    advance(&mut h, 1.0);
    *token.lock().unwrap() = Some(h.state.server_activation_token());
    gate.store(true, Ordering::SeqCst);
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    let new = h.newest(&[front]).unwrap();
    assert_eq!(
        h.state.wm.key_window(),
        Some(new),
        "the token it presented first counts"
    );
}

#[test]
fn a_token_made_before_the_person_moved_on_does_not() {
    let Some(mut h) = Harness::new() else { return };
    let (_a, front) = with_front_window(&mut h);
    let before = h.clients();
    let gate = Arc::new(AtomicBool::new(false));
    let token = Arc::new(Mutex::new(Some(h.state.server_activation_token())));
    let _launched =
        Client::toplevel_after(h.socket_path(), "org.example.stale", gate.clone(), token);
    h.wait_until("the program to connect", |s| client_count(s) > before);
    h.run_for(Duration::from_millis(150));
    // the model clock stands still in these tests: a second passes before the click
    advance(&mut h, 1.0);
    h.click_on(front);
    gate.store(true, Ordering::SeqCst);
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    assert_eq!(
        h.state.wm.key_window(),
        Some(front),
        "a click followed the token"
    );
}

#[test]
fn a_program_that_maps_while_the_session_is_locked_does_not_take_the_keyboard_at_unlock() {
    let Some(mut h) = Harness::new() else { return };
    let (_a, front) = with_front_window(&mut h);
    let locker = Client::locker(common::session_agent(&h.state));
    h.wait_until("the session to be locked", |_| locker.locked());
    let _intruder = Client::toplevel_after(
        h.socket_path(),
        "org.example.during-lock",
        Arc::new(AtomicBool::new(true)),
        Arc::default(),
    );
    h.wait_until("the second window to map", |s| {
        s.windows.iter().filter(|e| e.mapped).count() == 2
    });
    locker.unlock();
    h.wait_until("the session to unlock", |s| !s.session_lock.is_active());
    h.run_for(Duration::from_millis(100));
    assert_eq!(
        h.state.wm.key_window(),
        Some(front),
        "the keyboard goes back to the window that had it"
    );
}
