//! The Wi-Fi and Bluetooth tiles on NetworkManager and BlueZ: the panel's radio backends against stand-ins for the two
//! services on a private bus (`dbus-daemon --session`); skipped when the daemon is not available. The machine's own
//! services are never contacted.
#![cfg(feature = "dbus")]

use cupertino_controlcenter::backend::{
    Bluetooth, MemoryBrightness, MemoryLayout, MemoryMedia, MemorySwitch, MemoryVolume,
    RecordingLauncher, RecordingLock, RecordingOverlay, Wifi,
};
use cupertino_controlcenter::model::Detail;
use cupertino_controlcenter::radios::{Radio, Radios};
use cupertino_controlcenter::{
    Backends, Controller, Module, Phase, TileState, VolumeState, WifiStatus,
};
use cupertino_radios::fake::{FakeBluez, FakeNetworkManager, Silent, HEADPHONES, KEYBOARD};
use cupertino_test_bus::PrivateBus;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The private bus, or `None` with a note when `dbus-daemon` is missing.
fn bus() -> Option<PrivateBus> {
    let bus = PrivateBus::start();
    if bus.is_none() {
        eprintln!("skipped: dbus-daemon is not available");
    }
    bus
}

/// The radios of `bus`, and what hears which radio changed. A reading and a switch are waited for as long as a busy
/// machine may take, so each test reads the state its service has now.
fn radios(bus: &PrivateBus) -> (Radios, mpsc::Receiver<Radio>) {
    radios_waiting(bus, Duration::from_secs(20))
}

/// As [`radios`], waiting `wait` for a reading and for a switch.
fn radios_waiting(bus: &PrivateBus, wait: Duration) -> (Radios, mpsc::Receiver<Radio>) {
    let radios = Radios::at(&bus.address).waiting(wait, wait);
    let (tx, changes) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    radios.on_change(move |radio| {
        let _ = tx.lock().unwrap().send(radio);
    });
    (radios, changes)
}

/// Wait for a change signal, and let the rest of its burst pass.
fn changed(changes: &mpsc::Receiver<Radio>, what: &str) {
    changes
        .recv_timeout(Duration::from_secs(5))
        .unwrap_or_else(|_| panic!("no change signal after {what}"));
    while changes.recv_timeout(Duration::from_millis(50)).is_ok() {}
}

/// A controller whose radios are `radios` and whose other backends are in memory.
fn controller(radios: &Radios) -> Controller {
    Controller::new(Backends {
        wifi: Box::new(radios.wifi()),
        bluetooth: Box::new(radios.bluetooth()),
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
        layout: Box::new(MemoryLayout::default()),
    })
}

fn disabled(controller: &Controller, module: Module) -> Option<String> {
    match controller.controls().tile(module) {
        TileState::Disabled(reason) => Some(reason),
        _ => None,
    }
}

#[test]
fn the_wifi_backend_reads_switches_lists_and_joins_on_networkmanager() {
    let Some(bus) = bus() else { return };
    let (radios, _changes) = radios(&bus);
    let mut wifi = radios.wifi();
    assert_eq!(
        wifi.read().unwrap_err().to_string(),
        "no network service is running"
    );

    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    let status = |enabled, network: Option<&str>| WifiStatus {
        enabled,
        network: network.map(str::to_owned),
    };
    assert_eq!(wifi.read().unwrap(), status(true, None));
    nm.set_joined(Some("Home"));
    assert_eq!(wifi.read().unwrap(), status(true, Some("Home")));

    let rows: Vec<_> = wifi
        .networks()
        .unwrap()
        .into_iter()
        .map(|n| (n.ssid, n.bars, n.secured, n.known, n.connected))
        .collect();
    assert_eq!(
        rows,
        [
            ("Home".to_string(), Some(3), true, true, true),
            ("Cafe".to_string(), Some(2), false, false, false),
            ("Office".to_string(), Some(1), true, false, false)
        ]
    );
    wifi.join("Cafe").unwrap();
    assert_eq!(nm.joined(), Some("Cafe"));
    assert_eq!(wifi.read().unwrap(), status(true, Some("Cafe")));
    assert_eq!(
        wifi.join("Nowhere").unwrap_err().to_string(),
        "“Nowhere” is out of range."
    );

    wifi.set_enabled(false).unwrap();
    assert!(!nm.enabled());
    assert_eq!(wifi.read().unwrap(), status(false, None));
    assert!(
        wifi.networks().unwrap().is_empty(),
        "a radio that is off sees no network"
    );
    wifi.set_enabled(true).unwrap();
    assert_eq!(wifi.read().unwrap(), status(true, None));
}

#[test]
fn a_machine_without_a_wifi_device_or_an_adapter_has_unavailable_tiles() {
    let Some(bus) = bus() else { return };
    let (radios, _changes) = radios(&bus);
    let _nm = FakeNetworkManager::serve_wired(bus.blocking_builder()).unwrap();
    let _bluez = FakeBluez::serve_without_adapter(bus.blocking_builder()).unwrap();
    assert_eq!(
        radios.wifi().read().unwrap_err().to_string(),
        "there is no Wi-Fi device"
    );
    assert_eq!(
        radios.bluetooth().read().unwrap_err().to_string(),
        "there is no Bluetooth adapter"
    );
    assert_eq!(
        radios
            .bluetooth()
            .set_powered(true)
            .unwrap_err()
            .to_string(),
        "There is no Bluetooth adapter."
    );
}

#[test]
fn the_bluetooth_backend_reads_switches_and_connects_paired_devices_on_bluez() {
    let Some(bus) = bus() else { return };
    let (radios, changes) = radios(&bus);
    let mut bluetooth = radios.bluetooth();
    assert_eq!(
        bluetooth.read().unwrap_err().to_string(),
        "no Bluetooth service is running"
    );

    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    changed(&changes, "the service starting");
    assert!(bluetooth.read().unwrap());
    let devices = bluetooth.devices().unwrap();
    assert_eq!(
        devices
            .iter()
            .map(|d| (d.id.as_str(), d.name.as_str(), d.connected))
            .collect::<Vec<_>>(),
        [(HEADPHONES, "Headphones", false)],
        "the paired devices alone"
    );

    // the service answers a connection when the link is up: the call does not hold the panel, and its signal tells
    bluetooth.set_connected(HEADPHONES, true).unwrap();
    changed(&changes, "the device connecting");
    assert!(bluez.connected(HEADPHONES));
    assert!(bluetooth.devices().unwrap()[0].connected);
    bluetooth.set_connected(HEADPHONES, false).unwrap();
    changed(&changes, "the device disconnecting");
    assert!(!bluetooth.devices().unwrap()[0].connected);

    // a refused connection leaves the device as it was, and the panel is told to read again
    bluez.refuse_connect("Host is down");
    bluetooth.set_connected(HEADPHONES, true).unwrap();
    changed(&changes, "the refused connection");
    assert!(!bluetooth.devices().unwrap()[0].connected);

    bluetooth.set_powered(false).unwrap();
    assert!(!bluez.powered() && !bluetooth.read().unwrap());
    bluetooth.set_powered(true).unwrap();
    assert!(bluetooth.read().unwrap());
}

#[test]
fn a_shown_panel_follows_the_services_signals_and_its_tiles_act_on_them() {
    let Some(bus) = bus() else { return };
    let (radios, changes) = radios(&bus);
    let mut controller = controller(&radios);
    assert!(
        !controller.backends_changed(0.0),
        "a hidden panel reads nothing"
    );
    controller.toggle(0.0);
    assert_eq!(controller.phase(0.0), Phase::Shown);
    assert_eq!(
        disabled(&controller, Module::WiFi).as_deref(),
        Some("no network service is running")
    );
    assert_eq!(
        disabled(&controller, Module::Bluetooth).as_deref(),
        Some("no Bluetooth service is running")
    );
    assert_eq!(
        controller.controls().subtitle(Module::WiFi),
        Some("Unavailable")
    );

    // the services start: the tiles come to life without waiting for the periodic reading
    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    changed(&changes, "the services starting");
    assert!(controller.backends_changed(0.1));
    assert_eq!(controller.controls().tile(Module::WiFi), TileState::On);
    assert_eq!(controller.controls().subtitle(Module::WiFi), Some("On"));
    assert_eq!(controller.controls().tile(Module::Bluetooth), TileState::On);

    // the device joins a network, then the radio is switched off elsewhere (as from Settings)
    nm.set_joined(Some("Home"));
    changed(&changes, "the device joining");
    controller.backends_changed(0.2);
    assert_eq!(controller.controls().subtitle(Module::WiFi), Some("Home"));
    nm.set_enabled(false);
    changed(&changes, "the radio switched off elsewhere");
    controller.backends_changed(0.3);
    assert_eq!(controller.controls().tile(Module::WiFi), TileState::Off);
    assert_eq!(controller.controls().subtitle(Module::WiFi), Some("Off"));

    // the tile switches the radio back on, and the service follows
    assert!(controller.press(Module::WiFi));
    assert!(nm.enabled());
    changed(&changes, "the tile switching the radio on");
    controller.backends_changed(0.4);
    assert_eq!(controller.controls().tile(Module::WiFi), TileState::On);

    // the adapter is switched off elsewhere, and on again from its tile
    bluez.set_powered(false);
    changed(&changes, "the adapter switched off elsewhere");
    controller.backends_changed(0.5);
    assert_eq!(
        controller.controls().tile(Module::Bluetooth),
        TileState::Off
    );
    assert!(controller.press(Module::Bluetooth));
    assert!(bluez.powered());
    changed(&changes, "the tile switching the adapter on");
    controller.backends_changed(0.6);
    assert_eq!(controller.controls().tile(Module::Bluetooth), TileState::On);

    // the Bluetooth detail connects a paired device, and shows a link made elsewhere going away
    controller.show_detail(Some(Detail::Bluetooth));
    assert!(controller.connect(HEADPHONES));
    changed(&changes, "the detail connecting the device");
    controller.backends_changed(0.7);
    let connected = |controller: &Controller| {
        controller
            .controls()
            .devices
            .ready()
            .map(|devices| devices[0].connected)
    };
    assert_eq!(connected(&controller), Some(true));
    bluez.set_connected(HEADPHONES, false);
    changed(&changes, "the device going");
    controller.backends_changed(0.8);
    assert_eq!(connected(&controller), Some(false));

    // a service that stops disables its tile with the reason
    drop(nm);
    changed(&changes, "the network service stopping");
    controller.backends_changed(0.9);
    assert_eq!(
        disabled(&controller, Module::WiFi).as_deref(),
        Some("no network service is running")
    );
    assert_eq!(controller.controls().tile(Module::Bluetooth), TileState::On);
}

#[test]
fn services_that_never_answer_leave_the_panel_responsive_and_their_tiles_unavailable() {
    let Some(bus) = bus() else { return };
    let _nm = Silent::network_manager(bus.blocking_builder()).unwrap();
    let _bluez = Silent::bluez(bus.blocking_builder()).unwrap();
    // the services' answers are waited for 400 ms, and the panel's thread waits 10 ms for a reading or a switch
    cupertino_radios::set_call_timeout(Duration::from_millis(400));
    let (radios, changes) = radios_waiting(&bus, Duration::from_millis(10));
    let mut controller = controller(&radios);
    let started = Instant::now();
    // every call of the panel's thread returns at once, whatever the services do
    fn prompt(controller: &mut Controller, what: &str, call: impl FnOnce(&mut Controller)) {
        let before = Instant::now();
        call(controller);
        let took = before.elapsed();
        assert!(
            took < Duration::from_millis(300),
            "{what} held the panel {took:?}"
        );
    }

    prompt(&mut controller, "opening", |c| c.toggle(0.0));
    prompt(&mut controller, "the periodic reading", |c| c.refresh(0.1));
    prompt(&mut controller, "a detail view", |c| {
        c.show_detail(Some(Detail::WiFi))
    });
    prompt(&mut controller, "a press", |c| {
        assert!(
            !c.press(Module::WiFi),
            "a tile that was never read is not pressable"
        );
    });
    // while the first readings are under way the tiles are unavailable, and say what they wait for
    assert_eq!(
        disabled(&controller, Module::WiFi).as_deref(),
        Some("waiting for the network service")
    );
    assert_eq!(
        disabled(&controller, Module::Bluetooth).as_deref(),
        Some("waiting for the Bluetooth service")
    );

    // within the bound the readings fail, the panel is told, and the tiles say that the services do not answer
    let silent = |controller: &Controller| {
        disabled(controller, Module::WiFi).as_deref() == Some("the network service does not answer")
            && disabled(controller, Module::Bluetooth).as_deref()
                == Some("the Bluetooth service does not answer")
    };
    let mut now = 0.2;
    while !silent(&controller) {
        let radio = changes
            .recv_timeout(Duration::from_secs(5))
            .expect("a reading that failed late is told");
        now += 0.01;
        prompt(&mut controller, "a late reading", |c| {
            c.radios_changed(now, radio == Radio::Network, radio == Radio::Bluetooth);
        });
    }
    assert_eq!(
        controller.controls().subtitle(Module::WiFi),
        Some("Unavailable")
    );
    assert!(
        started.elapsed() < Duration::from_millis(1800),
        "unavailable within the bound: {:?}",
        started.elapsed()
    );

    // a switch that the service does not answer is given up at once, with the reason
    let before = Instant::now();
    assert_eq!(
        radios.wifi().set_enabled(false).unwrap_err().to_string(),
        "the network service does not answer"
    );
    assert_eq!(
        radios
            .bluetooth()
            .set_powered(false)
            .unwrap_err()
            .to_string(),
        "the Bluetooth service does not answer"
    );
    assert!(before.elapsed() < Duration::from_millis(200));
}

#[test]
fn a_burst_of_signal_strength_changes_has_the_panel_read_nothing() {
    let Some(bus) = bus() else { return };
    let (radios, changes) = radios(&bus);
    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    let mut controller = controller(&radios);
    controller.toggle(0.0);
    assert_eq!(controller.controls().tile(Module::Bluetooth), TileState::On);
    // the service's start may be told after the first reading
    while changes.recv_timeout(Duration::from_millis(100)).is_ok() {}
    let read = bluez.readings();

    // a device is heard again and again while the adapter discovers, then the adapter is switched off elsewhere
    for rssi in 0..200 {
        bluez.advertise(KEYBOARD, -90 + (rssi % 40));
    }
    bluez.set_powered(false);
    assert_eq!(
        changes.recv_timeout(Duration::from_secs(5)),
        Ok(Radio::Bluetooth)
    );
    assert_eq!(bluez.readings(), read, "nothing was read meanwhile");
    assert!(
        changes.recv_timeout(Duration::from_millis(100)).is_err(),
        "the changes of signal strength told the panel nothing: one change, the switch"
    );
    assert!(controller.radios_changed(0.1, false, true));
    assert_eq!(
        controller.controls().tile(Module::Bluetooth),
        TileState::Off
    );
}
