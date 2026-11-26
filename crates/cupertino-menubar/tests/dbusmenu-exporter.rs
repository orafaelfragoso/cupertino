//! The DBusMenu client against fake exporters on a private session bus: the menu's own properties laid on the tree
//! and followed as they change, `AboutToShowGroup` and `EventGroup` for commands that come together and the single
//! calls for an exporter without them, `opened` / `closed` / `clicked` with their times, `closed` for the menus still
//! open when the bar lets the exporter go, a `LayoutUpdated` below the root reading only that part, and
//! `ItemActivationRequested` reported to the bars.

// a test owns its thread: it blocks on the bus and looks up the daemon's binary
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use async_io::Timer;
use cupertino_menubar::dbus::dbusmenu::{make_node, MenuEvent, RawNode};
use cupertino_menubar::item::{ItemId, MenuItem};
use cupertino_menubar::source::{KdeAppMenu, MenuSource};
use cupertino_menubar::worker::{Event, Request, Worker};
use futures_util::future::{select, Either};
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};
use zbus::Connection;

const PATH: &str = "/com/canonical/menu/7";
/// The longest an answer may take to arrive.
const ARRIVAL: Duration = Duration::from_secs(10);
/// How long the exporter takes over a single call, so the commands that follow wait and go together.
const SLOW: Duration = Duration::from_millis(200);
/// The input clock's time of the events sent, in milliseconds.
const TIME: u32 = 81_000;

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

/// What an exporter was asked.
#[derive(Clone, Debug, PartialEq)]
enum Call {
    GetLayout(i32),
    AboutToShow(i32),
    AboutToShowGroup(Vec<i32>),
    Event(i32, String, u32),
    EventGroup(Vec<(i32, String, u32)>),
}

fn label(text: &str) -> HashMap<String, OwnedValue> {
    HashMap::from([(
        "label".to_owned(),
        OwnedValue::try_from(Value::from(text)).unwrap(),
    )])
}

fn node(id: i32, text: &str, children: Vec<RawNode>) -> RawNode {
    make_node(id, label(text), children).unwrap()
}

/// File (1): New (2), Open Recent (3: notes (4)); Edit (10): Undo (11). `parent` is the part asked for.
fn tree(parent: i32) -> RawNode {
    let recent = node(3, "Open Recent", vec![node(4, "notes", Vec::new())]);
    let file = node(1, "File", vec![node(2, "New", Vec::new()), recent.clone()]);
    let edit = node(10, "Edit", vec![node(11, "Undo", Vec::new())]);
    match parent {
        3 => recent,
        1 => file,
        10 => edit,
        _ => make_node(0, HashMap::new(), vec![file, edit]).unwrap(),
    }
}

/// An exporter of the current version, with the group calls and the menu's properties.
struct Grouped {
    calls: async_channel::Sender<Call>,
    status: String,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Grouped {
    fn get_layout(&self, parent: i32, _depth: i32, _names: Vec<String>) -> (u32, RawNode) {
        let _ = self.calls.try_send(Call::GetLayout(parent));
        (1, tree(parent))
    }

    async fn about_to_show(&self, id: i32) -> bool {
        let _ = self.calls.try_send(Call::AboutToShow(id));
        Timer::after(SLOW).await;
        false
    }

    fn about_to_show_group(&self, ids: Vec<i32>) -> (Vec<i32>, Vec<i32>) {
        let _ = self.calls.try_send(Call::AboutToShowGroup(ids.clone()));
        // Open Recent changed
        (ids.into_iter().filter(|id| *id == 3).collect(), Vec::new())
    }

    async fn event(&self, id: i32, event: String, _data: OwnedValue, time: u32) {
        let _ = self.calls.try_send(Call::Event(id, event, time));
        Timer::after(SLOW).await;
    }

    fn event_group(&self, events: Vec<(i32, String, OwnedValue, u32)>) -> Vec<i32> {
        let events = events.into_iter().map(|(id, e, _, t)| (id, e, t)).collect();
        let _ = self.calls.try_send(Call::EventGroup(events));
        Vec::new()
    }

    #[zbus(signal)]
    async fn layout_updated(
        emitter: &SignalEmitter<'_>,
        revision: u32,
        parent: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn item_activation_requested(
        emitter: &SignalEmitter<'_>,
        id: i32,
        timestamp: u32,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> u32 {
        4
    }

    #[zbus(property)]
    fn status(&self) -> String {
        self.status.clone()
    }

    #[zbus(property)]
    fn text_direction(&self) -> String {
        "rtl".into()
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> Vec<String> {
        vec!["/opt/notes/icons".into()]
    }
}

/// An exporter of an earlier version: no group calls, no properties.
struct Plain {
    calls: async_channel::Sender<Call>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Plain {
    fn get_layout(&self, parent: i32, _depth: i32, _names: Vec<String>) -> (u32, RawNode) {
        let _ = self.calls.try_send(Call::GetLayout(parent));
        (1, tree(parent))
    }

    async fn about_to_show(&self, id: i32) -> bool {
        let _ = self.calls.try_send(Call::AboutToShow(id));
        Timer::after(SLOW).await;
        false
    }

    async fn event(&self, id: i32, event: String, _data: OwnedValue, time: u32) {
        let _ = self.calls.try_send(Call::Event(id, event, time));
        Timer::after(SLOW).await;
    }
}

async fn connect(address: &str) -> Connection {
    Builder::address(address).unwrap().build().await.unwrap()
}

/// What comes on `rx` within [`ARRIVAL`], panicking without it.
async fn within<T>(rx: &async_channel::Receiver<T>, what: &str) -> T {
    match select(Box::pin(rx.recv()), Timer::after(ARRIVAL)).await {
        Either::Left((Ok(value), _)) => value,
        Either::Left((Err(_), _)) => panic!("the channel closed waiting for {what}"),
        Either::Right(_) => panic!("no {what} within {ARRIVAL:?}"),
    }
}

/// The calls up to and with the first one `done` accepts.
async fn calls_until(
    calls: &async_channel::Receiver<Call>,
    what: &str,
    done: impl Fn(&Call) -> bool,
) -> Vec<Call> {
    let mut seen = Vec::new();
    loop {
        let call = within(calls, what).await;
        let last = done(&call);
        seen.push(call);
        if last {
            return seen;
        }
    }
}

/// The next tree the worker reports.
async fn next_tree(events: &async_channel::Receiver<Event>) -> MenuItem {
    loop {
        match within(events, "a tree").await {
            Event::Layout { root, .. } => return *root,
            Event::LayoutFailed { source } => panic!("the menu was given up: {source:?}"),
            _ => {}
        }
    }
}

/// The events of `calls`, single or grouped, in order.
fn sent_events(calls: &[Call]) -> Vec<(i32, String, u32)> {
    calls
        .iter()
        .flat_map(|c| match c {
            Call::Event(id, e, t) => vec![(*id, e.clone(), *t)],
            Call::EventGroup(group) => group.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn event(menu: &KdeAppMenu, item: i32, event: MenuEvent, time: u32) -> Request {
    Request::MenuEvent {
        menu: menu.clone(),
        item,
        event,
        time,
    }
}

#[test]
fn a_current_exporter_gets_group_calls_and_its_menus_properties_show() {
    let (_bus, address) = session_bus();
    async_io::block_on(async {
        let (bar, app) = (connect(&address).await, connect(&address).await);
        let (tx, calls) = async_channel::unbounded();
        let exporter = Grouped {
            calls: tx,
            status: "notice".into(),
        };
        app.object_server().at(PATH, exporter).await.unwrap();
        let menu = KdeAppMenu {
            service: app.unique_name().unwrap().to_string(),
            path: PATH.into(),
        };
        let (worker, events) = Worker::start(bar).await.unwrap();
        worker.send(Request::FetchDbusMenu(menu.clone())).unwrap();
        let root = next_tree(&events).await;
        assert!(root.notice, "Status notice");
        let notes = root.find(ItemId(4)).unwrap();
        assert!(notes.rtl, "TextDirection rtl");
        assert_eq!(notes.icon_dirs.len(), 1);

        // the menus about to show while the exporter answers the first are told together
        for item in [1, 3, 10] {
            worker
                .send(Request::AboutToShow {
                    menu: menu.clone(),
                    item,
                })
                .unwrap();
        }
        let seen = calls_until(&calls, "Open Recent read", |c| *c == Call::GetLayout(3)).await;
        let group = seen.iter().find_map(|c| match c {
            Call::AboutToShowGroup(ids) => Some(ids.clone()),
            _ => None,
        });
        let group = group.unwrap_or_else(|| panic!("no AboutToShowGroup: {seen:?}"));
        assert!(group.contains(&3) && group.contains(&10), "{seen:?}");

        // events that come together go as one EventGroup, with their times
        worker
            .send(event(&menu, 1, MenuEvent::Opened, TIME))
            .unwrap();
        worker
            .send(event(&menu, 3, MenuEvent::Opened, TIME + 10))
            .unwrap();
        worker
            .send(event(&menu, 3, MenuEvent::Closed, TIME + 20))
            .unwrap();
        let seen = calls_until(&calls, "the events", |c| {
            sent_events(std::slice::from_ref(c)).contains(&(3, "closed".into(), TIME + 20))
        })
        .await;
        assert_eq!(
            sent_events(&seen),
            [
                (1, "opened".to_owned(), TIME),
                (3, "opened".to_owned(), TIME + 10),
                (3, "closed".to_owned(), TIME + 20),
            ]
        );
        assert!(
            seen.iter().any(|c| matches!(c, Call::EventGroup(_))),
            "{seen:?}"
        );

        // a choice is clicked at its time
        worker
            .send(Request::ActivateDbusMenu {
                menu: menu.clone(),
                item: 2,
                time: TIME + 30,
            })
            .unwrap();
        let seen = calls_until(&calls, "the click", |c| matches!(c, Call::Event(2, ..))).await;
        assert_eq!(
            seen.last(),
            Some(&Call::Event(2, "clicked".into(), TIME + 30))
        );

        let iface = app
            .object_server()
            .interface::<_, Grouped>(PATH)
            .await
            .unwrap();
        // a layout changed below the root: only that part is read
        Grouped::layout_updated(iface.signal_emitter(), 2, 10)
            .await
            .unwrap();
        let seen = calls_until(&calls, "Edit read", |c| matches!(c, Call::GetLayout(_))).await;
        assert_eq!(seen.last(), Some(&Call::GetLayout(10)));

        // the status changes back
        iface.get_mut().await.status = "normal".into();
        iface
            .get()
            .await
            .status_changed(iface.signal_emitter())
            .await
            .unwrap();
        // trees of the reads before may come first
        let mut root = next_tree(&events).await;
        while root.notice {
            root = next_tree(&events).await;
        }
        assert!(!root.notice);

        // the exporter asks for Open Recent to be opened
        Grouped::item_activation_requested(iface.signal_emitter(), 3, 5)
            .await
            .unwrap();
        loop {
            if let Event::ActivationRequested { source, item } =
                within(&events, "the request").await
            {
                assert_eq!(source, MenuSource::Kde(menu.clone()));
                assert_eq!(item, 3);
                break;
            }
        }

        // the menus still open when the bar lets the exporter go are told they closed
        worker
            .send(event(&menu, 10, MenuEvent::Opened, TIME + 40))
            .unwrap();
        calls_until(&calls, "Edit opened", |c| {
            sent_events(std::slice::from_ref(c)).contains(&(10, "opened".into(), TIME + 40))
        })
        .await;
        worker.send(Request::ReleaseDbusMenu(menu.clone())).unwrap();
        let seen = calls_until(&calls, "Edit closed", |c| {
            !sent_events(std::slice::from_ref(c)).is_empty()
        })
        .await;
        // File, opened above and never closed, too: the latest opened first
        assert_eq!(
            sent_events(&seen),
            [
                (10, "closed".to_owned(), TIME + 40),
                (1, "closed".to_owned(), TIME + 40)
            ]
        );
    });
}

#[test]
fn an_exporter_without_group_calls_gets_the_single_ones() {
    let (_bus, address) = session_bus();
    async_io::block_on(async {
        let (bar, app) = (connect(&address).await, connect(&address).await);
        let (tx, calls) = async_channel::unbounded();
        app.object_server()
            .at(PATH, Plain { calls: tx })
            .await
            .unwrap();
        let menu = KdeAppMenu {
            service: app.unique_name().unwrap().to_string(),
            path: PATH.into(),
        };
        let (worker, events) = Worker::start(bar).await.unwrap();
        worker.send(Request::FetchDbusMenu(menu.clone())).unwrap();
        let root = next_tree(&events).await;
        assert!(!root.notice && !root.find(ItemId(4)).unwrap().rtl);
        for item in [1, 3, 10] {
            worker
                .send(Request::AboutToShow {
                    menu: menu.clone(),
                    item,
                })
                .unwrap();
        }
        let seen = calls_until(&calls, "Edit told", |c| *c == Call::AboutToShow(10)).await;
        let told: Vec<&Call> = seen
            .iter()
            .filter(|c| matches!(c, Call::AboutToShow(_)))
            .collect();
        assert_eq!(
            told,
            [
                &Call::AboutToShow(1),
                &Call::AboutToShow(3),
                &Call::AboutToShow(10)
            ]
        );
        worker
            .send(event(&menu, 1, MenuEvent::Opened, TIME))
            .unwrap();
        worker
            .send(event(&menu, 3, MenuEvent::Opened, TIME + 1))
            .unwrap();
        worker
            .send(event(&menu, 3, MenuEvent::Closed, TIME + 2))
            .unwrap();
        let seen = calls_until(&calls, "the events", |c| {
            *c == Call::Event(3, "closed".into(), TIME + 2)
        })
        .await;
        assert_eq!(
            sent_events(&seen),
            [
                (1, "opened".to_owned(), TIME),
                (3, "opened".to_owned(), TIME + 1),
                (3, "closed".to_owned(), TIME + 2),
            ]
        );
    });
}

#[test]
fn a_menu_the_exporter_does_not_hold_is_not_told_it_closed() {
    let (_bus, address) = session_bus();
    async_io::block_on(async {
        let (bar, app) = (connect(&address).await, connect(&address).await);
        let (tx, calls) = async_channel::unbounded();
        app.object_server()
            .at(PATH, Plain { calls: tx })
            .await
            .unwrap();
        let menu = KdeAppMenu {
            service: app.unique_name().unwrap().to_string(),
            path: PATH.into(),
        };
        let (worker, events) = Worker::start(bar).await.unwrap();
        worker.send(Request::FetchDbusMenu(menu.clone())).unwrap();
        next_tree(&events).await;
        // another application's menu id, closed after the bar changed applications
        worker
            .send(event(&menu, 77, MenuEvent::Closed, TIME))
            .unwrap();
        worker
            .send(event(&menu, 1, MenuEvent::Closed, TIME + 1))
            .unwrap();
        let seen = calls_until(&calls, "File closed", |c| matches!(c, Call::Event(..))).await;
        assert_eq!(sent_events(&seen), [(1, "closed".to_owned(), TIME + 1)]);
    });
}

#[test]
fn an_exporter_asking_again_and_again_opens_its_menus_once_a_batch() {
    let (_bus, address) = session_bus();
    async_io::block_on(async {
        let (bar, app) = (connect(&address).await, connect(&address).await);
        let (tx, _calls) = async_channel::unbounded();
        let exporter = Grouped {
            calls: tx,
            status: "normal".into(),
        };
        app.object_server().at(PATH, exporter).await.unwrap();
        let menu = KdeAppMenu {
            service: app.unique_name().unwrap().to_string(),
            path: PATH.into(),
        };
        let (worker, events) = Worker::start(bar).await.unwrap();
        worker.send(Request::FetchDbusMenu(menu.clone())).unwrap();
        next_tree(&events).await;

        let iface = app
            .object_server()
            .interface::<_, Grouped>(PATH)
            .await
            .unwrap();
        for id in [1, 3, 10] {
            Grouped::item_activation_requested(iface.signal_emitter(), id, 5)
                .await
                .unwrap();
        }
        let asked = |event: Event| match event {
            Event::ActivationRequested { item, .. } => Some(item),
            _ => None,
        };
        let first = loop {
            if let Some(item) = asked(within(&events, "the request").await) {
                break item;
            }
        };
        assert_eq!(first, 10, "the latest request of the batch");
        let deadline = Timer::after(SLOW);
        let more = select(Box::pin(events.recv()), deadline).await;
        if let Either::Left((Ok(event), _)) = more {
            assert_eq!(asked(event), None, "one request a batch");
        }
    });
}
