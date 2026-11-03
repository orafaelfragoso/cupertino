//! The exporter on a private bus: a private `dbus-daemon` is started for each test (skipped when the binary is not
//! installed); one connection exports, a second reads the menus as the menu bar does. Every wait has a deadline: the
//! daemon's address, each method call and each signal.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use cupertino_menu_model::standard::{self, Features};
use cupertino_menu_model::ActionName;
use cupertino_modifiers::Profile;
use cupertino_test_bus::PrivateBus;
use cupertino_ui_menus::{Activation, AppMenus, DbusProperties, Exporter, WindowExport, WindowId};
use zbus::blocking::proxy::SignalIterator;
use zbus::blocking::{Connection, Proxy};
use zbus::message::Message;
use zbus::zvariant::{OwnedValue, Signature, Value};

const WAIT: Duration = Duration::from_secs(5);

/// The first message of `signals`, or a panic after [`WAIT`].
fn next_signal(mut signals: SignalIterator<'static>) -> Message {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        if let Some(message) = signals.next() {
            let _ = tx.send(message);
        }
    });
    rx.recv_timeout(WAIT).expect("no signal within 5 s")
}

macro_rules! private_bus {
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

struct Fixture {
    bus: PrivateBus,
    exporter: Exporter,
    reader: Connection,
    activations: mpsc::Receiver<Activation>,
    props: DbusProperties,
}

fn features() -> Features {
    Features {
        about: true,
        settings: true,
        new: Some("New Note".to_string()),
        save: true,
        ..Features::default()
    }
}

fn fixture(bus: PrivateBus) -> Fixture {
    let (tx, activations) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let mut exporter = Exporter::new(
        bus.blocking_builder(),
        "org.example.Notes",
        Box::new(Profile::cupertino()),
        move |a| {
            tx.lock().unwrap().send(a).unwrap();
        },
    )
    .unwrap();
    let props = exporter
        .export(WindowId(1), &standard::menu_bar("Notes", &features()))
        .unwrap();
    let reader = bus.connect();
    exporter
        .set_reader(Some(reader.unique_name().unwrap().as_str()))
        .unwrap();
    Fixture {
        reader,
        bus,
        exporter,
        activations,
        props,
    }
}

fn proxy(f: &Fixture, path: &str, interface: &str) -> Proxy<'static> {
    proxy_on(&f.reader, f, path, interface)
}

fn proxy_on(conn: &Connection, f: &Fixture, path: &str, interface: &str) -> Proxy<'static> {
    Proxy::new(
        conn,
        f.props.unique_bus_name.clone(),
        path.to_string(),
        interface.to_string(),
    )
    .unwrap()
}

type RawMenus = Vec<(u32, u32, Vec<HashMap<String, OwnedValue>>)>;

fn text(item: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    item.get(key)
        .map(|v| <&str>::try_from(v).unwrap().to_string())
}

fn link(item: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    item.get(key)
        .map(|v| <(u32, u32)>::try_from(v.try_clone().unwrap()).unwrap().1)
}

/// The menu `id` read as the menu bar reads it: sections flattened with `---` between them, submenus as
/// `Title ▸` followed by their items indented, items as `label action accel`.
fn outline(
    menus: &HashMap<u32, Vec<HashMap<String, OwnedValue>>>,
    id: u32,
    indent: &str,
    out: &mut Vec<String>,
) {
    let mut first = true;
    for item in &menus[&id] {
        if let Some(section) = link(item, ":section") {
            if !first {
                out.push(format!("{indent}---"));
            }
            outline(menus, section, indent, out);
        } else if let Some(sub) = link(item, ":submenu") {
            out.push(format!("{indent}{} ▸", text(item, "label").unwrap()));
            outline(menus, sub, &format!("{indent}  "), out);
        } else {
            let line = [
                text(item, "label"),
                text(item, "action"),
                text(item, "accel"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
            out.push(format!("{indent}{line}"));
        }
        first = false;
    }
}

fn read_menus(f: &Fixture, path: &str) -> Vec<String> {
    let reply: RawMenus = proxy(f, path, "org.gtk.Menus")
        .call("Start", &(vec![0u32],))
        .unwrap();
    let menus = reply.into_iter().map(|(_, m, items)| (m, items)).collect();
    let mut out = Vec::new();
    outline(&menus, 0, "", &mut out);
    out
}

type Described = HashMap<String, (bool, Signature, Vec<OwnedValue>)>;

fn describe_all(f: &Fixture, path: &str) -> Vec<(String, bool)> {
    let all: Described = proxy(f, path, "org.gtk.Actions")
        .call("DescribeAll", &())
        .unwrap();
    let mut all: Vec<(String, bool)> = all.into_iter().map(|(k, (e, _, _))| (k, e)).collect();
    all.sort();
    all
}

fn activate(f: &Fixture, path: &str, name: &str, parameter: Vec<Value<'_>>) -> zbus::Result<()> {
    let platform_data: HashMap<&str, Value<'_>> = HashMap::new();
    proxy(f, path, "org.gtk.Actions")
        .call::<_, _, ()>("Activate", &(name, parameter, platform_data))
}

#[test]
fn a_window_announces_its_objects() {
    let f = fixture(private_bus!());
    assert_eq!(f.props.application_id, "org.example.Notes");
    assert_eq!(f.props.application_object_path, "/org/example/Notes");
    assert_eq!(f.props.app_menu_path, "/org/example/Notes/menus/appmenu");
    assert_eq!(f.props.window_object_path, "/org/example/Notes/window/1");
    assert_eq!(
        f.props.menubar_path,
        "/org/example/Notes/window/1/menus/menubar"
    );
    assert!(f.props.unique_bus_name.starts_with(':'));
}

#[test]
fn the_reader_gets_the_application_menu_and_the_menu_bar() {
    let f = fixture(private_bus!());
    assert_eq!(
        read_menus(&f, &f.props.app_menu_path),
        [
            "About Notes app.about",
            "---",
            "Settings… app.preferences <Super>comma",
            "---",
            "Quit Notes app.quit <Super>q",
        ]
    );
    assert_eq!(
        read_menus(&f, &f.props.menubar_path),
        [
            "File ▸",
            "  New Note app.new <Super>n",
            "  ---",
            "  Close Window win.close <Super>w",
            "  Save win.save <Super>s",
            "Edit ▸",
            "  Undo win.undo <Super>z",
            "  Redo win.redo <Shift><Super>z",
            "  ---",
            "  Cut win.cut <Super>x",
            "  Copy win.copy <Super>c",
            "  Paste win.paste <Super>v",
            "  Delete win.delete",
            "  Select All win.select-all <Super>a",
            "View ▸",
            "  Enter Full Screen win.fullscreen <Super><Control>f",
        ]
    );
    let enabled = |names: &[&str]| {
        names
            .iter()
            .map(|n| (n.to_string(), true))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        describe_all(&f, &f.props.application_object_path),
        enabled(&["about", "new", "preferences", "quit"])
    );
    assert_eq!(
        describe_all(&f, &f.props.window_object_path),
        enabled(&[
            "close",
            "copy",
            "cut",
            "delete",
            "fullscreen",
            "paste",
            "redo",
            "save",
            "select-all",
            "undo"
        ])
    );
}

#[test]
fn chosen_items_reach_the_application() {
    let f = fixture(private_bus!());
    activate(&f, &f.props.window_object_path, "save", vec![]).unwrap();
    assert_eq!(
        f.activations.recv_timeout(WAIT).unwrap(),
        Activation {
            window: Some(WindowId(1)),
            action: ActionName::parse("win.save").unwrap(),
        }
    );
    activate(&f, &f.props.application_object_path, "quit", vec![]).unwrap();
    assert_eq!(
        f.activations.recv_timeout(WAIT).unwrap(),
        Activation {
            window: None,
            action: ActionName::parse("app.quit").unwrap(),
        }
    );
    assert!(activate(&f, &f.props.window_object_path, "quit", vec![]).is_err());
    assert!(activate(
        &f,
        &f.props.window_object_path,
        "save",
        vec![Value::from(1u32)]
    )
    .is_err());
    assert!(f.activations.try_recv().is_err());
}

#[test]
fn a_disabled_action_is_announced_and_does_nothing() {
    let f = fixture(private_bus!());
    let actions = proxy(&f, &f.props.window_object_path, "org.gtk.Actions");
    let changed = actions.receive_signal("Changed").unwrap();
    f.exporter
        .set_enabled(WindowId(1), &ActionName::parse("win.undo").unwrap(), false)
        .unwrap();
    let signal = next_signal(changed);
    let (removed, enabled, _, added): (
        Vec<String>,
        HashMap<String, bool>,
        HashMap<String, OwnedValue>,
        Described,
    ) = signal.body().deserialize().unwrap();
    assert_eq!(
        (removed, enabled, added.len()),
        (vec![], [("undo".to_string(), false)].into(), 0)
    );
    assert!(describe_all(&f, &f.props.window_object_path).contains(&("undo".to_string(), false)));
    activate(&f, &f.props.window_object_path, "undo", vec![]).unwrap();
    assert!(f
        .activations
        .recv_timeout(Duration::from_millis(200))
        .is_err());
}

#[test]
fn a_profile_change_rewrites_the_accelerators() {
    let mut f = fixture(private_bus!());
    let menus = proxy(&f, &f.props.menubar_path, "org.gtk.Menus");
    let changed = menus.receive_signal("Changed").unwrap();
    f.exporter
        .set_modifier_map(Box::new(Profile::classic_pc()))
        .unwrap();
    next_signal(changed);
    let bar = read_menus(&f, &f.props.menubar_path);
    assert!(
        bar.contains(&"  New Note app.new <Control>n".to_string()),
        "{bar:?}"
    );
    assert!(
        bar.contains(&"  Enter Full Screen win.fullscreen <Control><Super>f".to_string()),
        "{bar:?}"
    );
    assert!(read_menus(&f, &f.props.app_menu_path)
        .contains(&"Quit Notes app.quit <Control>q".to_string()));
}

#[test]
fn a_closed_window_withdraws_its_menus() {
    let mut f = fixture(private_bus!());
    let second = f
        .exporter
        .export(
            WindowId(2),
            &standard::menu_bar("Notes", &Features::default()),
        )
        .unwrap();
    f.exporter.remove(WindowId(1)).unwrap();
    let start: zbus::Result<RawMenus> =
        proxy(&f, &f.props.menubar_path, "org.gtk.Menus").call("Start", &(vec![0u32],));
    assert!(start.is_err());
    assert_eq!(
        read_menus(&f, &f.props.app_menu_path),
        ["Quit Notes app.quit <Super>q"]
    );
    assert_eq!(
        describe_all(&f, &second.application_object_path),
        [("quit".to_string(), true)]
    );
}

/// The calls a reader makes, each from `conn`: whether every one was refused with `AccessDenied`.
fn all_refused(conn: &Connection, f: &Fixture) -> bool {
    let denied = |r: zbus::Error| matches!(r, zbus::Error::MethodError(ref name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied");
    let menus = proxy_on(conn, f, &f.props.menubar_path, "org.gtk.Menus");
    let actions = proxy_on(conn, f, &f.props.window_object_path, "org.gtk.Actions");
    let platform_data: HashMap<&str, Value<'_>> = HashMap::new();
    [
        menus.call::<_, _, RawMenus>("Start", &(vec![0u32],)).err(),
        menus.call::<_, _, ()>("End", &(vec![0u32],)).err(),
        actions.call::<_, _, Vec<String>>("List", &()).err(),
        actions
            .call::<_, _, (bool, Signature, Vec<OwnedValue>)>("Describe", &("save",))
            .err(),
        actions.call::<_, _, Described>("DescribeAll", &()).err(),
        actions
            .call::<_, _, ()>(
                "Activate",
                &("save", Vec::<Value<'_>>::new(), &platform_data),
            )
            .err(),
        actions
            .call::<_, _, ()>("SetState", &("save", Value::from(true), &platform_data))
            .err(),
    ]
    .into_iter()
    .all(|e| e.is_some_and(denied))
}

#[test]
fn only_the_menu_bar_the_compositor_named_is_answered() {
    let f = fixture(private_bus!());
    let other = f.bus.connect();
    assert!(all_refused(&other, &f));
    assert!(f.activations.try_recv().is_err());
    // the reader is still answered
    activate(&f, &f.props.window_object_path, "save", vec![]).unwrap();
    assert!(f.activations.recv_timeout(WAIT).is_ok());
}

#[test]
fn without_a_reader_every_call_is_refused() {
    let f = fixture(private_bus!());
    f.exporter.set_reader(None).unwrap();
    assert!(all_refused(&f.reader, &f));
    assert!(f.activations.try_recv().is_err());
}

#[test]
fn a_new_reader_takes_the_right_over() {
    let f = fixture(private_bus!());
    let next = f.bus.connect();
    f.exporter
        .set_reader(Some(next.unique_name().unwrap().as_str()))
        .unwrap();
    assert!(all_refused(&f.reader, &f));
    let platform_data: HashMap<&str, Value<'_>> = HashMap::new();
    proxy_on(&next, &f, &f.props.window_object_path, "org.gtk.Actions")
        .call::<_, _, ()>(
            "Activate",
            &("save", Vec::<Value<'_>>::new(), platform_data),
        )
        .unwrap();
    assert!(f.activations.recv_timeout(WAIT).is_ok());
    assert!(f
        .exporter
        .set_reader(Some("org.example.NotAUniqueName"))
        .is_err());
}

#[test]
fn changes_are_signalled_to_the_reader_alone() {
    let f = fixture(private_bus!());
    let other = f.bus.connect();
    let theirs = proxy_on(&other, &f, &f.props.window_object_path, "org.gtk.Actions");
    let ours = proxy(&f, &f.props.window_object_path, "org.gtk.Actions");
    let (tx, overheard) = mpsc::channel();
    let mut their_changes = theirs.receive_signal("Changed").unwrap();
    std::thread::spawn(move || {
        if their_changes.next().is_some() {
            let _ = tx.send(());
        }
    });
    let mut our_changes = ours.receive_signal("Changed").unwrap();
    f.exporter
        .set_enabled(WindowId(1), &ActionName::parse("win.undo").unwrap(), false)
        .unwrap();
    our_changes.next().unwrap();
    assert!(overheard.recv_timeout(Duration::from_millis(300)).is_err());
}

/// The unique names on `bus`, as `ListNames` gives them.
fn unique_names(bus: &PrivateBus) -> usize {
    let names: Vec<String> = Proxy::new(
        &bus.connect(),
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .unwrap()
    .call("ListNames", &())
    .unwrap();
    // without the connection that asked
    names.iter().filter(|n| n.starts_with(':')).count() - 1
}

/// Three windows of one application, each waking its own channel.
struct Windows {
    windows: Vec<(WindowExport, DbusProperties, mpsc::Receiver<()>)>,
    reader: Connection,
}

fn three_windows(bus: &PrivateBus) -> Windows {
    let address = bus.address.clone();
    let mut apps = AppMenus::on(move || {
        zbus::blocking::connection::Builder::address(address.as_str())
            .map(|b| b.method_timeout(WAIT))
    });
    let windows: Vec<_> = (0..3)
        .map(|_| {
            let (woken, wakes) = mpsc::channel();
            let export = apps
                .open(
                    "org.example.Notes",
                    Box::new(Profile::cupertino()),
                    move || {
                        let _ = woken.send(());
                    },
                )
                .unwrap();
            let props = export
                .export(&standard::menu_bar("Notes", &features()))
                .unwrap();
            (export, props, wakes)
        })
        .collect();
    let reader = bus.connect();
    windows[0]
        .0
        .set_reader(Some(reader.unique_name().unwrap().as_str()))
        .unwrap();
    Windows { windows, reader }
}

fn activate_at(reader: &Connection, props: &DbusProperties, name: &str) -> zbus::Result<()> {
    let platform_data: HashMap<&str, Value<'_>> = HashMap::new();
    Proxy::new(
        reader,
        props.unique_bus_name.clone(),
        props.window_object_path.clone(),
        "org.gtk.Actions",
    )
    .unwrap()
    .call::<_, _, ()>("Activate", &(name, Vec::<Value<'_>>::new(), platform_data))
}

/// The actions chosen in each window once window `at` was woken.
fn chosen(w: &Windows, at: usize) -> Vec<Vec<String>> {
    w.windows[at].2.recv_timeout(WAIT).unwrap();
    w.windows
        .iter()
        .map(|(export, _, _)| export.activations().map(|a| a.action.full()).collect())
        .collect()
}

#[test]
fn the_windows_of_an_application_share_one_connection_and_each_has_its_own_objects() {
    let bus = private_bus!();
    let before = unique_names(&bus);
    let w = three_windows(&bus);
    assert_eq!(
        unique_names(&bus),
        before + 2,
        "the exporter and the reader"
    );

    let props: Vec<&DbusProperties> = w.windows.iter().map(|(_, p, _)| p).collect();
    assert!(props
        .iter()
        .all(|p| p.unique_bus_name == props[0].unique_bus_name));
    assert_eq!(
        props
            .iter()
            .map(|p| p.window_object_path.as_str())
            .collect::<Vec<_>>(),
        [
            "/org/example/Notes/window/1",
            "/org/example/Notes/window/2",
            "/org/example/Notes/window/3"
        ]
    );

    activate_at(&w.reader, props[1], "save").unwrap();
    assert_eq!(
        chosen(&w, 1),
        [vec![], vec!["win.save".to_string()], vec![]]
    );
}

#[test]
fn a_closed_window_takes_only_its_own_objects_and_app_actions_go_to_the_newest_window() {
    let bus = private_bus!();
    let mut w = three_windows(&bus);
    let (first, first_props, _) = w.windows.remove(0);
    drop(first);

    assert!(activate_at(&w.reader, &first_props, "save").is_err());
    activate_at(&w.reader, &w.windows[0].1, "save").unwrap();
    assert_eq!(chosen(&w, 0), [vec!["win.save".to_string()], vec![]]);

    let app = DbusProperties {
        window_object_path: first_props.application_object_path.clone(),
        ..first_props
    };
    activate_at(&w.reader, &app, "quit").unwrap();
    assert_eq!(chosen(&w, 1), [vec![], vec!["app.quit".to_string()]]);
}
