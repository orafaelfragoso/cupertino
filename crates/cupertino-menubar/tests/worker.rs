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

/// Serves a [`FakeCentre`] on `bus`; [`Settled::wait`] returns once the worker has handled every earlier request.
struct Settled {
    _centre: Connection,
    toggled: mpsc::Receiver<()>,
}

impl Settled {
    fn on(bus: &PrivateBus) -> Settled {
        let (tx, toggled) = mpsc::channel();
        let centre = bus.connect();
        block_on(async {
            centre
                .object_server()
                .at("/org/cupertino/NotificationCenter1", FakeCentre(tx))
                .await
                .unwrap();
            centre
                .request_name("org.cupertino.NotificationCenter1")
                .await
                .unwrap();
        });
        Settled {
            _centre: centre,
            toggled,
        }
    }

    fn wait(&self, worker: &Worker) {
        worker.send(Request::ToggleCentre).unwrap();
        self.toggled.recv_timeout(Duration::from_secs(5)).unwrap();
    }
}

fn registrar_owner(conn: &Connection) -> Option<String> {
    block_on(conn.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetNameOwner",
        &("com.canonical.AppMenu.Registrar",),
    ))
    .ok()
    .map(|reply| reply.body().deserialize().unwrap())
}

#[test]
fn the_worker_owns_the_app_menu_registrar_only_while_asked_and_unless_another_process_does() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let other = bus.connect();
    let settled = Settled::on(&bus);
    let worker_conn = bus.connect();
    let worker_name = worker_conn.unique_name().unwrap().to_string();
    let (worker, _events) = Worker::start(worker_conn, || {}).unwrap();
    assert_eq!(registrar_owner(&other), None, "no bar yet");

    worker.send(Request::AppMenuRegistrar(true)).unwrap();
    settled.wait(&worker);
    assert_eq!(
        registrar_owner(&other).as_deref(),
        Some(worker_name.as_str())
    );
    block_on(other.call_method(
        Some("com.canonical.AppMenu.Registrar"),
        "/com/canonical/AppMenu/Registrar",
        Some("com.canonical.AppMenu.Registrar"),
        "RegisterWindow",
        &(7u32, ObjectPath::from_static_str_unchecked("/MenuBar/1")),
    ))
    .unwrap();
    let menu: (String, OwnedObjectPath) = block_on(other.call_method(
        Some("com.canonical.AppMenu.Registrar"),
        "/com/canonical/AppMenu/Registrar",
        Some("com.canonical.AppMenu.Registrar"),
        "GetMenuForWindow",
        &(7u32,),
    ))
    .unwrap()
    .body()
    .deserialize()
    .unwrap();
    assert_eq!(
        menu,
        (String::new(), OwnedObjectPath::try_from("/").unwrap()),
        "registrations are not kept"
    );

    worker.send(Request::AppMenuRegistrar(false)).unwrap();
    settled.wait(&worker);
    assert_eq!(registrar_owner(&other), None, "the last bar went");

    worker.send(Request::AppMenuRegistrar(true)).unwrap();
    settled.wait(&worker);
    assert_eq!(
        registrar_owner(&other).as_deref(),
        Some(worker_name.as_str())
    );

    // a registrar of another desktop keeps its name, also when the worker gives the name up
    let second_bus = PrivateBus::start().unwrap();
    let foreign = second_bus.connect();
    block_on(foreign.request_name("com.canonical.AppMenu.Registrar")).unwrap();
    let foreign_name = foreign.unique_name().unwrap().to_string();
    let second_settled = Settled::on(&second_bus);
    let (second, _events) = Worker::start(second_bus.connect(), || {}).unwrap();
    for owned in [true, false] {
        second.send(Request::AppMenuRegistrar(owned)).unwrap();
        second_settled.wait(&second);
        assert_eq!(
            registrar_owner(&foreign).as_deref(),
            Some(foreign_name.as_str())
        );
    }
}
