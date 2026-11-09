//! The control interface against a private session bus (`dbus-daemon --session`); skipped when the
//! daemon is not available.
#![cfg(feature = "app")]

use cupertino_launcher::control::{serve, Call, BUS_NAME, OBJECT_PATH};
use cupertino_test_bus::PrivateBus;
use std::sync::mpsc;
use std::time::Duration;
use zbus::{block_on, Connection};

#[test]
fn both_toggles_reach_the_launcher_and_a_second_launcher_cannot_take_the_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (toggled_tx, toggled_rx) = mpsc::channel();
    let _launcher =
        serve(bus.builder(), move |call| toggled_tx.send(call).unwrap()).expect("claims the name");

    // what the server does: call `Toggle` or `ToggleApplications` on the well-known name
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    for (member, call) in [
        ("Toggle", Call::Toggle),
        ("ToggleApplications", Call::Applications),
    ] {
        block_on(caller.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some("org.cupertino.Launcher1"),
            member,
            &(),
        ))
        .expect(member);
        assert_eq!(
            toggled_rx.recv_timeout(Duration::from_secs(5)),
            Ok(call),
            "the callback ran for {member}"
        );
        assert_eq!(toggled_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    let second = serve(bus.builder(), |_| {});
    assert!(
        matches!(second, Err(zbus::Error::NameTaken)),
        "{:?}",
        second.err()
    );
}

/// `Toggle` and `ToggleApplications` from a process of the person that is neither the server nor the launcher (a
/// `dbus-send` this test starts: a child, not the parent) are refused by name, and the panel does not move.
#[test]
fn another_process_cannot_toggle_the_launcher() {
    if let Err(error) = std::process::Command::new("dbus-send")
        .arg("--version")
        .output()
    {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "run dbus-send: {error}"
        );
        cupertino_test_bus::missing_tool("dbus-send");
        return;
    }
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (toggled_tx, toggled_rx) = mpsc::channel();
    let _launcher =
        serve(bus.builder(), move |call| toggled_tx.send(call).unwrap()).expect("claims the name");
    for member in ["Toggle", "ToggleApplications"] {
        let sent = std::process::Command::new("dbus-send")
            .arg(format!("--bus={}", bus.address))
            .args(["--print-reply", &format!("--dest={BUS_NAME}"), OBJECT_PATH])
            .arg(format!("org.cupertino.Launcher1.{member}"))
            .output()
            .expect("dbus-send");
        assert!(!sent.status.success());
        let error = String::from_utf8_lossy(&sent.stderr);
        assert!(
            error.contains("org.cupertino.Error.UntrustedCaller"),
            "{member}: {error}"
        );
    }
    assert!(toggled_rx.recv_timeout(Duration::from_millis(300)).is_err());
}
