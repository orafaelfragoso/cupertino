//! The control interface against a private session bus (`dbus-daemon --session`) with in-memory backends; skipped when
//! the daemon is not available. The panel never touches the machine's radios, volume or backlight here.
#![cfg(feature = "dbus")]

use cupertino_controlcenter::backend::{
    MemoryBluetooth, MemoryBrightness, MemoryMedia, MemorySwitch, MemoryVolume, MemoryWifi,
    RecordingLauncher, RecordingLock, RecordingOverlay,
};
use cupertino_controlcenter::control::{serve, BUS_NAME, OBJECT_PATH};
use cupertino_controlcenter::{
    Backends, Controller, Module, Phase, TileState, VolumeState, WifiStatus,
};
use cupertino_test_bus::PrivateBus;
use std::sync::mpsc;
use std::time::Duration;
use zbus::{block_on, Connection};

fn fake_backends() -> Backends {
    Backends {
        wifi: Box::new(MemoryWifi(
            WifiStatus {
                enabled: true,
                network: None,
            },
            Vec::new(),
        )),
        bluetooth: Box::new(MemoryBluetooth(false, Vec::new())),
        microphone: Box::new(MemorySwitch(true)),
        dock_hiding: Box::new(MemorySwitch(false)),
        keyboard: Box::new(MemorySwitch(false)),
        focus_mode: Box::new(MemorySwitch(false)),
        brightness: Box::new(MemoryBrightness(0.5)),
        volume: Box::new(MemoryVolume(
            VolumeState {
                level: 0.5,
                muted: false,
            },
            Vec::new(),
        )),
        media: Box::new(MemoryMedia(None)),
        lock: Box::new(RecordingLock::default()),
        launcher: Box::new(RecordingLauncher::default()),
        overlay: Box::new(RecordingOverlay::default()),
    }
}

#[test]
fn toggle_opens_and_closes_the_panel_and_a_second_panel_cannot_take_the_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (toggled_tx, toggled_rx) = mpsc::channel();
    let _panel =
        serve(bus.builder(), move || toggled_tx.send(()).unwrap()).expect("claims the name");
    let mut controller = Controller::new(fake_backends());
    assert_eq!(controller.phase(0.0), Phase::Hidden);

    // what the server and the menu bar do: call `Toggle` on the well-known name
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    let call = || {
        block_on(caller.call_method(
            Some(BUS_NAME),
            OBJECT_PATH,
            Some("org.cupertino.ControlPanel1"),
            "Toggle",
            &(),
        ))
        .expect("Toggle")
    };

    call();
    toggled_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the toggle callback ran");
    controller.toggle(1.0);
    assert_eq!(controller.phase(1.0), Phase::Shown);
    assert_eq!(controller.controls().tile(Module::WiFi), TileState::On);

    call();
    toggled_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the second toggle callback ran");
    controller.toggle(2.0);
    assert_eq!(controller.phase(2.0), Phase::Hiding);
    assert_eq!(controller.phase(2.3), Phase::Hidden);
    assert_eq!(toggled_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    let second = serve(bus.builder(), || {});
    assert!(
        matches!(second, Err(zbus::Error::NameTaken)),
        "{:?}",
        second.err()
    );
}

/// `Toggle` from another process of the person (a `dbus-send` this test starts, as the menu bar is to the panel: no
/// relation) reaches the panel; from a connection the bus cannot name a process for (one over TCP, authenticated
/// anonymously) it is refused by name and the panel does not move.
#[test]
fn any_unsandboxed_process_may_toggle_but_a_caller_the_bus_cannot_name_may_not() {
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
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir={}</listen>
  <listen>tcp:host=127.0.0.1,bind=127.0.0.1,port=0,family=ipv4</listen>
  <auth>EXTERNAL</auth>
  <auth>ANONYMOUS</auth>
  <allow_anonymous/>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
    <allow user="*"/>
  </policy>
</busconfig>"#,
            dir.path().display()
        ),
    )
    .unwrap();
    let Some(bus) = PrivateBus::start_with_config(&config) else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let address = |transport: &str| {
        bus.address
            .split(';')
            .find(|a| a.starts_with(transport))
            .unwrap_or_else(|| panic!("no {transport} address in {}", bus.address))
            .to_owned()
    };
    let (unix, tcp) = (address("unix:"), address("tcp:"));
    let (toggled_tx, toggled_rx) = mpsc::channel();
    let _panel = serve(
        zbus::connection::Builder::address(unix.as_str()).unwrap(),
        move || toggled_tx.send(()).unwrap(),
    )
    .expect("claims the name");

    let sent = std::process::Command::new("dbus-send")
        .arg(format!("--bus={unix}"))
        .args(["--print-reply", &format!("--dest={BUS_NAME}"), OBJECT_PATH])
        .arg("org.cupertino.ControlPanel1.Toggle")
        .output()
        .expect("dbus-send");
    assert!(
        sent.status.success(),
        "{}",
        String::from_utf8_lossy(&sent.stderr)
    );
    toggled_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the toggle callback ran");

    let anonymous: Connection = block_on(
        zbus::connection::Builder::address(tcp.as_str())
            .unwrap()
            .auth_mechanism(zbus::AuthMechanism::Anonymous)
            .build(),
    )
    .expect("connect over TCP");
    let refused = block_on(anonymous.call_method(
        Some(BUS_NAME),
        OBJECT_PATH,
        Some("org.cupertino.ControlPanel1"),
        "Toggle",
        &(),
    ));
    match refused {
        Err(zbus::Error::MethodError(name, _, _)) => {
            assert_eq!(name.as_str(), "org.cupertino.Error.AnonymousCaller")
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(toggled_rx.recv_timeout(Duration::from_millis(300)).is_err());
}
