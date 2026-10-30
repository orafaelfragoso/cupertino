//! The lock screen as assistive technology sees it, on a private bus with a fake accessibility registry: the password
//! field is there and says nothing of what is typed, and while a password is typed the process sends no keyboard event
//! to the registry and no message that holds the password. The lock screen runs in a copy of this test binary, started
//! with the bus addresses AccessKit reads from its environment.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_login::access::{lock_tree, LockView};
use cupertino_login::layout::Layout;
use cupertino_login::model::{Key, LockScreen};
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::AccessibilityAdapter;
use cupertino_ui_platform::WindowId;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::OwnedObjectPath;

/// Set in the copy of the test binary that plays the lock screen: the file it writes once the password is typed.
const LOCK_SCREEN: &str = "CUPERTINO_TEST_LOCK_SCREEN";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const SECRET: &str = "hunter2";
/// AT-SPI's role number of password text (`AtspiRole`).
const PASSWORD_TEXT: u32 = 40;

/// A message the lock screen sent: its interface, member, the serial it answers, and its body.
type Sent = (String, String, Option<u32>, Vec<u8>);

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The lock screen: it sends its tree once accessibility is active, types the password a key at a time with its tree
/// sent after each key, as the app does after each frame, once its input says go, writes `typed` and waits for its input
/// to close.
fn lock_screen(typed: std::path::PathBuf) {
    let mut adapter = AtspiAdapter::new(Waker::from(Arc::new(Unpark(std::thread::current()))));
    let window = WindowId(1);
    adapter.window_focus(window, true);
    let layout = Layout::new(1280.0, 800.0, 0.25);
    let mut screen = LockScreen::new(0.0, 30.0);
    let send = |adapter: &mut AtspiAdapter, screen: &LockScreen| {
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
    let deadline = Instant::now() + Duration::from_secs(10);
    while !adapter.is_active() {
        assert!(Instant::now() < deadline, "accessibility never turned on");
        std::thread::park_timeout(Duration::from_millis(20));
    }
    send(&mut adapter, &screen);
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    for c in SECRET.chars() {
        screen.press(1.0, Key::Char(c));
        send(&mut adapter, &screen);
    }
    std::fs::write(typed, b"typed").unwrap();
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

#[test]
fn typing_a_password_sends_assistive_technology_neither_keys_nor_the_password() {
    const NAME: &str = "typing_a_password_sends_assistive_technology_neither_keys_nor_the_password";
    if let Some(typed) = std::env::var_os(LOCK_SCREEN) {
        return lock_screen(typed.into());
    }
    let marker = tempfile::tempdir().unwrap();
    let typed_file = marker.path().join("typed");
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
        .env(LOCK_SCREEN, &typed_file)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let peer = embeds
        .recv_timeout(Duration::from_secs(10))
        .expect("the root embedded");

    // everything the lock screen sends from here on
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
            let header = message.header();
            let interface = header
                .interface()
                .map(|i| i.to_string())
                .unwrap_or_default();
            let member = header.member().map(|m| m.to_string()).unwrap_or_default();
            let reply = header.reply_serial().map(|s| s.get());
            let body = message.data().to_vec();
            record
                .lock()
                .unwrap()
                .push((interface, member, reply, body));
        }
    });

    let conn = bus.connect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        let windows: Vec<(String, OwnedObjectPath)> = accessible(&conn, &peer, ROOT)
            .call("GetChildren", &())
            .unwrap_or_default();
        if let Some(window) = windows.into_iter().next() {
            break window.1;
        }
        assert!(Instant::now() < deadline, "the lock screen never appeared");
        std::thread::sleep(Duration::from_millis(50));
    };
    let password = |conn: &Connection| -> OwnedObjectPath {
        let nodes: Vec<(String, OwnedObjectPath)> = accessible(conn, &peer, window.as_str())
            .call("GetChildren", &())
            .unwrap();
        nodes
            .into_iter()
            .map(|(_, path)| path)
            .find(|path| {
                let role: u32 = accessible(conn, &peer, path.as_str())
                    .call("GetRole", &())
                    .unwrap();
                role == PASSWORD_TEXT
            })
            .expect("a password field")
    };
    let field = password(&conn);

    let mut stdin = app.stdin.take().unwrap();
    writeln!(stdin).unwrap();
    stdin.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !typed_file.exists() {
        assert!(Instant::now() < deadline, "the lock screen never typed");
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = Proxy::new(
        &conn,
        peer.clone(),
        field.as_str().to_owned(),
        "org.a11y.atspi.Text",
    )
    .unwrap();
    let shown: zbus::Result<String> = text.call("GetText", &(0i32, -1i32));
    assert!(
        shown.map_or(true, |t| t.is_empty()),
        "the password reached assistive technology"
    );
    // the bus keeps the lock screen's messages in order: once the monitor has its answer to this call, it has
    // everything the lock screen sent before
    let answered = |sent: &Mutex<Vec<Sent>>| {
        sent.lock()
            .unwrap()
            .iter()
            .filter(|m| m.2.is_some())
            .count()
    };
    let before = answered(&sent);
    let _: u32 = accessible(&conn, &peer, field.as_str())
        .call("GetRole", &())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while answered(&sent) == before {
        assert!(
            Instant::now() < deadline,
            "the monitor never saw the answer"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(stdin);
    assert!(app.wait().unwrap().success(), "the lock screen failed");

    let sent = sent.lock().unwrap();
    let holds = |text: &[u8]| {
        sent.iter()
            .any(|m| m.3.windows(text.len()).any(|w| w == text))
    };
    assert!(
        holds(b"Password"),
        "the monitor reads what the lock screen sends"
    );
    let keys: Vec<_> = sent
        .iter()
        .filter(|(interface, member, _, _)| {
            interface.contains("DeviceEventController") || member.contains("Key")
        })
        .collect();
    assert!(
        keys.is_empty(),
        "keyboard events went to the registry: {keys:?}"
    );
    assert!(!holds(SECRET.as_bytes()), "a message held the password");
}
