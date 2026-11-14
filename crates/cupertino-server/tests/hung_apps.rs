//! Hung applications against a headless compositor and a client on its socket: the client is pinged while the desktop is
//! used, answers, goes silent, is unresponsive two seconds after the ping it does not answer (the busy cursor shows over
//! its window) and is responsive again when it answers; a desktop nobody uses pings no one. The model clock stands still in these tests and moves where the test moves it. Tests skip (pass)
//! when no EGL device is available.

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::Client;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::frame::CursorChoice;
use cupertino_server::Cupertino;
use smithay::input::pointer::CursorIcon;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    now: f64,
    /// The roundtrips of the test's clients, for [`Harness::settle`].
    clients: Vec<common::Rounds>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("create test compositor");
        match headless::init(&mut state, (1000, 700), 1.0) {
            Ok(()) => Some(Harness {
                state,
                event_loop,
                now: 100.0,
                clients: Vec::new(),
            }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
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

    /// Step at the model time `now` until each client has every event sent before and the server handled their
    /// answers ([`Client::barrier`]).
    fn settle(&mut self) {
        let barriers: Vec<_> = self.clients.iter().map(common::Rounds::barrier).collect();
        self.wait_until("the clients' roundtrips", move |_| {
            barriers.iter().all(|b| b())
        });
    }

    /// `client`, which [`Harness::settle`] waits for from now on.
    fn track(&mut self, client: Client) -> Client {
        self.clients.push(client.rounds());
        client
    }

    /// Move the model clock on by `seconds`, a few steps so that every tick sees it, the person using the desktop.
    fn advance(&mut self, seconds: f64) {
        self.now += seconds;
        self.state.fixed_time = Some(self.now);
        self.state.input_came();
        self.settle();
    }

    /// Input comes after `seconds`, and the loop pass that handles it reads the clock a hair later, as on the real
    /// clock.
    fn touch(&mut self, seconds: f64) {
        self.now += seconds;
        self.state.fixed_time = Some(self.now);
        self.state.input_came();
        self.now += 1e-6;
        self.settle();
    }

    /// Move the model clock on by `seconds` with nobody using the desktop.
    fn rest(&mut self, seconds: f64) {
        self.now += seconds;
        self.settle();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }
}

fn mapped_window(h: &mut Harness) -> (Client, cupertino_wm::WindowId) {
    let client = h.track(Client::toplevel(h.socket_path()));
    h.wait_until("the window to map", |s| s.windows.iter().any(|e| e.mapped));
    let id = h.state.windows.iter().find(|e| e.mapped).unwrap().id;
    h.settle();
    (client, id)
}

#[test]
fn advancing_model_time_waits_for_a_delayed_responsive_client() {
    let mut h = Harness::new().expect("EGL is required for the clock barrier proof");
    let (client, id) = mapped_window(&mut h);
    client.mute(true);
    h.advance(1.1);
    assert!(
        h.state.hung.awaiting_pong(),
        "client must hold an outstanding ping"
    );

    let started = Instant::now();
    let target = h.now + 5.0;
    common::set_time_with(&mut h.state, target, |state| {
        if started.elapsed() >= Duration::from_millis(300) {
            client.mute(false);
        }
        headless::step(state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    });
    h.state.tick();
    assert_eq!(h.state.now(), target);
    assert!(
        !h.state.window_is_unresponsive(id),
        "a delayed responsive client must remain responsive across a model clock jump"
    );
}

#[test]
fn a_client_that_stops_answering_is_unresponsive_two_seconds_after_the_ping_and_recovers() {
    let Some(mut h) = Harness::new() else { return };
    let (client, id) = mapped_window(&mut h);
    // pinged and answered, second after second
    for _ in 0..3 {
        h.advance(1.1);
        assert!(!h.state.window_is_unresponsive(id));
    }

    client.mute(true);
    h.advance(1.1); // the ping that is not answered goes out
    assert!(
        !h.state.window_is_unresponsive(id),
        "not yet: 0 s since that ping"
    );
    h.advance(1.5);
    assert!(!h.state.window_is_unresponsive(id), "1.5 s");
    h.advance(1.0);
    assert!(h.state.window_is_unresponsive(id), "2.5 s without a pong");

    client.mute(false);
    h.settle();
    assert!(!h.state.window_is_unresponsive(id), "it answers at last");
}

#[test]
fn a_desktop_nobody_uses_pings_no_one_and_the_first_input_pings_at_once() {
    let Some(mut h) = Harness::new() else { return };
    let (client, id) = mapped_window(&mut h);
    h.advance(1.1);
    // the ping that follows the last input, answered
    h.rest(1.1);
    client.mute(true);
    // nobody touches the desktop: no ping goes out, so none waits for an answer, and the clock has nothing to wake the
    // server for
    for _ in 0..4 {
        h.rest(1.1);
        assert!(!h.state.hung.awaiting_pong());
        assert!(!h.state.window_is_unresponsive(id));
    }
    assert_eq!(h.state.hung.next_deadline(), None);
    // the person comes back: a ping goes out at once, and the silent client is unresponsive two seconds later
    h.advance(0.1);
    assert!(h.state.hung.awaiting_pong());
    h.rest(2.1);
    assert!(h.state.window_is_unresponsive(id));
    client.mute(false);
    h.settle();
    assert!(!h.state.window_is_unresponsive(id));
}

#[test]
fn a_client_that_hangs_on_the_first_input_after_a_rest_is_unresponsive_two_seconds_after_the_next_ping(
) {
    let Some(mut h) = Harness::new() else { return };
    let (client, id) = mapped_window(&mut h);
    h.advance(1.1);
    h.rest(1.1);
    h.rest(5.0);
    assert!(!h.state.hung.awaiting_pong(), "the desktop rests");
    // one input: the ping that goes out a hair after it is answered, and then the client blocks handling the input
    h.touch(0.1);
    assert!(!h.state.hung.awaiting_pong(), "answered");
    client.mute(true);
    assert!(
        h.state.hung.next_deadline().is_some(),
        "one more ping is due"
    );
    h.rest(1.1);
    assert!(
        h.state.hung.awaiting_pong(),
        "the ping a second after the input"
    );
    h.rest(2.1);
    assert!(h.state.window_is_unresponsive(id));
    client.mute(false);
    h.settle();
    assert!(!h.state.window_is_unresponsive(id));
    // answered, the extra ping is the last until the next input
    h.rest(1.1);
    h.rest(5.0);
    assert!(!h.state.hung.awaiting_pong());
    assert_eq!(h.state.hung.next_deadline(), None);
}

#[test]
fn the_busy_cursor_shows_over_the_window_of_a_client_that_does_not_answer() {
    let Some(mut h) = Harness::new() else { return };
    let (client, id) = mapped_window(&mut h);
    h.state.tick();
    let frame = h.state.wm.window(id).unwrap().geometry();
    h.state
        .pointer_moved_to((frame.x + 120.0, frame.y + 150.0).into(), 0, None);
    h.settle();
    assert!(!h.state.pointer_over_unresponsive());
    assert!(!matches!(
        h.state.cursor_choice(),
        CursorChoice::Icon(CursorIcon::Wait)
    ));

    client.mute(true);
    h.advance(1.1);
    h.advance(2.2);
    assert!(h.state.window_is_unresponsive(id));
    assert!(h.state.pointer_over_unresponsive());
    assert!(matches!(
        h.state.cursor_choice(),
        CursorChoice::Icon(CursorIcon::Wait)
    ));

    // away from the window the cursor is the ordinary one
    h.state.pointer_moved_to((2.0, 2.0).into(), 0, None);
    h.settle();
    assert!(!h.state.pointer_over_unresponsive());
}
