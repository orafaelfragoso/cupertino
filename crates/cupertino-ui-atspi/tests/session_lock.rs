//! An application's live region while the session is locked, on a private bus with a fake accessibility registry: an
//! application that hasn't learnt yet whether the session is locked announces nothing, nor does one whose session is
//! locked, and neither unlocking nor locking announces anything by itself. The application runs in a copy of this test
//! binary, started with the bus addresses AccessKit reads from its environment. It announces a marker once it is
//! unmuted and again at its end; AccessKit sends its events in order, so what lies before and between the markers is
//! exactly what each stage sent. The application that draws the lock screen is never muted by the lock it holds: the answer
//! to a password attempt is announced.
//!
//! Acting as an assistive technology, a test reads an application's window while the session is locked: before the
//! application learns the session is unlocked and while it is locked, its window is an empty frame without a name, and a
//! click on a button read before the lock never reaches it; after the unlock the whole window reads again and the click
//! does.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::num::NonZeroU32;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_geometry::Rect;
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, AccessibleStates, Live, NodeId,
    Role,
};
use cupertino_ui_platform::WindowId;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Message;

/// Set in the copy of the test binary that plays the application.
const APPLICATION: &str = "CUPERTINO_TEST_ATSPI_SESSION_LOCK";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const WINDOW: WindowId = WindowId(1);
const READY: &str = "Test part starts";
const PLAYED: &str = "Test part ends";

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::MIN)
}

fn text(name: &str, live: Option<Live>) -> AccessibilityNode {
    AccessibilityNode {
        role: Role::StaticText,
        name: Some(name.into()),
        description: None,
        value: None,
        states: AccessibleStates::default(),
        bounds: Rect::new(0.0, 0.0, 100.0, 20.0),
        actions: Vec::new(),
        relations: Vec::new(),
        children: Vec::new(),
        live,
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
        sort: None,
    }
}

/// A window whose polite status reads `status`, with a live node for each of `markers`.
fn tree(status: &str, markers: &[&str]) -> AccessibilityUpdate {
    let mut nodes = vec![(id(2), text(status, Some(Live::Polite)))];
    for (i, marker) in markers.iter().enumerate() {
        nodes.push((id(100 + i as u32), text(marker, Some(Live::Assertive))));
    }
    let window = AccessibilityNode {
        role: Role::Window,
        children: nodes.iter().map(|(id, _)| *id).collect(),
        ..text("Downloads", None)
    };
    nodes.insert(0, (id(1), window));
    AccessibilityUpdate {
        window: WINDOW,
        root: id(1),
        focus: None,
        nodes,
        removed: Vec::new(),
        full: true,
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The application: started muted, its status changes before it learns the session isn't locked; then it announces
/// [`READY`], is locked, changes its status, is unlocked, changes it again and announces [`PLAYED`].
fn application() {
    let mut adapter = AtspiAdapter::muted(Waker::from(Arc::new(Unpark(std::thread::current()))));
    adapter.window_focus(WINDOW, true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !adapter.is_active() {
        assert!(Instant::now() < deadline, "accessibility never turned on");
        std::thread::park_timeout(Duration::from_millis(20));
    }
    adapter.update(&tree("Starting", &[]));
    adapter.update(&tree("Started before the lock state", &[]));
    adapter.session_locked(false);
    adapter.update(&tree("Started before the lock state", &[READY]));
    adapter.session_locked(true);
    adapter.update(&tree("Finished behind the lock", &[READY]));
    adapter.session_locked(false);
    adapter.update(&tree("Finished after the lock", &[READY]));
    adapter.update(&tree("Finished after the lock", &[READY, PLAYED]));
    std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
}

/// The lock screen: the session is locked from its start, its first answer to an attempt changes the status after
/// [`READY`], and it ends with [`PLAYED`].
fn lock_screen() {
    let mut adapter =
        AtspiAdapter::lock_screen(Waker::from(Arc::new(Unpark(std::thread::current()))));
    adapter.window_focus(WINDOW, true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !adapter.is_active() {
        assert!(Instant::now() < deadline, "accessibility never turned on");
        std::thread::park_timeout(Duration::from_millis(20));
    }
    adapter.session_locked(true);
    adapter.update(&tree("Enter the password", &[]));
    adapter.update(&tree("Enter the password", &[READY]));
    adapter.update(&tree("Incorrect password", &[READY]));
    adapter.update(&tree("Incorrect password", &[READY, PLAYED]));
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

/// The text of an announcement.
fn announcement(message: &Message) -> Option<String> {
    type Event = (String, i32, i32, OwnedValue, HashMap<String, OwnedValue>);
    let header = message.header();
    if header.interface()? != "org.a11y.atspi.Event.Object" || header.member()? != "Announcement" {
        return None;
    }
    let (_, _, _, said, _): Event = message.body().deserialize().unwrap();
    Some(String::try_from(said).unwrap())
}

/// What the application that `part` names announced, from the start to [`PLAYED`]; `None` without `dbus-daemon`.
fn announced(name: &str, part: &str) -> Option<Vec<String>> {
    let bus = PrivateBus::start()?;
    let (embedded, embeds) = channel();
    let _status = bus
        .blocking_builder()
        .name("org.a11y.Bus")
        .unwrap()
        .serve_at("/org/a11y/bus", Status)
        .unwrap()
        .build()
        .unwrap();
    let _registry = bus
        .blocking_builder()
        .name("org.a11y.atspi.Registry")
        .unwrap()
        .serve_at(ROOT, Socket(embedded))
        .unwrap()
        .build()
        .unwrap();
    let monitor = bus.connect();
    monitor
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus.Monitoring"),
            "BecomeMonitor",
            &(vec!["type='signal',member='Announcement'"], 0u32),
        )
        .unwrap();
    let said = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&said);
    std::thread::spawn(move || {
        for message in MessageIterator::from(&monitor).flatten() {
            if let Some(text) = announcement(&message) {
                record.lock().unwrap().push(text);
            }
        }
    });
    let mut app = Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(APPLICATION, part)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    embeds
        .recv_timeout(Duration::from_secs(10))
        .expect("the root embedded");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !said.lock().unwrap().iter().any(|s| s == PLAYED) {
        assert!(
            Instant::now() < deadline,
            "the application never played its part"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(app.stdin.take());
    assert!(app.wait().unwrap().success(), "the application failed");
    let said = said.lock().unwrap().clone();
    Some(said)
}

#[test]
fn a_live_region_says_nothing_until_the_session_is_known_unlocked_nor_while_it_is_locked() {
    const NAME: &str =
        "a_live_region_says_nothing_until_the_session_is_known_unlocked_nor_while_it_is_locked";
    if std::env::var_os(APPLICATION).is_some() {
        return application();
    }
    let Some(said) = announced(NAME, "application") else {
        return;
    };
    assert_eq!(said, [READY, "Finished after the lock", PLAYED]);
}

#[test]
fn the_lock_screen_announces_the_answer_to_an_attempt_while_the_session_is_locked() {
    const NAME: &str =
        "the_lock_screen_announces_the_answer_to_an_attempt_while_the_session_is_locked";
    if std::env::var_os(APPLICATION).is_some() {
        return lock_screen();
    }
    let Some(said) = announced(NAME, "lock-screen") else {
        return;
    };
    let from_ready: Vec<&str> = said
        .iter()
        .map(String::as_str)
        .skip_while(|s| *s != READY)
        .collect();
    assert_eq!(from_ready, [READY, "Incorrect password", PLAYED]);
}

/// What the reader application prints when it has done what the test asked.
const ACK: &str = "ATSPI-ACK";

/// A window titled "Downloads" with a status text and an OK button.
fn content(status: &str) -> AccessibilityUpdate {
    let mut update = tree(status, &[]);
    let ok = AccessibilityNode {
        role: Role::Button,
        actions: vec![cupertino_ui_core::Action::Click],
        ..text("OK", None)
    };
    update.nodes.push((id(3), ok));
    update.nodes[0].1.children.push(id(3));
    update.nodes[0].1.name = None;
    update
}

/// The application the test reads: started muted, it learns the lock state from the commands on its standard input
/// (`unlock`, `lock`), sends its window after each and acknowledges it on its standard output. A Click on OK while the
/// session is locked fails it; one after the unlock is acknowledged. It ends with its input.
fn reader_application() {
    let mut adapter = AtspiAdapter::muted(Waker::from(Arc::new(Unpark(std::thread::current()))));
    adapter.window_focus(WINDOW, true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !adapter.is_active() {
        assert!(Instant::now() < deadline, "accessibility never turned on");
        std::thread::park_timeout(Duration::from_millis(20));
    }
    let (commands, received) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let _ = commands.send(line.unwrap());
        }
    });
    adapter.window_title(WINDOW, "Downloads");
    adapter.update(&content("Ready"));
    println!("{ACK} start");
    let (mut locked, mut actions) = (true, Vec::new());
    loop {
        adapter.drain_actions(&mut actions);
        for request in actions.drain(..) {
            assert!(
                !locked,
                "{:?} reached the application behind the lock",
                request.action
            );
            println!("{ACK} {:?}", request.action);
        }
        match received.recv_timeout(Duration::from_millis(20)) {
            Ok(command) => {
                locked = command == "lock";
                adapter.session_locked(locked);
                adapter.update(&content(if locked {
                    "Behind the lock"
                } else {
                    "Unlocked"
                }));
                println!("{ACK} {command}");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn accessible<'a>(conn: &Connection, peer: &str, path: &str) -> Proxy<'a> {
    Proxy::new(
        conn,
        peer.to_owned(),
        path.to_owned(),
        "org.a11y.atspi.Accessible",
    )
    .unwrap()
}

fn children_of(conn: &Connection, peer: &str, path: &str) -> Vec<OwnedObjectPath> {
    accessible(conn, peer, path)
        .call::<_, _, Vec<(String, OwnedObjectPath)>>("GetChildren", &())
        .unwrap_or_default()
        .into_iter()
        .map(|(_, path)| path)
        .collect()
}

fn name_of(conn: &Connection, peer: &str, path: &str) -> String {
    accessible(conn, peer, path)
        .get_property("Name")
        .unwrap_or_default()
}

/// Wait until the window at `window` reads as `shown` (its content and title) or as an empty frame without a name.
fn reads(conn: &Connection, peer: &str, window: &str, shown: bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (children, name) = (children_of(conn, peer, window), name_of(conn, peer, window));
        let now = if shown {
            children.len() == 2 && name == "Downloads"
        } else {
            children.is_empty() && name.is_empty()
        };
        if now {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the window never read as {}: {} children, named {name:?}",
            if shown { "shown" } else { "hidden" },
            children.len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn behind_the_lock_an_application_window_reads_as_an_empty_frame_and_takes_no_action() {
    const NAME: &str =
        "behind_the_lock_an_application_window_reads_as_an_empty_frame_and_takes_no_action";
    if std::env::var_os(APPLICATION).is_some() {
        return reader_application();
    }
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let (embedded, embeds) = channel();
    let _status = bus
        .blocking_builder()
        .name("org.a11y.Bus")
        .unwrap()
        .serve_at("/org/a11y/bus", Status)
        .unwrap()
        .build()
        .unwrap();
    let _registry = bus
        .blocking_builder()
        .name("org.a11y.atspi.Registry")
        .unwrap()
        .serve_at(ROOT, Socket(embedded))
        .unwrap()
        .build()
        .unwrap();
    let mut app = Command::new(std::env::current_exe().unwrap())
        .args([NAME, "--exact", "--nocapture"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(APPLICATION, "reader")
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let (said, acks) = channel();
    let out = BufReader::new(app.stdout.take().unwrap());
    std::thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            if let Some(ack) = line.strip_prefix(ACK) {
                let _ = said.send(ack.trim().to_owned());
            }
        }
    });
    let ack = |what: &str| {
        let got = acks
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("the application never acknowledged {what}"));
        assert_eq!(got, what);
    };
    let mut input = app.stdin.take().unwrap();
    let mut tell = |command: &str| writeln!(input, "{command}").unwrap();
    let peer = embeds
        .recv_timeout(Duration::from_secs(10))
        .expect("the root embedded");
    ack("start");
    let conn = bus.connect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        if let Some(window) = children_of(&conn, &peer, ROOT).into_iter().next() {
            break window;
        }
        assert!(Instant::now() < deadline, "the window never appeared");
        std::thread::sleep(Duration::from_millis(20));
    };
    // started without knowing whether the session is locked: nothing of it reads
    reads(&conn, &peer, window.as_str(), false);

    tell("unlock");
    ack("unlock");
    reads(&conn, &peer, window.as_str(), true);
    let ok = children_of(&conn, &peer, window.as_str())
        .into_iter()
        .find(|path| name_of(&conn, &peer, path.as_str()) == "OK")
        .expect("the OK button reads");
    let click = |path: &OwnedObjectPath| -> zbus::Result<bool> {
        Proxy::new(
            &conn,
            peer.clone(),
            path.as_str().to_owned(),
            "org.a11y.atspi.Action",
        )
        .unwrap()
        .call("DoAction", &(0i32,))
    };

    tell("lock");
    ack("lock");
    reads(&conn, &peer, window.as_str(), false);
    // a button read before the lock takes no click behind it; the application fails if one reaches it
    let _ = click(&ok);

    tell("unlock");
    ack("unlock");
    reads(&conn, &peer, window.as_str(), true);
    let ok = children_of(&conn, &peer, window.as_str())
        .into_iter()
        .find(|path| name_of(&conn, &peer, path.as_str()) == "OK")
        .expect("the OK button reads again");
    let _ = click(&ok);
    ack("Click");
    drop(input);
    assert!(app.wait().unwrap().success(), "the application failed");
}
