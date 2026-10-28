//! A CupertinoKit application's exported menus read by the bar's own `org.gtk.Menus` reader, on a private session bus
//! (`dbus-daemon --session`); skipped when the daemon is not available.
#![cfg(feature = "dbus")]

use cupertino_menu_model::standard::{self, Features};
use cupertino_menubar::dbus::gtk::GtkMenuClient;
use cupertino_menubar::item::MenuItem;
use cupertino_menubar::source::GtkMenuProps;
use cupertino_modifiers::Profile;
use cupertino_ui_menus::{Exporter, WindowId};
use zbus::block_on;

use cupertino_test_bus::PrivateBus;

fn find<'a>(items: &'a [MenuItem], label: &str) -> Option<&'a MenuItem> {
    items.iter().find_map(|item| {
        (item.label == label)
            .then_some(item)
            .or_else(|| find(&item.children, label))
    })
}

#[test]
fn the_bar_reads_the_labels_a_kit_application_exports() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let exporting = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap();
    let mut exporter = Exporter::new(
        exporting,
        "org.example.Notes",
        Box::new(Profile::cupertino()),
        |_| {},
    )
    .unwrap();
    let features = Features {
        about: true,
        new: Some("New my_note".to_string()),
        ..Features::default()
    };
    let exported = exporter
        .export(WindowId(1), &standard::menu_bar("Notes", &features))
        .unwrap();

    block_on(async {
        let reader = bus.connect_async().await;
        exporter
            .set_reader(Some(reader.unique_name().unwrap().as_str()))
            .unwrap();
        let props = GtkMenuProps {
            bus_name: exported.unique_bus_name,
            application_id: Some(exported.application_id),
            application_object_path: Some(exported.application_object_path),
            window_object_path: Some(exported.window_object_path),
            menubar_object_path: Some(exported.menubar_path),
            app_menu_object_path: Some(exported.app_menu_path),
        };
        let (_client, items) = GtkMenuClient::load(&reader, &props).await.unwrap();

        let file = find(&items, "File").expect("the File menu");
        assert!(file.has_submenu);
        let new = find(&items, "New my_note").expect("the New item, its underscore kept");
        assert_eq!(new.mnemonic, None);
        assert_eq!(
            new.accel.as_deref(),
            Some("<Super>n"),
            "New has its chord in the keys of the profile"
        );
    });
}

/// A load the exporter refuses because the application does not know yet that the bar reads its menus (it learns that
/// over its own Wayland connection, which can come after the bar's first load) is made again, and the menus arrive
/// once the application is told.
#[cfg(feature = "app")]
#[test]
fn the_bar_loads_the_menus_of_an_application_told_its_reader_after_the_first_load() {
    use cupertino_menubar::worker::{Event, Request, Worker};
    use std::time::{Duration, Instant};

    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let exporting = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap();
    let mut exporter = Exporter::new(
        exporting,
        "org.example.Notes",
        Box::new(Profile::cupertino()),
        |_| {},
    )
    .unwrap();
    let exported = exporter
        .export(
            WindowId(1),
            &standard::menu_bar("Notes", &Features::default()),
        )
        .unwrap();
    let bar = zbus::block_on(bus.connect_async());
    let bar_name = bar.unique_name().unwrap().to_string();
    let (worker, events) = Worker::start(bar, || {}).unwrap();

    worker
        .send(Request::FetchGtk(GtkMenuProps {
            bus_name: exported.unique_bus_name,
            application_id: Some(exported.application_id),
            application_object_path: Some(exported.application_object_path),
            window_object_path: Some(exported.window_object_path),
            menubar_object_path: Some(exported.menubar_path),
            app_menu_object_path: Some(exported.app_menu_path),
        }))
        .unwrap();
    // the first load, and the first one made again, are refused
    std::thread::sleep(Duration::from_millis(150));
    exporter.set_reader(Some(&bar_name)).unwrap();

    let end = Instant::now() + Duration::from_secs(5);
    let root = loop {
        let wait = end.saturating_duration_since(Instant::now());
        match events
            .recv_timeout(wait)
            .expect("the menus once the reader is known")
        {
            Event::Layout { root, .. } => break root,
            _ => continue,
        }
    };
    assert!(find(&root.children, "File").is_some());
}

/// A load the exporter keeps refusing past the worker's retries is reported as failed, so the bar loads it again when
/// the server announces the menus again.
#[cfg(feature = "app")]
#[test]
fn a_load_refused_past_the_retries_is_reported_as_failed() {
    use cupertino_menubar::source::MenuSource;
    use cupertino_menubar::worker::{Event, Request, Worker};
    use std::time::{Duration, Instant};

    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let exporting = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap();
    let mut exporter = Exporter::new(
        exporting,
        "org.example.Notes",
        Box::new(Profile::cupertino()),
        |_| {},
    )
    .unwrap();
    let exported = exporter
        .export(
            WindowId(1),
            &standard::menu_bar("Notes", &Features::default()),
        )
        .unwrap();
    let (worker, events) = Worker::start(zbus::block_on(bus.connect_async()), || {}).unwrap();
    let props = GtkMenuProps {
        bus_name: exported.unique_bus_name,
        application_id: Some(exported.application_id),
        application_object_path: Some(exported.application_object_path),
        window_object_path: Some(exported.window_object_path),
        menubar_object_path: Some(exported.menubar_path),
        app_menu_object_path: Some(exported.app_menu_path),
    };
    worker.send(Request::FetchGtk(props.clone())).unwrap();

    let end = Instant::now() + Duration::from_secs(10);
    let failed = loop {
        let wait = end.saturating_duration_since(Instant::now());
        match events
            .recv_timeout(wait)
            .expect("the load to be reported as failed")
        {
            Event::LayoutFailed { source } => break source,
            _ => continue,
        }
    };
    assert_eq!(failed, MenuSource::Gtk(props));
}

/// An exporter the bar's worker reads with its reader set, and the worker's events.
#[cfg(feature = "app")]
struct Followed {
    _bus: PrivateBus,
    exporter: Exporter,
    _worker: cupertino_menubar::worker::Worker,
    events: std::sync::mpsc::Receiver<cupertino_menubar::worker::Event>,
}

#[cfg(feature = "app")]
impl Followed {
    /// Notes with a New item (`app.new`), disabled before the bar loads it.
    fn start() -> Option<Followed> {
        use cupertino_menu_model::ActionName;
        use cupertino_menubar::worker::{Request, Worker};

        let bus = PrivateBus::start()?;
        let exporting = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap();
        let mut exporter = Exporter::new(
            exporting,
            "org.example.Notes",
            Box::new(Profile::cupertino()),
            |_| {},
        )
        .unwrap();
        let exported = exporter.export(WindowId(1), &notes("New Note")).unwrap();
        exporter
            .set_enabled(WindowId(1), &ActionName::parse("app.new").unwrap(), false)
            .unwrap();
        let bar = zbus::block_on(bus.connect_async());
        exporter
            .set_reader(Some(bar.unique_name().unwrap().as_str()))
            .unwrap();
        let (worker, events) = Worker::start(bar, || {}).unwrap();
        worker
            .send(Request::FetchGtk(GtkMenuProps {
                bus_name: exported.unique_bus_name,
                application_id: Some(exported.application_id),
                application_object_path: Some(exported.application_object_path),
                window_object_path: Some(exported.window_object_path),
                menubar_object_path: Some(exported.menubar_path),
                app_menu_object_path: Some(exported.app_menu_path),
            }))
            .unwrap();
        Some(Followed {
            _bus: bus,
            exporter,
            _worker: worker,
            events,
        })
    }

    /// The next tree the worker sends whose `label` item satisfies `want`, within 5 s.
    fn tree_where(&self, label: &str, want: impl Fn(&MenuItem) -> bool) -> MenuItem {
        use cupertino_menubar::worker::Event;
        use std::time::{Duration, Instant};

        let end = Instant::now() + Duration::from_secs(5);
        loop {
            let wait = end.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(wait) {
                Ok(Event::Layout { root, .. }) => {
                    if find(&root.children, label).is_some_and(&want) {
                        return root;
                    }
                }
                Ok(_) => {}
                Err(_) => panic!("no tree with {label} as wanted within 5 s"),
            }
        }
    }
}

#[cfg(feature = "app")]
fn notes(new_label: &str) -> cupertino_menu_model::MenuBar {
    standard::menu_bar(
        "Notes",
        &Features {
            new: Some(new_label.to_string()),
            ..Features::default()
        },
    )
}

/// An item the application enables after the bar loaded its menus is enabled in the bar, as an item that needs a
/// selection is once something is selected.
#[cfg(feature = "app")]
#[test]
fn an_item_enabled_after_the_load_is_enabled_in_the_bar() {
    use cupertino_menu_model::ActionName;
    let Some(followed) = Followed::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    followed.tree_where("New Note", |item| !item.enabled);
    followed
        .exporter
        .set_enabled(WindowId(1), &ActionName::parse("app.new").unwrap(), true)
        .unwrap();
    followed.tree_where("New Note", |item| item.enabled);
}

/// A label the application changes after the bar loaded its menus is the bar's label.
#[cfg(feature = "app")]
#[test]
fn a_label_changed_after_the_load_is_the_bars_label() {
    let Some(mut followed) = Followed::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    followed.tree_where("New Note", |_| true);
    followed
        .exporter
        .export(WindowId(1), &notes("New Notebook"))
        .unwrap();
    followed.tree_where("New Notebook", |_| true);
}

/// An application that changes its menus without pause reaches the bar as a few trees, the last one current.
#[cfg(feature = "app")]
#[test]
fn many_changes_reach_the_bar_as_few_trees() {
    use cupertino_menu_model::ActionName;
    use cupertino_menubar::worker::Event;
    use std::time::{Duration, Instant};

    let Some(followed) = Followed::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    followed.tree_where("New Note", |item| !item.enabled);
    let new = ActionName::parse("app.new").unwrap();
    let start = Instant::now();
    for i in 0..200 {
        followed
            .exporter
            .set_enabled(WindowId(1), &new, i % 2 == 0)
            .unwrap();
    }
    followed.tree_where("New Note", |item| !item.enabled);
    let elapsed = start.elapsed();
    // what else comes within the next interval belongs to the same burst
    std::thread::sleep(Duration::from_millis(200));
    let more = followed
        .events
        .try_iter()
        .filter(|e| matches!(e, Event::Layout { .. }))
        .count();
    // one tree per 50 ms at most, with room for the first and last of the burst
    let bound = (elapsed.as_millis() / 50) as usize + 3;
    assert!(
        more <= bound,
        "{more} more trees after a burst of {elapsed:?}"
    );
}
