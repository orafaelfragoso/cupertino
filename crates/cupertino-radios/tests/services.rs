//! The radios on NetworkManager and BlueZ, against the stand-ins of `cupertino_radios::fake` on a private bus.

#![cfg(feature = "fake")]

use std::collections::HashSet;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use cupertino_radios::fake::{FakeBluez, FakeNetworkManager, HEADPHONES, KEYBOARD};
use cupertino_radios::{detect, refusal, watch, BluetoothBackend, Found, NetworkBackend};
use cupertino_test_bus::PrivateBus;

/// The private bus, or `None` with a note when `dbus-daemon` is missing.
fn bus() -> Option<PrivateBus> {
    let bus = PrivateBus::start();
    if bus.is_none() {
        eprintln!("skipped: dbus-daemon is not available");
    }
    bus
}

const NM: NetworkBackend = NetworkBackend::NetworkManager;
const BLUEZ: BluetoothBackend = BluetoothBackend::BlueZ;

#[test]
fn detection_prefers_the_cupertino_daemons_and_finds_nothing_on_an_empty_bus() {
    let Some(bus) = bus() else { return };
    let client = bus.connect();
    assert_eq!(detect(&client).unwrap(), Found::default());
    let _nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    assert_eq!(
        detect(&client).unwrap(),
        Found {
            network: Some(NM),
            bluetooth: None
        }
    );
    let _bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    assert_eq!(
        detect(&client).unwrap(),
        Found {
            network: Some(NM),
            bluetooth: Some(BLUEZ)
        }
    );
    let _daemons = bus
        .blocking_builder()
        .name("org.cupertino.Network1")
        .unwrap()
        .name("org.cupertino.Bluetooth1")
        .unwrap()
        .build()
        .unwrap();
    assert_eq!(
        detect(&client).unwrap(),
        Found {
            network: Some(NetworkBackend::Cupertino),
            bluetooth: Some(BluetoothBackend::Cupertino)
        }
    );
}

#[test]
fn the_link_on_networkmanager_is_enabled_disabled_connected_disconnected_and_failed() {
    let Some(bus) = bus() else { return };
    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    let client = bus.connect();
    let link = || NM.link(&client).unwrap();

    let first = link();
    assert!(first.enabled);
    let device = first.device.clone().expect("the Wi-Fi device");
    assert_eq!(
        (
            device.name.as_str(),
            device.kind.as_str(),
            device.state.as_str()
        ),
        ("wlan0", "wifi", "disconnected")
    );
    assert_eq!(first.network(), None);

    nm.set_joined(Some("Home"));
    assert_eq!(link().network(), Some("Home"));
    assert_eq!(link().device.unwrap().state, "connected");

    nm.fail();
    let failed = link().device.unwrap();
    assert_eq!(failed.state, "disconnected");
    assert!(failed.failure.starts_with("failed"), "{}", failed.failure);

    NM.set_wireless_enabled(&client, false).unwrap();
    assert!(!nm.enabled());
    let off = link();
    assert!(!off.enabled);
    assert_eq!(off.device.unwrap().state, "unavailable");
    NM.set_wireless_enabled(&client, true).unwrap();
    assert!(link().enabled);
    assert_eq!(
        nm.calls(),
        ["WirelessEnabled=false", "WirelessEnabled=true"]
    );
}

#[test]
fn a_machine_without_a_wifi_device_has_a_radio_and_no_link() {
    let Some(bus) = bus() else { return };
    let _nm = FakeNetworkManager::serve_wired(bus.blocking_builder()).unwrap();
    let client = bus.connect();
    assert_eq!(NM.link(&client).unwrap().device, None);
    assert!(!NM.wifi(&client).unwrap().has_wifi_device());
}

#[test]
fn networks_are_listed_joined_left_and_scanned_on_networkmanager() {
    let Some(bus) = bus() else { return };
    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    let client = bus.connect();

    let wifi = NM.wifi(&client).unwrap();
    let rows: Vec<_> = wifi
        .networks
        .iter()
        .map(|n| (n.ssid.as_str(), n.security.as_str(), n.saved, n.connected))
        .collect();
    assert_eq!(
        rows,
        [
            ("Home", "wpa2", true, false),
            ("Cafe", "open", false, false),
            ("Office", "wpa2", false, false)
        ],
        "strongest first"
    );

    // a saved network joins with its connection, an open one with a new one, a secured one with its passphrase
    NM.join(&client, "Home", "").unwrap();
    assert_eq!(nm.joined(), Some("Home"));
    let joined = NM.wifi(&client).unwrap();
    assert!(joined.networks[0].connected && joined.networks[0].ssid == "Home");
    NM.join(&client, "Cafe", "").unwrap();
    assert_eq!(nm.joined(), Some("Cafe"));
    NM.join(&client, "Office", "correct horse").unwrap();
    assert_eq!(nm.joined(), Some("Office"));
    let out_of_range = NM.join(&client, "Nowhere", "").unwrap_err();
    assert_eq!(refusal(&out_of_range), "“Nowhere” is out of range.");

    NM.disconnect(&client).unwrap();
    assert_eq!(nm.joined(), None);
    NM.scan(&client).unwrap();

    let calls = nm.calls();
    assert!(
        calls[0].starts_with("ActivateConnection(/org/freedesktop/NetworkManager/Settings/1,"),
        "{calls:?}"
    );
    assert!(
        calls[1].starts_with("AddAndActivateConnection(None,"),
        "{calls:?}"
    );
    assert!(
        calls[2].starts_with("AddAndActivateConnection(Some(\"correct horse\"),"),
        "{calls:?}"
    );
    assert_eq!(calls[3..], ["Disconnect", "RequestScan"]);
}

#[test]
fn the_adapter_on_bluez_is_powered_and_its_devices_connect_disconnect_and_refuse() {
    let Some(bus) = bus() else { return };
    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    let client = bus.connect();
    let adapter = || BLUEZ.adapter(&client, &HashSet::new()).unwrap();

    let first = adapter();
    assert!(first.available && first.powered && !first.discovering);
    let rows: Vec<_> = first
        .devices
        .iter()
        .map(|d| (d.id.as_str(), d.title(), d.paired, d.connected))
        .collect();
    assert_eq!(
        rows,
        [
            (HEADPHONES, "Headphones".to_string(), true, false),
            (KEYBOARD, "Keyboard".to_string(), false, false)
        ]
    );

    BLUEZ.connect(&client, HEADPHONES).unwrap();
    assert!(bluez.connected(HEADPHONES) && adapter().devices[0].connected);
    BLUEZ.disconnect(&client, HEADPHONES).unwrap();
    assert!(!adapter().devices[0].connected);

    bluez.refuse_connect("Host is down");
    let refused = BLUEZ.connect(&client, HEADPHONES).unwrap_err();
    assert_eq!(refusal(&refused), "Host is down");
    let unknown = BLUEZ
        .connect(&client, "/org/bluez/hci0/dev_00")
        .unwrap_err();
    assert_eq!(refusal(&unknown), "The device is no longer available.");

    bluez.set_connected(KEYBOARD, true);
    assert!(adapter().devices[1].connected);
    BLUEZ.set_powered(&client, false).unwrap();
    let off = adapter();
    assert!(off.available && !off.powered && !bluez.powered());
    assert!(
        off.devices.iter().all(|d| !d.connected),
        "an adapter switched off has no links"
    );
    BLUEZ.set_powered(&client, true).unwrap();
    assert!(adapter().powered);
}

#[test]
fn a_bluez_without_an_adapter_reads_as_unavailable_and_refuses_changes() {
    let Some(bus) = bus() else { return };
    let _bluez = FakeBluez::serve_without_adapter(bus.blocking_builder()).unwrap();
    let client = bus.connect();
    let adapter = BLUEZ.adapter(&client, &HashSet::new()).unwrap();
    assert!(!adapter.available && !adapter.powered && adapter.devices.is_empty());
    let refused = BLUEZ.set_powered(&client, true).unwrap_err();
    assert_eq!(refusal(&refused), "There is no Bluetooth adapter.");
}

#[test]
fn the_services_signals_and_their_coming_and_going_are_followed() {
    let Some(bus) = bus() else { return };
    let client = bus.connect();
    let (tx, heard) = mpsc::channel();
    let tx = std::sync::Mutex::new(tx);
    zbus::block_on(watch::follow(
        client.inner(),
        Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        }),
    ))
    .unwrap();
    let wait = |what: &str| {
        heard
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("no signal after {what}"));
        // the rest of a burst
        while heard.recv_timeout(Duration::from_millis(50)).is_ok() {}
    };

    let nm = FakeNetworkManager::serve(bus.blocking_builder()).unwrap();
    wait("NetworkManager starting");
    nm.set_joined(Some("Home"));
    wait("the device joining a network");
    NetworkBackend::NetworkManager
        .set_wireless_enabled(&client, false)
        .unwrap();
    wait("the radio switched off");

    let bluez = FakeBluez::serve(bus.blocking_builder()).unwrap();
    wait("BlueZ starting");
    bluez.set_connected(HEADPHONES, true);
    wait("a device connecting");
    bluez.set_powered(false);
    wait("the adapter switched off");
    drop(bluez);
    wait("BlueZ stopping");
}
