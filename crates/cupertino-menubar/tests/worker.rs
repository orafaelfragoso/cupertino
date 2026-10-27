//! The session bus worker against a private session bus (`dbus-daemon --session`); skipped when
//! the daemon is not available.
#![cfg(feature = "app")]

use cupertino_menubar::worker::{Request, StartError, Worker};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use zbus::zvariant::{ObjectPath, OwnedObjectPath};
use zbus::{block_on, connection, Connection};

struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout)
            .ok()
            .filter(|_| out.status.success())?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next()?.to_string(),
            pid: lines.next()?.to_string(),
        })
    }

    fn connect(&self) -> Connection {
        block_on(
            connection::Builder::address(self.address.as_str())
                .expect("address")
                .build(),
        )
        .expect("connect")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        Command::new("kill")
            .arg(&self.pid)
            .status()
            .expect("kill the private bus");
    }
}

/// Stands in for the notification process.
struct FakeCentre(mpsc::Sender<()>);

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl FakeCentre {
    fn toggle_centre(&self) {
        self.0.send(()).unwrap();
    }
}

#[test]
fn the_worker_owns_the_watcher_and_relays_the_centre_toggle() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (worker, _events) = Worker::start(bus.connect(), || {}).unwrap();

    // the clock click reaches the notification process
    let (toggled_tx, toggled_rx) = mpsc::channel();
    let centre = bus.connect();
    block_on(async {
        centre
            .object_server()
            .at("/org/cupertino/NotificationCenter1", FakeCentre(toggled_tx))
            .await
            .unwrap();
        centre
            .request_name("org.cupertino.NotificationCenter1")
            .await
            .unwrap();
    });
    worker.send(Request::ToggleCentre).unwrap();
    toggled_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    // a second process cannot take the watcher's name
    let second = Worker::start(bus.connect(), || {});
    assert!(
        matches!(second, Err(StartError::Watcher(zbus::Error::NameTaken))),
        "{:?}",
        second.err()
    );
}

#[test]
fn the_worker_owns_the_app_menu_registrar_unless_another_process_does() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let other = bus.connect();
    let get_menu = |conn: &Connection| -> (String, OwnedObjectPath) {
        block_on(conn.call_method(
            Some("com.canonical.AppMenu.Registrar"),
            "/com/canonical/AppMenu/Registrar",
            Some("com.canonical.AppMenu.Registrar"),
            "GetMenuForWindow",
            &(7u32,),
        ))
        .unwrap()
        .body()
        .deserialize()
        .unwrap()
    };

    let (_worker, _events) = Worker::start(bus.connect(), || {}).unwrap();
    block_on(other.call_method(
        Some("com.canonical.AppMenu.Registrar"),
        "/com/canonical/AppMenu/Registrar",
        Some("com.canonical.AppMenu.Registrar"),
        "RegisterWindow",
        &(7u32, ObjectPath::from_static_str_unchecked("/MenuBar/1")),
    ))
    .unwrap();
    assert_eq!(
        get_menu(&other),
        (String::new(), OwnedObjectPath::try_from("/").unwrap()),
        "registrations are not kept"
    );

    // a registrar of another desktop keeps its name and the worker starts all the same
    let second_bus = PrivateBus::start().unwrap();
    let foreign = second_bus.connect();
    block_on(foreign.request_name("com.canonical.AppMenu.Registrar")).unwrap();
    assert!(Worker::start(second_bus.connect(), || {}).is_ok());
    let owner: String = block_on(foreign.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetNameOwner",
        &("com.canonical.AppMenu.Registrar",),
    ))
    .unwrap()
    .body()
    .deserialize()
    .unwrap();
    assert_eq!(owner, foreign.unique_name().unwrap().as_str());
}
