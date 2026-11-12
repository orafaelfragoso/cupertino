//! The real login process holds the session lock under a headless server, with its Kit lock screen: it locks when the
//! system is about to sleep and lets the sleep go on only once the lock is up; while the session is locked an
//! application gets no key, no pointer motion and no button; and a return from another console (the session made active
//! again) leaves the session locked, the lock surface with the keyboard and the application still without input; and a
//! login process killed while locked leaves the session locked, without input to the application, until the restarted one
//! takes the lock; the right password typed on the lock screen unlocks the session and gives the application its
//! input back, while a wrong password, or the right one for an account the stack refuses, keeps it locked. The
//! login process finds its session through a stand-in logind on a private system bus and serves its interface on a
//! private session bus; every directory it reads is a fixture's, and its PAM transactions start from a private policy
//! (`cupertino-test-pam`), never from the machine's. Tests skip (pass) when no EGL device is available.

use crate::common;
#[path = "lock-screen/control.rs"]
mod desktop_control;

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::Client;
use cupertino_server::backend::headless;
use cupertino_server::cli::start_shell_supervision;
use cupertino_server::config::{Config, ShellProcess};
use cupertino_server::events::BTN_LEFT;
use cupertino_server::Cupertino;
use cupertino_test_pam::{Code, Pam, Script};
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Display;

const OUTPUT: (i32, i32) = (800, 600);
/// How long the test waits after the wake signal before it counts the sleep delays: the login process ignores the
/// signal, so nothing it does can say it has read it.
const WAKE_QUIET: Duration = Duration::from_millis(300);
const APP_ID: &str = "org.example.underneath";
/// Evdev codes the tests type: the letters of a password and Return.
const TYPED: [u32; 4] = [35, 18, 38, 28];
/// The password [`TYPED`] submits.
const TYPED_PASSWORD: &str = "hel";

struct Server {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    time: u32,
    /// The login process's standard error, shown when a wait times out.
    log: PathBuf,
}

impl Server {
    /// A server with an offscreen output that supervises `login`; `None` without an EGL device.
    fn start(login: ShellProcess, log: PathBuf) -> Option<Server> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            Config {
                shell_processes: vec![login],
                ..Config::default()
            },
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        if let Err(e) = headless::init(&mut state, OUTPUT, 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        start_shell_supervision(&state, &event_loop).expect("the supervision timer is inserted");
        Some(Server {
            state,
            event_loop,
            time: 0,
            log,
        })
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut condition: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while !condition(&self.state) {
            assert!(
                Instant::now() < end,
                "timed out waiting for {what}; the login process said:\n{}",
                log_text(&self.log)
            );
            self.step();
        }
    }

    fn run_for(&mut self, time: Duration) {
        let end = Instant::now() + time;
        while Instant::now() < end {
            self.step();
        }
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// A mapped toplevel of [`APP_ID`] that records its input, settled and focused, and a point inside it.
    fn application(&mut self) -> (Client, (f64, f64)) {
        let client = Client::listening(self.socket_path(), APP_ID);
        self.wait_until("the application to be mapped and focused", |s| {
            s.wm.key_window()
                .is_some_and(|w| s.entry(w).is_some_and(|e| e.mapped))
        });
        let time = self.state.now() + 5.0;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.state.tick();
        let id = self.state.wm.key_window().unwrap();
        let g = self.state.wm.window(id).unwrap().geometry();
        (client, (g.x + g.w / 2.0, g.y + g.h / 2.0))
    }

    /// Wait for `client`'s [`Client::barrier`]: it has every event sent before, and the server handled its answers.
    fn sync(&mut self, client: &Client) {
        let done = client.barrier();
        self.wait_until("the application's roundtrips", move |_| done());
    }

    /// Keys typed, the pointer moved over `point` and back, and a click there; returns once `application` has heard all
    /// that reached it.
    fn input(&mut self, application: &Client, point: (f64, f64)) {
        for code in TYPED {
            for state in [KeyState::Pressed, KeyState::Released] {
                self.time += 10;
                self.state
                    .key_from_device(Keycode::new(code + 8), state, self.time);
                self.step();
            }
        }
        for at in [(point.0 - 20.0, point.1), point] {
            self.time += 10;
            self.state.pointer_moved_to(at.into(), self.time, None);
            self.step();
        }
        for state in [ButtonState::Pressed, ButtonState::Released] {
            self.time += 10;
            self.state.pointer_button(BTN_LEFT, state, self.time);
            self.step();
        }
        self.sync(application);
    }
}

/// The session is locked and the lock screen covers `point` and has the keyboard.
fn lock_screen_up(state: &Cupertino, point: (f64, f64)) -> bool {
    let lock = |surface: Option<WlSurface>| surface.is_some_and(|s| state.is_lock_surface(&s));
    state.session_lock.is_active()
        && lock(state.surface_under(point.into()).map(|(s, _)| s))
        && lock(state.seat.get_keyboard().unwrap().current_focus())
}

/// What the application got: keys, motions and button presses.
fn got(client: &Client) -> (usize, usize, u32) {
    (
        client.keys().len(),
        client.motions().len(),
        client.presses(),
    )
}

/// The fixture home and XDG directories.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        for sub in [
            "home",
            "config",
            "config-dirs",
            "data",
            "data-dirs",
            "state",
            "cache",
        ] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        let runtime = root.join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        Fixture { _dir: dir, root }
    }

    fn path(&self, sub: &str) -> String {
        self.root.join(sub).display().to_string()
    }

    fn log(&self) -> PathBuf {
        self.root.join("login.log")
    }

    /// The real login process on the session bus `bus`, finding its session through the stand-in logind on `system`,
    /// with every XDG directory in the fixture, its PAM transactions under `pam` and its standard error in
    /// [`Fixture::log`].
    fn login_process(&self, bus: &str, system: &str, pam: &Pam) -> ShellProcess {
        let mut login = common::shell::process("cupertino-login", bus, &[]);
        let program = login.command.pop().unwrap();
        login.command.extend(pam.env_words());
        login.command.extend([
            format!("DBUS_SYSTEM_BUS_ADDRESS={system}"),
            format!("XDG_SESSION_ID={}", fake_logind::SESSION_ID),
            format!("HOME={}", self.path("home")),
            format!("XDG_CONFIG_HOME={}", self.path("config")),
            format!("XDG_CONFIG_DIRS={}", self.path("config-dirs")),
            format!("XDG_DATA_HOME={}", self.path("data")),
            format!("XDG_DATA_DIRS={}", self.path("data-dirs")),
            format!("XDG_STATE_HOME={}", self.path("state")),
            format!("XDG_CACHE_HOME={}", self.path("cache")),
            format!("XDG_RUNTIME_DIR={}", self.path("runtime")),
            format!("CUPERTINO_SPEC_DIR={}", common::spawn::spec_dir().display()),
            "sh".into(),
            "-c".into(),
            "exec \"$0\" 2>>\"$1\"".into(),
            program,
            self.log().display().to_string(),
        ]);
        login
    }
}

fn log_text(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

/// The buses, the stand-in logind, the fixture and the server with the login process, once that process has taken its
/// sleep inhibitor; `None` without an EGL device.
struct Session {
    server: Server,
    logind: fake_logind::Logind,
    fixture: Fixture,
    _buses: (
        cupertino_test_bus::PrivateBus,
        cupertino_test_bus::PrivateBus,
    ),
    /// The lock screen's PAM policy; dropped last, it checks the login process reached nothing of the machine's.
    pam: Pam,
}

impl Session {
    /// A session whose lock screen accepts another password than [`TYPED_PASSWORD`]: what the tests type while locked
    /// never opens it.
    fn start() -> Option<Session> {
        Session::start_with(&Script::default())
    }

    fn start_with(script: &Script) -> Option<Session> {
        let pam = Pam::new(script);
        let bus = common::shell::private_bus();
        let system = common::shell::private_bus();
        let logind = fake_logind::serve(&system.address);
        let fixture = Fixture::new();
        let login = fixture.login_process(&bus.address, &system.address, &pam);
        let mut server = Server::start(login, fixture.log())?;
        let held = Arc::clone(&logind.held);
        server.wait_until("the login process to take its sleep inhibitor", |_| {
            held.lock().unwrap().iter().any(|i| i.what == "sleep")
        });
        Some(Session {
            server,
            logind,
            fixture,
            _buses: (bus, system),
            pam,
        })
    }

    /// Lock the session, wait for the lock screen over `center`, then type [`TYPED`] on it (and move and click there);
    /// returns once the login process said `outcome`.
    fn lock_and_type(&mut self, application: &Client, center: (f64, f64), outcome: &str) {
        self.server.state.lock_session_now();
        self.server.wait_until("the lock screen to be up", |state| {
            lock_screen_up(state, center)
        });
        self.heard_locked();
        self.server.input(application, center);
        let log = self.fixture.log();
        self.server
            .wait_until(&format!("the login process to say {outcome:?}"), |_| {
                log_text(&log).contains(outcome)
            });
    }

    fn log(&self) -> String {
        log_text(&self.fixture.log())
    }

    /// Runs the server until the login process says it heard the session is locked; it says so from its own process
    /// once the server's answer arrives, which can be after the lock screen is up.
    fn heard_locked(&mut self) {
        let log = self.fixture.log();
        self.server
            .wait_until("the login process to hear the session is locked", |_| {
                log_text(&log).contains("cupertino-login: locked")
            });
    }
}

#[test]
fn the_login_process_locks_before_the_system_sleeps_and_only_then_lets_it() {
    let Some(mut session) = Session::start() else {
        return;
    };
    let (_application, center) = session.server.application();
    assert!(!session.server.state.session_lock.is_active());
    session.logind.prepare_for_sleep(true);

    let held = Arc::clone(&session.logind.held);
    let mut locked_at_release = None;
    session
        .server
        .wait_until("the sleep inhibitor to be released", |state| {
            let released = held
                .lock()
                .unwrap()
                .iter_mut()
                .any(|i| i.what == "sleep" && i.released());
            if released {
                locked_at_release = Some(state.session_lock.is_active());
            }
            released
        });
    assert_eq!(
        locked_at_release,
        Some(true),
        "the sleep went on before the session was locked:\n{}",
        session.log()
    );
    session
        .server
        .wait_until("the lock screen to cover the application", |state| {
            lock_screen_up(state, center)
        });
    session.heard_locked();

    // the system woke up: still locked, and the delay is not taken again before the person unlocks
    session.logind.prepare_for_sleep(false);
    session.server.run_for(WAKE_QUIET);
    assert!(lock_screen_up(&session.server.state, center));
    let delays = held
        .lock()
        .unwrap()
        .iter()
        .filter(|i| i.what == "sleep")
        .count();
    assert_eq!(delays, 1, "a sleep delay was taken while locked");
}

#[test]
fn while_locked_an_application_gets_no_input_and_a_return_from_another_console_keeps_it_so() {
    let Some(mut session) = Session::start() else {
        return;
    };
    let (application, center) = session.server.application();
    // the application gets input while the session is open, so the test would see it
    session.server.input(&application, center);
    let open = got(&application);
    assert!(
        open.0 > 0 && open.1 > 0 && open.2 > 0,
        "the application got no input while the session was open: {open:?}"
    );

    session.server.state.lock_session_now();
    session
        .server
        .wait_until("the lock screen to be up", |state| {
            lock_screen_up(state, center)
        });
    session.server.sync(&application);
    let before = got(&application);
    session.server.input(&application, center);
    assert_eq!(
        got(&application),
        before,
        "the application got input while the session was locked"
    );

    // the console went to another session and came back to this one
    // the server acts on it at once: it fades in, and tells no client
    session.server.state.session_activated();
    session.server.sync(&application);
    assert!(
        lock_screen_up(&session.server.state, center),
        "the return from another console opened the session:\n{}",
        session.log()
    );
    session.server.input(&application, center);
    assert_eq!(
        got(&application),
        before,
        "the application got input after the return from another console"
    );
    session.heard_locked();
}

#[test]
fn a_login_process_killed_while_locked_leaves_the_session_locked_until_its_successor_takes_the_lock(
) {
    common::events::memory();
    let Some(mut session) = Session::start() else {
        return;
    };
    let (application, center) = session.server.application();
    session.server.state.lock_session_now();
    session
        .server
        .wait_until("the lock screen to be up", |state| {
            lock_screen_up(state, center)
        });
    session.server.sync(&application);
    let before = got(&application);

    let pid = session
        .server
        .state
        .supervisor
        .pid_of("cupertino-login")
        .expect("the login process runs");
    // SAFETY: kill only sends a signal to the process the server started
    assert_eq!(
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) },
        0,
        "killing the login process"
    );
    session
        .server
        .wait_until("the killed lock screen's surface to go", |state| {
            !state
                .surface_under(center.into())
                .is_some_and(|(s, _)| state.is_lock_surface(&s))
        });
    assert!(
        session.server.state.session_lock.is_active(),
        "the crash of the lock screen opened the session"
    );
    session.server.input(&application, center);
    assert_eq!(
        got(&application),
        before,
        "the application got input after the lock screen crashed"
    );

    session
        .server
        .wait_until("the restarted login process to take the lock", |state| {
            state
                .supervisor
                .pid_of("cupertino-login")
                .is_some_and(|p| p != pid)
                && lock_screen_up(state, center)
        });
    session.server.input(&application, center);
    assert_eq!(
        got(&application),
        before,
        "the application got input under the successor's lock screen"
    );

    // the diagnostic events of the crash: the lock client lost while locked, the server's own cover, and the supervised
    // process's exit by the signal, restarted
    let lost = common::events::recorded("server.lock.client_lost");
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(
        lost[0].severity,
        cupertino_diagnostic_events::Severity::Error
    );
    assert_eq!(lost[0].fields.get("stage").unwrap(), "locked");
    assert_eq!(lost[0].fields.get("stays_locked").unwrap(), true);
    let covered = common::events::recorded("server.lock.fallback_shown");
    assert_eq!(covered.len(), 1, "{covered:?}");
    assert_eq!(covered[0].fields.get("reason").unwrap(), "lock_client_lost");
    let exited: Vec<_> = common::events::recorded("server.shell.exited")
        .into_iter()
        .filter(|e| e.fields.get("role").unwrap() == "cupertino-login")
        .collect();
    assert_eq!(exited.len(), 1, "{exited:?}");
    assert_eq!(exited[0].fields.get("signal").unwrap(), libc::SIGKILL);
    assert!(exited[0].fields.get("restart_in").is_some());
}

#[test]
fn the_right_password_unlocks_the_session_and_the_application_gets_its_input_back() {
    let Some(mut session) = Session::start_with(&Script {
        password: Some(TYPED_PASSWORD.into()),
        ..Script::default()
    }) else {
        return;
    };
    let (application, center) = session.server.application();
    session.lock_and_type(&application, center, "cupertino-login: unlocked");
    session
        .server
        .wait_until("the session to be unlocked", |state| {
            !state.session_lock.is_active()
        });
    assert_eq!(
        session.pam.steps(),
        ["authenticate -> success", "account -> success"]
    );
    assert!(
        session.pam.calls()[0].contains(" service=cupertino-login ")
            && session.pam.calls()[0].contains(" answer=right "),
        "{:?}",
        session.pam.calls()
    );
    // the application has the keyboard and the pointer again
    session.server.sync(&application);
    let before = got(&application);
    session.server.input(&application, center);
    let after = got(&application);
    assert!(
        after.0 > before.0 && after.1 > before.1 && after.2 > before.2,
        "the application got no input after the unlock: {before:?} then {after:?}"
    );
}

#[test]
fn a_wrong_password_keeps_the_session_locked() {
    let Some(mut session) = Session::start_with(&Script {
        password: Some("not what is typed".into()),
        ..Script::default()
    }) else {
        return;
    };
    let (application, center) = session.server.application();
    session.lock_and_type(
        &application,
        center,
        "cupertino-login: authentication failed",
    );
    session.server.run_for(WAKE_QUIET);
    assert!(
        lock_screen_up(&session.server.state, center),
        "a wrong password opened the session:\n{}",
        session.log()
    );
    assert_eq!(session.pam.steps(), ["authenticate -> auth_err"]);
    assert!(!session.log().contains("cupertino-login: unlocked"));
}

#[test]
fn the_right_password_of_an_account_the_stack_refuses_keeps_the_session_locked() {
    let Some(mut session) = Session::start_with(&Script {
        password: Some(TYPED_PASSWORD.into()),
        account: Some(Code::AcctExpired),
        ..Script::default()
    }) else {
        return;
    };
    let (application, center) = session.server.application();
    session.lock_and_type(
        &application,
        center,
        "cupertino-login: authentication error: account not available",
    );
    session.server.run_for(WAKE_QUIET);
    assert!(
        lock_screen_up(&session.server.state, center),
        "an expired account opened the session:\n{}",
        session.log()
    );
    assert_eq!(
        session.pam.steps(),
        ["authenticate -> success", "account -> acct_expired"]
    );
}

/// A stand-in for logind for the login process: session [`SESSION_ID`] is active on `seat0`, every power action is
/// allowed, and each inhibitor is one end of a socket pair whose other end the stand-in keeps, so the test sees when the
/// login process lets go of it. The test sends the manager's `PrepareForSleep`.
mod fake_logind {
    use super::*;
    use std::os::fd::OwnedFd;
    use zbus::zvariant::{self, OwnedObjectPath};

    pub const SESSION_ID: &str = "1";
    const PATH: &str = "/org/freedesktop/login1";
    const SESSION_PATH: &str = "/org/freedesktop/login1/session/_31";

    /// An inhibitor the login process took.
    pub struct Inhibitor {
        pub what: String,
        kept: UnixStream,
    }

    impl Inhibitor {
        /// The login process closed its end.
        pub fn released(&mut self) -> bool {
            self.kept.set_nonblocking(true).unwrap();
            let mut byte = [0];
            matches!(self.kept.read(&mut byte), Ok(0))
        }
    }

    struct Manager {
        held: Arc<Mutex<Vec<Inhibitor>>>,
    }

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Manager {
        fn get_session(&self, id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
            if id != SESSION_ID {
                return Err(zbus::fdo::Error::Failed(format!("no session {id}")));
            }
            Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
        }

        fn inhibit(
            &self,
            what: &str,
            _who: &str,
            _why: &str,
            _mode: &str,
        ) -> zbus::fdo::Result<zvariant::OwnedFd> {
            let (kept, given) =
                UnixStream::pair().map_err(|e| zbus::fdo::Error::IOError(e.to_string()))?;
            self.held.lock().unwrap().push(Inhibitor {
                what: what.to_owned(),
                kept,
            });
            Ok(OwnedFd::from(given).into())
        }

        fn can_suspend(&self) -> String {
            "yes".to_owned()
        }

        fn can_reboot(&self) -> String {
            "yes".to_owned()
        }

        fn can_power_off(&self) -> String {
            "yes".to_owned()
        }
    }

    struct Session;

    #[zbus::interface(name = "org.freedesktop.login1.Session")]
    impl Session {
        #[zbus(property)]
        fn id(&self) -> String {
            SESSION_ID.to_owned()
        }

        #[zbus(property, name = "Type")]
        fn session_type(&self) -> String {
            "wayland".to_owned()
        }

        #[zbus(property)]
        fn active(&self) -> bool {
            true
        }

        #[zbus(property, name = "VTNr")]
        fn vt_nr(&self) -> u32 {
            2
        }

        #[zbus(property)]
        fn seat(&self) -> (String, OwnedObjectPath) {
            (
                "seat0".to_owned(),
                OwnedObjectPath::try_from("/org/freedesktop/login1/seat/seat0").unwrap(),
            )
        }
    }

    /// The stand-in, served for as long as it lives.
    pub struct Logind {
        connection: zbus::blocking::Connection,
        pub held: Arc<Mutex<Vec<Inhibitor>>>,
    }

    impl Logind {
        /// The system is about to sleep (`true`) or woke up.
        pub fn prepare_for_sleep(&self, start: bool) {
            self.connection
                .emit_signal(
                    None::<&str>,
                    PATH,
                    "org.freedesktop.login1.Manager",
                    "PrepareForSleep",
                    &(start,),
                )
                .unwrap();
        }
    }

    /// Serve the stand-in on the bus at `address`.
    pub fn serve(address: &str) -> Logind {
        let held = Arc::new(Mutex::new(Vec::new()));
        let connection = zbus::blocking::connection::Builder::address(address)
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at(
                PATH,
                Manager {
                    held: Arc::clone(&held),
                },
            )
            .unwrap()
            .serve_at(SESSION_PATH, Session)
            .unwrap()
            .build()
            .unwrap();
        Logind { connection, held }
    }
}
