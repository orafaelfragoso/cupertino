//! The exporter on a private bus: a private `dbus-daemon` is started for each test (skipped when the binary is not
//! installed); one connection exports, a second reads the menus as the menu bar does. Every wait has a deadline: the
//! daemon's address, each method call and each signal.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use cupertino_menu_model::standard::{self, Features};
use cupertino_menu_model::ActionName;
use cupertino_modifiers::Profile;
use cupertino_ui_menus::{Activation, DbusProperties, Exporter, WindowId};
use zbus::blocking::proxy::SignalIterator;
use zbus::blocking::{connection, Connection, Proxy};
use zbus::message::Message;
use zbus::zvariant::{OwnedValue, Signature, Value};

const WAIT: Duration = Duration::from_secs(5);

/// A private `dbus-daemon`, a child of the test that is killed on drop and dies with the test's thread if the test is
/// killed first.
struct PrivateBus {
    address: String,
    daemon: Child,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let mut command = Command::new("dbus-daemon");
        command
            .args(["--session", "--nofork", "--print-address=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // Safety: the closure runs in the child between fork and exec and calls only prctl, which is
        // async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut daemon = command.spawn().ok()?;
        let stdout = daemon.stdout.take().expect("piped stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = BufReader::new(stdout).read_line(&mut line);
            let _ = tx.send(line);
        });
        let address = rx
            .recv_timeout(WAIT)
            .expect("dbus-daemon printed no address within 5 s");
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon exited before it listened");
        Some(PrivateBus { address, daemon })
    }

    fn builder(&self) -> connection::Builder<'_> {
        connection::Builder::address(self.address.as_str())
            .unwrap()
            .method_timeout(WAIT)
    }

    fn connect(&self) -> Connection {
        self.builder().build().unwrap()
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

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
    _bus: PrivateBus,
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
        bus.builder(),
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
    Fixture {
        reader: bus.connect(),
        _bus: bus,
        exporter,
        activations,
        props,
    }
}

fn proxy(f: &Fixture, path: &str, interface: &str) -> Proxy<'static> {
    Proxy::new(
        &f.reader,
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
