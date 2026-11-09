//! ST-24: one settings process per session through `org.cupertino.Settings1`, on a private session bus.
#![cfg(feature = "app")]

mod common;

use std::sync::mpsc;
use std::time::Duration;

use cupertino_settings::instance::{serve, show};
use cupertino_settings::PaneId;

#[test]
fn about_has_an_independent_owner_and_only_its_own_program_receives_launches() {
    let bus = common::bus().expect("dbus-daemon is needed to test About instance handling");
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let _settings = serve(bus.builder(), |_, _| {
        panic!("About must not forward to a Settings pane")
    })
    .unwrap();
    let _about = cupertino_settings::instance::serve_about(bus.builder(), move |token| {
        tx.lock().unwrap().send(token).unwrap()
    })
    .unwrap();
    assert!(matches!(
        cupertino_settings::instance::serve_about(bus.builder(), |_| {}),
        Err(zbus::Error::NameTaken)
    ));
    cupertino_settings::instance::show_about(bus.builder(), "example-token").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok("example-token".into())
    );
    assert!(cupertino_settings::instance::show_about(bus.builder(), &"x".repeat(4097)).is_err());
    assert!(rx.try_recv().is_err());
    // what the process records goes to a store of the test's own
    let state = tempfile::tempdir().unwrap();
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_cupertino-settings"))
        .arg("--about")
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("XDG_STATE_HOME", state.path())
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("XDG_ACTIVATION_TOKEN")
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "a launch never hands its activation token to an About of another program"
    );
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
    drop(_about);
    // the bus releases a closed connection's names on its own time; claim the name again once it is free
    let watcher = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .unwrap();
    let daemon = zbus::blocking::fdo::DBusProxy::new(&watcher).unwrap();
    let about =
        zbus::names::BusName::try_from(cupertino_settings::instance::ABOUT_BUS_NAME).unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while daemon.name_has_owner(about.clone()).unwrap() {
        assert!(
            std::time::Instant::now() < end,
            "the dropped About owner keeps its name"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let _replacement = cupertino_settings::instance::serve_about(bus.builder(), |_| {}).unwrap();
}

#[test]
fn show_pane_reaches_the_owner_and_rejects_an_unknown_id() {
    let Some(bus) = common::bus() else { return };
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let _owner = serve(bus.builder(), move |pane, token| {
        tx.lock().unwrap().send((pane, token)).unwrap()
    })
    .expect("claims the name");
    assert!(
        matches!(serve(bus.builder(), |_, _| {}), Err(zbus::Error::NameTaken)),
        "a second owner is refused"
    );
    show(bus.builder(), "keyboard", "").expect("a known pane");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok((PaneId::Keyboard, String::new()))
    );
    show(bus.builder(), "wifi", "fresh-token").unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok((PaneId::Wifi, "fresh-token".into()))
    );
    assert!(show(bus.builder(), "wifi", &"x".repeat(4097)).is_err());
    match show(bus.builder(), "general", "") {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), "org.freedesktop.DBus.Error.InvalidArgs")
        }
        other => panic!("an unknown pane is InvalidArgs, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "nothing is shown for it");
}

#[test]
fn a_second_start_hands_its_pane_to_the_first_and_its_token_only_to_its_own_program() {
    let Some(bus) = common::bus() else { return };
    let (tx, rx) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    let _first = serve(bus.builder(), move |pane, token| {
        tx.lock().unwrap().send((pane, token)).unwrap()
    })
    .expect("claims the name");
    // what the process records goes to a store of the test's own
    let state = tempfile::tempdir().unwrap();
    let second = |token: Option<&str>| {
        // no display: the second start must not need one
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cupertino-settings"));
        command
            .args(["--pane", "sound"])
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("XDG_STATE_HOME", state.path())
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_ACTIVATION_TOKEN");
        if let Some(token) = token {
            command.env("XDG_ACTIVATION_TOKEN", token);
        }
        command.status().expect("the binary runs")
    };
    // the owner here is the test binary, another program: a pane switch carries no grant and reaches it
    let status = second(None);
    assert!(status.success(), "{status}");
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(5)),
        Ok((PaneId::Sound, String::new()))
    );
    // a launch's activation token never goes to another program
    let status = second(Some("fresh-second-launch-token"));
    assert!(!status.success(), "{status}");
    assert!(rx.recv_timeout(Duration::from_millis(500)).is_err());
}
