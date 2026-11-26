//! An open menu survives its exporter rebuilding the whole tree under new ids, as a Chromium exporter (Electron
//! applications) does when told a menu is about to show: it answers `AboutToShow` with `false`, signals
//! `ItemsPropertiesUpdated` and `LayoutUpdated` on the root, and the tree read again holds the same menus under fresh
//! ids. The menu stays open with the rows of the new tree, and closes only when its item is really gone. The exporter
//! runs on a private session bus. An exporter's submenu the App menu took in tells the exporter it shows as well.

// a test owns its thread: it blocks on the bus and looks up the daemon's binary
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use async_io::Timer;
use cupertino_kit_shell::menu_bar::{ActiveApp, Update, WindowMenu};
use cupertino_menubar::controller::{Effect, MenuController, Outcome};
use cupertino_menubar::dbus::dbusmenu::{make_node, RawNode};
use cupertino_menubar::desktop::AppName;
use cupertino_menubar::feed::{BarFeed, Shared};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::{ItemId, MenuItem};
use cupertino_menubar::layout::{BarContent, MonoMeasure, SlotKey};
use cupertino_menubar::routing::{self, Action, Context};
use cupertino_menubar::source::MenuSource;
use cupertino_menubar::worker::{Event, Request, Worker};
use futures_util::future::{select, Either};
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

const PATH: &str = "/com/canonical/menu/1";
/// The longest a tree may take to arrive.
const ARRIVAL: Duration = Duration::from_secs(10);
/// How far the exporter moves every id when it rebuilds its tree.
const RENUMBER: i32 = 100;
/// A click: the release this long after the press leaves the menu open.
const CLICK: f64 = 0.05;

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn session_bus() -> (Bus, String) {
    let daemon = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("dbus-daemon"))
        .find(|p| p.is_file())
        .expect("dbus-daemon is installed");
    let mut process = Command::new(daemon)
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut address = String::new();
    BufReader::new(process.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    (Bus(process), address.trim().to_owned())
}

/// The menus of the exporter, by title and rows; an item's id is its place in this list plus the exporter's base.
const MENUS: [(&str, &[&str]); 3] = [
    ("File", &["New Thread", "Open Folder"]),
    ("Edit", &["Undo", "Redo", "Copy"]),
    ("View", &["Reload", "Zoom In"]),
];

/// An exporter that gives every item a new id each time a menu is about to show.
struct Renumbering {
    base: i32,
    revision: u32,
}

fn label(text: &str) -> HashMap<String, OwnedValue> {
    HashMap::from([(
        "label".to_owned(),
        OwnedValue::try_from(zbus::zvariant::Value::from(text)).unwrap(),
    )])
}

impl Renumbering {
    fn tree(&self) -> RawNode {
        let mut id = self.base;
        let menus = MENUS
            .iter()
            .map(|(title, rows)| {
                id += 1;
                let menu = id;
                let rows = rows
                    .iter()
                    .map(|row| {
                        id += 1;
                        make_node(id, label(row), Vec::new()).unwrap()
                    })
                    .collect();
                let mut props = label(title);
                props.insert(
                    "children-display".into(),
                    OwnedValue::try_from(zbus::zvariant::Value::from("submenu")).unwrap(),
                );
                make_node(menu, props, rows).unwrap()
            })
            .collect();
        make_node(0, HashMap::new(), menus).unwrap()
    }
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Renumbering {
    fn get_layout(&self, parent: i32, _depth: i32, _names: Vec<String>) -> (u32, RawNode) {
        assert_eq!(parent, 0, "the root is read after a change of it");
        (self.revision, self.tree())
    }

    async fn about_to_show(
        &mut self,
        id: i32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> bool {
        self.base += RENUMBER;
        self.revision += 1;
        let display = HashMap::from([(
            "children-display".to_owned(),
            OwnedValue::try_from(zbus::zvariant::Value::from("submenu")).unwrap(),
        )]);
        Self::items_properties_updated(&emitter, vec![(id, display)], Vec::new())
            .await
            .unwrap();
        Self::layout_updated(&emitter, self.revision, 0)
            .await
            .unwrap();
        false
    }

    #[zbus(signal)]
    async fn items_properties_updated(
        emitter: &SignalEmitter<'_>,
        updated: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "normal".into()
    }
}

/// The bar of one output, fed by the worker as the component feeds it.
struct Bar {
    feed: BarFeed,
    shared: Shared,
    controller: MenuController,
}

impl Bar {
    fn new(service: &str) -> Bar {
        let mut shared = Shared::default();
        let mut feed = BarFeed::new(0, false, false);
        let update = Update {
            active_app: Some(ActiveApp {
                app_id: "t3code".into(),
                title: "T3 Code".into(),
            }),
            menu: Some(WindowMenu::DbusMenu {
                service: service.into(),
                path: PATH.into(),
            }),
            ..Update::default()
        };
        feed.apply(&update, &mut shared, |_| AppName {
            name: "T3 Code".into(),
            has_entry: false,
        });
        let output = Rect::new(0.0, 0.0, 1600.0, 900.0);
        let controller = MenuController::new(output, Box::new(MonoMeasure::default()));
        Bar {
            feed,
            shared,
            controller,
        }
    }

    /// The requests the controller's effects stand for.
    fn requests(&self, outcome: Outcome) -> Vec<Request> {
        let cx = Context {
            feed: &self.feed,
            shared: &self.shared,
            layout: self.controller.layout(),
            origin: (0, 0),
        };
        outcome
            .effects
            .into_iter()
            .filter_map(|effect| match routing::resolve(effect, &cx) {
                Some(Action::Request(request)) => Some(request),
                _ => None,
            })
            .collect()
    }

    fn apply(&mut self, source: &MenuSource, root: MenuItem) {
        self.shared.apply_layout(source, root);
        let content = self.feed.content(&self.shared);
        self.controller.set_content(content);
    }

    /// Click the title of the menu `title`.
    fn click(&mut self, title: &str, now: f64) -> Vec<Request> {
        let id = self.menu_id(title).expect("the menu is on the bar");
        let slot = self.controller.layout().slot(&SlotKey::Menu(id)).unwrap();
        let at = Point::new(slot.rect.x + slot.rect.w / 2.0, slot.rect.y + 1.0);
        let pressed = self.controller.pointer_press(at, now);
        let mut requests = self.requests(pressed);
        let released = self.controller.pointer_release(at, now + CLICK);
        requests.extend(self.requests(released));
        requests
    }

    fn menu_id(&self, title: &str) -> Option<ItemId> {
        let menus = &self.controller.content().menus;
        menus.iter().find(|m| m.label == title).map(|m| m.id)
    }

    /// The title and rows of the open menu.
    fn open_menu(&self) -> Option<(String, Vec<String>)> {
        let level = self.controller.levels().first()?;
        let rows = level.layout.rows.iter().map(|r| r.item.label.clone());
        Some((level.menu.label.clone(), rows.collect()))
    }
}

/// The next tree the worker reports.
async fn next_tree(events: &async_channel::Receiver<Event>) -> (MenuSource, MenuItem) {
    let tree = async {
        loop {
            match events.recv().await.unwrap() {
                Event::Layout { source, root } => break (source, *root),
                Event::LayoutFailed { source } => panic!("the menu was given up: {source:?}"),
                _ => {}
            }
        }
    };
    match select(Box::pin(tree), Timer::after(ARRIVAL)).await {
        Either::Left((tree, _)) => tree,
        Either::Right(_) => panic!("no tree within {ARRIVAL:?}"),
    }
}

fn rows(title: &str) -> Vec<String> {
    let (_, rows) = MENUS.iter().find(|(t, _)| *t == title).unwrap();
    rows.iter().map(|r| (*r).to_owned()).collect()
}

#[test]
fn a_menu_stays_open_when_its_exporter_renumbers_the_tree_as_it_shows() {
    let (_bus, address) = session_bus();
    async_io::block_on(async {
        let bar_conn = Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let app = Builder::address(address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let exporter = Renumbering {
            base: 0,
            revision: 1,
        };
        app.object_server().at(PATH, exporter).await.unwrap();
        let service = app.unique_name().unwrap().to_string();
        let (worker, events) = Worker::start(bar_conn).await.unwrap();
        let mut bar = Bar::new(&service);
        let fetch = bar.feed.request_layout(&mut bar.shared).expect("a load");
        worker.send(fetch).unwrap();
        let (source, root) = next_tree(&events).await;
        bar.apply(&source, root);

        for (i, title) in ["File", "Edit", "View"].into_iter().enumerate() {
            let shown = bar.menu_id(title).unwrap();
            let requests = bar.click(title, i as f64);
            let told: Vec<i32> = requests
                .iter()
                .filter_map(|r| match r {
                    Request::AboutToShow { item, .. } => Some(*item),
                    _ => None,
                })
                .collect();
            assert_eq!(told, [shown.0], "the exporter is told {title} shows");
            for request in requests {
                worker.send(request).unwrap();
            }
            let (source, root) = next_tree(&events).await;
            bar.apply(&source, root);
            let renumbered = bar.menu_id(title).unwrap();
            assert_ne!(renumbered, shown, "the exporter gave {title} a new id");
            let (open, shown_rows) = bar.open_menu().expect("a menu stays open");
            assert_eq!(open, title);
            // the bar adds its own rows below some menus (View's full screen item)
            assert!(shown_rows.starts_with(&rows(title)), "{shown_rows:?}");
            assert_eq!(bar.controller.open_slot(), Some(&SlotKey::Menu(renumbered)));
            // the rows are the new tree's, so a choice reaches the item the exporter has now
            let level = &bar.controller.levels()[0];
            assert!(level
                .layout
                .rows
                .iter()
                .take(rows(title).len())
                .all(|r| r.item.id.0 > renumbered.0));
        }
    });
}

fn content(menus: Vec<MenuItem>) -> BarContent {
    BarContent {
        app_name: "T3 Code".into(),
        menus,
        ..BarContent::default()
    }
}

/// File (`base` + 1) with a row and a submenu Open Recent (`base` + 3) of two rows.
fn file(base: i32, recent: bool) -> MenuItem {
    let mut rows = vec![MenuItem::new(base + 2, "New Thread")];
    if recent {
        rows.push(MenuItem::submenu(
            base + 3,
            "Open Recent",
            vec![
                MenuItem::new(base + 4, "notes"),
                MenuItem::new(base + 5, "todo"),
            ],
        ));
    }
    MenuItem::submenu(base + 1, "File", rows)
}

fn edit(base: i32) -> MenuItem {
    MenuItem::submenu(base + 10, "Edit", vec![MenuItem::new(base + 11, "Undo")])
}

fn controller(menus: Vec<MenuItem>) -> MenuController {
    let output = Rect::new(0.0, 0.0, 1600.0, 900.0);
    let mut controller = MenuController::new(output, Box::new(MonoMeasure::default()));
    controller.set_content(content(menus));
    controller
}

/// Click the title of `id` and open the submenu of its row `row` with the keyboard.
fn open_submenu(controller: &mut MenuController, id: i32, row: usize) {
    let slot = controller
        .layout()
        .slot(&SlotKey::Menu(ItemId(id)))
        .unwrap()
        .rect;
    let at = Point::new(slot.x + slot.w / 2.0, slot.y + 1.0);
    controller.pointer_press(at, 0.0);
    controller.pointer_release(at, CLICK);
    let rect = controller.levels()[0].layout.rows[row].rect;
    let at = Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    controller.pointer_move(at, 1.0);
    controller.tick(10.0);
    assert_eq!(controller.levels().len(), 2, "the submenu opened");
}

#[test]
fn an_open_submenu_follows_its_item_to_its_new_id() {
    let mut c = controller(vec![file(0, true), edit(0)]);
    open_submenu(&mut c, 1, 1);
    let outcome = c.set_content(content(vec![file(RENUMBER, true), edit(RENUMBER)]));
    assert!(!outcome.effects.contains(&Effect::Closed));
    assert_eq!(c.open_slot(), Some(&SlotKey::Menu(ItemId(RENUMBER + 1))));
    let ids: Vec<ItemId> = c.levels().iter().map(|l| l.menu.id).collect();
    assert_eq!(ids, [ItemId(RENUMBER + 1), ItemId(RENUMBER + 3)]);
    assert_eq!(
        c.levels()[0].selected_item().map(|i| i.id),
        Some(ItemId(RENUMBER + 3)),
        "the row of the open submenu stays selected"
    );
}

#[test]
fn an_open_submenu_whose_item_is_gone_closes_and_its_menu_stays() {
    let mut c = controller(vec![file(0, true), edit(0)]);
    open_submenu(&mut c, 1, 1);
    c.set_content(content(vec![file(RENUMBER, false), edit(RENUMBER)]));
    let ids: Vec<ItemId> = c.levels().iter().map(|l| l.menu.id).collect();
    assert_eq!(ids, [ItemId(RENUMBER + 1)]);
}

#[test]
fn a_submenu_of_the_exporters_items_in_the_app_menu_tells_the_exporter_it_shows() {
    let mut bar = Bar::new(":1.51");
    bar.feed.request_layout(&mut bar.shared).expect("a load");
    let app = MenuItem::submenu(
        1,
        "T3 Code",
        vec![
            MenuItem::new(2, "About T3 Code"),
            MenuItem::submenu(3, "Services", vec![MenuItem::new(4, "Share")]),
        ],
    );
    let source = bar.feed.source(&bar.shared);
    bar.apply(&source, MenuItem::submenu(0, "", vec![app]).exported());
    let shown = Outcome {
        effects: vec![Effect::AboutToShow {
            slot: SlotKey::AppMenu,
            item: ItemId(3),
        }],
        consumed: true,
    };
    assert!(matches!(
        bar.requests(shown).as_slice(),
        [Request::AboutToShow { item: 3, .. }]
    ));
}

#[test]
fn an_open_menu_whose_item_is_gone_closes() {
    let mut c = controller(vec![file(0, true), edit(0)]);
    open_submenu(&mut c, 1, 1);
    let outcome = c.set_content(content(vec![edit(RENUMBER)]));
    assert!(outcome.effects.contains(&Effect::Closed));
    assert!(!c.is_open());
}
