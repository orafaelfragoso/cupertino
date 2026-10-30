//! The lock screen as assistive technology sees it, on a private bus with a fake accessibility registry: the password
//! field is there and says nothing of what is typed, while a password is typed the process sends no keyboard event to
//! the registry and no message that holds the password, and every answer to an attempt is announced once. The lock
//! screen runs in a copy of this test binary, started with the bus addresses AccessKit reads from its environment.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_login::access::{lock_tree, LockView, DENIED};
use cupertino_login::layout::Layout;
use cupertino_login::model::{AuthOutcome, Key, LockScreen};
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::AccessibilityAdapter;
use cupertino_ui_platform::WindowId;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Message;

/// Set in the copy of the test binary that plays the lock screen: the file it writes once it has played its part.
const LOCK_SCREEN: &str = "CUPERTINO_TEST_LOCK_SCREEN";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const SECRET: &str = "hunter2";
/// AT-SPI's role number of password text (`AtspiRole`).
const PASSWORD_TEXT: u32 = 40;
/// AT-SPI's politeness number of an assertive announcement (`AtspiLive`).
const ASSERTIVE: i32 = 2;

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The lock screen: it sends its tree once accessibility is active, then once its input says go plays `part` with a
/// way to send its tree, as the app does after each frame, writes `done` and waits for its input to close.
fn lock_screen(done: PathBuf, part: impl FnOnce(&mut LockScreen, &mut dyn FnMut(&LockScreen))) {
    let mut adapter = AtspiAdapter::new(Waker::from(Arc::new(Unpark(std::thread::current()))));
    let window = WindowId(1);
    adapter.window_focus(window, true);
    let layout = Layout::new(1280.0, 800.0, 0.25);
    let mut screen = LockScreen::new(0.0, 30.0);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !adapter.is_active() {
        assert!(Instant::now() < deadline, "accessibility never turned on");
        std::thread::park_timeout(Duration::from_millis(20));
    }
    let mut send = |screen: &LockScreen| {
        adapter.update(&lock_tree(
            window,
            &LockView {
                screen,
                layout: &layout,
                size: (1280.0, 800.0),
                user_name: "Ada",
                time: "9:41",
                date: None,
                caps_lock: false,
                help: None,
                power: false,
            },
        ));
    };
    send(&screen);
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    part(&mut screen, &mut send);
    std::fs::write(done, b"done").unwrap();
    std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
}

struct Status;

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        true
    }
}

struct Socket(Sender<String>);

#[zbus::interface(name = "org.a11y.atspi.Socket")]
impl Socket {
    fn embed(
        &self,
        plug: (String, OwnedObjectPath),
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> (String, OwnedObjectPath) {
        let _ = self.0.send(plug.0);
        let registry = conn
            .unique_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        (registry, OwnedObjectPath::try_from(ROOT).unwrap())
    }
}

fn accessible<'a>(conn: &'a Connection, peer: &str, path: &str) -> Proxy<'a> {
    Proxy::new(
        conn,
        peer.to_owned(),
        path.to_owned(),
        "org.a11y.atspi.Accessible",
    )
    .unwrap()
}

/// A lock screen started by test `name` in a copy of this binary, embedded in a fake registry, with every message it
/// sends from then on recorded.
struct Locked {
    app: Child,
    stdin: ChildStdin,
    peer: String,
    sent: Arc<Mutex<Vec<Message>>>,
    conn: Connection,
    done: PathBuf,
    _marker: tempfile::TempDir,
    _services: (Connection, Connection),
    _bus: PrivateBus,
}

impl Locked {
    fn start(name: &str) -> Option<Locked> {
        let marker = tempfile::tempdir().unwrap();
        let done = marker.path().join("done");
        let bus = PrivateBus::start()?;
        let (embedded, embeds) = channel();
        let status = bus
            .blocking_builder()
            .name("org.a11y.Bus")
            .unwrap()
            .serve_at("/org/a11y/bus", Status)
            .unwrap()
            .build()
            .unwrap();
        let registry = bus
            .blocking_builder()
            .name("org.a11y.atspi.Registry")
            .unwrap()
            .serve_at(ROOT, Socket(embedded))
            .unwrap()
            .build()
            .unwrap();
        let mut app = Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--nocapture"])
            .env(LOCK_SCREEN, &done)
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("AT_SPI_BUS_ADDRESS", &bus.address)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let peer = embeds
            .recv_timeout(Duration::from_secs(10))
            .expect("the root embedded");
        let monitor = bus.connect();
        monitor
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus.Monitoring"),
                "BecomeMonitor",
                &(vec![format!("sender='{peer}'")], 0u32),
            )
            .unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&sent);
        std::thread::spawn(move || {
            for message in MessageIterator::from(&monitor).flatten() {
                record.lock().unwrap().push(message);
            }
        });
        let stdin = app.stdin.take().unwrap();
        Some(Locked {
            app,
            stdin,
            peer,
            sent,
            conn: bus.connect(),
            done,
            _marker: marker,
            _services: (status, registry),
            _bus: bus,
        })
    }

    /// Tells the lock screen to play its part and waits until it has.
    fn play(&mut self) {
        writeln!(self.stdin).unwrap();
        self.stdin.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.done.exists() {
            assert!(
                Instant::now() < deadline,
                "the lock screen never played its part"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Everything the lock screen sent until now. The bus keeps its messages in order: once the monitor has its answer
    /// to a call made now, it has everything sent before.
    fn sent(&self) -> Vec<Message> {
        let answered = || {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter(|m| m.header().reply_serial().is_some())
                .count()
        };
        let before = answered();
        let _: zbus::Result<u32> = accessible(&self.conn, &self.peer, ROOT).call("GetRole", &());
        let deadline = Instant::now() + Duration::from_secs(10);
        while answered() == before {
            assert!(
                Instant::now() < deadline,
                "the monitor never saw the answer"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        self.sent.lock().unwrap().clone()
    }

    fn finish(mut self) {
        drop(self.stdin);
        assert!(self.app.wait().unwrap().success(), "the lock screen failed");
    }
}

#[test]
fn typing_a_password_sends_assistive_technology_neither_keys_nor_the_password() {
    const NAME: &str = "typing_a_password_sends_assistive_technology_neither_keys_nor_the_password";
    if let Some(done) = std::env::var_os(LOCK_SCREEN) {
        return lock_screen(done.into(), |screen, send| {
            for c in SECRET.chars() {
                screen.press(1.0, Key::Char(c));
                send(screen);
            }
        });
    }
    let Some(mut locked) = Locked::start(NAME) else {
        return;
    };
    let (conn, peer) = (&locked.conn, locked.peer.as_str());
    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        let windows: Vec<(String, OwnedObjectPath)> = accessible(conn, peer, ROOT)
            .call("GetChildren", &())
            .unwrap_or_default();
        if let Some(window) = windows.into_iter().next() {
            break window.1;
        }
        assert!(Instant::now() < deadline, "the lock screen never appeared");
        std::thread::sleep(Duration::from_millis(50));
    };
    let nodes: Vec<(String, OwnedObjectPath)> = accessible(conn, peer, window.as_str())
        .call("GetChildren", &())
        .unwrap();
    let field = nodes
        .into_iter()
        .map(|(_, path)| path)
        .find(|path| {
            let role: u32 = accessible(conn, peer, path.as_str())
                .call("GetRole", &())
                .unwrap();
            role == PASSWORD_TEXT
        })
        .expect("a password field");
    // its name comes back through the monitor, which so reads the lock screen's messages
    let name: String = accessible(conn, peer, field.as_str())
        .get_property("Name")
        .unwrap();
    assert_eq!(name, "Password");

    locked.play();
    let text = Proxy::new(
        &locked.conn,
        locked.peer.clone(),
        field.as_str().to_owned(),
        "org.a11y.atspi.Text",
    )
    .unwrap();
    let shown: zbus::Result<String> = text.call("GetText", &(0i32, -1i32));
    assert!(
        shown.map_or(true, |t| t.is_empty()),
        "the password reached assistive technology"
    );
    let sent = locked.sent();
    locked.finish();

    let holds = |text: &[u8]| {
        sent.iter().any(|m| {
            let body = m.data();
            body.windows(text.len()).any(|w| w == text)
        })
    };
    assert!(
        holds(b"Password"),
        "the monitor reads what the lock screen sends"
    );
    let keys: Vec<_> = sent
        .iter()
        .map(|m| {
            let header = m.header();
            (
                header
                    .interface()
                    .map(|i| i.to_string())
                    .unwrap_or_default(),
                header.member().map(|m| m.to_string()).unwrap_or_default(),
            )
        })
        .filter(|(interface, member)| {
            interface.contains("DeviceEventController") || member.contains("Key")
        })
        .collect();
    assert!(
        keys.is_empty(),
        "keyboard events went to the registry: {keys:?}"
    );
    assert!(!holds(SECRET.as_bytes()), "a message held the password");
}

#[test]
fn every_answer_to_an_attempt_is_announced_once_and_assertively() {
    const NAME: &str = "every_answer_to_an_attempt_is_announced_once_and_assertively";
    const LOCKED_OUT: &str = "The account is locked.";
    if let Some(done) = std::env::var_os(LOCK_SCREEN) {
        return lock_screen(done.into(), |screen, send| {
            let outcomes = [
                AuthOutcome::Denied,
                AuthOutcome::Denied,
                AuthOutcome::Error(LOCKED_OUT.into()),
            ];
            for (i, outcome) in outcomes.into_iter().enumerate() {
                let now = 1.0 + i as f64;
                screen.press(now, Key::Char('x'));
                screen.press(now, Key::Return);
                send(screen);
                screen.finish_auth(now + 0.5, outcome).unwrap();
                // a redraw with nothing new to say
                send(screen);
                send(screen);
            }
        });
    }
    let Some(mut locked) = Locked::start(NAME) else {
        return;
    };
    locked.play();
    let sent = locked.sent();
    locked.finish();

    type Event = (String, i32, i32, OwnedValue, HashMap<String, OwnedValue>);
    let announced: Vec<(String, i32)> = sent
        .iter()
        .filter(|m| {
            let header = m.header();
            header
                .interface()
                .is_some_and(|i| i == "org.a11y.atspi.Event.Object")
                && header.member().is_some_and(|m| m == "Announcement")
        })
        .map(|m| {
            let (_, politeness, _, said, _): Event = m.body().deserialize().unwrap();
            (String::try_from(said).unwrap(), politeness)
        })
        .collect();
    assert_eq!(
        announced,
        [
            (DENIED.to_owned(), ASSERTIVE),
            (DENIED.to_owned(), ASSERTIVE),
            (LOCKED_OUT.to_owned(), ASSERTIVE),
        ]
    );
}
