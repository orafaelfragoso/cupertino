//! The bar's worker against an exporter that floods `org.gtk.Menus` `Changed` while its menus load, on a private
//! session bus (`dbus-daemon --session`); skipped when the daemon is not available.
#![cfg(feature = "app")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use cupertino_menubar::item::MenuItem;
use cupertino_menubar::source::{GtkMenuProps, MenuSource};
use cupertino_menubar::worker::{Event, Request, Worker};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Str};

use cupertino_test_bus::PrivateBus;

type Items = Vec<HashMap<String, OwnedValue>>;

fn item(label: &str) -> HashMap<String, OwnedValue> {
    HashMap::from([(
        "label".to_string(),
        OwnedValue::from(Str::from(label.to_string())),
    )])
}

/// Menus whose first `Start` emits `flood` changes of its one item and then answers with the item as it was before
/// them, as an exporter whose menus change while the bar loads them; later loads have the item as the changes left it.
struct Flooding {
    flood: usize,
    flooded: AtomicBool,
}

#[zbus::interface(name = "org.gtk.Menus")]
impl Flooding {
    async fn start(
        &self,
        _groups: Vec<u32>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Vec<(u32, u32, Items)> {
        if self.flooded.swap(true, Ordering::SeqCst) {
            return vec![(0, 0, vec![item(&format!("v{}", self.flood))])];
        }
        for i in 1..=self.flood {
            let change = vec![(0, 0, 0, 1, vec![item(&format!("v{i}"))])];
            let _ = Self::changed(&emitter, change).await;
        }
        // the answer was made before the changes: only a later load has them
        vec![(0, 0, vec![item("v0")])]
    }

    fn end(&self, _groups: Vec<u32>) {}

    #[zbus(signal)]
    async fn changed(
        emitter: &SignalEmitter<'_>,
        changes: Vec<(u32, u32, u32, u32, Items)>,
    ) -> zbus::Result<()>;
}

fn props(bus_name: &str, path: &str) -> GtkMenuProps {
    GtkMenuProps {
        bus_name: bus_name.to_string(),
        application_id: None,
        application_object_path: None,
        window_object_path: None,
        menubar_object_path: Some(path.to_string()),
        app_menu_object_path: None,
    }
}

/// The next tree of `path` within 10 s.
fn tree_of(events: &Receiver<Event>, path: &str) -> MenuItem {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let wait = end.saturating_duration_since(Instant::now());
        match events.recv_timeout(wait) {
            Ok(Event::Layout {
                source: MenuSource::Gtk(props),
                root,
            }) if props.menubar_object_path.as_deref() == Some(path) => return root,
            Ok(_) => {}
            Err(_) => panic!("no tree of {path} within 10 s"),
        }
    }
}

/// More changes than the bus connection's signal queue holds arrive while the menus load: the load still ends, the
/// tree is the last one, and the bar's connection still answers for another exporter.
#[test]
fn changes_that_flood_the_load_neither_stop_the_bar_nor_leave_it_behind() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let exporter = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .serve_at(
            "/flood",
            Flooding {
                flood: 500,
                flooded: AtomicBool::new(false),
            },
        )
        .unwrap()
        .serve_at(
            "/calm",
            Flooding {
                flood: 0,
                flooded: AtomicBool::new(true),
            },
        )
        .unwrap()
        .build()
        .unwrap();
    let name = exporter.unique_name().unwrap().to_string();
    let (worker, events) = Worker::start(zbus::block_on(bus.connect_async()), || {}).unwrap();

    worker
        .send(Request::FetchGtk(props(&name, "/flood")))
        .unwrap();
    let mut root = tree_of(&events, "/flood");
    // a tree loaded while changes came is loaded again
    while root.children.first().map(|i| i.label.as_str()) != Some("v500") {
        root = tree_of(&events, "/flood");
    }

    worker
        .send(Request::FetchGtk(props(&name, "/calm")))
        .unwrap();
    let calm = tree_of(&events, "/calm");
    assert_eq!(calm.children.first().map(|i| i.label.as_str()), Some("v0"));
}
