//! Menus a program registers with `com.canonical.AppMenu.Registrar` instead of announcing them over Wayland, as
//! Electron does, read by the bar's worker on a private session bus (`dbus-daemon --session`): an X11 program's window
//! shown by a stand-in for the X11 bridge, and a window on Wayland matched by its process. Skipped when the daemon is
//! not available.
#![cfg(feature = "app")]

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use cupertino_menubar::dbus::dbusmenu::{make_node, RawNode};
use cupertino_menubar::desktop::AppName;
use cupertino_menubar::feed::{BarFeed, Shared};
use cupertino_menubar::item::{ItemId, MenuItem, Toggle};
use cupertino_menubar::registered::{Registration, X11Title, X11Windows};
use cupertino_menubar::source::{KdeAppMenu, MenuSource};
use cupertino_menubar::worker::{Event, Request, Worker};
use cupertino_shell_client::{
    ActiveApp, ActiveClient, AppWindow, MenuBarUpdate, WindowMenu, WindowState,
};
use cupertino_test_bus::PrivateBus;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{block_on, fdo, Connection};

const WAIT: Duration = Duration::from_secs(10);
const REGISTRAR: &str = "com.canonical.AppMenu.Registrar";
const REGISTRAR_PATH: &str = "/com/canonical/AppMenu/Registrar";
/// The X11 window of the stand-in program, and where it exports its menus.
const X11_WINDOW: u32 = 0x40_0004;
const MENU_PATH: &str = "/com/canonical/menu/400004";
/// The submenu the program fills only when it is about to show.
const RECENT: i32 = 6;

fn props(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap()))
        .collect()
}

/// File: New Window, Word Wrap (checked), Light and Dark (radio, Dark selected), Open Recent (filled on open).
fn layout(recent_filled: bool) -> RawNode {
    let item = |id, pairs| make_node(id, props(pairs), vec![]).unwrap();
    let recent_items = if recent_filled {
        vec![item(7, vec![("label", "notes.txt".into())])]
    } else {
        Vec::new()
    };
    let recent = make_node(
        RECENT,
        props(vec![
            ("label", "Open Recent".into()),
            ("children-display", "submenu".into()),
        ]),
        recent_items,
    )
    .unwrap();
    let file = make_node(
        1,
        props(vec![
            ("label", "&File".into()),
            ("children-display", "submenu".into()),
        ]),
        vec![
            item(
                2,
                vec![
                    ("label", "New Window".into()),
                    ("shortcut", vec![vec!["Control", "Shift", "N"]].into()),
                ],
            ),
            item(
                3,
                vec![
                    ("label", "Word Wrap".into()),
                    ("toggle-type", "checkmark".into()),
                    ("toggle-state", 1i32.into()),
                ],
            ),
            item(
                4,
                vec![
                    ("label", "Light".into()),
                    ("toggle-type", "radio".into()),
                    ("toggle-state", 0i32.into()),
                ],
            ),
            item(
                5,
                vec![
                    ("label", "Dark".into()),
                    ("toggle-type", "radio".into()),
                    ("toggle-state", 1i32.into()),
                ],
            ),
            recent,
        ],
    )
    .unwrap();
    make_node(
        0,
        props(vec![("children-display", "submenu".into())]),
        vec![file],
    )
    .unwrap()
}

#[derive(Default)]
struct Heard {
    clicked: Vec<i32>,
    about_to_show: Vec<i32>,
    recent_filled: bool,
}

/// The program's DBusMenu tree.
struct Program(Arc<Mutex<Heard>>);

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Program {
    fn get_layout(
        &self,
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> fdo::Result<(u32, RawNode)> {
        Ok((1, layout(self.0.lock().unwrap().recent_filled)))
    }

    fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
        if event_id == "clicked" {
            self.0.lock().unwrap().clicked.push(id);
        }
    }

    fn about_to_show(&self, id: i32) -> bool {
        let mut heard = self.0.lock().unwrap();
        heard.about_to_show.push(id);
        let fills = id == RECENT && !heard.recent_filled;
        heard.recent_filled |= id == RECENT;
        fills
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
}

/// The X server of a stand-in X11 bridge: the windows it has, by title.
#[derive(Clone, Default)]
struct FakeBridge(Arc<Mutex<HashMap<u32, String>>>);

impl X11Windows for FakeBridge {
    fn title(&mut self, _bridge: u32, window: u32) -> X11Title {
        self.0
            .lock()
            .unwrap()
            .get(&window)
            .map_or(X11Title::Gone, |t| X11Title::Title(t.clone()))
    }

    fn focused(&mut self, _bridge: u32) -> Option<u32> {
        None
    }
}

/// The bar's worker and what it reports.
struct Bar {
    worker: Worker,
    events: mpsc::Receiver<Event>,
    shared: Shared,
    feed: BarFeed,
}

impl Bar {
    fn start(bus: &PrivateBus) -> Bar {
        let (worker, events) = Worker::start(block_on(bus.connect_async()), || {}).unwrap();
        worker.send(Request::AppMenuRegistrar(true)).unwrap();
        Bar {
            worker,
            events,
            shared: Shared::default(),
            feed: BarFeed::new(1, true, true),
        }
    }

    /// Apply the worker's events until `done` holds of one; the matching event is returned.
    fn until(&mut self, mut done: impl FnMut(&Event) -> bool) -> Event {
        loop {
            let event = self.events.recv_timeout(WAIT).expect("the worker reports");
            match &event {
                Event::Registrations(list) => {
                    self.shared.registrations = list.clone();
                    self.feed.match_registered(&mut self.shared);
                    let gone = self.shared.take_gone();
                    if !gone.is_empty() {
                        self.worker
                            .send(Request::ForgetRegistrations(gone))
                            .unwrap();
                    }
                }
                Event::Layout { source, root } => self.shared.apply_layout(source, root.clone()),
                _ => {}
            }
            if done(&event) {
                return event;
            }
        }
    }

    fn registrations(&mut self, count: usize) -> Vec<Registration> {
        match self.until(|e| matches!(e, Event::Registrations(l) if l.len() == count)) {
            Event::Registrations(list) => list,
            _ => unreachable!(),
        }
    }

    /// The server's group for an active window that announced no menu.
    fn focus(&mut self, title: &str, client: ActiveClient) {
        let update = MenuBarUpdate {
            active_app: Some(ActiveApp {
                app_id: "org.example.notes".into(),
                title: title.into(),
            }),
            menu: Some(WindowMenu::None),
            window_state: Some(WindowState {
                can_minimize: true,
                can_zoom: true,
                full_screen: false,
                workspace: 1,
                workspaces: 1,
            }),
            windows: Some(vec![AppWindow {
                id: 4,
                title: title.into(),
                active: true,
                minimized: false,
            }]),
            active_client: Some(client),
            ..MenuBarUpdate::default()
        };
        self.feed.apply(&update, &mut self.shared, |_| AppName {
            name: "Notes".into(),
            has_entry: true,
        });
    }

    /// Load the active window's menus as the process does, and wait for them.
    fn load(&mut self) -> MenuItem {
        let request = self
            .feed
            .request_layout(&mut self.shared)
            .expect("the bar asks for the registered menus");
        let Request::FetchDbusMenu(menu) = &request else {
            panic!("a DBusMenu tree: {request:?}");
        };
        let menu = menu.clone();
        self.worker.send(request).unwrap();
        self.layout(&menu)
    }

    fn layout(&mut self, menu: &KdeAppMenu) -> MenuItem {
        let source = MenuSource::Kde(menu.clone());
        match self.until(|e| matches!(e, Event::Layout { source: s, .. } if *s == source)) {
            Event::Layout { root, .. } => root,
            _ => unreachable!(),
        }
    }

    /// The labels of the bar's File menu, once the program's menus are shown.
    fn file_menu(&self) -> Option<MenuItem> {
        let content = self.feed.content(&self.shared);
        content.menus.into_iter().find(|m| m.label == "File")
    }
}

fn register(conn: &Connection, window: u32, path: &'static str) {
    let body = (window, ObjectPath::from_static_str_unchecked(path));
    block_on(conn.call_method(
        Some(REGISTRAR),
        REGISTRAR_PATH,
        Some(REGISTRAR),
        "RegisterWindow",
        &body,
    ))
    .unwrap();
}

fn unregister(conn: &Connection, window: u32) {
    block_on(conn.call_method(
        Some(REGISTRAR),
        REGISTRAR_PATH,
        Some(REGISTRAR),
        "UnregisterWindow",
        &(window,),
    ))
    .unwrap();
}

fn menu_for(conn: &Connection, window: u32) -> (String, OwnedObjectPath) {
    block_on(conn.call_method(
        Some(REGISTRAR),
        REGISTRAR_PATH,
        Some(REGISTRAR),
        "GetMenuForWindow",
        &(window,),
    ))
    .unwrap()
    .body()
    .deserialize()
    .unwrap()
}

/// A connection serving the program's menus at [`MENU_PATH`].
fn program(bus: &PrivateBus, heard: &Arc<Mutex<Heard>>) -> Connection {
    block_on(async {
        bus.builder()
            .serve_at(MENU_PATH, Program(heard.clone()))
            .unwrap()
            .build()
            .await
            .unwrap()
    })
}

#[test]
fn registrar_shows_the_menus_an_x11_window_registered_and_drops_them_when_it_unregisters() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut bar = Bar::start(&bus);
    let bridge = FakeBridge::default();
    bridge
        .0
        .lock()
        .unwrap()
        .insert(X11_WINDOW, "notes.txt".into());
    bridge
        .0
        .lock()
        .unwrap()
        .insert(0x60_0002, "Terminal".into());
    bar.shared.x11 = Some(Box::new(bridge.clone()));

    let heard = Arc::new(Mutex::new(Heard::default()));
    let electron = program(&bus, &heard);
    let service = electron.unique_name().unwrap().to_string();
    register(&electron, X11_WINDOW, MENU_PATH);
    let list = bar.registrations(1);
    assert_eq!(
        (
            list[0].service.as_str(),
            list[0].path.as_str(),
            list[0].window,
            list[0].pid
        ),
        (service.as_str(), MENU_PATH, X11_WINDOW, std::process::id()),
        "the address is the caller's unique name and its path, with its process"
    );
    assert_eq!(
        menu_for(&electron, X11_WINDOW),
        (
            service.clone(),
            OwnedObjectPath::try_from(MENU_PATH).unwrap()
        )
    );

    // another X11 program's window, with no registration, keeps the bar's own menus
    bar.focus("Terminal", ActiveClient::X11(77));
    assert_eq!(bar.feed.source(&bar.shared), MenuSource::Synthetic);
    assert!(bar.feed.request_layout(&mut bar.shared).is_none());

    // the registered window: the bar reads its tree
    bar.focus("notes.txt", ActiveClient::X11(77));
    let menu = KdeAppMenu {
        service: service.clone(),
        path: MENU_PATH.into(),
    };
    assert_eq!(bar.feed.source(&bar.shared), MenuSource::Kde(menu.clone()));
    let root = bar.load();
    assert!(root.find(ItemId(RECENT)).unwrap().children.is_empty());
    let file = bar.file_menu().expect("the program's File menu");
    let toggle = |label: &str| {
        file.children
            .iter()
            .find(|i| i.label == label)
            .map(|i| i.toggle)
    };
    assert_eq!(toggle("Word Wrap"), Some(Toggle::Check(true)));
    assert_eq!(toggle("Light"), Some(Toggle::Radio(false)));
    assert_eq!(toggle("Dark"), Some(Toggle::Radio(true)));
    let new_window = file
        .children
        .iter()
        .find(|i| i.label == "New Window")
        .unwrap();
    assert_eq!(
        new_window.shortcut.as_ref().unwrap().display(),
        "Ctrl+Shift+N"
    );

    // a click reaches the program
    bar.worker
        .send(Request::ActivateDbusMenu {
            menu: menu.clone(),
            item: 2,
        })
        .unwrap();
    // a submenu filled only when it opens: the program hears it is about to show, then the tree is read again
    bar.worker
        .send(Request::AboutToShow {
            menu: menu.clone(),
            item: RECENT,
        })
        .unwrap();
    let root = bar.layout(&menu);
    let recent = root.find(ItemId(RECENT)).unwrap();
    assert_eq!(
        recent
            .children
            .iter()
            .map(|i| i.label.as_str())
            .collect::<Vec<_>>(),
        ["notes.txt"]
    );
    {
        let heard = heard.lock().unwrap();
        assert_eq!(heard.clicked, [2]);
        assert!(heard.about_to_show.contains(&RECENT));
    }

    // the program unregisters its window: the bar's own menus come back
    unregister(&electron, X11_WINDOW);
    bar.registrations(0);
    assert_eq!(bar.feed.source(&bar.shared), MenuSource::Synthetic);
    assert_eq!(menu_for(&electron, X11_WINDOW).0, "");
}

#[test]
fn registrar_forgets_a_program_that_leaves_the_bus_and_an_x11_window_that_is_gone() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut bar = Bar::start(&bus);
    let bridge = FakeBridge::default();
    bridge
        .0
        .lock()
        .unwrap()
        .insert(X11_WINDOW, "notes.txt".into());
    bar.shared.x11 = Some(Box::new(bridge.clone()));
    let heard = Arc::new(Mutex::new(Heard::default()));
    let other = block_on(bus.connect_async());

    // the program leaves the bus without unregistering
    let electron = program(&bus, &heard);
    register(&electron, X11_WINDOW, MENU_PATH);
    bar.registrations(1);
    bar.focus("notes.txt", ActiveClient::X11(77));
    assert!(bar.feed.source(&bar.shared).is_exported());
    drop(electron);
    bar.registrations(0);
    assert_eq!(bar.feed.source(&bar.shared), MenuSource::Synthetic);
    assert_eq!(menu_for(&other, X11_WINDOW).0, "");

    // the X11 window closes while the program stays: the bar finds it gone and the registrar forgets it
    let electron = program(&bus, &heard);
    register(&electron, X11_WINDOW, MENU_PATH);
    bar.registrations(1);
    bridge.0.lock().unwrap().remove(&X11_WINDOW);
    bar.focus("notes.txt", ActiveClient::X11(77));
    assert_eq!(bar.feed.source(&bar.shared), MenuSource::Synthetic);
    let gone = bar.shared.take_gone();
    assert_eq!(
        gone,
        [(electron.unique_name().unwrap().to_string(), X11_WINDOW)]
    );
    bar.worker.send(Request::ForgetRegistrations(gone)).unwrap();
    bar.registrations(0);
    assert_eq!(menu_for(&other, X11_WINDOW).0, "");
}

#[test]
fn registrar_matches_a_wayland_window_to_its_own_process() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut bar = Bar::start(&bus);
    let heard = Arc::new(Mutex::new(Heard::default()));
    // on Wayland the program names its window by a number of its own, which the X server never had
    let electron = program(&bus, &heard);
    register(&electron, 1, MENU_PATH);
    bar.registrations(1);

    bar.focus("Notes", ActiveClient::Wayland(std::process::id() + 1));
    assert_eq!(
        bar.feed.source(&bar.shared),
        MenuSource::Synthetic,
        "another process's window"
    );
    bar.focus("Notes", ActiveClient::Wayland(std::process::id()));
    assert_eq!(
        bar.feed.source(&bar.shared),
        MenuSource::Kde(KdeAppMenu {
            service: electron.unique_name().unwrap().to_string(),
            path: MENU_PATH.into(),
        })
    );
    bar.load();
    assert!(bar.file_menu().is_some(), "the program's menus show");

    // a window that announced its menus over Wayland keeps them
    let announced = MenuBarUpdate {
        menu: Some(WindowMenu::DbusMenu {
            service: ":1.99".into(),
            path: "/MenuBar/1".into(),
        }),
        ..MenuBarUpdate::default()
    };
    bar.feed.apply(&announced, &mut bar.shared, |_| AppName {
        name: "Notes".into(),
        has_entry: true,
    });
    register(&electron, 2, MENU_PATH);
    bar.registrations(2);
    assert_eq!(
        bar.feed.source(&bar.shared),
        MenuSource::Kde(KdeAppMenu {
            service: ":1.99".into(),
            path: "/MenuBar/1".into(),
        })
    );
}
