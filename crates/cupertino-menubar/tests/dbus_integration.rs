//! Integration tests against a private session bus (`dbus-daemon --session`); skipped when the
//! daemon is not available.
#![cfg(feature = "dbus")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cupertino_menubar::controller::{Effect, MenuController, FLASH_OFF, FLASH_ON};
use cupertino_menubar::dbus::dbusmenu::{
    apply_properties_update, make_node, DBusMenuClient, MenuUpdate, RawNode,
};
use cupertino_menubar::dbus::gtk::{GtkMenuClient, RawAction, RawChange, RawMenu};
use cupertino_menubar::dbus::sni::{self, ItemStatus, WatcherClient, WatcherEvent, WatcherService};
use cupertino_menubar::geom::{Point, Rect};
use cupertino_menubar::item::{ItemId, Toggle};
use cupertino_menubar::layout::{BarContent, MonoMeasure, SlotKey};
use cupertino_menubar::source::GtkMenuProps;
use futures_util::{Stream, StreamExt};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Signature, Value};
use zbus::{block_on, fdo};

use cupertino_test_bus::PrivateBus;

macro_rules! bus_or_skip {
    () => {
        match PrivateBus::start() {
            Some(b) => b,
            None => {
                eprintln!("skipping: dbus-daemon is not available");
                return;
            }
        }
    };
}

async fn next_within<S: Stream + Unpin>(s: &mut S, secs: f64) -> Option<S::Item> {
    use futures_util::future::{select, Either};
    let timer = async_io::Timer::after(Duration::from_secs_f64(secs));
    match select(Box::pin(s.next()), Box::pin(timer)).await {
        Either::Left((v, _)) => v,
        Either::Right(_) => None,
    }
}

fn props(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
    pairs
        .into_iter()
        .map(|(k, v)| (k.to_string(), OwnedValue::try_from(v).unwrap()))
        .collect()
}

fn sample_layout() -> RawNode {
    let new = make_node(
        2,
        props(vec![
            ("label", "_New".into()),
            ("shortcut", vec![vec!["Control", "N"]].into()),
        ]),
        vec![],
    )
    .unwrap();
    let sep = make_node(3, props(vec![("type", "separator".into())]), vec![]).unwrap();
    let check = make_node(
        4,
        props(vec![
            ("label", "Show Hidden".into()),
            ("toggle-type", "checkmark".into()),
            ("toggle-state", 1i32.into()),
        ]),
        vec![],
    )
    .unwrap();
    let file = make_node(
        1,
        props(vec![
            ("label", "_File".into()),
            ("children-display", "submenu".into()),
        ]),
        vec![new, sep, check],
    )
    .unwrap();
    let edit = make_node(
        5,
        props(vec![
            ("label", "Edit".into()),
            ("children-display", "submenu".into()),
        ]),
        vec![],
    )
    .unwrap();
    make_node(
        0,
        props(vec![("children-display", "submenu".into())]),
        vec![file, edit],
    )
    .unwrap()
}

#[derive(Default)]
struct Recorded {
    events: Vec<(i32, String)>,
    about: Vec<i32>,
}

struct FakeDBusMenu {
    rec: Arc<Mutex<Recorded>>,
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl FakeDBusMenu {
    fn get_layout(
        &self,
        _parent_id: i32,
        _recursion_depth: i32,
        _property_names: Vec<String>,
    ) -> fdo::Result<(u32, RawNode)> {
        Ok((7, sample_layout()))
    }

    fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
        self.rec
            .lock()
            .unwrap()
            .events
            .push((id, event_id.to_string()));
    }

    fn about_to_show(&self, id: i32) -> bool {
        self.rec.lock().unwrap().about.push(id);
        id == 5
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

#[test]
fn dbusmenu_client_fetches_activates_and_follows_updates() {
    let bus = bus_or_skip!();
    block_on(async {
        let rec = Arc::new(Mutex::new(Recorded::default()));
        let server = bus
            .builder()
            .serve_at("/MenuBar/1", FakeDBusMenu { rec: rec.clone() })
            .unwrap()
            .build()
            .await
            .unwrap();
        let service = server.unique_name().unwrap().to_string();

        let client_conn = bus.connect_async().await;
        let client = DBusMenuClient::connect(&client_conn, &service, "/MenuBar/1")
            .await
            .unwrap();
        assert!(!client.about_to_show(ItemId(1)).await.unwrap());
        assert!(
            client.about_to_show(ItemId(5)).await.unwrap(),
            "the service asked for a refetch"
        );
        let (revision, root) = client.fetch(ItemId(0), -1).await.unwrap();
        assert_eq!(revision, 7);
        assert_eq!(root.children.len(), 2);
        let file = &root.children[0];
        assert_eq!(file.label, "File");
        assert_eq!(
            file.children[0].shortcut.as_ref().unwrap().display(),
            "Ctrl+N"
        );
        assert_eq!(file.children[2].toggle, Toggle::Check(true));
        assert!(root.children[1].needs_load());

        client.activate(ItemId(2), 123).await.unwrap();
        client.hover(ItemId(2), 124).await.unwrap();
        client.notify_open(ItemId(1), true, 125).await.unwrap();
        {
            let r = rec.lock().unwrap();
            assert_eq!(
                r.events,
                vec![
                    (2, "clicked".to_string()),
                    (2, "hovered".to_string()),
                    (1, "opened".to_string())
                ]
            );
            assert_eq!(r.about, vec![1, 5]);
        }

        let mut updates = client.updates().await.unwrap();
        let emitter = SignalEmitter::new(&server, "/MenuBar/1").unwrap();
        FakeDBusMenu::layout_updated(&emitter, 8, 5).await.unwrap();
        FakeDBusMenu::items_properties_updated(
            &emitter,
            vec![(2, props(vec![("label", "New _Tab".into())]))],
            vec![(4, vec!["toggle-state".to_string()])],
        )
        .await
        .unwrap();
        let mut saw_layout = false;
        let mut root = root;
        for _ in 0..2 {
            match next_within(&mut updates, 5.0).await.expect("update") {
                MenuUpdate::Layout { revision, parent } => {
                    assert_eq!((revision, parent), (8, ItemId(5)));
                    saw_layout = true;
                }
                MenuUpdate::Properties { updated, removed } => {
                    apply_properties_update(&mut root, &updated, &removed)
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        assert!(saw_layout);
        assert_eq!(root.find(ItemId(2)).unwrap().label, "New Tab");
        assert_eq!(root.find(ItemId(4)).unwrap().toggle, Toggle::None);
    });
}

#[test]
fn controller_drives_a_dbusmenu_session_end_to_end() {
    let bus = bus_or_skip!();
    block_on(async {
        let rec = Arc::new(Mutex::new(Recorded::default()));
        let server = bus
            .builder()
            .serve_at("/MenuBar/1", FakeDBusMenu { rec: rec.clone() })
            .unwrap()
            .build()
            .await
            .unwrap();
        let client_conn = bus.connect_async().await;
        let client = DBusMenuClient::connect(
            &client_conn,
            server.unique_name().unwrap().as_str(),
            "/MenuBar/1",
        )
        .await
        .unwrap();
        let (_, root) = client.fetch(ItemId(0), -1).await.unwrap();

        let mut c = MenuController::new(
            Rect::new(0.0, 0.0, 1920.0, 1080.0),
            Box::new(MonoMeasure::default()),
        );
        c.set_content(BarContent {
            app_name: "Files".into(),
            menus: root.children.clone(),
            clock: "10:42".into(),
            ..Default::default()
        });
        let file = SlotKey::Menu(ItemId(1));
        let r = c.layout().slot(&file).unwrap().rect;
        let press = Point::new(r.x + 2.0, r.y + 2.0);
        let out = c.pointer_press(press, 0.0);
        assert!(out.effects.contains(&Effect::AboutToShow {
            slot: file.clone(),
            item: ItemId(1)
        }));
        client.about_to_show(ItemId(1)).await.unwrap();
        let row = c.levels()[0].layout.rows[0].rect;
        let target = Point::new(row.x + 5.0, row.y + 5.0);
        c.pointer_move(target, 0.1);
        c.pointer_release(target, 0.2);
        // the row flashes, then is chosen
        let out = c.tick(0.2 + FLASH_OFF + FLASH_ON);
        let activated = out
            .effects
            .iter()
            .find_map(|e| {
                if let Effect::Activate { item, .. } = e {
                    Some(*item)
                } else {
                    None
                }
            })
            .expect("activation");
        client.activate(activated, 1).await.unwrap();
        assert_eq!(rec.lock().unwrap().events, vec![(2, "clicked".to_string())]);
    });
}

type ToolTip = (String, Vec<(i32, i32, Vec<u8>)>, String, String);

struct FakeItem {
    activations: Arc<Mutex<Vec<(String, i32, i32)>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl FakeItem {
    fn activate(&self, x: i32, y: i32) {
        self.activations
            .lock()
            .unwrap()
            .push(("activate".into(), x, y));
    }

    fn secondary_activate(&self, x: i32, y: i32) {
        self.activations
            .lock()
            .unwrap()
            .push(("secondary".into(), x, y));
    }

    fn context_menu(&self, x: i32, y: i32) {
        self.activations
            .lock()
            .unwrap()
            .push(("context".into(), x, y));
    }

    fn scroll(&self, delta: i32, _orientation: &str) {
        self.activations
            .lock()
            .unwrap()
            .push(("scroll".into(), delta, 0));
    }

    #[zbus(property)]
    fn id(&self) -> String {
        "org.example.net".into()
    }

    #[zbus(property)]
    fn category(&self) -> String {
        "SystemServices".into()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        "Network".into()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        "NeedsAttention".into()
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        "network-wired".into()
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> String {
        "network-error".into()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        vec![(1, 1, vec![255, 1, 2, 3]), (2, 2, vec![0; 16])]
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/Menu").unwrap()
    }

    #[zbus(property)]
    fn tool_tip(&self) -> ToolTip {
        (
            "network-wired".into(),
            vec![],
            "Wired".into(),
            "connected".into(),
        )
    }
}

#[test]
fn status_notifier_watcher_host_and_item_roundtrip() {
    let bus = bus_or_skip!();
    block_on(async {
        let shell = bus.connect_async().await;
        let (watcher, events) = WatcherService::start(&shell).await.unwrap();

        let activations = Arc::new(Mutex::new(Vec::new()));
        let rec = Arc::new(Mutex::new(Recorded::default()));
        let app = bus
            .builder()
            .serve_at(
                "/StatusNotifierItem",
                FakeItem {
                    activations: activations.clone(),
                },
            )
            .unwrap()
            .serve_at("/Menu", FakeDBusMenu { rec: rec.clone() })
            .unwrap()
            .build()
            .await
            .unwrap();

        let client = WatcherClient::connect(&app).await.unwrap();
        assert!(client.is_host_registered().await.unwrap());
        client.register_item("/StatusNotifierItem").await.unwrap();
        let app_name = app.unique_name().unwrap().to_string();
        let id = format!("{app_name}/StatusNotifierItem");
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            WatcherEvent::ItemRegistered {
                id: id.clone(),
                service: app_name.clone(),
                path: "/StatusNotifierItem".into()
            }
        );
        assert_eq!(watcher.items(), vec![id.clone()]);
        assert_eq!(client.items().await.unwrap(), vec![id.clone()]);
        client.register_item("/StatusNotifierItem").await.unwrap();
        assert_eq!(watcher.items().len(), 1, "registering twice is idempotent");

        let item = sni::fetch_item(&shell, &app_name, "/StatusNotifierItem")
            .await
            .unwrap();
        assert_eq!(
            (item.title.as_str(), item.app_id.as_str()),
            ("Network", "org.example.net")
        );
        assert_eq!(item.status, ItemStatus::NeedsAttention);
        assert_eq!(item.current_icon_name(), "network-error");
        assert_eq!(item.icon_pixmaps.len(), 2);
        assert_eq!(
            sni::best_pixmap(&item.icon_pixmaps, 1)
                .unwrap()
                .to_rgba()
                .unwrap(),
            vec![1, 2, 3, 255]
        );
        assert_eq!(item.tooltip_title, "Wired");
        assert_eq!(item.menu_path.as_deref(), Some("/Menu"));
        assert_eq!(item.primary_action(), sni::PrimaryAction::ShowMenu);

        let menu = sni::load_menu(&shell, &item).await.expect("menu");
        assert_eq!(menu.children.len(), 2);
        let entry = sni::status_entry(&item, Some(menu), 16.0);
        assert_eq!(entry.id, id);

        sni::activate_item(&shell, &item, 10, 20).await.unwrap();
        sni::secondary_activate_item(&shell, &item, 11, 21)
            .await
            .unwrap();
        sni::context_menu_item(&shell, &item, 12, 22).await.unwrap();
        sni::scroll_item(&shell, &item, -3, "vertical")
            .await
            .unwrap();
        assert_eq!(
            *activations.lock().unwrap(),
            vec![
                ("activate".to_string(), 10, 20),
                ("secondary".to_string(), 11, 21),
                ("context".to_string(), 12, 22),
                ("scroll".to_string(), -3, 0)
            ]
        );

        let other = bus.connect_async().await;
        let other_client = WatcherClient::connect(&other).await.unwrap();
        let mut watcher_events = other_client.events().await.unwrap();
        other_client
            .register_item("org.example.legacy-item")
            .await
            .unwrap();
        match next_within(&mut watcher_events, 5.0).await.expect("signal") {
            WatcherEvent::ItemRegistered { service, path, .. } => assert_eq!(
                (service.as_str(), path.as_str()),
                ("org.example.legacy-item", "/StatusNotifierItem")
            ),
            e => panic!("unexpected {e:?}"),
        }
        events.recv_timeout(Duration::from_secs(5)).unwrap();

        drop(client);
        drop(app);
        assert_eq!(
            events.recv_timeout(Duration::from_secs(5)).unwrap(),
            WatcherEvent::ItemUnregistered { id }
        );
        match next_within(&mut watcher_events, 5.0).await.expect("signal") {
            WatcherEvent::ItemUnregistered { .. } => {}
            e => panic!("unexpected {e:?}"),
        }
        assert_eq!(watcher.items().len(), 1);
    });
}

struct FakeGtkMenus;

fn gtk_item(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
    props(pairs)
}

#[zbus::interface(name = "org.gtk.Menus")]
impl FakeGtkMenus {
    fn start(&self, subscriptions: Vec<u32>) -> Vec<RawMenu> {
        assert!(subscriptions.contains(&0));
        vec![
            (
                0,
                0,
                vec![gtk_item(vec![
                    ("label", "_File".into()),
                    (":submenu", Value::from((0u32, 1u32))),
                ])],
            ),
            (
                0,
                1,
                vec![
                    gtk_item(vec![(":section", Value::from((0u32, 2u32)))]),
                    gtk_item(vec![(":section", Value::from((0u32, 3u32)))]),
                ],
            ),
            (
                0,
                2,
                vec![gtk_item(vec![
                    ("label", "_New".into()),
                    ("action", "app.new".into()),
                    ("accel", "<Primary>n".into()),
                ])],
            ),
            (
                0,
                3,
                vec![gtk_item(vec![
                    ("label", "Mode".into()),
                    ("action", "win.mode".into()),
                    ("target", "fast".into()),
                ])],
            ),
        ]
    }

    fn end(&self, _subscriptions: Vec<u32>) {}

    #[zbus(signal)]
    async fn changed(emitter: &SignalEmitter<'_>, changes: Vec<RawChange>) -> zbus::Result<()>;
}

struct FakeGtkActions {
    scope: &'static str,
    log: Arc<Mutex<Vec<(String, String, usize)>>>,
}

#[zbus::interface(name = "org.gtk.Actions")]
impl FakeGtkActions {
    fn describe_all(&self) -> HashMap<String, RawAction> {
        let mut m = HashMap::new();
        if self.scope == "app" {
            m.insert("new".to_string(), (true, Signature::Unit, vec![]));
        } else {
            m.insert(
                "mode".to_string(),
                (
                    true,
                    Signature::Str,
                    vec![OwnedValue::try_from(Value::from("fast")).unwrap()],
                ),
            );
        }
        m
    }

    fn activate(
        &self,
        action_name: &str,
        parameter: Vec<Value<'_>>,
        _platform_data: HashMap<String, Value<'_>>,
    ) {
        self.log.lock().unwrap().push((
            self.scope.to_string(),
            action_name.to_string(),
            parameter.len(),
        ));
    }
}

#[test]
fn gtk_menu_model_is_read_and_activated() {
    let bus = bus_or_skip!();
    block_on(async {
        let log = Arc::new(Mutex::new(Vec::new()));
        let app = bus
            .builder()
            .serve_at("/org/example/menus/menubar", FakeGtkMenus)
            .unwrap()
            .serve_at(
                "/org/example",
                FakeGtkActions {
                    scope: "app",
                    log: log.clone(),
                },
            )
            .unwrap()
            .serve_at(
                "/org/example/window/1",
                FakeGtkActions {
                    scope: "win",
                    log: log.clone(),
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap();

        let shell = bus.connect_async().await;
        let props = GtkMenuProps {
            bus_name: app.unique_name().unwrap().to_string(),
            application_object_path: Some("/org/example".into()),
            window_object_path: Some("/org/example/window/1".into()),
            menubar_object_path: Some("/org/example/menus/menubar".into()),
            ..Default::default()
        };
        let (client, items) = GtkMenuClient::load(&shell, &props).await.unwrap();
        assert_eq!(items.len(), 1);
        let file = &items[0];
        assert_eq!(file.label, "File");
        let labels: Vec<&str> = file.children.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["New", "", "Mode"]);
        assert_eq!(file.children[0].accel.as_deref(), Some("<Primary>n"));
        assert_eq!(
            file.children[2].toggle,
            Toggle::Radio(true),
            "the action state equals the item target"
        );

        assert!(client.activate(file.children[0].id).await.unwrap());
        assert!(client.activate(file.children[2].id).await.unwrap());
        assert!(
            !client.activate(file.id).await.unwrap(),
            "submenus have no action"
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                ("app".to_string(), "new".to_string(), 0),
                ("win".to_string(), "mode".to_string(), 1)
            ]
        );
    });
}
