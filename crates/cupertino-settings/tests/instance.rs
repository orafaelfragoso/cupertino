//! ST-24: one settings process per session through `org.cupertino.Settings1`, on a private session bus.
#![cfg(feature = "app")]

mod common;

use std::sync::mpsc;
use std::time::Duration;

use cupertino_settings::instance::{serve, show};
use cupertino_settings::PaneId;

#[test]
fn show_pane_reaches_the_owner_and_rejects_an_unknown_id() {
    let Some(bus) = common::bus() else { return };
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let _owner = serve(bus.builder(), move |pane| {
        tx.lock().unwrap().send(pane).unwrap()
    })
    .expect("claims the name");
    assert!(
        matches!(serve(bus.builder(), |_| {}), Err(zbus::Error::NameTaken)),
        "a second owner is refused"
    );
    show(bus.builder(), "keyboard").expect("a known pane");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok(PaneId::Keyboard)
    );
    match show(bus.builder(), "general") {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.InvalidArgs")
        }
        other => panic!("an unknown pane is InvalidArgs, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "nothing is shown for it");
}

#[test]
fn a_second_start_hands_its_pane_to_the_first_and_exits_0() {
    let Some(bus) = common::bus() else { return };
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let _first = serve(bus.builder(), move |pane| {
        tx.lock().unwrap().send(pane).unwrap()
    })
    .expect("claims the name");
    // no display: the second start must not need one
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_cupertino-settings"))
        .args(["--pane", "sound"])
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env_remove("WAYLAND_DISPLAY")
        .status()
        .expect("the binary runs");
    assert!(status.success(), "{status}");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(PaneId::Sound));
}
