//! The login screen's screen reader toggle: Super+Alt+S from a keyboard sends `toggle <socket>` to the session manager's
//! worker over the socket it gave the compositor, and nothing else does.

use std::os::fd::OwnedFd;

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::backend::input::KeyState;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

const KEY_S: u32 = 31;
const KEY_LEFTALT: u32 = 56;
const KEY_LEFTCTRL: u32 = 29;
const KEY_SUPER: u32 = 125;

struct Harness {
    state: Cupertino,
    _event_loop: EventLoop<'static, Cupertino>,
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
        match headless::init(&mut state, (800, 600), 1.0) {
            Ok(()) => Some(Harness {
                state,
                _event_loop: event_loop,
            }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
                None
            }
        }
    }

    fn key(&mut self, evdev: u32, pressed: bool) {
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.state
            .key_from_device(Keycode::new(evdev + 8), state, 0);
    }

    fn chord(&mut self, keys: &[u32]) {
        for key in keys {
            self.key(*key, true);
        }
        for key in keys.iter().rev() {
            self.key(*key, false);
        }
    }

    /// The worker's end of a toggle socket handed to the server, with the `ready` it sent first.
    fn at_login_screen(&mut self) -> (OwnedFd, String) {
        let (worker, server) = rustix::net::socketpair(
            rustix::net::AddressFamily::UNIX,
            rustix::net::SocketType::SEQPACKET,
            rustix::net::SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        self.state.start_screen_reader(server);
        let ready = next(&worker).expect("the server says its socket");
        (worker, ready)
    }
}

/// The next request on `worker`, `None` when there is none.
fn next(worker: &OwnedFd) -> Option<String> {
    let mut frame = [0u8; 128];
    match rustix::net::recv(worker, &mut frame, rustix::net::RecvFlags::DONTWAIT) {
        Ok((count, _)) => Some(String::from_utf8_lossy(&frame[..count]).into_owned()),
        Err(rustix::io::Errno::AGAIN) => None,
        Err(e) => panic!("recv: {e}"),
    }
}

#[test]
fn super_alt_s_from_a_keyboard_asks_the_worker_to_toggle_the_reader() {
    let Some(mut h) = Harness::new() else { return };
    let (worker, ready) = h.at_login_screen();
    let socket = h.state.socket_name.clone().unwrap();
    assert_eq!(ready, format!("ready {socket}"));
    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    assert_eq!(next(&worker), Some(format!("toggle {socket}")));
    assert_eq!(next(&worker), None, "the release sends nothing");
    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    assert_eq!(next(&worker), Some(format!("toggle {socket}")));
}

#[test]
fn other_chords_send_nothing() {
    let Some(mut h) = Harness::new() else { return };
    let (worker, _) = h.at_login_screen();
    h.chord(&[KEY_SUPER, KEY_S]);
    h.chord(&[KEY_LEFTALT, KEY_S]);
    h.chord(&[KEY_SUPER, KEY_LEFTCTRL, KEY_LEFTALT, KEY_S]);
    h.chord(&[KEY_S]);
    assert_eq!(next(&worker), None);
}

#[test]
fn away_from_the_login_screen_there_is_no_socket_and_no_toggle() {
    std::env::set_var(cupertino_server::screen_reader::READER_FD_ENV, "3");
    assert!(cupertino_server::screen_reader::take(false).is_none());
    assert!(
        std::env::var_os(cupertino_server::screen_reader::READER_FD_ENV).is_none(),
        "no child inherits the variable"
    );
    std::env::set_var(cupertino_server::screen_reader::READER_FD_ENV, "4");
    assert!(
        cupertino_server::screen_reader::take(true).is_none(),
        "only descriptor 3 is taken"
    );
    let Some(mut h) = Harness::new() else { return };
    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    assert!(h.state.screen_reader.is_none());
}
