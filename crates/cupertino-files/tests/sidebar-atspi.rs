//! On a private bus, a test acting as the accessibility registry and as an assistive technology reads a Files window's
//! sidebar over AT-SPI: a list whose items are named by their places, the folder shown selected, the focused row
//! focused, a removable file system described and a busy one busy, and a mounted one's eject button named for it,
//! whose click reaches the window. The window's tree, published with the AT-SPI adapter, comes from a copy of this test
//! binary.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::thread::Thread;
use std::time::{Duration, Instant};

use cupertino_files::access::{window_tree, SidebarNode, WindowView, WINDOW};
use cupertino_files::geom::Size;
use cupertino_files::places::{Place, PlaceGroup, PlaceKind};
use cupertino_files::window::{Bars, WindowLayout};
use cupertino_test_bus::PrivateBus;
use cupertino_ui_atspi::AtspiAdapter;
use cupertino_ui_core::{AccessibilityAdapter, Action};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

/// Set in the copy of the test binary that plays the window.
const WINDOW_PROCESS: &str = "CUPERTINO_TEST_FILES_SIDEBAR_WINDOW";
const ROOT: &str = "/org/a11y/atspi/accessible/root";
/// AT-SPI's role numbers (`AtspiRole`) and state bits (`AtspiStateType`).
const LIST: u32 = 31;
const LIST_ITEM: u32 = 32;
const BUTTON: u32 = 43;
const BUSY: u32 = 3;
const FOCUSED: u32 = 12;
const SELECTED: u32 = 23;

const DOWNLOADS: usize = 1;
const BACKUP: usize = 2;

fn places() -> Vec<Place> {
    let folder = |name: &str, path: &str, kind| Place {
        name: name.into(),
        path: PathBuf::from(path),
        group: PlaceGroup::Favorites,
        kind,
    };
    vec![
        folder("rafael", "/home/rafael", PlaceKind::Home),
        folder("Downloads", "/home/rafael/Downloads", PlaceKind::Downloads),
        Place::removable(
            "Backup Disk",
            Some(Path::new("/run/media/rafael/Backup")),
            false,
        ),
        Place::removable("KINGSTON", None, false),
        Place::removable(
            "Camera Card",
            Some(Path::new("/run/media/rafael/CARD")),
            true,
        ),
    ]
}

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The window: it shows Downloads with the keyboard on Backup Disk's row. Once the eject button's click has arrived it
/// waits for the test to close its input and exits successfully; it fails when no click arrives.
fn window() {
    let places = places();
    let layout = WindowLayout::new(Size { w: 920.0, h: 436.0 }, &places, Bars::HIDDEN);
    let tree = window_tree(&WindowView {
        title: "Downloads",
        layout: &layout,
        places: &places,
        dir: Path::new("/home/rafael/Downloads"),
        sidebar_focus: Some(BACKUP),
    });
    let mut adapter = AtspiAdapter::new(Waker::from(Arc::new(Unpark(std::thread::current()))));
    adapter.window_focus(WINDOW, true);
    let mut sent = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if adapter.is_active() && !sent {
            adapter.update(&tree);
            sent = true;
        }
        let mut requests = Vec::new();
        adapter.drain_actions(&mut requests);
        if requests.iter().any(|r| {
            r.action == Action::Click
                && SidebarNode::of(r.target) == Some(SidebarNode::Eject(BACKUP))
        }) {
            std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
            return;
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    panic!("the eject button's click never arrived");
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

/// An object of the window as assistive technology reads it.
#[derive(Clone)]
struct Object<'a> {
    conn: &'a Connection,
    peer: &'a str,
    path: OwnedObjectPath,
}

impl<'a> Object<'a> {
    fn proxy(&self, interface: &'static str) -> Proxy<'_> {
        Proxy::new(
            self.conn,
            self.peer.to_owned(),
            self.path.to_string(),
            interface,
        )
        .unwrap()
    }

    fn accessible(&self) -> Proxy<'_> {
        self.proxy("org.a11y.atspi.Accessible")
    }

    fn children(&self) -> Vec<Object<'a>> {
        let found: zbus::Result<Vec<(String, OwnedObjectPath)>> =
            self.accessible().call("GetChildren", &());
        found
            .unwrap_or_default()
            .into_iter()
            .map(|(_, path)| Object {
                conn: self.conn,
                peer: self.peer,
                path,
            })
            .collect()
    }

    fn role(&self) -> u32 {
        self.accessible().call("GetRole", &()).unwrap()
    }

    fn name(&self) -> String {
        self.accessible().get_property("Name").unwrap()
    }

    fn description(&self) -> String {
        self.accessible().get_property("Description").unwrap()
    }

    fn has(&self, state: u32) -> bool {
        let states: Vec<u32> = self.accessible().call("GetState", &()).unwrap();
        states[(state / 32) as usize] & (1 << (state % 32)) != 0
    }

    /// Every object under this one, depth first, whose role is `role`.
    fn all(&self, role: u32) -> Vec<Object<'a>> {
        let mut found = Vec::new();
        for child in self.children() {
            if child.role() == role {
                found.push(child.clone());
            }
            found.extend(child.all(role));
        }
        found
    }
}

#[test]
fn an_assistive_technology_reads_the_sidebar_rows_and_ejects_a_mounted_drive() {
    const NAME: &str = "an_assistive_technology_reads_the_sidebar_rows_and_ejects_a_mounted_drive";
    if std::env::var_os(WINDOW_PROCESS).is_some() {
        return window();
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
        .args([NAME, "--exact", "--nocapture"])
        .env(WINDOW_PROCESS, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let peer = embeds
        .recv_timeout(Duration::from_secs(10))
        .expect("the window never embedded its root");
    let conn = bus.connect();
    let root = Object {
        conn: &conn,
        peer: &peer,
        path: OwnedObjectPath::try_from(ROOT).unwrap(),
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let rows = loop {
        let rows = root.all(LIST_ITEM);
        if rows.len() == places().len() {
            break rows;
        }
        assert!(
            Instant::now() < deadline,
            "the sidebar's rows never appeared"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let list = root.all(LIST);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name(), "Sidebar");
    let names: Vec<String> = rows.iter().map(Object::name).collect();
    assert_eq!(
        names,
        [
            "rafael",
            "Downloads",
            "Backup Disk",
            "KINGSTON",
            "Camera Card"
        ]
    );
    let descriptions: Vec<String> = rows.iter().map(Object::description).collect();
    assert_eq!(
        descriptions,
        ["", "", "Removable", "Removable, not mounted", "Removable"]
    );
    let flagged =
        |state| -> Vec<usize> { (0..rows.len()).filter(|i| rows[*i].has(state)).collect() };
    assert_eq!(flagged(SELECTED), [DOWNLOADS]);
    assert_eq!(flagged(FOCUSED), [BACKUP]);
    assert_eq!(flagged(BUSY), [4]);
    let buttons = root.all(BUTTON);
    assert_eq!(
        buttons.len(),
        1,
        "only the mounted, idle drive has an eject button"
    );
    assert_eq!(buttons[0].name(), "Eject Backup Disk");
    let action = buttons[0].proxy("org.a11y.atspi.Action");
    let name: String = action.call("GetName", &(0i32,)).unwrap();
    assert_eq!(name, "click");
    let _: bool = action.call("DoAction", &(0i32,)).unwrap();
    drop(app.stdin.take());
    assert!(
        app.wait().unwrap().success(),
        "the eject button's click did not reach the window"
    );
}
