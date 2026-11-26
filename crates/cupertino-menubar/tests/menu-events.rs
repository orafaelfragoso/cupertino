//! The controller tells the source of every menu that shows and stops showing (`MenuOpened`, `MenuClosed`), however
//! it opened or closed, with the input clock's time; a choice carries the time of the input that made it; a source's
//! request opens a menu as if clicked; and the routing makes DBusMenu requests of them.

use cupertino_kit_shell::menu_bar::{ActiveApp, Update, WindowMenu};
use cupertino_menubar::controller::{Effect, Key, MenuController, Outcome};
use cupertino_menubar::dbus::dbusmenu::MenuEvent;
use cupertino_menubar::desktop::AppName;
use cupertino_menubar::feed::{BarFeed, Shared};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::{ItemId, MenuItem};
use cupertino_menubar::layout::{BarContent, MonoMeasure, SlotKey};
use cupertino_menubar::routing::{self, Action, Context};
use cupertino_menubar::worker::Request;

/// A click: the release this long after the press leaves the menu open.
const CLICK: f64 = 0.05;
/// The input clock's time of the first input, in milliseconds.
const INPUT: u32 = 40_000;

fn content(menus: Vec<MenuItem>) -> BarContent {
    BarContent {
        app_name: "Notes".into(),
        menus,
        ..BarContent::default()
    }
}

/// File (1): New (2), Open Recent (3: notes (4)); Edit (10): Undo (11). Exporter's items.
fn menus() -> Vec<MenuItem> {
    vec![
        MenuItem::submenu(
            1,
            "File",
            vec![
                MenuItem::new(2, "New"),
                MenuItem::submenu(3, "Open Recent", vec![MenuItem::new(4, "notes")]),
            ],
        )
        .exported(),
        MenuItem::submenu(10, "Edit", vec![MenuItem::new(11, "Undo")]).exported(),
    ]
}

fn controller() -> MenuController {
    let output = Rect::new(0.0, 0.0, 1600.0, 900.0);
    let mut c = MenuController::new(output, Box::new(MonoMeasure::default()));
    c.set_content(content(menus()));
    c
}

/// The menus told opened (`true`) and closed, in order.
fn told(outcome: &Outcome) -> Vec<(bool, i32)> {
    outcome
        .effects
        .iter()
        .filter_map(|e| match e {
            Effect::MenuOpened { item, .. } => Some((true, item.0)),
            Effect::MenuClosed { item, .. } => Some((false, item.0)),
            _ => None,
        })
        .collect()
}

fn title(c: &MenuController, id: i32) -> Point {
    let r = c.layout().slot(&SlotKey::Menu(ItemId(id))).unwrap().rect;
    Point::new(r.x + r.w / 2.0, r.y + 1.0)
}

fn click(c: &mut MenuController, at: Point, now: f64) -> Outcome {
    let mut out = c.pointer_press(at, now);
    out.effects
        .extend(c.pointer_release(at, now + CLICK).effects);
    out
}

fn row(c: &MenuController, level: usize, row: usize) -> Point {
    let r = c.levels()[level].layout.rows[row].rect;
    Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Open File and its Open Recent submenu.
fn open_recent(c: &mut MenuController) -> Vec<(bool, i32)> {
    let mut seen = told(&click(c, title(c, 1), 0.0));
    let at = row(c, 0, 1);
    seen.extend(told(&c.pointer_move(at, 1.0)));
    seen.extend(told(&c.tick(10.0)));
    seen
}

#[test]
fn menus_are_told_they_open_shallowest_first_and_close_deepest_first() {
    let mut c = controller();
    assert_eq!(open_recent(&mut c), [(true, 1), (true, 3)]);
    let closed = c.key(Key::Escape, 11.0);
    assert_eq!(told(&closed), [(false, 3)], "Esc closes the submenu");
    let closed = c.key(Key::Escape, 12.0);
    assert_eq!(told(&closed), [(false, 1)]);
}

#[test]
fn a_click_outside_closes_every_menu() {
    let mut c = controller();
    open_recent(&mut c);
    let out = c.pointer_press(Point::new(800.0, 600.0), 11.0);
    assert_eq!(told(&out), [(false, 3), (false, 1)]);
}

#[test]
fn switching_titles_closes_one_and_opens_the_other() {
    let mut c = controller();
    told(&{
        let at = title(&c, 1);
        click(&mut c, at, 0.0)
    });
    let out = {
        let at = title(&c, 10);
        c.pointer_move(at, 1.0)
    };
    assert_eq!(told(&out), [(false, 1), (true, 10)]);
}

#[test]
fn choosing_a_row_closes_the_menu_once_the_choice_is_made() {
    let mut c = controller();
    c.cfg.flash_choice = false;
    told(&{
        let at = title(&c, 10);
        click(&mut c, at, 0.0)
    });
    let out = {
        let at = row(&c, 0, 0);
        click(&mut c, at, 1.0)
    };
    let effects = &out.effects;
    let chose = effects.iter().position(|e| {
        matches!(
            e,
            Effect::Activate {
                item: ItemId(11),
                ..
            }
        )
    });
    let closed = effects.iter().position(|e| {
        matches!(
            e,
            Effect::MenuClosed {
                item: ItemId(10),
                ..
            }
        )
    });
    assert!(chose.unwrap() < closed.unwrap(), "{effects:?}");
}

#[test]
fn a_menu_whose_item_is_gone_is_told_it_closed_and_a_renumbered_one_is_not() {
    let mut c = controller();
    open_recent(&mut c);
    // the exporter gave every item a new id: the menus still show, nothing is told
    let renumbered: Vec<MenuItem> = vec![MenuItem::submenu(
        101,
        "File",
        vec![
            MenuItem::new(102, "New"),
            MenuItem::submenu(103, "Open Recent", vec![MenuItem::new(104, "notes")]),
        ],
    )
    .exported()];
    let out = c.set_content(content(renumbered));
    assert_eq!(told(&out), []);
    let without = vec![MenuItem::submenu(101, "File", vec![MenuItem::new(102, "New")]).exported()];
    let out = c.set_content(content(without));
    assert_eq!(
        told(&out),
        [(false, 103)],
        "the submenu closes under its id"
    );
    let out = c.set_content(content(Vec::new()));
    assert_eq!(told(&out), [(false, 101)], "the application went");
}

#[test]
fn the_events_and_a_choice_carry_the_input_clocks_time() {
    let mut c = controller();
    c.cfg.flash_choice = false;
    c.set_input_time(INPUT, 2.0);
    let opened = {
        let at = title(&c, 10);
        c.pointer_press(at, 2.0)
    };
    assert!(opened.effects.contains(&Effect::MenuOpened {
        slot: SlotKey::Menu(ItemId(10)),
        item: ItemId(10),
        time: INPUT,
    }));
    c.set_input_time(INPUT + 50, 2.05);
    let at = title(&c, 10);
    c.pointer_release(at, 2.05);
    // a later input, 1.5 s on
    c.set_input_time(INPUT + 1500, 3.5);
    let at = row(&c, 0, 0);
    let chose = click(&mut c, at, 3.5);
    let time = chose.effects.iter().find_map(|e| match e {
        Effect::Activate { time, .. } => Some(*time),
        _ => None,
    });
    // chosen on the release, a click's length later
    assert_eq!(time, Some(INPUT + 1550));
    // a timer advances the clock from the latest input
    let mut c = controller();
    c.set_input_time(INPUT, 0.0);
    {
        let at = title(&c, 1);
        click(&mut c, at, 0.0)
    };
    let at = row(&c, 0, 1);
    c.pointer_move(at, 1.0);
    let opened = c.tick(10.0);
    assert!(opened.effects.contains(&Effect::MenuOpened {
        slot: SlotKey::Menu(ItemId(1)),
        item: ItemId(3),
        time: INPUT + 10_000,
    }));
}

#[test]
fn a_request_to_open_an_item_opens_its_menus_as_if_clicked() {
    let mut c = controller();
    let out = c.open_item(ItemId(4));
    assert!(out.consumed);
    assert_eq!(told(&out), [(true, 1), (true, 3)]);
    assert_eq!(c.open_slot(), Some(&SlotKey::Menu(ItemId(1))));
    assert_eq!(c.levels()[0].selected_item().map(|i| i.id), Some(ItemId(3)));
    assert_eq!(c.levels()[1].selected_item().map(|i| i.id), Some(ItemId(4)));
    // a title's menu
    let out = c.open_item(ItemId(10));
    assert_eq!(told(&out), [(false, 3), (false, 1), (true, 10)]);
    assert_eq!(c.levels().len(), 1);
    assert_eq!(c.levels()[0].selected, None);
    // an item no menu holds
    let out = c.open_item(ItemId(99));
    assert!(!out.consumed && told(&out).is_empty());
}

/// A bar fed an exporter's tree, as the component feeds it.
fn bar() -> (BarFeed, Shared, MenuController) {
    let mut shared = Shared::default();
    let mut feed = BarFeed::new(0, false, false);
    let update = Update {
        active_app: Some(ActiveApp {
            app_id: "notes".into(),
            title: "Notes".into(),
        }),
        menu: Some(WindowMenu::DbusMenu {
            service: ":1.70".into(),
            path: "/menu".into(),
        }),
        ..Update::default()
    };
    feed.apply(&update, &mut shared, |_| AppName {
        name: "Notes".into(),
        has_entry: false,
    });
    feed.request_layout(&mut shared).expect("a load");
    let source = feed.source(&shared);
    shared.apply_layout(&source, MenuItem::submenu(0, "", menus()));
    let output = Rect::new(0.0, 0.0, 1600.0, 900.0);
    let mut controller = MenuController::new(output, Box::new(MonoMeasure::default()));
    controller.set_content(feed.content(&shared));
    (feed, shared, controller)
}

fn requests(feed: &BarFeed, shared: &Shared, c: &MenuController, out: Outcome) -> Vec<Request> {
    let cx = Context {
        feed,
        shared,
        layout: c.layout(),
        origin: (0, 0),
    };
    out.effects
        .into_iter()
        .filter_map(|e| match routing::resolve(e, &cx) {
            Some(Action::Request(r)) => Some(r),
            _ => None,
        })
        .collect()
}

#[test]
fn the_exporter_is_sent_its_menus_events_and_the_click_with_their_times() {
    let (feed, shared, mut c) = bar();
    c.cfg.flash_choice = false;
    c.set_input_time(INPUT, 0.0);
    let at = title(&c, 1);
    let opened = click(&mut c, at, 0.0);
    let sent = requests(&feed, &shared, &c, opened);
    assert!(
        sent.iter().any(|r| matches!(
            r,
            Request::MenuEvent {
                item: 1,
                event: MenuEvent::Opened,
                time: INPUT,
                ..
            }
        )),
        "{sent:?}"
    );
    c.set_input_time(INPUT + 700, 0.7);
    let at = row(&c, 0, 0);
    let chose = click(&mut c, at, 0.7);
    let sent = requests(&feed, &shared, &c, chose);
    assert!(
        sent.iter().any(|r| matches!(
            r,
            Request::ActivateDbusMenu { item: 2, time, .. } if *time == INPUT + 750
        )),
        "{sent:?}"
    );
    assert!(
        sent.iter().any(|r| matches!(
            r,
            Request::MenuEvent {
                item: 1,
                event: MenuEvent::Closed,
                ..
            }
        )),
        "{sent:?}"
    );
}

#[test]
fn the_bars_own_menus_are_not_sent_events() {
    let (feed, shared, mut c) = bar();
    // the App menu: the bar's own
    let r = c.layout().slot(&SlotKey::AppMenu).unwrap().rect;
    let opened = click(&mut c, Point::new(r.x + r.w / 2.0, r.y + 1.0), 0.0);
    assert!(told(&opened).contains(&(true, i32::MIN)));
    let sent = requests(&feed, &shared, &c, opened);
    assert!(
        !sent.iter().any(|r| matches!(r, Request::MenuEvent { .. })),
        "{sent:?}"
    );
}

#[test]
fn an_exporter_asking_to_be_noticed_marks_the_application_name() {
    let (feed, mut shared, _) = bar();
    assert!(!feed.content(&shared).notice);
    let source = feed.source(&shared);
    let mut root = MenuItem::submenu(0, "", menus());
    root.notice = true;
    shared.apply_layout(&source, root);
    assert!(feed.content(&shared).notice);
}
