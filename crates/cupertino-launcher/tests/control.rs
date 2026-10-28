//! The control interface against a private session bus (`dbus-daemon --session`); skipped when the
//! daemon is not available.
#![cfg(feature = "app")]

use cupertino_launcher::control::{serve, BUS_NAME, OBJECT_PATH};
use cupertino_test_bus::PrivateBus;
use std::sync::mpsc;
use std::time::Duration;
use zbus::{block_on, Connection};

#[test]
fn toggle_reaches_the_launcher_and_a_second_launcher_cannot_take_the_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (toggled_tx, toggled_rx) = mpsc::channel();
    let _launcher =
        serve(bus.builder(), move || toggled_tx.send(()).unwrap()).expect("claims the name");

    // what the server does: call `Toggle` on the well-known name
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    block_on(caller.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some("org.cupertino.Launcher1"),
        "Toggle",
        &(),
    ))
    .expect("Toggle");
    toggled_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the toggle callback ran");
    assert_eq!(toggled_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    let second = serve(bus.builder(), || {});
    assert!(
        matches!(second, Err(zbus::Error::NameTaken)),
        "{:?}",
        second.err()
    );
}
