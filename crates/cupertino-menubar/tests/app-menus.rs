//! The menus Settings and Files publish, read by the bar's worker on a private session bus (`dbus-daemon --session`),
//! opened by dragging from its title and chosen: the application hears the item's action. Skipped when the daemon is not
//! available.
#![cfg(feature = "app")]

use cupertino_menu_model::{ActionName, MenuBar};
use cupertino_menubar::controller::{Effect, MenuController, FLASH_OFF, FLASH_ON};
use cupertino_menubar::desktop::AppName;
use cupertino_menubar::feed::{BarFeed, Shared};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::layout::{MonoMeasure, SlotKey};
use cupertino_menubar::routing::{self, Action, Context};
use cupertino_menubar::worker::{Event, Worker};
use cupertino_modifiers::Profile;
use cupertino_shell_client::{ActiveApp, GtkMenu, MenuBarUpdate, WindowMenu};
use cupertino_test_bus::PrivateBus;
use cupertino_ui_menus::{Activation, Exporter, WindowId};
use std::sync::mpsc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(10);

fn centre(r: Rect) -> Point {
    Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Publishes `menus` as the application `app_id` on `bus`, shows them in a bar whose worker reads them, opens the menu
/// `title` and chooses its item `label`; returns what the application heard.
fn choose(bus: &PrivateBus, app_id: &str, menus: &MenuBar, title: &str, label: &str) -> Activation {
    let (heard, activations) = mpsc::channel();
    let exporting = zbus::blocking::connection::Builder::address(bus.address.as_str()).unwrap();
    let mut exporter = Exporter::new(
        exporting,
        app_id,
        Box::new(Profile::cupertino()),
        move |activation| {
            let _ = heard.send(activation);
        },
    )
    .unwrap();
    let exported = exporter.export(WindowId(1), menus).unwrap();

    let reader = zbus::block_on(bus.connect_async());
    exporter
        .set_reader(Some(reader.unique_name().unwrap().as_str()))
        .unwrap();
    let (worker, events) = Worker::start(reader, || {}).unwrap();

    let mut feed = BarFeed::new(1, true, true);
    let mut shared = Shared::default();
    let update = MenuBarUpdate {
        active_app: Some(ActiveApp {
            app_id: app_id.to_owned(),
            title: String::new(),
        }),
        menu: Some(WindowMenu::Gtk(GtkMenu {
            bus_name: exported.unique_bus_name,
            application_id: Some(exported.application_id),
            application_object_path: Some(exported.application_object_path),
            window_object_path: Some(exported.window_object_path),
            menubar_object_path: Some(exported.menubar_path),
            app_menu_object_path: Some(exported.app_menu_path),
        })),
        ..MenuBarUpdate::default()
    };
    feed.apply(&update, &mut shared, |_| AppName {
        name: title.to_owned(),
        has_entry: true,
    });
    let request = feed
        .request_layout(&mut shared)
        .expect("the bar asks for the window's menus");
    worker.send(request).unwrap();
    loop {
        match events.recv_timeout(WAIT).expect("the worker answers") {
            Event::Layout { source, root } => {
                shared.apply_layout(&source, root);
                break;
            }
            Event::LayoutFailed { .. } => panic!("the bar could not read the menus of {app_id}"),
            _ => {}
        }
    }

    let mut controller = MenuController::new(
        Rect::new(0.0, 0.0, 1440.0, 900.0),
        Box::new(MonoMeasure::default()),
    );
    controller.set_content(feed.content(&shared));
    let menu = controller
        .content()
        .menus
        .iter()
        .find(|m| m.label == title)
        .unwrap_or_else(|| panic!("{app_id} publishes no {title} menu"))
        .id;
    let slot = controller
        .layout()
        .slot(&SlotKey::Menu(menu))
        .expect("the menu has a title in the bar")
        .rect;
    controller.pointer_press(centre(slot), 0.0);
    let row = controller.levels()[0]
        .layout
        .rows
        .iter()
        .find(|r| r.item.label == label)
        .unwrap_or_else(|| panic!("the {title} menu has no {label}"))
        .rect;
    controller.pointer_move(centre(row), 0.1);
    controller.pointer_release(centre(row), 0.2);
    // the row flashes, then is chosen
    let outcome = controller.tick(0.2 + FLASH_OFF + FLASH_ON);
    let chosen: Vec<_> = outcome
        .effects
        .into_iter()
        .filter(|e| matches!(e, Effect::Activate { .. }))
        .collect();
    assert_eq!(chosen.len(), 1, "releasing over {label} chooses it");
    let cx = Context {
        feed: &feed,
        shared: &shared,
        layout: controller.layout(),
        origin: (0, 0),
    };
    match routing::resolve(chosen[0].clone(), &cx) {
        Some(Action::Request(request)) => worker.send(request).unwrap(),
        other => panic!("{label} asks the exporter, not {other:?}"),
    }
    activations
        .recv_timeout(WAIT)
        .unwrap_or_else(|_| panic!("{app_id} hears {label}"))
}

#[test]
fn a_files_menu_item_chosen_in_the_bar_reaches_files() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let menus =
        cupertino_files::menu_bar::menu_bar(&cupertino_files::menu_bar::MenuState::default());
    let heard = choose(&bus, "org.cupertino.Files", &menus, "File", "New Folder");
    assert_eq!(
        heard,
        Activation {
            window: Some(WindowId(1)),
            action: cupertino_files::menu_bar::MenuAction::NewFolder.name(),
        }
    );
}

#[test]
fn a_settings_menu_item_chosen_in_the_bar_reaches_settings() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let menus =
        cupertino_settings::menu_bar::menu_bar(&cupertino_settings::menu_bar::MenuState::default());
    let heard = choose(&bus, "org.cupertino.Settings", &menus, "Edit", "Find");
    assert_eq!(
        heard,
        Activation {
            window: Some(WindowId(1)),
            action: ActionName::new(cupertino_menu_model::Scope::Win, "find").unwrap(),
        }
    );
}
