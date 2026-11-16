//! A status item whose menu changes, against the bar's worker, feed, controller and routing on a private session bus
//! (`dbus-daemon --session`): the rows the bar shows and the ids it sends back are the exporter's current ones. The
//! same following serves an application's menus. Skipped when the daemon is not available.
#![cfg(feature = "app")]

use cupertino_menubar::controller::{Effect, MenuController, Outcome, FLASH_OFF, FLASH_ON};
use cupertino_menubar::dbus::dbusmenu::{make_node, RawNode};
use cupertino_menubar::dbus::sni::WatcherClient;
use cupertino_menubar::feed::{BarFeed, Shared, StatusState};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::{ItemId, MenuItem, Toggle};
use cupertino_menubar::layout::{MonoMeasure, SlotKey};
use cupertino_menubar::routing::{self, Action, Context};
use cupertino_menubar::source::{KdeAppMenu, MenuSource};
use cupertino_menubar::worker::{Event, Request, Waits, Worker};
use cupertino_test_bus::PrivateBus;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{block_on, fdo, Connection};

const WAIT: Duration = Duration::from_secs(10);
const ITEM_PATH: &str = "/StatusNotifierItem";

/// One row of a stand-in exporter's menu.
#[derive(Clone, Debug)]
struct Row {
    id: i32,
    label: &'static str,
    submenu: bool,
    children: Vec<Row>,
}

fn row(id: i32, label: &'static str) -> Row {
    Row {
        id,
        label,
        submenu: false,
        children: Vec::new(),
    }
}

fn submenu(id: i32, label: &'static str, children: Vec<Row>) -> Row {
    Row {
        id,
        label,
        submenu: true,
        children,
    }
}

fn menu(children: Vec<Row>) -> Row {
    submenu(0, "", children)
}

fn value(v: Value<'static>) -> OwnedValue {
    OwnedValue::try_from(v).unwrap()
}

fn props(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), value(v)))
        .collect()
}

impl Row {
    fn find(&self, id: i32) -> Option<&Row> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }

    fn find_mut(&mut self, id: i32) -> Option<&mut Row> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter_mut().find_map(|c| c.find_mut(id))
    }

    fn node(&self) -> RawNode {
        let mut pairs = vec![("label", Value::from(self.label))];
        if self.submenu {
            pairs.push(("children-display", "submenu".into()));
        }
        let children = self.children.iter().map(Row::node).collect();
        make_node(self.id, props(pairs), children).unwrap()
    }
}

/// What a stand-in exporter was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Heard {
    /// `GetLayout` below this item.
    Read(i32),
    /// `AboutToShow` for this item.
    Shown(i32),
    /// `Event` `clicked` for this item.
    Clicked(i32),
}

struct MenuState {
    revision: u32,
    root: Row,
    /// What the exporter puts below an item when it is told the item's menu is about to show, asking for an update.
    on_show: HashMap<i32, Vec<Row>>,
    /// `AboutToShow` is never answered.
    silent: bool,
    /// The labels the exporter gives item 3, one signal each, before it answers the next `GetLayout`.
    burst: Vec<String>,
    /// This many of the next `GetLayout` calls fail: the object is there before its menu is.
    unready: u32,
}

/// A stand-in for an application's `com.canonical.dbusmenu` object.
struct FakeMenu {
    state: Arc<Mutex<MenuState>>,
    heard: mpsc::Sender<Heard>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl FakeMenu {
    async fn get_layout(
        &self,
        parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<(u32, RawNode)> {
        let _ = self.heard.send(Heard::Read(parent_id));
        let burst = std::mem::take(&mut self.state.lock().unwrap().burst);
        for label in burst {
            let label = vec![(3, props(vec![("label", label.into())]))];
            Self::items_properties_updated(&emitter, label, Vec::new()).await?;
        }
        let mut state = self.state.lock().unwrap();
        if state.unready > 0 {
            state.unready -= 1;
            return Err(fdo::Error::Failed("no menu yet".into()));
        }
        let parent = state
            .root
            .find(parent_id)
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("no item {parent_id}")))?;
        Ok((state.revision, parent.node()))
    }

    fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
        if event_id == "clicked" {
            let _ = self.heard.send(Heard::Clicked(id));
        }
    }

    async fn about_to_show(&self, id: i32) -> bool {
        let _ = self.heard.send(Heard::Shown(id));
        let silent = {
            let mut state = self.state.lock().unwrap();
            if let Some(children) = state.on_show.remove(&id) {
                state.root.find_mut(id).unwrap().children = children;
                state.revision += 1;
                return true;
            }
            state.silent
        };
        if silent {
            async_io::Timer::after(Duration::from_secs(120)).await;
        }
        false
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        3
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn items_properties_updated(
        emitter: &SignalEmitter<'_>,
        updated_props: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed_props: Vec<(i32, Vec<String>)>,
    ) -> zbus::Result<()>;
}

/// A stand-in for an application's status item.
struct FakeItem {
    /// The path of its menu; `/` for none.
    menu: Arc<Mutex<String>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl FakeItem {
    #[zbus(property)]
    fn id(&self) -> String {
        "org.example.chat".into()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        "Chat".into()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "Active".into()
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(self.menu.lock().unwrap().as_str()).unwrap()
    }

    #[zbus(signal)]
    async fn new_menu(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// An application on the bus: its connection, and the menus it exports.
struct App {
    conn: Connection,
    item_menu: Arc<Mutex<String>>,
}

/// One exported menu of an [`App`]: its state, and what it was asked.
struct Exported {
    conn: Connection,
    path: &'static str,
    state: Arc<Mutex<MenuState>>,
    heard: mpsc::Receiver<Heard>,
    /// Everything heard so far, in order.
    log: Vec<Heard>,
}

impl App {
    /// An application that serves its status item, without a menu and not announced yet. The item is served as the
    /// connection is made, so the connection answers calls from its first moment.
    fn start(bus: &PrivateBus) -> App {
        let item_menu = Arc::new(Mutex::new("/".to_string()));
        let item = FakeItem {
            menu: item_menu.clone(),
        };
        let builder = bus.builder().serve_at(ITEM_PATH, item).unwrap();
        App {
            conn: block_on(builder.build()).unwrap(),
            item_menu,
        }
    }

    fn name(&self) -> String {
        self.conn.unique_name().unwrap().to_string()
    }

    /// Export a menu of `rows` at `path`, its layout at `revision`.
    fn export(&self, path: &'static str, revision: u32, rows: Vec<Row>) -> Exported {
        let state = Arc::new(Mutex::new(MenuState {
            revision,
            root: menu(rows),
            on_show: HashMap::new(),
            silent: false,
            burst: Vec::new(),
            unready: 0,
        }));
        let (heard_tx, heard) = mpsc::channel();
        let fake = FakeMenu {
            state: state.clone(),
            heard: heard_tx,
        };
        block_on(self.conn.object_server().at(path, fake)).unwrap();
        Exported {
            conn: self.conn.clone(),
            path,
            state,
            heard,
            log: Vec::new(),
        }
    }

    /// Announce the status item to the bar's watcher, its menu at `menu` (`/` for none). Returns the id the bar knows
    /// the item by.
    fn register_item(&self, menu: &str) -> String {
        *self.item_menu.lock().unwrap() = menu.into();
        block_on(async {
            let watcher = WatcherClient::connect(&self.conn).await.unwrap();
            watcher.register_item(ITEM_PATH).await.unwrap();
        });
        format!("{}{ITEM_PATH}", self.name())
    }

    /// The item's menu is at `menu` from now on, and the item says so.
    fn move_menu(&self, menu: &str) {
        *self.item_menu.lock().unwrap() = menu.into();
        let emitter = SignalEmitter::new(&self.conn, ITEM_PATH).unwrap();
        block_on(FakeItem::new_menu(&emitter)).unwrap();
    }
}

impl Exported {
    fn source(&self) -> KdeAppMenu {
        KdeAppMenu {
            service: self.conn.unique_name().unwrap().to_string(),
            path: self.path.into(),
        }
    }

    /// Wait until the exporter is asked `want`.
    fn hears(&mut self, want: Heard) {
        loop {
            let heard = self
                .heard
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| panic!("the exporter hears {want:?}; it heard {:?}", self.log));
            self.log.push(heard);
            if heard == want {
                return;
            }
        }
    }

    /// Everything the exporter was asked so far.
    fn heard(&mut self) -> &[Heard] {
        self.log.extend(self.heard.try_iter());
        &self.log
    }

    /// The application replaces its menu with `rows` at `revision` and signals the new layout.
    fn replace(&self, revision: u32, rows: Vec<Row>) {
        {
            let mut state = self.state.lock().unwrap();
            state.revision = revision;
            state.root = menu(rows);
        }
        let emitter = SignalEmitter::new(&self.conn, self.path).unwrap();
        block_on(FakeMenu::layout_updated(&emitter, revision, 0)).unwrap();
    }

    fn signal_properties(
        &self,
        updated: Vec<(i32, HashMap<String, OwnedValue>)>,
        removed: Vec<(i32, Vec<String>)>,
    ) {
        let emitter = SignalEmitter::new(&self.conn, self.path).unwrap();
        block_on(FakeMenu::items_properties_updated(
            &emitter, updated, removed,
        ))
        .unwrap();
    }
}

fn centre(r: Rect) -> Point {
    Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// One bar as the process drives it: the worker's events go into what the bars share, the controller shows the
/// content, and the controller's effects go back to the worker through the routing.
struct Bar {
    worker: Worker,
    events: mpsc::Receiver<Event>,
    feed: BarFeed,
    shared: Shared,
    controller: MenuController,
    now: f64,
}

impl Bar {
    fn start(bus: &PrivateBus, waits: Waits) -> Bar {
        let (worker, events) =
            Worker::start_with(block_on(bus.connect_async()), waits, || {}).unwrap();
        let feed = BarFeed::new(1, true, true);
        let shared = Shared::default();
        let mut controller = MenuController::new(
            Rect::new(0.0, 0.0, 1440.0, 900.0),
            Box::new(MonoMeasure::default()),
        );
        controller.set_content(feed.content(&shared));
        Bar {
            worker,
            events,
            feed,
            shared,
            controller,
            now: 0.0,
        }
    }

    /// Take the worker's events until `done`.
    fn until(&mut self, what: &str, done: impl Fn(&Bar) -> bool) {
        while !done(self) {
            let event = self
                .events
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| panic!("{what}"));
            match event {
                Event::Layout { source, root } => self.shared.apply_layout(&source, *root),
                Event::LayoutFailed { source } => self.shared.layout_failed(&source),
                Event::StatusUpdated { item } => self.shared.set_status(StatusState {
                    item: *item,
                    icon: None,
                    menu: None,
                }),
                Event::StatusRemoved { id } => self.shared.remove_status(&id),
                Event::Registrations(_) => {}
            }
            let content = self.feed.content(&self.shared);
            if self.controller.content() != &content {
                let outcome = self.controller.set_content(content);
                self.act(outcome);
            }
        }
    }

    /// Carry out the effects of `outcome`; returns what was asked of the worker.
    fn act(&mut self, outcome: Outcome) -> Vec<Request> {
        let mut asked = Vec::new();
        for effect in outcome.effects {
            if let Some(Action::Request(request)) = self.resolve(effect) {
                self.worker.send(request.clone()).unwrap();
                asked.push(request);
            }
        }
        asked
    }

    fn resolve(&self, effect: Effect) -> Option<Action> {
        let cx = Context {
            feed: &self.feed,
            shared: &self.shared,
            layout: self.controller.layout(),
            origin: (0, 0),
        };
        routing::resolve(effect, &cx)
    }

    /// The menu the status item `id` has in the bar's content.
    fn status_menu(&self, id: &str) -> Option<&MenuItem> {
        let status = self.controller.content().status.iter().find(|s| s.id == id);
        status.and_then(|s| s.menu.as_ref())
    }

    /// The ids of the rows of the status item's menu in the bar's content; `None` without a menu.
    fn rows(&self, id: &str) -> Option<Vec<i32>> {
        self.status_menu(id)
            .map(|menu| menu.children.iter().map(|row| row.id.0).collect())
    }

    /// The ids of the rows the open menu displays at `level`; empty when no such menu is open.
    fn displayed(&self, level: usize) -> Vec<i32> {
        self.controller.levels().get(level).map_or(Vec::new(), |l| {
            l.layout.rows.iter().map(|row| row.item.id.0).collect()
        })
    }

    /// Press the status item's slot and hold the button: its menu opens.
    fn press(&mut self, id: &str) -> Vec<Request> {
        let slot = self
            .controller
            .layout()
            .slot(&SlotKey::Status(id.into()))
            .expect("the status item has a slot")
            .rect;
        self.now += 1.0;
        let outcome = self.controller.pointer_press(centre(slot), self.now);
        self.act(outcome)
    }

    /// Move the held pointer onto the row `item` of the open menu at `level` and rest there until its submenu opens.
    fn rest_on(&mut self, level: usize, item: i32) -> Vec<Request> {
        let target = self.row(level, item);
        self.now += 0.1;
        let outcome = self.controller.pointer_move(target, self.now);
        let mut asked = self.act(outcome);
        self.now += 2.0;
        let outcome = self.controller.tick(self.now);
        asked.extend(self.act(outcome));
        asked
    }

    fn row(&self, level: usize, item: i32) -> Point {
        let layout = &self.controller.levels()[level].layout;
        let index = layout
            .index_of(ItemId(item))
            .unwrap_or_else(|| panic!("the open menu displays no row {item}"));
        centre(layout.rows[index].rect)
    }

    /// Release the held pointer over the row `item` of the open menu at `level`: the row flashes, then is chosen.
    fn choose(&mut self, level: usize, item: i32) -> Vec<Request> {
        let target = self.row(level, item);
        self.now += 0.1;
        let outcome = self.controller.pointer_move(target, self.now);
        let mut asked = self.act(outcome);
        self.now += 0.1;
        let outcome = self.controller.pointer_release(target, self.now);
        asked.extend(self.act(outcome));
        self.now += FLASH_OFF + FLASH_ON;
        let outcome = self.controller.tick(self.now);
        asked.extend(self.act(outcome));
        asked
    }
}

fn activate(id: &str, item: i32) -> Effect {
    Effect::Activate {
        slot: SlotKey::Status(id.into()),
        item: ItemId(item),
    }
}

fn clicked(source: &KdeAppMenu, item: i32) -> Request {
    Request::ActivateDbusMenu {
        menu: source.clone(),
        item,
    }
}

macro_rules! bus_or_skip {
    () => {
        match PrivateBus::start() {
            Some(bus) => bus,
            None => {
                eprintln!("skipping: dbus-daemon is not available");
                return;
            }
        }
    };
}

#[test]
fn a_status_menu_replaced_while_it_is_open_shows_its_new_rows_and_a_click_sends_the_new_id() {
    let bus = bus_or_skip!();
    let mut bar = Bar::start(&bus, Waits::default());
    let app = App::start(&bus);
    let mut exported = app.export("/Menu", 5, vec![row(3, "Open"), row(4, "Quit")]);
    let id = app.register_item("/Menu");
    let source = exported.source();
    bar.until("the item's menu arrives", |bar| {
        bar.rows(&id) == Some(vec![3, 4])
    });
    assert_eq!(bar.status_menu(&id).unwrap().label, "Chat");
    // told before its first read, as an exporter that fills its menu only then expects
    exported.hears(Heard::Read(0));
    assert_eq!(exported.heard(), [Heard::Shown(0), Heard::Read(0)]);

    // the menu opens with the rows read, and its exporter is told
    let asked = bar.press(&id);
    assert_eq!(
        asked,
        [Request::AboutToShow {
            menu: source.clone(),
            item: 0
        }]
    );
    assert_eq!(bar.displayed(0), [3, 4]);
    exported.hears(Heard::Shown(0));

    // the application replaces its menu under the open one: the same rows under other ids
    exported.replace(18, vec![row(137, "Open"), row(139, "Quit")]);
    bar.until("the replaced menu arrives", |bar| {
        bar.rows(&id) == Some(vec![137, 139])
    });
    assert!(bar.controller.is_open(), "the menu stays open");
    assert_eq!(bar.displayed(0), [137, 139], "and displays the new rows");
    assert_eq!(
        bar.resolve(activate(&id, 3)),
        None,
        "a choice of a row that is gone does nothing"
    );

    assert_eq!(bar.choose(0, 137), [clicked(&source, 137)]);
    exported.hears(Heard::Clicked(137));
    assert!(!bar.controller.is_open());

    // and again from a closed menu
    bar.press(&id);
    assert_eq!(bar.displayed(0), [137, 139]);
    assert_eq!(bar.choose(0, 139), [clicked(&source, 139)]);
    exported.hears(Heard::Clicked(139));
    let stale: Vec<_> = exported
        .heard()
        .iter()
        .filter(|h| matches!(h, Heard::Clicked(3 | 4)))
        .collect();
    assert_eq!(stale, Vec::<&Heard>::new(), "no id of the replaced menu");
}

#[test]
fn a_status_menu_takes_its_exporters_property_changes() {
    let bus = bus_or_skip!();
    let mut bar = Bar::start(&bus, Waits::default());
    let app = App::start(&bus);
    let exported = app.export("/Menu", 1, vec![row(3, "Open"), row(4, "Mute")]);
    let id = app.register_item("/Menu");
    bar.until("the item's menu arrives", |bar| {
        bar.rows(&id) == Some(vec![3, 4])
    });
    let shown = |bar: &Bar, item: i32| {
        let menu = bar.status_menu(&id).unwrap();
        let row = menu.find(ItemId(item)).unwrap();
        (row.label.clone(), row.enabled, row.visible, row.toggle)
    };

    exported.signal_properties(
        vec![
            (
                3,
                props(vec![
                    ("label", "Open _Window".into()),
                    ("enabled", false.into()),
                ]),
            ),
            (
                4,
                props(vec![
                    ("toggle-type", "checkmark".into()),
                    ("toggle-state", 1i32.into()),
                    ("visible", false.into()),
                ]),
            ),
        ],
        Vec::new(),
    );
    bar.until("the changed properties arrive", |bar| {
        shown(bar, 3).0 == "Open Window"
    });
    assert_eq!(
        shown(&bar, 3),
        ("Open Window".into(), false, true, Toggle::None)
    );
    assert_eq!(
        shown(&bar, 4),
        ("Mute".into(), true, false, Toggle::Check(true))
    );
    // the open menu displays the row that shows, and a disabled row is not sent
    bar.press(&id);
    assert_eq!(bar.displayed(0), [3]);
    assert_eq!(bar.resolve(activate(&id, 3)), None);

    // removed properties return to their defaults
    exported.signal_properties(
        Vec::new(),
        vec![
            (3, vec!["enabled".to_string()]),
            (4, vec!["visible".to_string(), "toggle-state".to_string()]),
        ],
    );
    bar.until("the removed properties arrive", |bar| shown(bar, 3).1);
    assert_eq!(shown(&bar, 4), ("Mute".into(), true, true, Toggle::None));
    assert_eq!(bar.displayed(0), [3, 4]);
}

#[test]
fn a_status_menu_and_its_submenu_are_read_again_when_their_exporter_asks_as_they_open() {
    let bus = bus_or_skip!();
    let mut bar = Bar::start(&bus, Waits::default());
    let app = App::start(&bus);
    let rows = || vec![row(3, "Open"), submenu(5, "Recent", Vec::new())];
    let mut exported = app.export("/Menu", 1, rows());
    let id = app.register_item("/Menu");
    let source = exported.source();
    bar.until("the item's menu arrives", |bar| {
        bar.rows(&id) == Some(vec![3, 5])
    });
    // the application fills its menus only when they are about to show, and signals nothing
    exported.state.lock().unwrap().on_show = HashMap::from([
        (0, [rows(), vec![row(6, "New")]].concat()),
        (5, vec![row(51, "First"), row(52, "Second")]),
    ]);

    bar.press(&id);
    bar.until("the menu filled as it opened arrives", |bar| {
        bar.rows(&id) == Some(vec![3, 5, 6])
    });
    assert_eq!(bar.displayed(0), [3, 5, 6]);

    let asked = bar.rest_on(0, 5);
    assert_eq!(
        asked,
        [Request::AboutToShow {
            menu: source.clone(),
            item: 5
        }]
    );
    bar.until("the submenu filled as it opened arrives", |bar| {
        bar.displayed(1) == [51, 52]
    });
    exported.hears(Heard::Read(5));
    assert_eq!(
        exported
            .heard()
            .iter()
            .filter(|h| **h == Heard::Read(0))
            .count(),
        2,
        "the first read and the one the root asked for: the submenu is read by itself"
    );
    assert_eq!(bar.choose(1, 52), [clicked(&source, 52)]);
    exported.hears(Heard::Clicked(52));
}

#[test]
fn an_exporter_that_never_answers_about_to_show_delays_neither_the_menu_nor_its_changes_for_long() {
    let bus = bus_or_skip!();
    let waits = Waits {
        about_to_show: Duration::from_millis(150),
        ..Waits::default()
    };
    let mut bar = Bar::start(&bus, waits);
    let app = App::start(&bus);
    let mut exported = app.export("/Menu", 1, vec![row(3, "Open"), row(4, "Quit")]);
    exported.state.lock().unwrap().silent = true;
    let id = app.register_item("/Menu");
    let source = exported.source();
    // the first read comes once the wait for the unanswered call is over
    bar.until("the item's menu arrives", |bar| {
        bar.rows(&id) == Some(vec![3, 4])
    });

    // the menu is open at once, and a row chosen at once reaches the exporter while the call is unanswered
    bar.press(&id);
    assert_eq!(bar.displayed(0), [3, 4]);
    assert_eq!(bar.choose(0, 3), [clicked(&source, 3)]);
    exported.hears(Heard::Clicked(3));

    // a change signalled meanwhile is read once the wait is over
    bar.press(&id);
    exported.replace(2, vec![row(137, "Open")]);
    bar.until("the replaced menu arrives", |bar| {
        bar.rows(&id) == Some(vec![137])
    });
    assert_eq!(bar.displayed(0), [137]);
    assert_eq!(bar.choose(0, 137), [clicked(&source, 137)]);
    exported.hears(Heard::Clicked(137));
}

#[test]
fn signals_that_come_in_a_burst_while_a_layout_is_read_do_not_hold_up_its_reply() {
    let bus = bus_or_skip!();
    let mut bar = Bar::start(&bus, Waits::default());
    let app = App::start(&bus);
    let exported = app.export("/Menu", 1, vec![row(3, "Open"), row(4, "Quit")]);
    let id = app.register_item("/Menu");
    bar.until("the item's menu arrives", |bar| {
        bar.rows(&id) == Some(vec![3, 4])
    });
    // more signals than a connection queues for one listener, all sent before the layout the bar asked for
    let mut burst = vec!["Busy".to_string(); 199];
    burst.push("Ready".into());
    exported.state.lock().unwrap().burst = burst;
    exported.replace(2, vec![row(3, "Open"), row(5, "Close")]);
    bar.until(
        "the layout and the labels signalled before it arrive",
        |bar| {
            bar.rows(&id) == Some(vec![3, 5])
                && bar.status_menu(&id).unwrap().children[0].label == "Ready"
        },
    );
}

#[test]
fn a_status_item_whose_menu_appears_later_or_moves_shows_the_menu_it_names() {
    let bus = bus_or_skip!();
    let mut bar = Bar::start(&bus, Waits::default());
    let app = App::start(&bus);
    let id = app.register_item("/");
    bar.until("the item arrives", |bar| bar.shared.status.len() == 1);
    assert_eq!(bar.rows(&id), None, "no menu yet");

    // its menu appears
    let mut first = app.export("/Menu", 9, vec![row(3, "Open")]);
    app.move_menu("/Menu");
    bar.until("the menu arrives", |bar| bar.rows(&id) == Some(vec![3]));

    // the item names another object, whose menu the application has not ready yet
    let mut second = app.export("/Second", 1, vec![row(21, "Open"), row(22, "Quit")]);
    second.state.lock().unwrap().unready = 2;
    app.move_menu("/Second");
    bar.until("the item is read again", |bar| {
        bar.shared.status[0].item.menu_path.as_deref() == Some("/Second")
    });
    assert_eq!(bar.rows(&id), None, "the first menu's rows are gone");
    assert_eq!(bar.resolve(activate(&id, 3)), None);
    bar.until("the second menu arrives", |bar| {
        bar.rows(&id) == Some(vec![21, 22])
    });
    let reads = second.heard().iter().filter(|h| **h == Heard::Read(0));
    assert_eq!(reads.count(), 3, "read again until the menu was ready");

    bar.press(&id);
    assert_eq!(bar.choose(0, 21), [clicked(&second.source(), 21)]);
    second.hears(Heard::Clicked(21));
    assert!(!first.heard().contains(&Heard::Clicked(21)));

    // and the item loses its menu
    app.move_menu("/");
    bar.until("the item is read again", |bar| {
        bar.shared.status[0].item.menu_path.is_none()
    });
    assert_eq!(bar.rows(&id), None);

    // the item goes with its application
    drop(app);
    drop(first);
    drop(second);
    bar.until("the item is removed", |bar| bar.shared.status.is_empty());
}

#[test]
fn an_applications_menus_follow_their_exporter_the_same_way() {
    let bus = bus_or_skip!();
    let (worker, events) = Worker::start(block_on(bus.connect_async()), || {}).unwrap();
    let app = App::start(&bus);
    let file = |rows: Vec<Row>| vec![submenu(1, "File", rows)];
    let mut exported = app.export("/MenuBar", 4, file(vec![row(2, "New")]));
    let source = exported.source();
    let next_tree = |what: &str| loop {
        match events
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("{what}"))
        {
            Event::Layout { source: from, root } => {
                assert_eq!(from, MenuSource::Kde(source.clone()));
                break *root;
            }
            Event::LayoutFailed { .. } => panic!("{what}: the menus could not be read"),
            _ => {}
        }
    };
    let rows = |root: &MenuItem| -> Vec<(i32, String)> {
        let file = &root.children[0];
        let rows = file.children.iter().map(|r| (r.id.0, r.label.clone()));
        rows.collect()
    };

    worker.send(Request::FetchDbusMenu(source.clone())).unwrap();
    assert_eq!(rows(&next_tree("the menus arrive")), [(2, "New".into())]);
    assert_eq!(
        exported.heard(),
        [Heard::Read(0)],
        "an application's menus are not told that they show before the bar shows them"
    );

    // the application replaces its menus
    exported.replace(5, file(vec![row(12, "New"), row(13, "Close")]));
    assert_eq!(
        rows(&next_tree("the replaced menus arrive")),
        [(12, "New".into()), (13, "Close".into())]
    );

    // a property changes
    exported.signal_properties(
        vec![(13, props(vec![("label", "Close _All".into())]))],
        Vec::new(),
    );
    assert_eq!(
        rows(&next_tree("the changed label arrives"))[1],
        (13, "Close All".into())
    );

    // a menu the application fills as it opens
    exported
        .state
        .lock()
        .unwrap()
        .on_show
        .insert(1, vec![row(31, "Reopen")]);
    worker
        .send(Request::AboutToShow {
            menu: source.clone(),
            item: 1,
        })
        .unwrap();
    assert_eq!(
        rows(&next_tree("the menu filled as it opened arrives")),
        [(31, "Reopen".into())]
    );
    exported.hears(Heard::Read(1));

    // released and asked for again, the menus are read afresh
    worker
        .send(Request::ReleaseDbusMenu(source.clone()))
        .unwrap();
    worker.send(Request::FetchDbusMenu(source.clone())).unwrap();
    assert_eq!(
        rows(&next_tree("the menus arrive again")),
        [(31, "Reopen".into())]
    );
    exported.hears(Heard::Read(0));
    worker
        .send(Request::ActivateDbusMenu {
            menu: source.clone(),
            item: 31,
        })
        .unwrap();
    exported.hears(Heard::Clicked(31));
}
