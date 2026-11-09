//! On a private bus, a test acting as the accessibility registry and as an assistive technology: it sees the
//! application's window and nodes with their roles and names, its Click reaches the application's loop, and its attempt
//! to type into the password field never does; a row of a list that builds only some of its rows still says its place
//! among all of them. A text field tells where its caret and selection are, in characters, and a selection set from
//! the bus reaches the application in bytes and comes back as caret and selection events; a password field takes no
//! selection. With accessibility off, nothing is exported. The application runs in a
//! copy of this test binary, started with the bus addresses AccessKit reads from its environment.

use std::collections::HashMap;
use std::io::Read;
use std::num::NonZeroU32;
use std::ops::Range;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_geometry::Rect;
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, AccessibleStates,
    AccessibleValue, Action, ActionData, NodeId, Role, TextSemantics,
};
use cupertino_ui_platform::WindowId;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::OwnedObjectPath;

/// Set in the copy of the test binary that plays the application: `on` or `off`, as accessibility is.
const APPLICATION: &str = "CUPERTINO_TEST_ATSPI_APPLICATION";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const WINDOW: WindowId = WindowId(1);
/// AT-SPI's role numbers (`AtspiRole`).
const FRAME: u32 = 23;
const PASSWORD_TEXT: u32 = 40;
const PUSH_BUTTON: u32 = 43;

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::new(1).unwrap())
}

fn node(
    role: Role,
    name: &str,
    value: Option<AccessibleValue>,
    actions: Vec<Action>,
    children: Vec<NodeId>,
) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: Some(name.into()),
        description: None,
        value,
        states: AccessibleStates {
            focusable: role != Role::Window,
            ..AccessibleStates::default()
        },
        bounds: Rect::new(0.0, 0.0, 100.0, 40.0),
        actions,
        relations: Vec::new(),
        children,
        live: None,
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
        sort: None,
    }
}

/// A window named only by its title, with a text label, an OK button, a password field that, like a careless core, still carries its value, and
/// a list of 50,000 items that holds only the 40,000th, which has no name of its own but holds its text.
fn tree() -> AccessibilityUpdate {
    AccessibilityUpdate {
        window: WINDOW,
        root: id(1),
        focus: Some(id(2)),
        nodes: vec![
            (
                id(1),
                AccessibilityNode {
                    name: None,
                    ..node(
                        Role::Window,
                        "",
                        None,
                        Vec::new(),
                        vec![id(4), id(2), id(3), id(5)],
                    )
                },
            ),
            (
                id(4),
                node(
                    Role::StaticText,
                    "Version 1.0",
                    None,
                    Vec::new(),
                    Vec::new(),
                ),
            ),
            (
                id(2),
                node(Role::Button, "OK", None, vec![Action::Click], Vec::new()),
            ),
            (
                id(3),
                node(
                    Role::PasswordInput,
                    "Password",
                    Some(AccessibleValue::Text {
                        value: "hunter2".into(),
                        editing: None,
                    }),
                    vec![Action::Focus, Action::SetValue],
                    Vec::new(),
                ),
            ),
            (
                id(5),
                AccessibilityNode {
                    item_count: Some(50_000),
                    level: None,
                    ..node(Role::List, "Items", None, Vec::new(), vec![id(6)])
                },
            ),
            (
                id(6),
                AccessibilityNode {
                    item_index: Some(39_999),
                    name: None,
                    ..node(Role::ListItem, "", None, Vec::new(), vec![id(7)])
                },
            ),
            (
                id(7),
                node(
                    Role::StaticText,
                    "Item 40,000",
                    None,
                    Vec::new(),
                    Vec::new(),
                ),
            ),
        ],
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

/// The application: it sends its tree once accessibility is active and, on the first Click on OK, waits for the test to
/// close its input (AccessKit still answers the click meanwhile) and exits successfully. A value set on the password
/// field fails it, and so does staying active without accessibility.
fn application(enabled: bool) {
    let mut adapter = AtspiAdapter::new(Waker::from(Arc::new(Unpark(std::thread::current()))));
    adapter.window_focus(WINDOW, true);
    let deadline = Instant::now() + Duration::from_secs(if enabled { 10 } else { 1 });
    let (mut sent, mut actions) = (false, Vec::new());
    while Instant::now() < deadline {
        if adapter.is_active() && !sent {
            adapter.window_title(WINDOW, "About");
            adapter.update(&tree());
            sent = true;
        }
        adapter.drain_actions(&mut actions);
        for request in actions.drain(..) {
            assert_ne!(
                request.action,
                Action::SetValue,
                "a password field took a value"
            );
            if (request.target, request.action) == (id(2), Action::Click) {
                std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
                return;
            }
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    assert!(!enabled, "no Click arrived");
    assert!(!adapter.is_active(), "active without accessibility");
}

struct Status(bool);

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        self.0
    }
}

/// The registry: it records which bus name embedded its application root.
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

/// The bus, with the registry and accessibility `enabled` or not, and the application started on it.
struct Session {
    bus: PrivateBus,
    app: std::process::Child,
    /// The bus name the application embedded its root from.
    peer: String,
    _services: (Connection, Connection),
}

/// The session once the application embedded its root; `None` when it never did and exited.
fn run(name: &str, enabled: bool) -> Option<Session> {
    let bus = PrivateBus::start()?;
    let (embedded, embeds) = channel();
    let status = bus
        .blocking_builder()
        .name("org.a11y.Bus")
        .unwrap()
        .serve_at("/org/a11y/bus", Status(enabled))
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
    let app = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--nocapture"])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(APPLICATION, if enabled { "on" } else { "off" })
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let wait = if enabled {
        Duration::from_secs(10)
    } else {
        Duration::from_secs(2)
    };
    match embeds.recv_timeout(wait).ok() {
        Some(peer) => Some(Session {
            bus,
            app,
            peer,
            _services: (status, registry),
        }),
        None => {
            let mut app = app;
            assert!(app.wait().unwrap().success(), "the application failed");
            assert!(!enabled, "the application never embedded its root");
            None
        }
    }
}

fn children(
    conn: &Connection,
    peer: &str,
    path: &str,
) -> zbus::Result<Vec<(String, OwnedObjectPath)>> {
    let accessible = Proxy::new(
        conn,
        peer.to_owned(),
        path.to_owned(),
        "org.a11y.atspi.Accessible",
    )
    .unwrap();
    accessible.call("GetChildren", &())
}

fn described(conn: &Connection, peer: &str, path: &OwnedObjectPath) -> (String, u32) {
    let accessible = Proxy::new(
        conn,
        peer.to_owned(),
        path.as_str().to_owned(),
        "org.a11y.atspi.Accessible",
    )
    .unwrap();
    let name: String = accessible.get_property("Name").unwrap();
    let role: u32 = accessible.call("GetRole", &()).unwrap();
    (name, role)
}

#[test]
fn an_assistive_technology_reads_the_window_and_its_click_reaches_the_application() {
    const NAME: &str =
        "an_assistive_technology_reads_the_window_and_its_click_reaches_the_application";
    if std::env::var_os(APPLICATION).is_some() {
        return application(true);
    }
    let Some(Session {
        bus,
        mut app,
        peer,
        _services,
    }) = run(NAME, true)
    else {
        return;
    };
    let conn = bus.connect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        // the root's Accessible interface is served only once the registry answered the embed
        let windows = children(&conn, &peer, ROOT).unwrap_or_default();
        if let Some(window) = windows.into_iter().next() {
            break window;
        }
        assert!(Instant::now() < deadline, "the window never appeared");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(described(&conn, &peer, &window.1), ("About".into(), FRAME));
    let nodes: Vec<_> = children(&conn, &peer, window.1.as_str())
        .unwrap()
        .into_iter()
        .map(|(_, path)| (described(&conn, &peer, &path), path))
        .collect();
    let named = |name: &str| nodes.iter().find(|((n, _), _)| n == name).unwrap();
    assert_eq!(named("OK").0 .1, PUSH_BUTTON);
    assert!(
        nodes.iter().any(|((n, _), _)| n == "Version 1.0"),
        "a text label reaches assistive technology without its name: {nodes:?}"
    );
    assert_eq!(named("Password").0 .1, PASSWORD_TEXT);
    let items = children(&conn, &peer, named("Items").1.as_str()).unwrap();
    let item = Proxy::new(
        &conn,
        peer.clone(),
        items[0].1.as_str().to_owned(),
        "org.a11y.atspi.Accessible",
    )
    .unwrap();
    let attributes: HashMap<String, String> = item.call("GetAttributes", &()).unwrap();
    let item_name: String = item.get_property("Name").unwrap();
    assert_eq!(
        item_name, "Item 40,000",
        "an item is named by the text it holds"
    );
    assert_eq!(
        (
            attributes.get("posinset").map(String::as_str),
            attributes.get("setsize").map(String::as_str)
        ),
        (Some("40000"), Some("50000"))
    );
    let password = Proxy::new(
        &conn,
        peer.clone(),
        named("Password").1.as_str().to_owned(),
        "org.a11y.atspi.Text",
    )
    .unwrap();
    let shown: zbus::Result<String> = password.call("GetText", &(0i32, -1i32));
    assert!(
        shown.map_or(true, |text| text.is_empty()),
        "the password's value reached the bus"
    );
    let editable = Proxy::new(
        &conn,
        peer.clone(),
        named("Password").1.as_str().to_owned(),
        "org.a11y.atspi.EditableText",
    )
    .unwrap();
    let _: zbus::Result<bool> = editable.call("SetTextContents", &("typed",));
    let ok = Proxy::new(
        &conn,
        peer.clone(),
        named("OK").1.as_str().to_owned(),
        "org.a11y.atspi.Action",
    )
    .unwrap();
    let _: bool = ok.call("DoAction", &(0i32,)).unwrap();
    drop(app.stdin.take());
    assert!(
        app.wait().unwrap().success(),
        "the Click did not reach the application"
    );
}

#[test]
fn nothing_is_exported_while_accessibility_is_off() {
    const NAME: &str = "nothing_is_exported_while_accessibility_is_off";
    if std::env::var_os(APPLICATION).is_some() {
        return application(false);
    }
    assert!(
        run(NAME, false).is_none(),
        "the application embedded its root while accessibility was off"
    );
}

/// "Grüße Welt": 12 bytes, 10 characters.
const GREETING: &str = "Gr\u{fc}\u{df}e Welt";

/// A window with a focused text field showing [`GREETING`] with `selection` selected and the caret at its end, and a
/// password field that, like a careless core, carries a selection.
fn editor_tree(selection: Range<usize>, full: bool) -> AccessibilityUpdate {
    let text = |value: &str, selection: Range<usize>| {
        Some(AccessibleValue::Text {
            value: value.into(),
            editing: Some(TextSemantics {
                caret: selection.end,
                selection,
            }),
        })
    };
    let actions = vec![Action::Focus, Action::SetValue, Action::SetSelection];
    AccessibilityUpdate {
        window: WINDOW,
        root: id(1),
        focus: Some(id(2)),
        nodes: vec![
            (
                id(1),
                node(Role::Window, "Editor", None, Vec::new(), vec![id(2), id(3)]),
            ),
            (
                id(2),
                node(
                    Role::TextInput,
                    "Greeting",
                    text(GREETING, selection),
                    actions.clone(),
                    Vec::new(),
                ),
            ),
            (
                id(3),
                node(
                    Role::PasswordInput,
                    "Password",
                    text("hunter2", 0..7),
                    actions,
                    Vec::new(),
                ),
            ),
        ],
        removed: Vec::new(),
        full,
    }
}

/// The editor: it sends its tree with "Grüße" selected once accessibility is active and, on the first selection asked
/// of the text field, shows that selection, waits for the test to close its input and exits successfully. A request to
/// the password field fails it.
fn editor() {
    let mut adapter = AtspiAdapter::new(Waker::from(Arc::new(Unpark(std::thread::current()))));
    adapter.window_focus(WINDOW, true);
    let deadline = Instant::now() + Duration::from_secs(10);
    let (mut sent, mut actions) = (false, Vec::new());
    while Instant::now() < deadline {
        if adapter.is_active() && !sent {
            adapter.update(&editor_tree(0..7, true));
            sent = true;
        }
        adapter.drain_actions(&mut actions);
        for request in actions.drain(..) {
            assert_ne!(request.target, id(3), "the password field took a request");
            if let (Action::SetSelection, Some(ActionData::Selection(range))) =
                (request.action, request.data)
            {
                adapter.update(&editor_tree(range, false));
                std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
                return;
            }
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    panic!("no selection arrived");
}

#[test]
fn a_screen_reader_follows_the_caret_and_selection_of_a_text_field() {
    const NAME: &str = "a_screen_reader_follows_the_caret_and_selection_of_a_text_field";
    if std::env::var_os(APPLICATION).is_some() {
        return editor();
    }
    let Some(Session {
        bus,
        mut app,
        peer,
        _services,
    }) = run(NAME, true)
    else {
        return;
    };
    let conn = bus.connect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        if let Some(window) = children(&conn, &peer, ROOT)
            .unwrap_or_default()
            .into_iter()
            .next()
        {
            break window;
        }
        assert!(Instant::now() < deadline, "the window never appeared");
        std::thread::sleep(Duration::from_millis(50));
    };
    let nodes: Vec<_> = children(&conn, &peer, window.1.as_str())
        .unwrap()
        .into_iter()
        .map(|(_, path)| (described(&conn, &peer, &path).0, path))
        .collect();
    let text_of = |name: &str| {
        let path = &nodes.iter().find(|(n, _)| n == name).unwrap().1;
        // AccessKit signals no change of CaretOffset, so a cached one would stay stale
        zbus::blocking::proxy::Builder::<Proxy>::new(&conn)
            .destination(peer.clone())
            .unwrap()
            .path(path.as_str().to_owned())
            .unwrap()
            .interface("org.a11y.atspi.Text")
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .unwrap()
    };
    let (greeting, password) = (text_of("Greeting"), text_of("Password"));
    let caret_and_selection = || {
        let caret: i32 = greeting.get_property("CaretOffset").unwrap();
        let selection: (i32, i32) = greeting.call("GetSelection", &(0i32,)).unwrap();
        (caret, selection)
    };
    assert_eq!(caret_and_selection(), (5, (0, 5)), "Grüße is selected");
    let taken: zbus::Result<bool> = password.call("SetSelection", &(0i32, 0i32, 3i32));
    assert!(
        !taken.unwrap_or(false),
        "the password field took a selection"
    );

    // subscribed before asking, so no event is missed
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(peer.as_str())
        .unwrap()
        .interface("org.a11y.atspi.Event.Object")
        .unwrap()
        .build();
    let events = MessageIterator::for_match_rule(rule, &conn, None).unwrap();
    let (heard, hear) = channel();
    std::thread::spawn(move || {
        for message in events.flatten() {
            let header = message.header();
            let member = header.member().map(|m| m.to_string()).unwrap_or_default();
            let path = header.path().map(|p| p.to_string()).unwrap_or_default();
            type Event = (
                String,
                i32,
                i32,
                zbus::zvariant::OwnedValue,
                HashMap<String, zbus::zvariant::OwnedValue>,
            );
            let detail = message.body().deserialize::<Event>().map_or(-1, |e| e.1);
            if heard.send((member, path, detail)).is_err() {
                return;
            }
        }
    });
    let set: bool = greeting.call("SetSelection", &(0i32, 6i32, 10i32)).unwrap();
    assert!(set, "the text field refused a selection");
    let field = greeting.path().to_string();
    let (mut moved, mut selected) = (None, false);
    while moved.is_none() || !selected {
        let (member, path, detail) = hear
            .recv_timeout(Duration::from_secs(10))
            .expect("the caret and selection events");
        match member.as_str() {
            "TextCaretMoved" if path == field => moved = Some(detail),
            "TextSelectionChanged" if path == field => selected = true,
            _ => {}
        }
    }
    assert_eq!(moved, Some(10), "the caret moved to the end of Welt");
    assert_eq!(caret_and_selection(), (10, (6, 10)), "Welt is selected");
    drop(app.stdin.take());
    assert!(
        app.wait().unwrap().success(),
        "the selection did not reach the application"
    );
}
