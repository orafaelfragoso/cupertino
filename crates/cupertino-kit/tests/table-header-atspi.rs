//! On a private bus, a test acting as the accessibility registry and as an assistive technology finds a table's sorting
//! column header over AT-SPI as a focusable column header that says its order, and its click sorts the table in the
//! application. The application, a table on the headless window system with the AT-SPI adapter, runs in a copy of this
//! test binary.

use crate::support;

use std::io::Read;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_kit::controls::{Column, ColumnWidth, Selection, SortOrder, TableView, TextRole};
use cupertino_kit::geometry::{Point, Size};
use cupertino_kit::platform::{PlatformEvent, WindowEvent};
use cupertino_kit::prelude::*;
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_layout::ScrollState;
use support::Run;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

/// Set in the copy of the test binary that plays the application.
const APPLICATION: &str = "CUPERTINO_TEST_TABLE_HEADER_APPLICATION";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
/// AT-SPI's role number of a column header (`AtspiRole`) and bit of the focusable state (`AtspiStateType`).
const COLUMN_HEADER: u32 = 10;
const FOCUSABLE: u32 = 11;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Scroll(ScrollState),
    Sort((usize, SortOrder)),
}

struct Model {
    scroll: ScrollState,
    sorted: Vec<(usize, SortOrder)>,
}

struct Files;

impl Application for Files {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let scroll = ScrollState {
            offset: Point::ZERO,
            content: Size::ZERO,
            viewport: Size::ZERO,
        };
        (
            Model {
                scroll,
                sorted: Vec::new(),
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(scroll) => model.scroll = scroll,
            Message::Sort(sort) => model.sorted.push(sort),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        vec![Window {
            key: WindowKey("files".into()),
            spec: WindowSpec {
                title: "Files".into(),
                app_id: "org.example.Files".into(),
                size: Size::new(240.0, 120.0),
                min_size: None,
                max_size: None,
                decorations: cupertino_kit::platform::DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: cupertino_kit::core::CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::from(TableView {
                label: "Files".into(),
                columns: vec![Column {
                    title: "Name".into(),
                    width: ColumnWidth::Fill,
                }],
                sort: Some((0, SortOrder::Ascending)),
                on_sort: Some(Box::new(Message::Sort)),
                count: 2,
                row_height: 20.0,
                scroll: model.scroll,
                selection: Selection::default(),
                multiple: false,
                cell: Box::new(|row, _, selected| {
                    Element::new(Text {
                        content: AttributedText::plain(format!("file {row}"), TextStyle::new(13.0)),
                        role: TextRole::Body,
                        wrap: false,
                        secondary: false,
                        selected,
                    })
                }),
                type_select: None,
                on_scroll: Box::new(Message::Scroll),
                on_select: None,
                on_activate: None,
                on_row_key: None,
                header_top: 0.0,
            }),
            menus: None,
        }]
    }
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The application: once the header's click has sorted the table, it waits for the test to close its input and exits
/// successfully; it fails when no sort arrives.
fn application() {
    let adapter = AtspiAdapter::new(Waker::from(Arc::new(Unpark(std::thread::current()))));
    let mut run = Run::<Files>::with_accessibility(Some(Box::new(adapter)));
    run.send(WindowEvent::Focus(true));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        run.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Wake)
            .unwrap();
        run.settle();
        if !run.model().sorted.is_empty() {
            assert_eq!(run.model().sorted, [(0, SortOrder::Descending)]);
            std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
            return;
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    panic!("the header's click never sorted the table");
}

struct Status;

#[zbus::interface(name = "org.a11y.Status")]
impl Status {
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        true
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

fn proxy<'a>(conn: &Connection, peer: &str, path: &str, interface: &'a str) -> Proxy<'a> {
    Proxy::new(conn, peer.to_owned(), path.to_owned(), interface).unwrap()
}

fn children(conn: &Connection, peer: &str, path: &str) -> Vec<OwnedObjectPath> {
    let found: zbus::Result<Vec<(String, OwnedObjectPath)>> =
        proxy(conn, peer, path, "org.a11y.atspi.Accessible").call("GetChildren", &());
    found
        .unwrap_or_default()
        .into_iter()
        .map(|(_, p)| p)
        .collect()
}

/// The first node under `path`, depth first, whose role is `role`.
fn find(conn: &Connection, peer: &str, path: &str, role: u32) -> Option<OwnedObjectPath> {
    children(conn, peer, path).into_iter().find_map(|child| {
        let own: u32 = proxy(conn, peer, child.as_str(), "org.a11y.atspi.Accessible")
            .call("GetRole", &())
            .unwrap();
        if own == role {
            Some(child)
        } else {
            find(conn, peer, child.as_str(), role)
        }
    })
}

#[test]
fn an_assistive_technology_finds_a_focusable_sorted_column_header_whose_click_sorts_the_table() {
    const NAME: &str =
        "an_assistive_technology_finds_a_focusable_sorted_column_header_whose_click_sorts_the_table";
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
    let mut app = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            cupertino_test_bus::test_path(NAME).as_str(),
            "--exact",
            "--nocapture",
        ])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(APPLICATION, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let peer = embeds
        .recv_timeout(Duration::from_secs(10))
        .expect("the application never embedded its root");
    let conn = bus.connect();
    let deadline = Instant::now() + Duration::from_secs(10);
    let header = loop {
        if let Some(header) = find(&conn, &peer, ROOT, COLUMN_HEADER) {
            break header;
        }
        assert!(Instant::now() < deadline, "no column header appeared");
        std::thread::sleep(Duration::from_millis(50));
    };
    let state: Vec<u32> = proxy(&conn, &peer, header.as_str(), "org.a11y.atspi.Accessible")
        .call("GetState", &())
        .unwrap();
    assert_ne!(
        state[0] & (1 << FOCUSABLE),
        0,
        "the header is not focusable"
    );
    let description: String = proxy(&conn, &peer, header.as_str(), "org.a11y.atspi.Accessible")
        .get_property("Description")
        .unwrap();
    assert_eq!(description, "Sorted ascending");
    let action = proxy(&conn, &peer, header.as_str(), "org.a11y.atspi.Action");
    let name: String = action.call("GetName", &(0i32,)).unwrap();
    assert_eq!(name, "click");
    let _: bool = action.call("DoAction", &(0i32,)).unwrap();
    drop(app.stdin.take());
    assert!(
        app.wait().unwrap().success(),
        "the header's click did not sort the table"
    );
}
