//! The session bus worker against a private session bus (`dbus-daemon --session`); skipped when
//! the daemon is not available.
#![cfg(feature = "app")]

use cupertino_menubar::worker::{Request, StartError, Worker};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use zbus::{block_on, connection, Connection};

struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon").args(["--session", "--print-address=1", "--print-pid=1", "--fork"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let text = String::from_utf8(out.stdout).ok().filter(|_| out.status.success())?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus { address: lines.next()?.to_string(), pid: lines.next()?.to_string() })
    }

    fn connect(&self) -> Connection {
        block_on(connection::Builder::address(self.address.as_str()).expect("address").build()).expect("connect")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        Command::new("kill").arg(&self.pid).status().expect("kill the private bus");
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
        centre.object_server().at("/org/cupertino/NotificationCenter1", FakeCentre(toggled_tx)).await.unwrap();
        centre.request_name("org.cupertino.NotificationCenter1").await.unwrap();
    });
    worker.send(Request::ToggleCentre).unwrap();
    toggled_rx.recv_timeout(Duration::from_secs(5)).unwrap();

    // a second process cannot take the watcher's name
    let second = Worker::start(bus.connect(), || {});
    assert!(matches!(second, Err(StartError::Watcher(zbus::Error::NameTaken))), "{:?}", second.err());
}
