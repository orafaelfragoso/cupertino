//! Status notifier items through the worker's watcher on a private session bus: registration by object path and by
//! bus name, both interface names of the watcher and of an item, other hosts, every property read, the change signals
//! reading again what they name, clicks and scrolls sent to the item, and items that leave. Then what the bar draws of
//! an item: its attention icon, its tooltip text, its overlay, and an icon of its own icon directory, found as a menu
//! row's icon in its exporter's directories is.

// a test owns its thread: it blocks on the bus and writes its own icon files
#![allow(clippy::disallowed_methods)]

use std::path::PathBuf;

use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};
use zbus::Connection;

use cupertino_menubar::dbus::sni::{
    self, IconPixmap, ItemInterface, ItemStatus, Orientation, StatusAction, StatusItem,
    StatusToolTip, FREEDESKTOP_WATCHER_NAME, WATCHER_NAME, WATCHER_PATH,
};
use cupertino_menubar::icons::{self, Loader};
use cupertino_menubar::worker::{Event, Request, Worker};

use crate::bus::{next_signal, signals, soon, within, Bus};

const ITEM_PATH: &str = "/org/ayatana/NotificationItem/sync";
const KDE_WATCHER: &str = "org.kde.StatusNotifierWatcher";
const DRAFT_WATCHER: &str = "org.freedesktop.StatusNotifierWatcher";

type Pixmaps = Vec<(i32, i32, Vec<u8>)>;

/// One opaque pixmap of `edge` × `edge` in ARGB `colour`.
fn square(edge: i32, colour: [u8; 4]) -> Pixmaps {
    vec![(edge, edge, colour.repeat((edge * edge) as usize))]
}

/// A status item with every property, telling of each call it gets.
struct Item {
    title: String,
    status: String,
    overlay: String,
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

    #[zbus(property)]
    fn id(&self) -> String {
        "sync".into()
    }

    #[zbus(property)]
    fn category(&self) -> String {
        "Communications".into()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        self.title.clone()
    }

    #[zbus(property)]
    fn status(&self) -> String {
        self.status.clone()
    }

    #[zbus(property)]
    fn window_id(&self) -> i32 {
        42
    }

    #[zbus(property)]
    fn icon_name(&self) -> String {
        "sync-idle".into()
    }

    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        // sizes around the two the bar draws, which it keeps the best of, and one far larger, which it drops
        let mut pixmaps = square(48, [255, 0, 0, 255]);
        for edge in [8, 16, 22, 32] {
            pixmaps.extend(square(edge, [255, 0, 0, 255]));
        }
        pixmaps.push((4096, 1, vec![0; 4096 * 4]));
        pixmaps
    }

    #[zbus(property)]
    fn attention_icon_name(&self) -> String {
        String::new()
    }

    #[zbus(property)]
    fn attention_movie_name(&self) -> String {
        "sync-busy".into()
    }

    #[zbus(property)]
    fn overlay_icon_name(&self) -> String {
        self.overlay.clone()
    }

    #[zbus(property)]
    fn overlay_icon_pixmap(&self) -> Pixmaps {
        square(8, [255, 0, 0, 255])
    }

    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        "/opt/sync/icons".into()
    }

    #[zbus(property)]
    fn tool_tip(&self) -> (String, Pixmaps, String, String) {
        (
            "sync-tip".into(),
            square(2, [255, 1, 2, 3]),
            "Sync".into(),
            "<b>3</b> files &amp; folders".into(),
        )
    }

    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/MenuBar").unwrap()
    }

    #[zbus(signal)]
    async fn new_title(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_status(emitter: &SignalEmitter<'_>, status: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn new_overlay_icon(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// An item on the freedesktop draft's interface only.
struct DraftItem {
    calls: async_channel::Sender<String>,
}

#[zbus::interface(name = "org.freedesktop.StatusNotifierItem")]
impl DraftItem {
    fn activate(&self, x: i32, y: i32) {
        let _ = self.calls.try_send(format!("Activate {x} {y}"));
    }

    #[zbus(property)]
    fn id(&self) -> String {
        "draft".into()
    }

    #[zbus(property)]
    fn title(&self) -> String {
        "Draft".into()
    }
}

fn item(calls: async_channel::Sender<String>) -> Item {
    Item {
        title: "Sync".into(),
        status: "Active".into(),
        overlay: String::new(),
        calls,
    }
}

/// Call a method of the watcher under `interface`.
async fn watcher_call(
    conn: &Connection,
    watcher: &str,
    interface: &str,
    method: &str,
    arg: &str,
) -> zbus::Result<zbus::Message> {
    conn.call_method(
        Some(watcher),
        WATCHER_PATH,
        Some(interface),
        method,
        &(arg,),
    )
    .await
}

async fn watcher_property(
    conn: &Connection,
    watcher: &str,
    interface: &str,
    name: &str,
) -> OwnedValue {
    let props = zbus::fdo::PropertiesProxy::builder(conn)
        .destination(watcher.to_owned())
        .unwrap()
        .path(WATCHER_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    props
        .get(interface.try_into().unwrap(), name)
        .await
        .unwrap()
}

/// The next status item the worker read.
async fn next_item(events: &async_channel::Receiver<Event>) -> StatusItem {
    within("a status item", async {
        loop {
            if let Event::StatusUpdated { item } = events.recv().await.unwrap() {
                break *item;
            }
        }
    })
    .await
}

async fn next_removed(events: &async_channel::Receiver<Event>) -> String {
    within("a status item's removal", async {
        loop {
            if let Event::StatusRemoved { id } = events.recv().await.unwrap() {
                break id;
            }
        }
    })
    .await
}

#[test]
fn the_watcher_answers_under_both_names() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, client) = (bus.connect().await, bus.connect().await);
        let watcher = bar.unique_name().unwrap().to_string();
        let (_worker, _events) = Worker::start(bar).await.unwrap();
        let dbus = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        for name in [WATCHER_NAME, FREEDESKTOP_WATCHER_NAME] {
            let owner = dbus.get_name_owner(name.try_into().unwrap()).await.unwrap();
            assert_eq!(owner.as_str(), watcher, "{name}");
        }
        for interface in [KDE_WATCHER, DRAFT_WATCHER] {
            let version = watcher_property(&client, &watcher, interface, "ProtocolVersion").await;
            assert_eq!(i32::try_from(version).unwrap(), 0);
            let host = watcher_property(
                &client,
                &watcher,
                interface,
                "IsStatusNotifierHostRegistered",
            )
            .await;
            assert!(bool::try_from(host).unwrap());
        }
    });
}

#[test]
fn an_item_registered_by_its_path_is_read_whole() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app, watch) = (
            bus.connect().await,
            bus.connect().await,
            bus.connect().await,
        );
        let watcher = bar.unique_name().unwrap().to_string();
        let mut heard = signals(&watch, KDE_WATCHER).await;
        let (_worker, events) = Worker::start(bar).await.unwrap();
        let (calls, _) = async_channel::unbounded();
        app.object_server()
            .at(ITEM_PATH, item(calls))
            .await
            .unwrap();
        let service = app.unique_name().unwrap().to_string();
        let id = format!("{service}{ITEM_PATH}");

        watcher_call(
            &app,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierItem",
            ITEM_PATH,
        )
        .await
        .unwrap();
        let signal = next_signal(&mut heard, "StatusNotifierItemRegistered").await;
        assert_eq!(signal.body().deserialize::<String>().unwrap(), id);
        let listed = watcher_property(
            &watch,
            &watcher,
            KDE_WATCHER,
            "RegisteredStatusNotifierItems",
        )
        .await;
        assert_eq!(
            Vec::<String>::try_from(listed).unwrap(),
            std::slice::from_ref(&id)
        );

        let read = next_item(&events).await;
        assert_eq!(
            (read.id.as_str(), read.service.as_str()),
            (id.as_str(), service.as_str())
        );
        assert_eq!(read.interface, ItemInterface::Kde);
        assert_eq!(
            (read.app_id.as_str(), read.category.as_str()),
            ("sync", "Communications")
        );
        assert_eq!(
            (read.title.as_str(), read.status),
            ("Sync", ItemStatus::Active)
        );
        assert_eq!(read.window_id, 42);
        assert_eq!(read.icon_name, "sync-idle");
        let sizes: Vec<(i32, i32)> = read
            .icon_pixmaps
            .iter()
            .map(|p| (p.width, p.height))
            .collect();
        assert_eq!(
            sizes,
            [(16, 16), (32, 32)],
            "the best of the sizes the bar draws, at one and two pixels a point"
        );
        assert_eq!(sni::kept_pixmap_sizes(), [16, 32]);
        assert_eq!(read.attention_movie_name, "sync-busy");
        assert_eq!(read.overlay_pixmaps.len(), 1);
        assert_eq!(read.icon_theme_path, "/opt/sync/icons");
        assert_eq!(read.tooltip.icon_name, "sync-tip");
        assert_eq!(read.tooltip.text(), "Sync\n3 files & folders");
        assert_eq!(read.tooltip_text(), "Sync\n3 files & folders");
        assert!(read.item_is_menu);
        assert_eq!(read.menu_path.as_deref(), Some("/MenuBar"));

        // registering again changes nothing
        watcher_call(
            &app,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierItem",
            ITEM_PATH,
        )
        .await
        .unwrap();
        assert!(
            soon(next_signal(&mut heard, "StatusNotifierItemRegistered"))
                .await
                .is_none()
        );

        drop(app);
        assert_eq!(next_removed(&events).await, id);
        let signal = next_signal(&mut heard, "StatusNotifierItemUnregistered").await;
        assert_eq!(signal.body().deserialize::<String>().unwrap(), id);
    });
}

#[test]
fn an_item_registered_by_its_bus_name_goes_with_the_name() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app) = (bus.connect().await, bus.connect().await);
        let watcher = bar.unique_name().unwrap().to_string();
        let (_worker, events) = Worker::start(bar).await.unwrap();
        let (calls, _) = async_channel::unbounded();
        app.object_server()
            .at(sni::DEFAULT_ITEM_PATH, item(calls))
            .await
            .unwrap();
        let name = "org.kde.StatusNotifierItem-7-1";
        app.request_name(name).await.unwrap();

        watcher_call(
            &app,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierItem",
            name,
        )
        .await
        .unwrap();
        let read = next_item(&events).await;
        assert_eq!(read.id, format!("{name}/StatusNotifierItem"));
        assert_eq!(read.service, name);

        app.release_name(name).await.unwrap();
        assert_eq!(next_removed(&events).await, read.id);
    });
}

#[test]
fn an_item_registered_first_by_another_connection_stays_with_its_owner() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app, squatter, watch) = (
            bus.connect().await,
            bus.connect().await,
            bus.connect().await,
            bus.connect().await,
        );
        let watcher = bar.unique_name().unwrap().to_string();
        let mut heard = signals(&watch, KDE_WATCHER).await;
        let (_worker, events) = Worker::start(bar).await.unwrap();
        let (calls, _) = async_channel::unbounded();
        app.object_server()
            .at(sni::DEFAULT_ITEM_PATH, item(calls))
            .await
            .unwrap();
        let name = "org.kde.StatusNotifierItem-8-1";
        app.request_name(name).await.unwrap();

        // another connection registers the item under a name it does not own, then its owner does
        for conn in [&squatter, &app] {
            watcher_call(
                conn,
                &watcher,
                KDE_WATCHER,
                "RegisterStatusNotifierItem",
                name,
            )
            .await
            .unwrap();
        }
        let read = next_item(&events).await;
        next_signal(&mut heard, "StatusNotifierItemRegistered").await;

        // the item stays when the connection that registered it first leaves, and goes with its owner
        drop(squatter);
        assert!(
            soon(next_signal(&mut heard, "StatusNotifierItemUnregistered"))
                .await
                .is_none(),
            "the item stays with its owner"
        );
        drop(app);
        assert_eq!(next_removed(&events).await, read.id);
        let signal = next_signal(&mut heard, "StatusNotifierItemUnregistered").await;
        assert_eq!(signal.body().deserialize::<String>().unwrap(), read.id);
    });
}

#[test]
fn the_items_of_a_host_that_left_as_it_registered_are_told_gone() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app, watch) = (
            bus.connect().await,
            bus.connect().await,
            bus.connect().await,
        );
        let watcher = bar.unique_name().unwrap().to_string();
        let mut heard = signals(&watch, KDE_WATCHER).await;
        let (_worker, _events) = Worker::start(bar).await.unwrap();
        let (calls, _) = async_channel::unbounded();
        app.object_server()
            .at(ITEM_PATH, item(calls))
            .await
            .unwrap();
        let id = format!("{}{ITEM_PATH}", app.unique_name().unwrap());
        watcher_call(
            &app,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierItem",
            ITEM_PATH,
        )
        .await
        .unwrap();
        next_signal(&mut heard, "StatusNotifierItemRegistered").await;

        // a host registers and leaves before its registration is looked at: its item is told gone either way
        let host = "org.kde.StatusNotifierHost-11";
        let register = zbus::Message::method_call(WATCHER_PATH, "RegisterStatusNotifierHost")
            .unwrap()
            .destination(watcher.as_str())
            .unwrap()
            .interface(KDE_WATCHER)
            .unwrap()
            .with_flags(zbus::message::Flags::NoReplyExpected)
            .unwrap()
            .build(&(host,))
            .unwrap();
        app.send(&register).await.unwrap();
        drop(app);
        let signal = next_signal(&mut heard, "StatusNotifierItemUnregistered").await;
        assert_eq!(signal.body().deserialize::<String>().unwrap(), id);
        let listed = watcher_property(
            &watch,
            &watcher,
            KDE_WATCHER,
            "RegisteredStatusNotifierItems",
        )
        .await;
        assert!(Vec::<String>::try_from(listed).unwrap().is_empty());
    });
}

#[test]
fn a_name_without_owner_is_refused() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app) = (bus.connect().await, bus.connect().await);
        let watcher = bar.unique_name().unwrap().to_string();
        let (_worker, _events) = Worker::start(bar).await.unwrap();
        let answer = watcher_call(
            &app,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierItem",
            "org.example.Gone",
        )
        .await;
        match answer {
            Err(zbus::Error::MethodError(name, _, _)) => {
                assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.NameHasNoOwner")
            }
            other => panic!("a name without owner was answered {other:?}"),
        }
        let listed =
            watcher_property(&app, &watcher, KDE_WATCHER, "RegisteredStatusNotifierItems").await;
        assert!(Vec::<String>::try_from(listed).unwrap().is_empty());
    });
}

#[test]
fn an_item_of_the_draft_interface_is_read_and_activated() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app) = (bus.connect().await, bus.connect().await);
        let watcher = bar.unique_name().unwrap().to_string();
        let (worker, events) = Worker::start(bar).await.unwrap();
        let (calls, called) = async_channel::unbounded();
        app.object_server()
            .at(ITEM_PATH, DraftItem { calls })
            .await
            .unwrap();
        watcher_call(
            &app,
            &watcher,
            DRAFT_WATCHER,
            "RegisterStatusNotifierItem",
            ITEM_PATH,
        )
        .await
        .unwrap();
        let read = next_item(&events).await;
        assert_eq!(read.interface, ItemInterface::Freedesktop);
        assert_eq!(
            (read.app_id.as_str(), read.title.as_str()),
            ("draft", "Draft")
        );

        worker
            .send(Request::StatusAction {
                service: read.service,
                path: read.path,
                action: StatusAction::Activate { x: 3, y: 4 },
            })
            .unwrap();
        assert_eq!(
            within("Activate", called.recv()).await.unwrap(),
            "Activate 3 4"
        );
    });
}

#[test]
fn clicks_and_scrolls_reach_the_item() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app) = (bus.connect().await, bus.connect().await);
        let watcher = bar.unique_name().unwrap().to_string();
        let (worker, events) = Worker::start(bar).await.unwrap();
        let (calls, called) = async_channel::unbounded();
        app.object_server()
            .at(ITEM_PATH, item(calls))
            .await
            .unwrap();
        watcher_call(
            &app,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierItem",
            ITEM_PATH,
        )
        .await
        .unwrap();
        let read = next_item(&events).await;
        let actions = [
            (StatusAction::Activate { x: 10, y: 20 }, "Activate 10 20"),
            (
                StatusAction::SecondaryActivate { x: 11, y: 21 },
                "SecondaryActivate 11 21",
            ),
            (
                StatusAction::ContextMenu { x: 12, y: 22 },
                "ContextMenu 12 22",
            ),
            (
                StatusAction::Scroll {
                    delta: -3,
                    orientation: Orientation::Vertical,
                },
                "Scroll -3 vertical",
            ),
            (
                StatusAction::Scroll {
                    delta: 1,
                    orientation: Orientation::Horizontal,
                },
                "Scroll 1 horizontal",
            ),
        ];
        for (action, call) in actions {
            worker
                .send(Request::StatusAction {
                    service: read.service.clone(),
                    path: read.path.clone(),
                    action,
                })
                .unwrap();
            assert_eq!(within(call, called.recv()).await.unwrap(), call);
        }
    });
}

#[test]
fn a_signal_reads_again_what_it_names() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, app) = (bus.connect().await, bus.connect().await);
        let watcher = bar.unique_name().unwrap().to_string();
        let (_worker, events) = Worker::start(bar).await.unwrap();
        let (calls, _) = async_channel::unbounded();
        app.object_server()
            .at(ITEM_PATH, item(calls))
            .await
            .unwrap();
        watcher_call(
            &app,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierItem",
            ITEM_PATH,
        )
        .await
        .unwrap();
        assert_eq!(next_item(&events).await.title, "Sync");
        let iface = app
            .object_server()
            .interface::<_, Item>(ITEM_PATH)
            .await
            .unwrap();

        iface.get_mut().await.title = "Syncing".into();
        Item::new_title(iface.signal_emitter()).await.unwrap();
        assert_eq!(next_item(&events).await.title, "Syncing");

        // the status a signal carries is taken from it, though the property still says otherwise
        Item::new_status(iface.signal_emitter(), "NeedsAttention")
            .await
            .unwrap();
        let read = next_item(&events).await;
        assert_eq!(read.status, ItemStatus::NeedsAttention);
        assert_eq!(read.current_icon_name(), "sync-busy");

        iface.get_mut().await.overlay = "emblem-new".into();
        Item::new_overlay_icon(iface.signal_emitter())
            .await
            .unwrap();
        let read = next_item(&events).await;
        assert_eq!(read.overlay_icon_name, "emblem-new");
        // what that signal does not name is kept as last read
        assert_eq!(read.status, ItemStatus::NeedsAttention);
    });
}

#[test]
fn other_hosts_are_told_of_as_they_come_and_go() {
    let bus = Bus::start();
    async_io::block_on(async {
        let (bar, host, watch) = (
            bus.connect().await,
            bus.connect().await,
            bus.connect().await,
        );
        let watcher = bar.unique_name().unwrap().to_string();
        let mut heard = signals(&watch, DRAFT_WATCHER).await;
        let (_worker, _events) = Worker::start(bar).await.unwrap();
        let name = "org.kde.StatusNotifierHost-9";
        host.request_name(name).await.unwrap();
        watcher_call(
            &host,
            &watcher,
            KDE_WATCHER,
            "RegisterStatusNotifierHost",
            name,
        )
        .await
        .unwrap();
        next_signal(&mut heard, "StatusNotifierHostRegistered").await;
        drop(host);
        next_signal(&mut heard, "StatusNotifierHostUnregistered").await;
    });
}

#[test]
fn an_item_needing_attention_shows_its_attention_icon_or_movie() {
    let pixmap = IconPixmap {
        width: 1,
        height: 1,
        argb: vec![255; 4],
    };
    let mut item = StatusItem {
        icon_name: "idle".into(),
        icon_pixmaps: vec![pixmap.clone()],
        attention_movie_name: "busy".into(),
        ..StatusItem::default()
    };
    assert_eq!(item.current_icon_name(), "idle");
    assert_eq!(item.current_pixmaps().len(), 1);
    item.status = ItemStatus::NeedsAttention;
    // the movie stands for the attention icon, and the usual pixmap gives way to it
    assert_eq!(item.current_icon_name(), "busy");
    assert!(item.current_pixmaps().is_empty());
    item.attention_icon_name = "alert".into();
    assert_eq!(item.current_icon_name(), "alert");
    item.attention_pixmaps = vec![pixmap];
    assert_eq!(item.current_pixmaps().len(), 1);
    // an item needing attention that says nothing of how keeps its icon
    let plain = StatusItem {
        icon_name: "idle".into(),
        status: ItemStatus::NeedsAttention,
        ..StatusItem::default()
    };
    assert_eq!(plain.current_icon_name(), "idle");
}

#[test]
fn a_tooltip_is_shown_without_its_markup() {
    let tip = StatusToolTip {
        title: "Mail".into(),
        description:
            "<p>2 new</p>from <i>Ann</i> &lt;ann@example.org&gt;<br/>and&nbsp;Bo &unknown;".into(),
        ..StatusToolTip::default()
    };
    assert_eq!(
        tip.text(),
        "Mail\n2 new\nfrom Ann <ann@example.org>\nand\u{a0}Bo &unknown;"
    );
    assert_eq!(sni::plain_text("a < b"), "a < b");
    let untitled = StatusToolTip {
        description: "<b>only</b>".into(),
        ..StatusToolTip::default()
    };
    assert_eq!(untitled.text(), "only");
}

/// A directory of the test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("status-items-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an SVG icon of one colour at `path`.
fn svg(path: PathBuf, colour: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="{colour}"/></svg>"#
    );
    std::fs::write(path, text).unwrap();
}

/// The first pixel of an icon, premultiplied RGBA.
fn first_pixel(icon: &icons::Pixels) -> [u8; 4] {
    icon.rgba[..4].try_into().unwrap()
}

#[test]
fn an_icon_of_the_items_own_directory_is_found() {
    let themes = scratch("themes");
    svg(themes.join("hicolor/22x22/apps/sync.svg"), "#0000ff");
    svg(themes.join("hicolor/scalable/apps/sync.svg"), "#ff0000");
    svg(themes.join("hicolor/16x16/status/sync.svg"), "#00ff00");
    let found = |px| icons::find_in_theme_path(&themes, "sync", px).unwrap();
    assert!(
        found(22).ends_with("hicolor/22x22/apps/sync.svg"),
        "the size asked for"
    );
    assert!(
        found(40).ends_with("hicolor/scalable/apps/sync.svg"),
        "the scalable one over a smaller"
    );
    assert!(icons::find_in_theme_path(&themes, "other", 22).is_none());
    assert!(icons::find_in_theme_path(&themes, "/etc/passwd", 22).is_none());
    assert!(icons::find_in_theme_path(&themes.join("missing"), "sync", 22).is_none());

    // a theme directory of its own, and icons laid flat
    let theme = scratch("theme");
    svg(theme.join("24x24/apps/sync.svg"), "#0000ff");
    assert!(icons::find_in_theme_path(&theme, "sync", 24)
        .unwrap()
        .ends_with("24x24/apps/sync.svg"));
    let flat = scratch("flat");
    svg(flat.join("sync.svg"), "#00ff00");
    assert_eq!(
        icons::find_in_theme_path(&flat, "sync.svg", 24),
        Some(flat.join("sync.svg"))
    );

    // loaded for an item that names it with its directory
    let item = StatusItem {
        icon_name: "sync".into(),
        icon_theme_path: themes.to_string_lossy().into_owned(),
        ..StatusItem::default()
    };
    let icon = Loader::default().status_icon(&item, 22).unwrap();
    assert_eq!((icon.width, icon.height), (22, 22));
    assert_eq!(first_pixel(&icon), [0, 0, 255, 255]);
    let named = Loader::default()
        .named_icon("sync", std::slice::from_ref(&flat), 16)
        .unwrap();
    assert_eq!(first_pixel(&named), [0, 255, 0, 255]);
}

#[test]
fn a_menu_row_and_an_item_find_an_icon_of_their_own_theme_alike() {
    let themes = scratch("shared-themes");
    svg(themes.join("hicolor/16x16/apps/pause.svg"), "#00ff00");
    svg(themes.join("hicolor/24x24/apps/pause.svg"), "#0000ff");
    let loader = Loader::default();
    // a row names it with its exporter's IconThemePath, an empty directory ahead of it
    let row = loader
        .row_icon(&icons::Request {
            key: (1, 24),
            data: None,
            name: Some("pause".into()),
            dirs: vec![scratch("shared-empty"), themes.clone()],
            px: 24,
        })
        .unwrap();
    assert_eq!((row.width, row.height), (24, 24));
    assert_eq!(first_pixel(&row), [0, 0, 255, 255], "the size nearest");
    // an item names it with its own
    let item = StatusItem {
        icon_name: "pause".into(),
        icon_theme_path: themes.to_string_lossy().into_owned(),
        ..StatusItem::default()
    };
    let icon = loader.status_icon(&item, 16).unwrap();
    assert_eq!(first_pixel(&icon), [0, 255, 0, 255]);
    assert_eq!(
        loader
            .named_icon("pause", &[themes], 24)
            .map(|i| first_pixel(&i)),
        Some([0, 0, 255, 255])
    );
}

#[test]
fn an_overlay_covers_the_bottom_right_quarter() {
    let argb = |edge: i32, colour: [u8; 4]| IconPixmap {
        width: edge,
        height: edge,
        argb: colour.repeat((edge * edge) as usize),
    };
    let item = StatusItem {
        icon_pixmaps: vec![argb(4, [255, 255, 0, 0])],
        overlay_pixmaps: vec![argb(2, [255, 0, 0, 255])],
        ..StatusItem::default()
    };
    let icon = Loader::default().status_icon(&item, 4).unwrap();
    let pixel =
        |x: usize, y: usize| -> [u8; 4] { icon.rgba[(y * 4 + x) * 4..][..4].try_into().unwrap() };
    assert_eq!(pixel(0, 0), [255, 0, 0, 255]);
    assert_eq!(pixel(1, 3), [255, 0, 0, 255]);
    assert_eq!(pixel(2, 2), [0, 0, 255, 255]);
    assert_eq!(pixel(3, 3), [0, 0, 255, 255]);
    // a half-transparent overlay lets the icon through
    let faint = icons::overlaid(
        icons::Pixels {
            width: 2,
            height: 2,
            rgba: [255, 0, 0, 255].repeat(4),
        },
        &icons::Pixels {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 128, 128],
        },
    );
    assert_eq!(&faint.rgba[12..16], &[127, 0, 128, 255]);
}
