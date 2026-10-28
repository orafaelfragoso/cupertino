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

mod common;
use common::PrivateBus;

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
    let bar = bus.connect();
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
