//! The lock screen and the login screen (`cupertino-login`) as the server starts them, run in this process on a
//! connection with the roles the server gives them, against the real server, headless.
//! - The lock screen, on a bus of the test's own that stands for both buses, with stand-ins for logind and the session
//!   manager (`org.cupertino.LoginManager1`): it announces the session to the manager, takes the session lock when
//!   logind asks and covers the output with its lock surface, unlocks when the manager says the session's password was
//!   proved at the login screen (a fast user switch), and locks again when the server asks.
//! - The login screen, with a stand-in for the session manager's socket (`cupertino-sessiond`, speaking
//!   `cupertino-login-ipc`): it greets the manager, covers the output with its screens, and the first user it lists,
//!   chosen and given a password with the keys, is the account the attempt names; the password goes to the manager's
//!   prompt and the manager's acceptance starts the session.
//!
//! The lock screen's own password check is PAM's, which this test does not stand in for: a PAM module of the test's
//! own is not allowed and the machine's stack would check a real account. Its handling of an attempt up to PAM's
//! conversation (the shake of a wrong password, the lock that stays) is tested on the state machine
//! (`crates/cupertino-login/tests/attempts.rs`).

// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

/// The rig names a host program; no test here starts a shell host.
mod host {
    pub const MARK: &str = "cupertino-login-screens-tests-host";
}

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::IntoRawFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use cupertino_login::options::SessionOptions;
use cupertino_login_ipc::message::{MAX_EVENT_FRAME, MAX_REQUEST_FRAME};
use cupertino_login_ipc::{
    decode, encode, Account, Decoder, Event, Outcome, PowerMenu, Request, UserRow, VERSION,
};
use cupertino_server::authorization::ShellRole;
use cupertino_server::shell_connection::shell_process_connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};

use rig::{Rig, OUTPUT_SIZE};

/// The id of the login session the lock screen is in, as `pam_systemd` would set it, and its object at logind's.
const SESSION_ID: &str = "7";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/_37";

/// The users the session manager's stand-in lists, and the password the test types.
const ALICE: u32 = 60_001;
const BOB: u32 = 60_002;
const PASSWORD: &str = "correct horse";

/// The conversation the manager's stand-in asks about.
const CONVERSATION: u64 = 41;

/// How many pixels the password screen changes of the picker's at least: the tiles go, the chosen user's avatar, name
/// and field come (a few thousand pixels on the test's output).
const PASSWORD_SCREEN_CHANGES: usize = 2_000;

/// How long the stand-ins wait for the program to say something.
const ASKED: Duration = Duration::from_secs(60);

/// A bus of the test's own that starts no service, standing for both the session and the system bus.
struct Bus(Child, String);

const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

impl Bus {
    fn start(name: &str) -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("login-screens-{name}-bus.conf"));
        std::fs::write(&config, BUS_CONFIG).expect("the bus's configuration");
        let mut daemon = Process::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon gives its address");
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &address);
        Bus(daemon, address)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A directory of the test's own, new and empty, of mode 0700.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("login-screens-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// A home, settings, data, state and runtime directories of the test's own: nothing of the person's is read, and the
/// login screen's copy of the wallpaper is written there.
fn private_home(name: &str) {
    // a fixed clock: the screens open, switch and fade in at their end, so what they show settles at once
    std::env::set_var("CUPERTINO_TEST_CLOCK", "1700000000");
    let home = private_dir(&format!("{name}-home"));
    for (variable, dir) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_STATE_HOME", ".local/state"),
        ("XDG_CACHE_HOME", ".cache"),
    ] {
        let dir = home.join(dir);
        std::fs::create_dir_all(&dir).expect("a folder of the home");
        std::env::set_var(variable, dir);
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_RUNTIME_DIR", private_dir(&format!("{name}-runtime")));
}

/// Run `program` in this process on a connection holding `roles`, as the server's supervisor starts it.
fn start(
    rig: &mut Rig,
    roles: &[ShellRole],
    program: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> std::thread::JoinHandle<Result<(), String>> {
    let stream =
        shell_process_connection(&rig.state.display_handle, roles).expect("a shell connection");
    std::env::set_var("WAYLAND_SOCKET", stream.into_raw_fd().to_string());
    std::thread::spawn(program)
}

/// How many of the output's pixels differ from `before` by more than a step of rounding.
fn changed(rig: &mut Rig, before: &image::RgbaImage) -> usize {
    let now = rig.image();
    now.pixels()
        .zip(before.pixels())
        .filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > 2))
        .count()
}

/// Most of the output: what a full-screen surface covers, but for antialiased corners.
fn most_of_the_output() -> usize {
    (OUTPUT_SIZE.0 * OUTPUT_SIZE.1) as usize * 9 / 10
}

/// logind's manager, as far as a lock screen asks it.
struct Logind {
    inhibitors: Arc<AtomicU32>,
}

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Logind {
    fn get_session(&self, id: &str) -> zbus::fdo::Result<OwnedObjectPath> {
        if id == SESSION_ID {
            Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
        } else {
            Err(zbus::fdo::Error::Failed(format!("no session {id}")))
        }
    }

    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, _pid: u32) -> zbus::fdo::Result<OwnedObjectPath> {
        Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
    }

    fn inhibit(
        &self,
        what: &str,
        _who: &str,
        _why: &str,
        mode: &str,
    ) -> zbus::fdo::Result<OwnedFd> {
        assert_eq!(
            (what, mode),
            ("sleep", "delay"),
            "a delay inhibitor on sleep"
        );
        self.inhibitors.fetch_add(1, Ordering::SeqCst);
        let file = std::fs::File::open("/dev/null").expect("a descriptor to hand out");
        Ok(std::os::fd::OwnedFd::from(file).into())
    }

    #[zbus(signal)]
    async fn prepare_for_sleep(emitter: &SignalEmitter<'_>, start: bool) -> zbus::Result<()>;
}

/// The lock screen's login session at logind's.
struct LoginSession;

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl LoginSession {
    #[zbus(signal)]
    async fn lock(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// The session manager's bus interface, as far as a lock screen calls it.
struct LoginManager {
    ready: mpsc::Sender<()>,
}

#[zbus::interface(name = "org.cupertino.LoginManager1")]
impl LoginManager {
    fn session_ready(&self) {
        let _ = self.ready.send(());
    }

    #[zbus(signal)]
    async fn unlock(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// The stand-ins for logind and the session manager on `bus`, each on a connection of its own, as on the system bus.
fn stand_ins(
    bus: &Bus,
) -> (
    zbus::blocking::Connection,
    zbus::blocking::Connection,
    mpsc::Receiver<()>,
    Arc<AtomicU32>,
) {
    let inhibitors = Arc::new(AtomicU32::new(0));
    let logind = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .and_then(|b| b.name("org.freedesktop.login1"))
        .and_then(|b| {
            b.serve_at(
                "/org/freedesktop/login1",
                Logind {
                    inhibitors: Arc::clone(&inhibitors),
                },
            )
        })
        .and_then(|b| b.serve_at(SESSION_PATH, LoginSession))
        .and_then(|b| b.build())
        .expect("logind's stand-in");
    let (ready, readied) = mpsc::channel();
    let manager = zbus::blocking::connection::Builder::address(bus.1.as_str())
        .and_then(|b| b.name("org.cupertino.LoginManager1"))
        .and_then(|b| b.serve_at("/org/cupertino/LoginManager1", LoginManager { ready }))
        .and_then(|b| b.build())
        .expect("the session manager's stand-in");
    (logind, manager, readied, inhibitors)
}

#[test]
fn the_lock_screen_locks_for_logind_and_unlocks_for_the_session_manager() {
    private_home("lock");
    std::env::set_var("XDG_SESSION_ID", SESSION_ID);
    let bus = Bus::start("lock");
    let (logind, manager, readied, inhibitors) = stand_ins(&bus);
    let mut rig = Rig::new("login-screens-lock", &[], &[]);
    let unlocked = rig.image();
    let program = start(
        &mut rig,
        &[ShellRole::Session, ShellRole::SessionLock],
        || {
            cupertino_login::lock_screen::run(SessionOptions {
                idle_timeout: None,
                retries_until_hint: 0,
            })
        },
    );
    rig.until("the session is announced to the session manager", |_| {
        readied.try_recv().is_ok()
    });
    rig.until("sleep is held back while the session is unlocked", |_| {
        inhibitors.load(Ordering::SeqCst) == 1
    });
    assert!(
        !rig.state.session_lock.is_active(),
        "nothing locks by itself"
    );

    // `loginctl lock-session`
    zbus::block_on(logind.inner().emit_signal(
        None::<&str>,
        SESSION_PATH,
        "org.freedesktop.login1.Session",
        "Lock",
        &(),
    ))
    .expect("logind asks for the lock");
    rig.until("the lock screen holds the session lock", |rig| {
        rig.state.session_lock.covers()
    });
    rig.until("its lock surface covers the output", |rig| {
        changed(rig, &unlocked) >= most_of_the_output()
    });
    assert!(!program.is_finished(), "the lock screen runs on");

    // the person proved this session's password at the login screen
    zbus::block_on(manager.inner().emit_signal(
        None::<&str>,
        "/org/cupertino/LoginManager1",
        "org.cupertino.LoginManager1",
        "Unlock",
        &(),
    ))
    .expect("the session manager unlocks");
    rig.until("the session is unlocked", |rig| {
        !rig.state.session_lock.is_active()
    });
    rig.until("sleep is held back again", |_| {
        inhibitors.load(Ordering::SeqCst) == 2
    });

    // the server's own request, over the lock screen's connection
    rig.state.lock_session();
    rig.until("the lock screen locks again for the server", |rig| {
        rig.state.session_lock.covers()
    });
    assert!(!program.is_finished(), "the lock screen runs on");
}

/// Read the next request the login screen sends on `stream`.
fn request(stream: &mut UnixStream, decoder: &mut Decoder) -> Request {
    let mut buffer = [0u8; 4096];
    loop {
        let wanted = decoder.wanted().min(buffer.len());
        let count = stream
            .read(&mut buffer[..wanted])
            .expect("the login screen writes");
        assert_ne!(count, 0, "the login screen keeps the connection");
        if let Some(body) = decoder.push(&buffer[..count]).expect("a frame") {
            return decode(&body).expect("a request");
        }
    }
}

/// Send `event` to the login screen on `stream`.
fn send(stream: &mut UnixStream, event: &Event) {
    let frame = encode(event, MAX_EVENT_FRAME).expect("an event");
    stream.write_all(&frame).expect("the login screen reads");
}

fn row(uid: u32, login: &str, display: &str) -> UserRow {
    UserRow {
        uid,
        login: login.into(),
        display_name: display.into(),
        picture: None,
        logged_in: false,
        wallpaper: None,
    }
}

/// What the session manager's stand-in heard of the attempt: the account it named and the password it answered.
#[derive(Debug, PartialEq)]
struct Attempt {
    account: Account,
    password: String,
}

/// The session manager's side of the socket at `path`: greet the login screen, list Alice and Bob, ask for the
/// password of the attempt and accept it.
fn session_manager(path: &std::path::Path) -> mpsc::Receiver<Attempt> {
    let listener = UnixListener::bind(path).expect("the manager's socket");
    let (heard, attempts) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the login screen connects");
        stream.set_read_timeout(Some(ASKED)).unwrap();
        let mut decoder = Decoder::new(MAX_REQUEST_FRAME);
        assert!(
            matches!(request(&mut stream, &mut decoder), Request::Hello { version } if version == VERSION),
            "the login screen greets first"
        );
        send(&mut stream, &Event::Hello { version: VERSION });
        send(
            &mut stream,
            &Event::Welcome {
                users: vec![
                    row(ALICE, "alice", "Alice Liddell"),
                    row(BOB, "bob", "Bob Dylan"),
                ],
                show_other: true,
                power: PowerMenu {
                    restart: true,
                    shut_down: true,
                    sleep: false,
                },
                will_prompt: false,
                retries_until_hint: 0,
            },
        );
        let account = match request(&mut stream, &mut decoder) {
            Request::Authenticate { account } => account,
            other => panic!("an attempt, not {other:?}"),
        };
        send(
            &mut stream,
            &Event::Prompt {
                conversation: CONVERSATION,
                echo: false,
                text: "Password: ".into(),
            },
        );
        let password = match request(&mut stream, &mut decoder) {
            Request::Answer {
                conversation,
                secret,
            } if conversation == CONVERSATION => secret.expose().to_owned(),
            other => panic!("the answer to the prompt, not {other:?}"),
        };
        send(
            &mut stream,
            &Event::Result {
                outcome: Outcome::Accepted,
            },
        );
        let _ = heard.send(Attempt { account, password });
        // the manager keeps the connection while the session starts
        let _ = request(&mut stream, &mut decoder);
    });
    attempts
}

#[test]
fn the_login_screen_lists_the_users_and_starts_the_session_of_the_one_chosen() {
    private_home("greeter");
    // a socket's path is short (`SUN_LEN`): the system's temporary folder, as the test's buses
    let socket = std::env::temp_dir().join(format!("login-screens-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let attempts = session_manager(&socket);
    std::env::set_var(cupertino_login::manager::SOCKET_VARIABLE, &socket);
    let mut rig = Rig::new("login-screens-greeter", &[], &[]);
    let bare = rig.image();
    let program = start(
        &mut rig,
        &[ShellRole::Session],
        cupertino_login::greeter::run,
    );
    rig.until("the login screen covers the output", |rig| {
        changed(rig, &bare) >= most_of_the_output()
    });
    let picker = rig.image();
    // the first tile has the focus as the screen opens: Return chooses it, its password screen takes the keys
    rig.state.scripted_key("Return").unwrap();
    rig.until("the password screen replaces the tiles", |rig| {
        changed(rig, &picker) >= PASSWORD_SCREEN_CHANGES
    });
    rig.state.scripted_type(PASSWORD);
    rig.state.scripted_key("Return").unwrap();
    let mut attempt = None;
    rig.until("the session manager is asked to start a session", |_| {
        attempt = attempts.try_recv().ok();
        attempt.is_some()
    });
    assert_eq!(
        attempt.unwrap(),
        Attempt {
            account: Account::Listed { uid: ALICE },
            password: PASSWORD.into(),
        },
        "the first user listed, with the password typed"
    );
    assert!(
        !program.is_finished(),
        "the login screen runs on while the session starts"
    );
    let _ = std::fs::remove_file(&socket);
}
