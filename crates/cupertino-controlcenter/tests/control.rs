//! The control interface against a private session bus (`dbus-daemon --session`) with in-memory backends; skipped when
//! the daemon is not available. The panel never touches the machine's radios, volume or backlight here.
#![cfg(feature = "dbus")]

use cupertino_controlcenter::backend::{MemoryBattery, MemoryBluetooth, MemoryBrightness, MemoryFocusMode, MemoryVolume, MemoryWifi, RecordingOverlay};
use cupertino_controlcenter::control::{serve, BUS_NAME, OBJECT_PATH};
use cupertino_controlcenter::{Backends, Controller, Module, Phase, TileState, VolumeState, WifiStatus};
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

fn fake_backends() -> Backends {
    Backends {
        wifi: Box::new(MemoryWifi(WifiStatus { enabled: true, network: None })),
        bluetooth: Box::new(MemoryBluetooth(false)),
        focus_mode: Box::new(MemoryFocusMode(false)),
        brightness: Box::new(MemoryBrightness(0.5)),
        volume: Box::new(MemoryVolume(VolumeState { level: 0.5, muted: false })),
        battery: Box::new(MemoryBattery(None)),
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
    let _panel = serve(bus.builder(), move || toggled_tx.send(()).unwrap()).expect("claims the name");
    let mut controller = Controller::new(fake_backends());
    assert_eq!(controller.phase(0.0), Phase::Hidden);

    // what the server and the menu bar do: call `Toggle` on the well-known name
    let caller: Connection = block_on(bus.builder().build()).expect("connect");
    let call = || block_on(caller.call_method(Some(BUS_NAME), OBJECT_PATH, Some("org.cupertino.ControlPanel1"), "Toggle", &())).expect("Toggle");

    call();
    toggled_rx.recv_timeout(Duration::from_secs(5)).expect("the toggle callback ran");
    controller.toggle(1.0);
    assert_eq!(controller.phase(1.0), Phase::Shown);
    assert_eq!(controller.controls().tile(Module::WiFi), TileState::On);

    call();
    toggled_rx.recv_timeout(Duration::from_secs(5)).expect("the second toggle callback ran");
    controller.toggle(2.0);
    assert_eq!(controller.phase(2.0), Phase::Hiding);
    assert_eq!(controller.phase(2.3), Phase::Hidden);
    assert_eq!(toggled_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    let second = serve(bus.builder(), || {});
    assert!(matches!(second, Err(zbus::Error::NameTaken)), "{:?}", second.err());
}
