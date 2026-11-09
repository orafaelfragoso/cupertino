//! The login screen's screen reader toggle: Super+Alt+S from a keyboard sends `toggle <socket>` to the session manager's
//! worker over the socket it gave the compositor, and nothing else does. The login screen starts no program, so no process
//! but its own descends from its compositor. In a person's session the compositor runs a reader itself while
//! `screen_reader` is on, and the same chord turns it on or off.

mod common;

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

/// Whether `file` appears before the hang guard.
fn appears(file: &std::path::Path) -> bool {
    let end = std::time::Instant::now() + common::HANG_GUARD;
    while std::time::Instant::now() < end {
        if file.exists() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// Whether a program was started that names `program` among its arguments or wrote `file`. The server starts a program
/// before its call returns, so one it started is running then, or has run and written the file.
fn started(program: &std::path::Path, file: &std::path::Path) -> bool {
    let program = program.as_os_str().as_encoded_bytes();
    file.exists()
        || std::fs::read_dir("/proc").unwrap().flatten().any(|entry| {
            std::fs::read(entry.path().join("cmdline")).is_ok_and(|cmdline| {
                cmdline
                    .split(|b| *b == 0)
                    .any(|arg| arg.windows(program.len()).any(|w| w == program))
            })
        })
}

#[test]
fn the_login_screen_starts_no_program() {
    let Some(mut h) = Harness::new() else { return };
    let dir = tempfile::tempdir().unwrap();
    let touch = |name: &str| format!("touch {}", dir.path().join(name).display());
    h.state.config.screenshot_command = vec![
        "env".into(),
        "CUPERTINO_ASSET_SEARCH_PATH=".into(),
        "sh".into(),
        "-c".into(),
        touch("screenshot"),
    ];
    h.state.login_screen = true;
    h.state.spawn("sh", &["-c", &touch("spawned")]);
    h.state
        .start_screenshot(cupertino_input::ScreenshotRequest::Screen);
    let spawned = dir.path().join("spawned");
    assert!(
        !started(&spawned, &spawned),
        "a program at the login screen"
    );
    assert!(
        !dir.path().join("screenshot").exists(),
        "a screenshot at the login screen"
    );
    h.state.login_screen = false;
    h.state.spawn("sh", &["-c", &touch("spawned")]);
    h.state
        .start_screenshot(cupertino_input::ScreenshotRequest::Screen);
    assert!(
        appears(&dir.path().join("spawned")),
        "elsewhere the same program starts"
    );
    assert!(
        appears(&dir.path().join("screenshot")),
        "and so does the screenshot"
    );
}

/// A fake screen reader that writes its environment to `out/env` and waits, as the same process, until it is ended.
fn fake_reader(dir: &std::path::Path, out: &std::path::Path) -> std::path::PathBuf {
    let script = dir.join("fake-reader");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n/usr/bin/env > {out}/env.part\n/usr/bin/mv {out}/env.part {out}/env\nexec /usr/bin/sleep 300\n",
            out = out.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    script
}

/// Whether process `pid` is gone (reaped) before the hang guard.
fn ends(pid: u32) -> bool {
    let end = std::time::Instant::now() + common::HANG_GUARD;
    while std::time::Instant::now() < end {
        let alive = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            !stat[stat.rfind(')').unwrap() + 1..]
                .trim_start()
                .starts_with('Z')
        });
        if !alive {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// A person's session whose reader is a fake one, its settings stored in a scratch directory.
fn in_a_session(h: &mut Harness, dir: &std::path::Path) -> std::path::PathBuf {
    let out = dir.join("out");
    std::fs::create_dir(&out).unwrap();
    h.state.session_reader.program = Some(fake_reader(dir, &out));
    h.state
        .use_accessibility_store(dir.join("accessibility.conf"))
        .unwrap();
    out
}

fn stored(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("accessibility.conf")).unwrap_or_default()
}

#[test]
fn in_a_session_the_screen_reader_key_starts_the_reader_and_off_stops_it() {
    let Some(mut h) = Harness::new() else { return };
    let dir = tempfile::tempdir().unwrap();
    let out = in_a_session(&mut h, dir.path());
    assert_eq!(h.state.session_reader.pid(), None, "off by default");

    // what Settings' switch calls (`Session1.SetAccessibility`)
    h.state
        .set_session_accessibility("screen_reader", "on")
        .unwrap();
    let pid = h.state.session_reader.pid().expect("the reader runs");
    assert!(appears(&out.join("env")), "the reader started");
    let env = std::fs::read_to_string(out.join("env")).unwrap();
    let socket = h.state.socket_name.clone().unwrap();
    assert!(
        env.lines()
            .any(|l| l == format!("WAYLAND_DISPLAY={socket}")),
        "{env}"
    );
    assert!(env.contains("XDG_SESSION_TYPE=wayland"), "{env}");
    assert!(stored(dir.path()).contains("screen_reader = on"));

    h.state
        .set_session_accessibility("screen_reader", "off")
        .unwrap();
    assert!(ends(pid), "the reader ended");
    assert_eq!(h.state.session_reader.pid(), None);
    assert!(stored(dir.path()).contains("screen_reader = off"));
}

#[test]
fn in_a_session_super_alt_s_toggles_the_reader_and_stores_the_choice() {
    let Some(mut h) = Harness::new() else { return };
    let dir = tempfile::tempdir().unwrap();
    in_a_session(&mut h, dir.path());

    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    let pid = h
        .state
        .session_reader
        .pid()
        .expect("the chord starts the reader");
    assert!(h.state.accessibility.settings.screen_reader);
    assert!(stored(dir.path()).contains("screen_reader = on"));

    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    assert!(ends(pid), "the chord stops it");
    assert!(!h.state.accessibility.settings.screen_reader);
    assert!(stored(dir.path()).contains("screen_reader = off"));

    for chord in [
        &[KEY_SUPER, KEY_S][..],
        &[KEY_LEFTALT, KEY_S],
        &[KEY_SUPER, KEY_LEFTCTRL, KEY_LEFTALT, KEY_S],
    ] {
        h.chord(chord);
        assert_eq!(h.state.session_reader.pid(), None, "{chord:?}");
    }
}

#[test]
fn a_reader_that_ended_by_itself_starts_again_with_the_key_or_the_setting() {
    let Some(mut h) = Harness::new() else { return };
    let dir = tempfile::tempdir().unwrap();
    in_a_session(&mut h, dir.path());
    h.state
        .set_session_accessibility("screen_reader", "on")
        .unwrap();
    let first = h.state.session_reader.pid().unwrap();
    // Safety: `kill` takes integers; the pid is the reader's, held unreaped until it ends.
    unsafe { libc::kill(first as i32, libc::SIGKILL) };
    assert!(ends(first));
    assert_eq!(
        h.state.session_reader.pid(),
        None,
        "it is not restarted behind the person's back"
    );
    assert!(h.state.accessibility.settings.screen_reader, "still wanted");

    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    let second = h
        .state
        .session_reader
        .pid()
        .expect("the chord turns it on, not off");
    assert_ne!(second, first);
    unsafe { libc::kill(second as i32, libc::SIGKILL) };
    assert!(ends(second));
    h.state
        .set_session_accessibility("screen_reader", "on")
        .unwrap();
    assert!(
        h.state.session_reader.pid().is_some(),
        "and so does the switch"
    );
    h.state
        .set_session_accessibility("screen_reader", "off")
        .unwrap();
}

#[test]
fn without_a_reader_or_at_the_login_screen_nothing_starts_and_the_chord_is_not_taken() {
    let Some(mut h) = Harness::new() else { return };
    let dir = tempfile::tempdir().unwrap();
    let out = in_a_session(&mut h, dir.path());
    h.state.login_screen = true;
    h.state
        .set_session_accessibility("screen_reader", "on")
        .unwrap();
    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    assert_eq!(h.state.session_reader.pid(), None);
    assert!(
        !started(&dir.path().join("fake-reader"), &out.join("env")),
        "the login screen's compositor starts no reader of its own"
    );
    assert!(
        h.state.accessibility.settings.screen_reader,
        "the chord changed nothing"
    );

    h.state.login_screen = false;
    h.state.session_reader.program = None;
    h.state.sync_screen_reader();
    h.chord(&[KEY_SUPER, KEY_LEFTALT, KEY_S]);
    assert_eq!(h.state.session_reader.pid(), None, "no reader is installed");
    assert!(
        h.state.accessibility.settings.screen_reader,
        "without a reader the chord is an ordinary key"
    );
}
