//! An application's live region while the session is locked, on a private bus with a fake accessibility registry: an
//! application that hasn't learnt yet whether the session is locked announces nothing, nor does one whose session is
//! locked, and neither unlocking nor locking announces anything by itself. The application runs in a copy of this test
//! binary, started with the bus addresses AccessKit reads from its environment. It announces a marker once it is
//! unmuted and again at its end; AccessKit sends its events in order, so what lies before and between the markers is
//! exactly what each stage sent.

use std::collections::HashMap;
use std::io::Read;
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
use zbus::blocking::MessageIterator;
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
        item_index: None,
        column_index: None,
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

#[test]
fn a_live_region_says_nothing_until_the_session_is_known_unlocked_nor_while_it_is_locked() {
    const NAME: &str =
        "a_live_region_says_nothing_until_the_session_is_known_unlocked_nor_while_it_is_locked";
    if std::env::var_os(APPLICATION).is_some() {
        return application();
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
        .args([NAME, "--exact", "--nocapture"])
        .env(APPLICATION, "1")
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
    assert_eq!(
        *said.lock().unwrap(),
        [READY, "Finished after the lock", PLAYED]
    );
}
