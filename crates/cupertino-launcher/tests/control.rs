//! The control interface against a private session bus (`dbus-daemon --session`); skipped when the
//! daemon is not available.
#![cfg(feature = "app")]

use cupertino_launcher::control::{serve, BUS_NAME, OBJECT_PATH};
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

    fn builder(&self) -> connection::Builder<'static> {
        connection::Builder::address(self.address.as_str()).expect("address")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        Command::new("kill").arg(&self.pid).status().expect("kill the private bus");
    }
}

#[test]
fn toggle_reaches_the_launcher_and_a_second_launcher_cannot_take_the_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (toggled_tx, toggled_rx) = mpsc::channel();
    let _launcher = serve(bus.builder(), move || toggled_tx.send(()).unwrap()).expect("claims the name");

    // what the server does: call `Toggle` on the well-known name
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    block_on(caller.call_method(Some(BUS_NAME), OBJECT_PATH, Some("org.cupertino.Launcher1"), "Toggle", &())).expect("Toggle");
    toggled_rx.recv_timeout(Duration::from_secs(5)).expect("the toggle callback ran");
    assert_eq!(toggled_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    let second = serve(bus.builder(), || {});
    assert!(matches!(second, Err(zbus::Error::NameTaken)), "{:?}", second.err());
}
