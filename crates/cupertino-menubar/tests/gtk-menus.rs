//! The reader of GMenuModel menus against an exporter of `org.gtk.Menus` and `org.gtk.Actions` on a private session
//! bus: groups that come into the tree after the load are subscribed and those that leave it ended, the change records
//! of the menus and the actions apply at their place, and an activation hands the exporter the item's target and the
//! bar's activation token. The tree's rules (hidden items, marks, namespaces, icons) are checked on the model.

// a test owns its thread: it blocks on the bus and looks up the daemon's binary
#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_io::Timer;
use cupertino_menubar::dbus::gtk::{
    build_menus, GtkActionState, GtkActions, GtkMenus, MAX_SIGNAL_RECORDS,
};
use cupertino_menubar::item::{MenuItem, Toggle};
use cupertino_menubar::source::{GtkMenuProps, MenuSource};
use cupertino_menubar::worker::{Event, Request, Worker};
use futures_util::future::{select, Either};
use zbus::connection::Builder;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Signature, Value};
use zbus::Connection;

const APP: &str = "/org/example/Editor";
const WINDOW: &str = "/org/example/Editor/window/1";
const MENUBAR: &str = "/org/example/Editor/menus/menubar";
/// The longest a tree or a call may take to arrive.
const ARRIVAL: Duration = Duration::from_secs(10);

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

type Item = HashMap<String, OwnedValue>;
type RawAction = (bool, Signature, Vec<OwnedValue>);

fn owned<'a>(value: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).unwrap()
}

/// An item of `attributes`, each a key and a value.
fn item<'a>(attributes: impl IntoIterator<Item = (&'a str, Value<'a>)>) -> Item {
    attributes
        .into_iter()
        .map(|(k, v)| (k.to_owned(), owned(v)))
        .collect()
}

fn row(label: &str) -> Item {
    item([("label", Value::from(label))])
}

fn action_row(label: &str, action: &str) -> Item {
    item([
        ("label", Value::from(label)),
        ("action", Value::from(action)),
    ])
}

fn submenu(label: &str, group: u32, menu: u32) -> Item {
    item([
        ("label", Value::from(label)),
        (":submenu", Value::from((group, menu))),
    ])
}

fn section(group: u32, menu: u32) -> Item {
    item([(":section", Value::from((group, menu)))])
}

fn action(enabled: bool, parameter: &str, state: Option<Value<'_>>) -> RawAction {
    (
        enabled,
        Signature::try_from(parameter).unwrap(),
        state.into_iter().map(owned).collect(),
    )
}

fn copy_action(a: &RawAction) -> RawAction {
    (
        a.0,
        a.1.clone(),
        a.2.iter().map(|v| v.try_clone().unwrap()).collect(),
    )
}

fn copy_items(items: &[Item]) -> Vec<Item> {
    items
        .iter()
        .map(|it| {
            it.iter()
                .map(|(k, v)| (k.clone(), v.try_clone().unwrap()))
                .collect()
        })
        .collect()
}

/// A call the exporter served.
#[derive(Debug, PartialEq)]
enum Call {
    Start(Vec<u32>),
    End(Vec<u32>),
    Activate {
        path: &'static str,
        name: String,
        parameter: Vec<String>,
        platform_data: Vec<(String, String)>,
    },
}

/// The menus of the exporter, by `(group, menu)`.
type Menus = Arc<Mutex<HashMap<(u32, u32), Vec<Item>>>>;

struct MenusObject {
    menus: Menus,
    calls: async_channel::Sender<Call>,
}

#[zbus::interface(name = "org.gtk.Menus")]
impl MenusObject {
    async fn start(&self, groups: Vec<u32>) -> Vec<(u32, u32, Vec<Item>)> {
        let mut reply: Vec<(u32, u32, Vec<Item>)> = self
            .menus
            .lock()
            .unwrap()
            .iter()
            .filter(|((g, _), _)| groups.contains(g))
            .map(|((g, m), items)| (*g, *m, copy_items(items)))
            .collect();
        reply.sort_by_key(|(g, m, _)| (*g, *m));
        self.calls.send(Call::Start(groups)).await.unwrap();
        reply
    }

    async fn end(&self, groups: Vec<u32>) {
        self.calls.send(Call::End(groups)).await.unwrap();
    }

    #[zbus(signal)]
    async fn changed(
        emitter: &SignalEmitter<'_>,
        changes: Vec<(u32, u32, u32, u32, Vec<Item>)>,
    ) -> zbus::Result<()>;
}

struct ActionsObject {
    path: &'static str,
    actions: HashMap<String, RawAction>,
    calls: async_channel::Sender<Call>,
}

fn text(v: &OwnedValue) -> String {
    match &**v {
        Value::Str(s) => s.as_str().to_owned(),
        other => format!("{other:?}"),
    }
}

#[zbus::interface(name = "org.gtk.Actions")]
impl ActionsObject {
    fn describe_all(&self) -> HashMap<String, RawAction> {
        self.actions
            .iter()
            .map(|(k, v)| (k.clone(), copy_action(v)))
            .collect()
    }

    async fn activate(
        &self,
        name: String,
        parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) {
        let mut data: Vec<(String, String)> = platform_data
            .iter()
            .map(|(k, v)| (k.clone(), text(v)))
            .collect();
        data.sort();
        let call = Call::Activate {
            path: self.path,
            name,
            parameter: parameter.iter().map(text).collect(),
            platform_data: data,
        };
        self.calls.send(call).await.unwrap();
    }

    #[zbus(signal)]
    async fn changed(
        emitter: &SignalEmitter<'_>,
        removed: Vec<String>,
        enabled_changed: HashMap<String, bool>,
        state_changed: HashMap<String, OwnedValue>,
        added: HashMap<String, RawAction>,
    ) -> zbus::Result<()>;
}

/// An exporter on a private bus, and the bar's worker reading it.
struct Fixture {
    _bus: Bus,
    app: Connection,
    menus: Menus,
    calls: async_channel::Receiver<Call>,
    worker: Worker,
    events: async_channel::Receiver<Event>,
    props: GtkMenuProps,
}

impl Fixture {
    async fn new(
        menus: HashMap<(u32, u32), Vec<Item>>,
        app_actions: HashMap<String, RawAction>,
        win_actions: HashMap<String, RawAction>,
    ) -> Fixture {
        let (bus, address) = session_bus();
        let connect = || async {
            Builder::address(address.as_str())
                .unwrap()
                .build()
                .await
                .unwrap()
        };
        let (bar, app) = (connect().await, connect().await);
        let (tx, calls) = async_channel::unbounded();
        let menus = Arc::new(Mutex::new(menus));
        let server = app.object_server();
        let object = MenusObject {
            menus: menus.clone(),
            calls: tx.clone(),
        };
        server.at(MENUBAR, object).await.unwrap();
        for (path, actions) in [(APP, app_actions), (WINDOW, win_actions)] {
            let object = ActionsObject {
                path,
                actions,
                calls: tx.clone(),
            };
            server.at(path, object).await.unwrap();
        }
        let props = GtkMenuProps {
            bus_name: app.unique_name().unwrap().to_string(),
            application_id: Some("org.example.Editor".into()),
            application_object_path: Some(APP.into()),
            window_object_path: Some(WINDOW.into()),
            menubar_object_path: Some(MENUBAR.into()),
            app_menu_object_path: None,
        };
        let (worker, events) = Worker::start(bar).await.unwrap();
        worker.send(Request::FetchGtk(props.clone())).unwrap();
        Fixture {
            _bus: bus,
            app,
            menus,
            calls,
            worker,
            events,
            props,
        }
    }

    /// The next tree that satisfies `done`.
    async fn tree(&self, done: impl Fn(&MenuItem) -> bool) -> MenuItem {
        let tree = async {
            loop {
                match self.events.recv().await.unwrap() {
                    Event::Layout {
                        source: MenuSource::Gtk(props),
                        root,
                    } if props == self.props && done(&root) => break *root,
                    Event::LayoutFailed { source } => panic!("the menu was given up: {source:?}"),
                    _ => {}
                }
            }
        };
        match select(Box::pin(tree), Timer::after(ARRIVAL)).await {
            Either::Left((tree, _)) => tree,
            Either::Right(_) => panic!("no such tree within {ARRIVAL:?}"),
        }
    }

    /// The next call the exporter served that is not a `Start` of the load.
    async fn call(&self) -> Call {
        match select(Box::pin(self.calls.recv()), Timer::after(ARRIVAL)).await {
            Either::Left((call, _)) => call.unwrap(),
            Either::Right(_) => panic!("no call within {ARRIVAL:?}"),
        }
    }

    /// Change the exporter's menus to `menus` and signal `records`.
    async fn change_menus(
        &self,
        menus: impl FnOnce(&mut HashMap<(u32, u32), Vec<Item>>),
        records: Vec<(u32, u32, u32, u32, Vec<Item>)>,
    ) {
        menus(&mut self.menus.lock().unwrap());
        let emitter = SignalEmitter::new(&self.app, MENUBAR).unwrap();
        MenusObject::changed(&emitter, records).await.unwrap();
    }
}

/// The labels of the rows of `menu`, a separator as `-`.
fn labels(menu: &MenuItem) -> Vec<String> {
    menu.children
        .iter()
        .map(|c| {
            if c.separator {
                "-".to_owned()
            } else {
                c.label.clone()
            }
        })
        .collect()
}

fn child<'a>(menu: &'a MenuItem, label: &str) -> Option<&'a MenuItem> {
    menu.children.iter().find(|c| c.label == label)
}

#[test]
fn a_submenu_added_after_the_load_is_subscribed_and_shows_its_rows() {
    async_io::block_on(async {
        let menus = HashMap::from([
            ((0, 0), vec![submenu("_File", 1, 0)]),
            ((1, 0), vec![row("New")]),
        ]);
        let f = Fixture::new(menus, HashMap::new(), HashMap::new()).await;
        f.tree(|t| labels(t) == ["File"]).await;
        assert_eq!(f.call().await, Call::Start(vec![0]));
        assert_eq!(f.call().await, Call::Start(vec![1]));

        f.change_menus(
            |m| {
                m.insert((2, 0), vec![row("Zoom In"), row("Zoom Out")]);
            },
            vec![(0, 0, 1, 0, vec![submenu("_View", 2, 0)])],
        )
        .await;
        let tree = f
            .tree(|t| child(t, "View").is_some_and(|v| !v.children.is_empty()))
            .await;
        assert_eq!(
            labels(child(&tree, "View").unwrap()),
            ["Zoom In", "Zoom Out"]
        );
        assert_eq!(
            f.call().await,
            Call::Start(vec![2]),
            "only the new group is asked for"
        );
    });
}

#[test]
fn groups_the_tree_drops_are_ended_and_a_released_exporter_ends_the_rest() {
    async_io::block_on(async {
        let menus = HashMap::from([
            ((0, 0), vec![submenu("File", 1, 0), submenu("View", 2, 0)]),
            ((1, 0), vec![row("New")]),
            ((2, 0), vec![row("Zoom In")]),
        ]);
        let f = Fixture::new(menus, HashMap::new(), HashMap::new()).await;
        f.tree(|t| labels(t) == ["File", "View"]).await;
        assert_eq!(f.call().await, Call::Start(vec![0]));
        assert_eq!(f.call().await, Call::Start(vec![1, 2]));

        f.change_menus(
            |m| {
                m.remove(&(2, 0));
                m.get_mut(&(0, 0)).unwrap().pop();
            },
            vec![(0, 0, 1, 1, Vec::new())],
        )
        .await;
        f.tree(|t| labels(t) == ["File"]).await;
        assert_eq!(f.call().await, Call::End(vec![2]));

        f.worker.send(Request::ReleaseGtk(f.props.clone())).unwrap();
        assert_eq!(f.call().await, Call::End(vec![0, 1]));
    });
}

#[test]
fn change_records_apply_at_their_place_and_a_new_section_of_a_group_shows() {
    async_io::block_on(async {
        let menus = HashMap::from([
            ((0, 0), vec![submenu("Edit", 1, 0)]),
            ((1, 0), vec![section(1, 1)]),
            ((1, 1), vec![row("Undo"), row("Redo"), row("Cut")]),
        ]);
        let f = Fixture::new(menus, HashMap::new(), HashMap::new()).await;
        f.tree(|t| child(t, "Edit").is_some_and(|e| e.children.len() == 3))
            .await;

        // the exporter sends a new menu of a group subscribed before the record that links to it
        f.change_menus(
            |_| {},
            vec![
                (1, 1, 1, 1, vec![row("Paste")]),
                (1, 1, 2, 5, vec![row("Past the end")]),
                (1, 2, 0, 0, vec![row("Select All")]),
                (1, 0, 1, 0, vec![section(1, 2)]),
            ],
        )
        .await;
        let tree = f
            .tree(|t| child(t, "Edit").is_some_and(|e| e.children.len() == 5))
            .await;
        assert_eq!(
            labels(child(&tree, "Edit").unwrap()),
            ["Undo", "Paste", "Cut", "-", "Select All"]
        );
    });
}

#[test]
fn action_changes_apply_removals_enabled_and_state_changes_then_additions() {
    async_io::block_on(async {
        let menus = HashMap::from([
            ((0, 0), vec![submenu("Format", 1, 0)]),
            (
                (1, 0),
                vec![
                    action_row("Bold", "app.bold"),
                    action_row("Save", "app.save"),
                    action_row("Spell", "app.spell"),
                ],
            ),
        ]);
        let app = HashMap::from([
            (
                "bold".to_owned(),
                action(true, "", Some(Value::from(false))),
            ),
            ("save".to_owned(), action(true, "", None)),
        ]);
        let f = Fixture::new(menus, app, HashMap::new()).await;
        let format = |t: &MenuItem| child(t, "Format").cloned().unwrap_or_default();
        let tree = f.tree(|t| format(t).children.len() == 3).await;
        let rows = format(&tree).children;
        assert_eq!(rows[0].toggle, Toggle::Check(false));
        assert!(rows[1].enabled);
        assert!(
            !rows[2].enabled,
            "an action the exporter lacks disables its item"
        );

        let emitter = SignalEmitter::new(&f.app, APP).unwrap();
        ActionsObject::changed(
            &emitter,
            vec!["save".into()],
            HashMap::new(),
            HashMap::from([("bold".to_owned(), owned(true))]),
            HashMap::from([("spell".to_owned(), action(true, "", None))]),
        )
        .await
        .unwrap();
        let tree = f.tree(|t| format(t).children[2].enabled).await;
        let rows = format(&tree).children;
        assert_eq!(rows[0].toggle, Toggle::Check(true));
        assert!(!rows[1].enabled, "a removed action disables its item");

        ActionsObject::changed(
            &emitter,
            Vec::new(),
            HashMap::from([("spell".to_owned(), false)]),
            // a state of another type than the action's is not taken
            HashMap::from([("bold".to_owned(), owned("yes"))]),
            HashMap::new(),
        )
        .await
        .unwrap();
        let tree = f.tree(|t| !format(t).children[2].enabled).await;
        assert_eq!(format(&tree).children[0].toggle, Toggle::Check(true));
    });
}

#[test]
fn an_activation_hands_over_the_target_and_the_activation_token() {
    async_io::block_on(async {
        let target = |label: &str, value: &str| {
            item([
                ("label", Value::from(label)),
                ("action", Value::from("win.size")),
                ("target", Value::from(value)),
            ])
        };
        let menus = HashMap::from([
            ((0, 0), vec![submenu("View", 1, 0)]),
            (
                (1, 0),
                vec![target("Small", "small"), target("Large", "large")],
            ),
        ]);
        let win = HashMap::from([(
            "size".to_owned(),
            action(true, "s", Some(Value::from("small"))),
        )]);
        let f = Fixture::new(menus, HashMap::new(), win).await;
        let tree = f
            .tree(|t| child(t, "View").is_some_and(|v| v.children.len() == 2))
            .await;
        let view = child(&tree, "View").unwrap();
        assert_eq!(view.children[0].toggle, Toggle::Radio(true));
        assert_eq!(view.children[1].toggle, Toggle::Radio(false));
        while f.calls.try_recv().is_ok() {}

        f.worker
            .send(Request::ActivateGtk {
                props: f.props.clone(),
                item: view.children[1].id.0,
                token: Some("token-1".into()),
            })
            .unwrap();
        assert_eq!(
            f.call().await,
            Call::Activate {
                path: WINDOW,
                name: "size".into(),
                parameter: vec!["large".into()],
                platform_data: vec![
                    ("activation-token".into(), "token-1".into()),
                    ("desktop-startup-id".into(), "token-1".into()),
                ],
            }
        );
    });
}

#[test]
fn a_chosen_item_runs_its_own_action_when_the_tree_is_renumbered_before_the_token() {
    async_io::block_on(async {
        let target = |label: &str, value: &str| {
            item([
                ("label", Value::from(label)),
                ("action", Value::from("win.size")),
                ("target", Value::from(value)),
            ])
        };
        let menus = HashMap::from([
            ((0, 0), vec![submenu("View", 1, 0)]),
            (
                (1, 0),
                vec![target("Small", "small"), target("Large", "large")],
            ),
        ]);
        let win = HashMap::from([(
            "size".to_owned(),
            action(true, "s", Some(Value::from("small"))),
        )]);
        let f = Fixture::new(menus, HashMap::new(), win).await;
        let rows = |t: &MenuItem| child(t, "View").map_or(0, |v| v.children.len());
        let tree = f.tree(|t| rows(t) == 2).await;
        let large = child(&tree, "View").unwrap().children[1].id.0;
        while f.calls.try_recv().is_ok() {}

        // Large is chosen; before the bar's token comes the exporter adds a row above it, and the ids move along
        f.worker
            .send(Request::ChooseGtk {
                props: f.props.clone(),
                item: large,
                click: 7,
            })
            .unwrap();
        f.change_menus(
            |m| {
                m.get_mut(&(1, 0))
                    .unwrap()
                    .insert(0, target("Tiny", "tiny"));
            },
            vec![(1, 0, 0, 0, vec![target("Tiny", "tiny")])],
        )
        .await;
        let tree = f.tree(|t| rows(t) == 3).await;
        let moved = &child(&tree, "View").unwrap().children[1];
        assert_eq!((moved.id.0, moved.label.as_str()), (large, "Small"));

        // a token for no choice runs nothing; the choice's runs Large's action
        f.worker
            .send(Request::ActivateChosen {
                click: 8,
                token: None,
            })
            .unwrap();
        f.worker
            .send(Request::ActivateChosen {
                click: 7,
                token: Some("token-7".into()),
            })
            .unwrap();
        assert_eq!(
            f.call().await,
            Call::Activate {
                path: WINDOW,
                name: "size".into(),
                parameter: vec!["large".into()],
                platform_data: vec![
                    ("activation-token".into(), "token-7".into()),
                    ("desktop-startup-id".into(), "token-7".into()),
                ],
            }
        );
    });
}

#[test]
fn a_signal_applies_a_bounded_number_of_records() {
    async_io::block_on(async {
        let menus = HashMap::from([
            ((0, 0), vec![submenu("Edit", 1, 0)]),
            ((1, 0), vec![row("Undo")]),
        ]);
        let f = Fixture::new(menus, HashMap::new(), HashMap::new()).await;
        let edit = |t: &MenuItem| child(t, "Edit").map(labels).unwrap_or_default();
        f.tree(|t| edit(t) == ["Undo"]).await;

        // records past the bound are dropped: the last of this signal is one of them
        let mut records: Vec<(u32, u32, u32, u32, Vec<Item>)> = (0..MAX_SIGNAL_RECORDS)
            .map(|_| (1, 0, 5, 1, Vec::new()))
            .collect();
        records.push((1, 0, 1, 0, vec![row("Dropped")]));
        f.change_menus(|_| {}, records).await;
        f.change_menus(
            |m| m.get_mut(&(1, 0)).unwrap().push(row("Redo")),
            vec![(1, 0, 1, 0, vec![row("Redo")])],
        )
        .await;
        let tree = f.tree(|t| edit(t).contains(&"Redo".to_owned())).await;
        assert_eq!(edit(&tree), ["Undo", "Redo"]);
    });
}

fn state(enabled: bool, parameter: Option<&str>, state: Option<Value<'_>>) -> GtkActionState {
    GtkActionState::from_raw(&action(enabled, parameter.unwrap_or(""), state))
}

#[test]
fn the_tree_hides_marks_and_names_items_as_a_menu_tracker_does() {
    let hidden = |label: &str, action: &str, when: &str| {
        item([
            ("label", Value::from(label)),
            ("action", Value::from(action)),
            ("hidden-when", Value::from(when)),
        ])
    };
    let themed = Value::from(("themed", Value::from(vec!["document-save", "document"])));
    let rows = vec![
        hidden("Gone", "app.missing", "action-missing"),
        hidden("Off", "app.off", "action-disabled"),
        hidden("Also gone", "app.missing", "action-disabled"),
        hidden("Elsewhere", "app.save", "macos-menubar"),
        item([
            ("label", Value::from("_Save as __draft")),
            ("action", Value::from("app.save")),
            ("accel", Value::from("<Primary>s")),
            ("icon", themed),
        ]),
        // a target the action takes no parameter for
        item([
            ("label", Value::from("Mismatch")),
            ("action", Value::from("app.save")),
            ("target", Value::from(1_i32)),
        ]),
        item([
            ("label", Value::from("Level 2")),
            ("action", Value::from("app.level")),
            ("target", Value::from(2_i32)),
        ]),
        item([
            ("label", Value::from("Spaced")),
            ("action", Value::from("unity.spaced")),
            ("verb-icon", Value::from("zoom-in")),
        ]),
        // a link's namespace prefixes the actions of the menu it links to, not its own
        item([
            (":section", Value::from((0_u32, 2_u32))),
            ("action-namespace", Value::from("app")),
        ]),
    ];
    let reply = vec![
        (0, 0, vec![submenu("File", 0, 1)]),
        (0, 1, rows),
        (0, 2, vec![action_row("Print", "print")]),
    ];
    let menus = GtkMenus::from_reply(&[0], reply);
    let actions: GtkActions = HashMap::from([
        ("app.off".to_owned(), state(false, None, None)),
        ("app.save".to_owned(), state(true, None, None)),
        ("app.print".to_owned(), state(true, None, None)),
        (
            "app.level".to_owned(),
            state(true, Some("i"), Some(Value::from(2_i32))),
        ),
    ]);
    let built = build_menus(&menus, &GtkMenus::default(), &actions);
    let file = &built.items[0];
    assert_eq!(
        labels(file),
        [
            "Save as _draft",
            "Mismatch",
            "Level 2",
            "Spaced",
            "-",
            "Print"
        ]
    );
    let rows = &file.children;
    assert_eq!(rows[0].mnemonic, Some('S'));
    assert_eq!(rows[0].accel.as_deref(), Some("<Primary>s"));
    assert_eq!(rows[0].icon_name.as_deref(), Some("document-save"));
    assert!(rows[0].enabled);
    assert!(
        !rows[1].enabled,
        "a target the action does not take disables the item"
    );
    assert_eq!(rows[2].toggle, Toggle::Radio(true));
    assert!(
        !rows[3].enabled,
        "an action of another prefix belongs to no object"
    );
    assert_eq!(rows[3].icon_name.as_deref(), Some("zoom-in"));
    assert!(rows[5].enabled, "the section's namespace names its action");
    assert_eq!(rows[5].action.as_deref(), Some("app.print"));
}
