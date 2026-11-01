//! The lock screen as assistive technology sees it, on a private bus with a fake accessibility registry: the password
//! field is there and says nothing of what is typed, typing a password sends assistive technology nothing at all, and
//! every answer to an attempt is announced once. The lock screen runs in a copy of this test binary, started with the
//! bus addresses AccessKit reads from its environment. It announces a marker before and after its part, and AccessKit
//! sends its events in order, so what lies between the two markers is exactly what the part sent.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_geometry::Rect;
use cupertino_login::access::{lock_tree, LockView, DENIED};
use cupertino_login::layout::Layout;
use cupertino_login::model::{AuthOutcome, Key, LockScreen};
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, AccessibleStates, Live, NodeId,
    Role,
};
use cupertino_ui_platform::WindowId;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Message;

/// Set in the copy of the test binary that plays the lock screen.
const LOCK_SCREEN: &str = "CUPERTINO_TEST_LOCK_SCREEN";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const SECRET: &str = "hunter2";
/// AT-SPI's role number of password text (`AtspiRole`).
const PASSWORD_TEXT: u32 = 40;
/// AT-SPI's politeness number of an assertive announcement (`AtspiLive`).
const ASSERTIVE: i32 = 2;
/// What the lock screen announces just before and just after its part.
const READY: &str = "Test part starts";
const PLAYED: &str = "Test part ends";
/// How long one step may take on a heavily loaded machine; on an idle one each takes milliseconds. Below nextest's
/// 120 s budget for the whole test, so a hang fails with its own message.
const PATIENCE: Duration = Duration::from_secs(60);

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// `update` with a live node for each of `markers` added under its root; ids from 100 stay clear of the lock screen's.
fn with_markers(mut update: AccessibilityUpdate, markers: &[&str]) -> AccessibilityUpdate {
    for (i, marker) in markers.iter().enumerate() {
        let id = NodeId::new(100 + i as u32, NonZeroU32::MIN);
        update.nodes.push((
            id,
            AccessibilityNode {
                role: Role::StaticText,
                name: Some((*marker).to_owned()),
                description: None,
                value: None,
                states: AccessibleStates::default(),
                bounds: Rect::new(0.0, 0.0, 1.0, 1.0),
                actions: Vec::new(),
                relations: Vec::new(),
                children: Vec::new(),
                live: Some(Live::Assertive),
                item_count: None,
                level: None,
                item_index: None,
                column_index: None,
            },
        ));
        let root = update.root;
        let (_, root) = update.nodes.iter_mut().find(|(id, _)| *id == root).unwrap();
        root.children.push(id);
    }
    update
}

/// The lock screen: it sends its tree once accessibility is active, and once its input says go announces [`READY`],
/// plays `part` with a way to send its tree, as the app does after each frame, announces [`PLAYED`] and waits for its
/// input to close.
fn lock_screen(part: impl FnOnce(&mut LockScreen, &mut dyn FnMut(&LockScreen))) {
    let adapter = RefCell::new(AtspiAdapter::new(Waker::from(Arc::new(Unpark(
        std::thread::current(),
    )))));
    let window = WindowId(1);
    adapter.borrow_mut().window_focus(window, true);
    let layout = Layout::new(1280.0, 800.0, 0.25);
    let mut screen = LockScreen::new(0.0, 30.0);
    let markers = RefCell::new(Vec::new());
    let send = |screen: &LockScreen| {
        let tree = lock_tree(
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
        );
        adapter
            .borrow_mut()
            .update(&with_markers(tree, &markers.borrow()));
    };
    let deadline = Instant::now() + PATIENCE;
    while !adapter.borrow().is_active() {
        assert!(Instant::now() < deadline, "accessibility never turned on");
        std::thread::park_timeout(Duration::from_millis(20));
    }
    send(&screen);
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    markers.borrow_mut().push(READY);
    send(&screen);
    part(&mut screen, &mut |screen| send(screen));
    markers.borrow_mut().push(PLAYED);
    send(&screen);
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

/// The text and politeness of an announcement.
fn announcement(message: &Message) -> Option<(String, i32)> {
    type Event = (String, i32, i32, OwnedValue, HashMap<String, OwnedValue>);
    let header = message.header();
    if header.interface()? != "org.a11y.atspi.Event.Object" || header.member()? != "Announcement" {
        return None;
    }
    let (_, politeness, _, said, _): Event = message.body().deserialize().unwrap();
    Some((String::try_from(said).unwrap(), politeness))
}

/// What the lock screen sent while it played its part: the messages between its two markers, and the object paths of
/// the markers.
struct Part {
    sent: Vec<Message>,
    markers: [String; 2],
}

impl Part {
    /// The signals of the part that are not about a marker.
    fn signals(&self) -> Vec<&Message> {
        self.sent
            .iter()
            .filter(|m| m.message_type() == Type::Signal)
            .filter(|m| {
                let path = m.header().path().map(|p| p.to_string()).unwrap_or_default();
                let body = m.data();
                !self.markers.iter().any(|marker| {
                    path == *marker || body.windows(marker.len()).any(|w| w == marker.as_bytes())
                })
            })
            .collect()
    }
}

/// A lock screen started by test `name` in a copy of this binary, embedded in a fake registry, with every message it
/// sends from then on recorded.
struct Locked {
    app: Child,
    stdin: ChildStdin,
    peer: String,
    sent: Arc<(Mutex<Vec<Message>>, Condvar)>,
    conn: Connection,
    _services: (Connection, Connection),
    _bus: PrivateBus,
}

impl Locked {
    fn start(name: &str) -> Option<Locked> {
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
            .env(LOCK_SCREEN, "1")
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("AT_SPI_BUS_ADDRESS", &bus.address)
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let peer = embeds.recv_timeout(PATIENCE).expect("the root embedded");
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
        let sent = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let record = Arc::clone(&sent);
        std::thread::spawn(move || {
            for message in MessageIterator::from(&monitor).flatten() {
                record.0.lock().unwrap().push(message);
                record.1.notify_all();
            }
        });
        let stdin = app.stdin.take().unwrap();
        Some(Locked {
            app,
            stdin,
            peer,
            sent,
            conn: bus.connect(),
            _services: (status, registry),
            _bus: bus,
        })
    }

    /// Tells the lock screen to play its part and waits until it has announced both markers.
    fn play(&mut self) -> Part {
        writeln!(self.stdin).unwrap();
        self.stdin.flush().unwrap();
        let marker = |sent: &[Message], text: &str| {
            sent.iter()
                .position(|m| announcement(m).is_some_and(|(said, _)| said == text))
        };
        let deadline = Instant::now() + PATIENCE;
        let (lock, recorded) = &*self.sent;
        let mut sent = lock.lock().unwrap();
        loop {
            if let (Some(ready), Some(played)) = (marker(&sent, READY), marker(&sent, PLAYED)) {
                let path = |i: usize| sent[i].header().path().unwrap().to_string();
                return Part {
                    markers: [path(ready), path(played)],
                    sent: sent[ready + 1..played].to_vec(),
                };
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the lock screen never played its part");
            sent = recorded.wait_timeout(sent, left).unwrap().0;
        }
    }

    /// Everything the lock screen sent until now.
    fn sent(&self) -> Vec<Message> {
        self.sent.0.lock().unwrap().clone()
    }

    fn finish(mut self) {
        drop(self.stdin);
        assert!(self.app.wait().unwrap().success(), "the lock screen failed");
    }
}

#[test]
fn typing_a_password_sends_assistive_technology_nothing() {
    const NAME: &str = "typing_a_password_sends_assistive_technology_nothing";
    if std::env::var_os(LOCK_SCREEN).is_some() {
        return lock_screen(|screen, send| {
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
    let deadline = Instant::now() + PATIENCE;
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

    let part = locked.play();
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

    let typed: Vec<_> = part
        .signals()
        .iter()
        .map(|m| {
            let header = m.header();
            (
                header.interface().map(|i| i.to_string()),
                header.member().map(|m| m.to_string()),
            )
        })
        .collect();
    assert!(typed.is_empty(), "typing sent signals: {typed:?}");
    let holds = |text: &[u8]| {
        sent.iter()
            .any(|m| m.data().windows(text.len()).any(|w| w == text))
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
    if std::env::var_os(LOCK_SCREEN).is_some() {
        return lock_screen(|screen, send| {
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
    let part = locked.play();
    locked.finish();

    let announced: Vec<_> = part.sent.iter().filter_map(announcement).collect();
    assert_eq!(
        announced,
        [
            (DENIED.to_owned(), ASSERTIVE),
            (DENIED.to_owned(), ASSERTIVE),
            (LOCKED_OUT.to_owned(), ASSERTIVE),
        ]
    );
}
