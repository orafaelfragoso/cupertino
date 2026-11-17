//! The readings' types: labels of radio-supplied names, devices and links.

use cupertino_radios::{
    device_kind, device_title, display_name, Adapter, BluetoothBackend, BtDevice, Device, Found,
    KnownNetwork, Link, LinkState, Network, NetworkBackend, Wifi, MAX_NAME,
};

fn bt(name: Option<&str>, icon: &str, address: &str) -> BtDevice {
    BtDevice {
        id: "id".into(),
        address: address.into(),
        name: name.map(str::to_owned),
        icon: icon.into(),
        paired: false,
        connected: false,
        pairing: false,
        connecting: false,
    }
}

fn device(kind: &str, state: &str, ssid: &str) -> Device {
    Device {
        index: 1,
        name: "wlan0".into(),
        kind: kind.into(),
        state: state.into(),
        ssid: ssid.into(),
        failure: String::new(),
        address: String::new(),
    }
}

fn network(security: &str, saved: bool) -> Network {
    Network {
        ssid: "Home".into(),
        name: "Home".into(),
        signal: -50,
        security: security.into(),
        saved,
        connected: false,
    }
}

#[test]
fn a_name_keeps_its_text_and_loses_its_outer_white_space() {
    assert_eq!(display_name("  Café Wi-Fi \t"), "Café Wi-Fi");
    assert_eq!(display_name(""), "");
    assert_eq!(display_name("   "), "");
}

#[test]
fn a_name_loses_control_characters() {
    assert_eq!(display_name("a\u{0}b\nc\u{7}d\u{85}e\u{7f}"), "abcde");
}

#[test]
fn a_name_loses_the_bidirectional_marks_embeddings_overrides_and_isolates() {
    let marks = "\u{061c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}";
    assert_eq!(display_name(&format!("a{marks}b")), "ab");
    assert_eq!(display_name("\u{202e}evil\u{202c}"), "evil");
}

#[test]
fn a_name_is_cut_to_the_longest_a_row_shows_in_characters() {
    let long = "é".repeat(MAX_NAME + 10);
    let cut = display_name(&long);
    assert_eq!(cut.chars().count(), MAX_NAME);
    assert_eq!(display_name(&"a".repeat(MAX_NAME)), "a".repeat(MAX_NAME));
    assert_eq!(MAX_NAME, 128);
}

#[test]
fn a_name_is_trimmed_after_the_cut_and_the_removals() {
    assert_eq!(display_name("\u{202e} x \u{202c}"), "x");
}

#[test]
fn a_device_kind_comes_from_the_icon_name_of_either_daemon() {
    for (icon, kind) in [
        ("computer", "Computer"),
        ("tablet", "Tablet"),
        ("input-tablet", "Tablet"),
        ("phone", "Phone"),
        ("watch", "Watch"),
        ("display", "Display"),
        ("video-display", "Display"),
        ("headphones", "Headphones"),
        ("audio-headphones", "Headphones"),
        ("headset", "Headset"),
        ("audio-headset", "Headset"),
        ("speaker", "Speaker"),
        ("audio", "Audio Device"),
        ("audio-card", "Audio Device"),
        ("multimedia-player", "Audio Device"),
        ("keyboard", "Keyboard"),
        ("input-keyboard", "Keyboard"),
        ("mouse", "Mouse"),
        ("input-mouse", "Mouse"),
        ("trackpad", "Trackpad"),
        ("gamepad", "Game Controller"),
        ("input-gaming", "Game Controller"),
        ("remote-control", "Remote Control"),
        ("camera", "Camera"),
        ("camera-photo", "Camera"),
        ("camera-video", "Camera"),
        ("printer", "Printer"),
        ("scanner", "Scanner"),
    ] {
        assert_eq!(device_kind(icon), kind, "{icon}");
    }
}

#[test]
fn an_unknown_or_empty_icon_is_a_bluetooth_device() {
    assert_eq!(device_kind("something-else"), "Bluetooth Device");
    assert_eq!(device_kind(""), "Bluetooth Device");
}

#[test]
fn a_device_with_a_name_is_titled_by_it() {
    assert_eq!(
        device_title(Some("  Ann's Mouse "), "mouse", "AA:BB:CC:DD:EE:FF"),
        "Ann's Mouse"
    );
}

#[test]
fn a_device_without_a_name_is_titled_by_its_kind_and_the_end_of_its_address() {
    assert_eq!(
        device_title(None, "keyboard", "AA:BB:CC:DD:EE:FF"),
        "Keyboard (EE:FF)"
    );
    assert_eq!(
        device_title(None, "", "AA:BB:CC:DD:EE:FF"),
        "Bluetooth Device (EE:FF)"
    );
}

#[test]
fn a_name_that_is_empty_after_cleaning_counts_as_none() {
    assert_eq!(
        device_title(Some(""), "mouse", "AA:BB:CC:DD:EE:FF"),
        "Mouse (EE:FF)"
    );
    assert_eq!(
        device_title(Some("\u{202e}\u{7} "), "mouse", "AA:BB:CC:DD:EE:FF"),
        "Mouse (EE:FF)"
    );
}

#[test]
fn a_short_address_is_shown_whole() {
    assert_eq!(device_title(None, "mouse", "EE:FF"), "Mouse (EE:FF)");
    assert_eq!(device_title(None, "mouse", "plain"), "Mouse (plain)");
    assert_eq!(device_title(None, "mouse", "DD:EE:FF"), "Mouse (EE:FF)");
}

#[test]
fn an_address_is_cleaned_like_a_name_before_it_is_shown() {
    assert_eq!(
        device_title(None, "mouse", "AA:BB:CC:DD:E\u{7}E:FF\u{202e}"),
        "Mouse (EE:FF)"
    );
}

#[test]
fn a_bluetooth_device_is_titled_by_its_own_fields() {
    assert_eq!(bt(Some("Ann"), "mouse", "AA:BB:CC:DD:EE:FF").title(), "Ann");
    assert_eq!(
        bt(None, "mouse", "AA:BB:CC:DD:EE:FF").title(),
        "Mouse (EE:FF)"
    );
}

#[test]
fn a_device_is_pairing_before_it_is_connected_before_it_is_connecting() {
    let mut d = bt(None, "", "");
    assert_eq!(d.state(), LinkState::NotConnected);
    d.connecting = true;
    assert_eq!(d.state(), LinkState::Connecting);
    d.connected = true;
    assert_eq!(d.state(), LinkState::Connected);
    d.pairing = true;
    assert_eq!(d.state(), LinkState::Pairing);
}

#[test]
fn a_link_state_has_its_label() {
    assert_eq!(LinkState::Connected.label(), "Connected");
    assert_eq!(LinkState::Connecting.label(), "Connecting…");
    assert_eq!(LinkState::Pairing.label(), "Pairing…");
    assert_eq!(LinkState::NotConnected.label(), "Not Connected");
}

#[test]
fn only_connecting_and_pairing_are_busy() {
    assert!(LinkState::Connecting.busy());
    assert!(LinkState::Pairing.busy());
    assert!(!LinkState::Connected.busy());
    assert!(!LinkState::NotConnected.busy());
}

#[test]
fn a_secured_network_that_is_not_saved_needs_a_passphrase() {
    assert!(network("wpa2", false).needs_passphrase());
    assert!(network("wep", false).needs_passphrase());
}

#[test]
fn an_open_or_saved_network_needs_none() {
    assert!(!network("open", false).needs_passphrase());
    assert!(!network("open", true).needs_passphrase());
    assert!(!network("wpa2", true).needs_passphrase());
}

#[test]
fn a_reading_has_a_wifi_device_when_one_of_its_devices_is_wifi() {
    let mut wifi = Wifi::default();
    assert!(!wifi.has_wifi_device());
    wifi.devices.push(device("ethernet", "connected", ""));
    assert!(!wifi.has_wifi_device());
    wifi.devices.push(device("wifi", "disconnected", ""));
    assert!(wifi.has_wifi_device());
}

#[test]
fn a_link_names_the_network_only_while_its_device_is_connected_to_one() {
    let link = |d: Option<Device>| Link {
        enabled: true,
        device: d,
    };
    assert_eq!(
        link(Some(device("wifi", "connected", "Home"))).network(),
        Some("Home")
    );
    assert_eq!(link(Some(device("wifi", "connected", ""))).network(), None);
    assert_eq!(
        link(Some(device("wifi", "disconnected", "Home"))).network(),
        None
    );
    assert_eq!(link(None).network(), None);
}

#[test]
fn nothing_is_read_by_default() {
    let wifi = Wifi::default();
    assert!(!wifi.enabled && !wifi.can_forget);
    assert!(wifi.devices.is_empty() && wifi.networks.is_empty() && wifi.known.is_empty());
    assert_eq!(wifi.joining, None);
    assert_eq!(wifi.failure, None);
    let link = Link::default();
    assert!(!link.enabled && link.device.is_none());
    let adapter = Adapter::default();
    assert!(!adapter.available && !adapter.powered && !adapter.discovering);
    assert!(adapter.devices.is_empty());
}

#[test]
fn no_service_is_found_by_default() {
    let found = Found::default();
    assert_eq!(found.network, None);
    assert_eq!(found.bluetooth, None);
    assert_ne!(NetworkBackend::Cupertino, NetworkBackend::NetworkManager);
    assert_ne!(BluetoothBackend::Cupertino, BluetoothBackend::BlueZ);
}

#[test]
fn a_known_network_may_leave_auto_join_unsaid() {
    let known = KnownNetwork {
        ssid: "Home".into(),
        name: "Home".into(),
        auto_join: None,
        in_range: false,
        connected: false,
        secured: true,
    };
    assert_eq!(known.clone(), known);
    assert_eq!(known.auto_join, None);
}
