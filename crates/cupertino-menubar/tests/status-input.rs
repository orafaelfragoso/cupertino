//! Clicks and scrolls on status items, from the controller through the routing and the worker to an item on a private
//! session bus, as the StatusNotifierItem specification has them: the primary button activates an item, the secondary
//! one opens its menu or, without one, asks it for its context menu, the middle one sends its secondary action, each
//! at the pointer's place in the compositor space; an item that is only a menu opens it for the primary button too;
//! scrolls arrive as whole steps, a touchpad's gathered into them. Then an item's tip: after the pointer rests on it,
//! put away by a press, leaving it or the bar, and an open menu.

// a test owns its thread: it blocks on the bus
#![allow(clippy::disallowed_methods)]

use cupertino_menubar::controller::{
    Button, Effect, MenuController, Outcome, STATUS_TOOLTIP_DELAY,
};
use cupertino_menubar::dbus::sni::{StatusItem, StatusToolTip};
use cupertino_menubar::feed::{BarFeed, Shared, StatusState};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::MenuItem;
use cupertino_menubar::layout::{MonoMeasure, SlotKey};
use cupertino_menubar::routing::{self, Action, Context};
use cupertino_menubar::view;
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
        shared.set_status(state("menu-item", service, true, true));
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

        // an item with a menu is activated by the primary button and opens its menu for the secondary one; the
        // middle one sends its secondary action
        let with_menu = bar.on("with-menu");
        let (x, y) = screen(with_menu);
        let out = bar.click(with_menu, Button::Primary, 3.0);
        assert!(
            !bar.controller.is_open(),
            "the primary button opens no menu"
        );
        sent.extend(bar.requests(out));
        expected.push(format!("Activate {x} {y}"));
        let out = bar.click(with_menu, Button::Secondary, 4.0);
        assert!(
            bar.requests(out).is_empty(),
            "the secondary button opens the menu"
        );
        assert_eq!(
            bar.controller.open_slot(),
            Some(&SlotKey::Status("with-menu".into()))
        );
        bar.controller.close();
        let out = bar.click(with_menu, Button::Middle, 5.0);
        assert!(!bar.controller.is_open());
        sent.extend(bar.requests(out));
        expected.push(format!("SecondaryActivate {x} {y}"));

        // an item that is only a menu opens it for the primary button as for the secondary one
        let menu_item = bar.on("menu-item");
        for (i, button) in [Button::Primary, Button::Secondary].into_iter().enumerate() {
            let out = bar.click(menu_item, button, 5.5 + i as f64 * 0.1);
            assert!(bar.requests(out).is_empty(), "{button:?} opens the menu");
            assert_eq!(
                bar.controller.open_slot(),
                Some(&SlotKey::Status("menu-item".into()))
            );
            bar.controller.close();
        }

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

#[test]
fn an_items_tip_shows_once_the_pointer_rests_on_it() {
    let mut bar = Bar::new(":1.1");
    let (plain, with_menu) = (bar.on("plain"), bar.on("with-menu"));
    let shown = |bar: &Bar| bar.controller.tooltip().cloned();
    let tip = Some(SlotKey::Status("plain".into()));

    bar.controller.pointer_move(plain, 1.0);
    assert_eq!(shown(&bar), None, "not before the pointer rested");
    assert_eq!(
        bar.controller.next_deadline(),
        Some(1.0 + STATUS_TOOLTIP_DELAY)
    );
    // moving within the item keeps the wait going
    bar.controller
        .pointer_move(Point::new(plain.x + 1.0, plain.y), 1.3);
    let out = bar.controller.tick(1.0 + STATUS_TOOLTIP_DELAY);
    assert!(out.effects.contains(&Effect::Redraw));
    assert_eq!(shown(&bar), tip);

    // a press puts it away until the pointer leaves the item and comes back
    bar.controller.button_press(plain, Button::Middle, 2.0);
    bar.controller.button_release(plain, Button::Middle, 2.05);
    assert_eq!(shown(&bar), None);
    bar.controller.tick(5.0);
    assert_eq!(shown(&bar), None, "it stays away while the pointer stays");
    bar.controller.pointer_move(with_menu, 6.0);
    bar.controller.pointer_move(plain, 6.1);
    bar.controller.tick(6.1 + STATUS_TOOLTIP_DELAY);
    assert_eq!(shown(&bar), tip);

    // the pointer leaving the bar puts it away, and an open menu hides it
    bar.controller.pointer_leave(7.0);
    assert_eq!(shown(&bar), None);
    assert_eq!(bar.controller.next_deadline(), None);
    bar.controller.pointer_move(with_menu, 8.0);
    bar.controller.tick(8.0 + STATUS_TOOLTIP_DELAY);
    assert!(shown(&bar).is_some());
    bar.click(with_menu, Button::Secondary, 9.0);
    assert!(bar.controller.is_open());
    assert_eq!(shown(&bar), None);
}

#[test]
fn an_items_tip_is_its_tooltip_on_one_line_else_its_title() {
    let mut shared = Shared::default();
    let mut tipped = state("tipped", ":1.1", false, false);
    tipped.item.tooltip = StatusToolTip {
        title: "Sync".into(),
        description: "<b>3</b> files<br/>up to date".into(),
        ..StatusToolTip::default()
    };
    shared.set_status(tipped);
    shared.set_status(state("titled", ":1.1", false, false));
    let mut bare = state("bare", ":1.1", false, false);
    bare.item.title = " ".into();
    shared.set_status(bare);
    assert_eq!(
        view::tip_text(&shared, "tipped").as_deref(),
        Some(
            ["Sync", "3 files", "up to date"]
                .join(view::LINE_JOIN)
                .as_str()
        )
    );
    assert_eq!(view::tip_text(&shared, "titled").as_deref(), Some("titled"));
    assert_eq!(view::tip_text(&shared, "bare"), None);
    assert_eq!(view::tip_text(&shared, "gone"), None);
}
