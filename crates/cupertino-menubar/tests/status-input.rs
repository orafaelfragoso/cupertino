//! Clicks and scrolls on status items, from the controller through the routing and the worker to an item on a private
//! session bus: the primary button activates an item without a menu, the secondary one asks it for its context menu,
//! the middle one sends its secondary action, each at the pointer's place in the compositor space; an item with a menu
//! opens it for either button and is sent nothing; scrolls arrive as whole steps, a touchpad's gathered into them.

// a test owns its thread: it blocks on the bus
#![allow(clippy::disallowed_methods)]

use cupertino_menubar::controller::{Button, Effect, MenuController, Outcome};
use cupertino_menubar::dbus::sni::StatusItem;
use cupertino_menubar::feed::{BarFeed, Shared, StatusState};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::MenuItem;
use cupertino_menubar::layout::{MonoMeasure, SlotKey};
use cupertino_menubar::routing::{self, Action, Context};
use cupertino_menubar::worker::{Request, Worker};

use crate::bus::{within, Bus};

const ITEM_PATH: &str = "/StatusNotifierItem";
/// Where the bar's output lies in the compositor space.
const ORIGIN: (i32, i32) = (1920, 40);
/// A click: the release this long after the press.
const CLICK: f64 = 0.05;

/// A status item telling of each call it gets.
struct Item {
    calls: async_channel::Sender<String>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    fn activate(&self, x: i32, y: i32) {
        let _ = self.calls.try_send(format!("Activate {x} {y}"));
    }

    fn secondary_activate(&self, x: i32, y: i32) {
        let _ = self.calls.try_send(format!("SecondaryActivate {x} {y}"));
    }

    fn context_menu(&self, x: i32, y: i32) {
        let _ = self.calls.try_send(format!("ContextMenu {x} {y}"));
    }

    fn scroll(&self, delta: i32, orientation: &str) {
        let _ = self.calls.try_send(format!("Scroll {delta} {orientation}"));
    }
}

/// The status item `id` of `service`, with a menu of one row when `menu`, and only a menu when `item_is_menu`.
fn state(id: &str, service: &str, menu: bool, item_is_menu: bool) -> StatusState {
    StatusState {
        item: StatusItem {
            id: id.into(),
            service: service.into(),
            path: ITEM_PATH.into(),
            title: id.into(),
            item_is_menu,
            menu_path: menu.then(|| "/MenuBar".into()),
            ..StatusItem::default()
        },
        icon: None,
        menu: menu.then(|| MenuItem::submenu(0, "", vec![MenuItem::new(1, "Pause")])),
    }
}

struct Bar {
    feed: BarFeed,
    shared: Shared,
    controller: MenuController,
}

impl Bar {
    fn new(service: &str) -> Self {
        let mut shared = Shared::default();
        shared.set_status(state("plain", service, false, false));
        shared.set_status(state("menu-only", service, false, true));
        shared.set_status(state("with-menu", service, true, false));
        let feed = BarFeed::new(0, false, false);
        let output = Rect::new(0.0, 0.0, 1600.0, 900.0);
        let mut controller = MenuController::new(output, Box::new(MonoMeasure::default()));
        controller.set_content(feed.content(&shared));
        Self {
            feed,
            shared,
            controller,
        }
    }

    /// A point inside the slot of item `id`, off its middle.
    fn on(&self, id: &str) -> Point {
        let r = self
            .controller
            .layout()
            .slot(&SlotKey::Status(id.into()))
            .unwrap_or_else(|| panic!("a slot for {id}"))
            .rect;
        Point::new(r.x + r.w * 0.25, r.y + r.h * 0.75)
    }

    fn click(&mut self, at: Point, button: Button, now: f64) -> Outcome {
        let mut out = self.controller.button_press(at, button, now);
        let released = self.controller.button_release(at, button, now + CLICK);
        out.effects.extend(released.effects);
        out
    }

    fn requests(&self, out: Outcome) -> Vec<Request> {
        let cx = Context {
            feed: &self.feed,
            shared: &self.shared,
            layout: self.controller.layout(),
            origin: ORIGIN,
        };
        out.effects
            .into_iter()
            .filter_map(|e| match routing::resolve(e, &cx) {
                Some(Action::Request(r @ Request::StatusAction { .. })) => Some(r),
                _ => None,
            })
            .collect()
    }
}

/// `at` in the compositor space, as the item is told it.
fn screen(at: Point) -> (i32, i32) {
    (
        at.x.round() as i32 + ORIGIN.0,
        at.y.round() as i32 + ORIGIN.1,
    )
}

#[test]
fn clicks_and_scrolls_on_an_item_reach_it_with_the_pointers_place() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar_conn, app) = (bus.connect().await, bus.connect().await);
        let service = app.unique_name().unwrap().to_string();
        let (worker, _events) = Worker::start(bar_conn).await.unwrap();
        let (calls, called) = async_channel::unbounded();
        app.object_server()
            .at(ITEM_PATH, Item { calls })
            .await
            .unwrap();

        let mut bar = Bar::new(&service);
        let mut expected = Vec::new();
        let mut sent = Vec::new();

        let plain = bar.on("plain");
        let (x, y) = screen(plain);
        for (button, call) in [
            (Button::Primary, "Activate"),
            (Button::Secondary, "ContextMenu"),
            (Button::Middle, "SecondaryActivate"),
        ] {
            let out = bar.click(plain, button, 1.0);
            sent.extend(bar.requests(out));
            expected.push(format!("{call} {x} {y}"));
        }

        // an item that is only a menu, without one the bar read, shows its own for the primary button too
        let menu_only = bar.on("menu-only");
        let (x, y) = screen(menu_only);
        let out = bar.click(menu_only, Button::Primary, 2.0);
        sent.extend(bar.requests(out));
        expected.push(format!("ContextMenu {x} {y}"));

        // an item with a menu opens it for the primary and the secondary button, the middle one acts
        let with_menu = bar.on("with-menu");
        for (i, button) in [Button::Primary, Button::Secondary].into_iter().enumerate() {
            let now = 3.0 + i as f64;
            let out = bar.click(with_menu, button, now);
            assert!(bar.requests(out).is_empty(), "{button:?} opens the menu");
            assert_eq!(
                bar.controller.open_slot(),
                Some(&SlotKey::Status("with-menu".into()))
            );
            bar.controller.close();
        }
        let (x, y) = screen(with_menu);
        let out = bar.click(with_menu, Button::Middle, 5.0);
        assert!(!bar.controller.is_open());
        sent.extend(bar.requests(out));
        expected.push(format!("SecondaryActivate {x} {y}"));

        // a touchpad's movement is gathered into steps, a turn back starts again, a wheel's detents are steps
        for (steps, call) in [
            ((0.0, 0.4), None),
            ((0.0, 0.4), None),
            ((0.0, 0.4), Some("Scroll 1 vertical")),
            ((0.0, -2.0), Some("Scroll -2 vertical")),
            ((1.0, 0.0), Some("Scroll 1 horizontal")),
        ] {
            let out = bar.controller.pointer_scroll(plain, steps, 6.0);
            assert!(out.consumed);
            let requests = bar.requests(out);
            assert_eq!(requests.len(), usize::from(call.is_some()), "{steps:?}");
            sent.extend(requests);
            expected.extend(call.map(str::to_owned));
        }
        // a scroll over another slot drops what was gathered over this one
        bar.controller.pointer_scroll(plain, (0.0, 0.6), 7.0);
        bar.controller.pointer_scroll(menu_only, (0.0, 0.6), 7.0);
        let out = bar.controller.pointer_scroll(plain, (0.0, 0.6), 7.0);
        assert!(bar.requests(out).is_empty());

        assert_eq!(sent.len(), expected.len());
        for (request, call) in sent.into_iter().zip(expected) {
            worker.send(request).unwrap();
            assert_eq!(within(&call, called.recv()).await.unwrap(), call);
        }
    });
}

#[test]
fn a_press_is_released_by_its_own_button() {
    let mut bar = Bar::new(":1.1");
    let plain = bar.on("plain");
    bar.controller.button_press(plain, Button::Middle, 1.0);
    let other = bar.controller.button_release(plain, Button::Primary, 1.05);
    assert!(
        other.effects.is_empty(),
        "the primary button was not pressed"
    );
    let own = bar.controller.button_release(plain, Button::Middle, 1.1);
    assert!(own.effects.iter().any(|e| matches!(
        e,
        Effect::SlotActivated {
            button: Button::Middle,
            ..
        }
    )));
}
