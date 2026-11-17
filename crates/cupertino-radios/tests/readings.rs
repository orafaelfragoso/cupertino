//! The readings' types: labels of radio-supplied names, devices and links.

use cupertino_radios::{
    device_kind, device_title, display_name, BtDevice, Device, Link, LinkState, Network, Wifi,
    MAX_NAME,
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
fn a_name_loses_outer_white_space_control_characters_and_bidirectional_marks() {
    let marks = "\u{061c}\u{200e}\u{200f}\u{202a}\u{202b}\u{202c}\u{202d}\u{202e}\u{2066}\u{2067}\u{2068}\u{2069}";
    for (name, expected) in [
        ("  Café Wi-Fi \t".to_owned(), "Café Wi-Fi"),
        (String::new(), ""),
        ("   ".into(), ""),
        ("a\u{0}b\nc\u{7}d\u{85}e\u{7f}".into(), "abcde"),
        (format!("a{marks}b"), "ab"),
        ("\u{202e}evil\u{202c}".into(), "evil"),
        // trimmed after the removals
        ("\u{202e} x \u{202c}".into(), "x"),
    ] {
        assert_eq!(display_name(&name), expected, "{name:?}");
    }
}

#[test]
fn a_name_is_cut_to_the_longest_a_row_shows_in_characters() {
    assert_eq!(
        display_name(&"é".repeat(MAX_NAME + 10)),
        "é".repeat(MAX_NAME)
    );
    assert_eq!(display_name(&"a".repeat(MAX_NAME)), "a".repeat(MAX_NAME));
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
        ("something-else", "Bluetooth Device"),
        ("", "Bluetooth Device"),
    ] {
        assert_eq!(device_kind(icon), kind, "{icon}");
    }
}

#[test]
fn a_device_is_titled_by_its_clean_name_or_its_kind_and_the_end_of_its_address() {
    const ADDRESS: &str = "AA:BB:CC:DD:EE:FF";
    for (name, icon, address, expected) in [
        (Some("  Ann's Mouse "), "mouse", ADDRESS, "Ann's Mouse"),
        (None, "keyboard", ADDRESS, "Keyboard (EE:FF)"),
        (None, "", ADDRESS, "Bluetooth Device (EE:FF)"),
        (Some(""), "mouse", ADDRESS, "Mouse (EE:FF)"),
        (Some("\u{202e}\u{7} "), "mouse", ADDRESS, "Mouse (EE:FF)"),
        (None, "mouse", "EE:FF", "Mouse (EE:FF)"),
        (None, "mouse", "plain", "Mouse (plain)"),
        (None, "mouse", "DD:EE:FF", "Mouse (EE:FF)"),
        (
            None,
            "mouse",
            "AA:BB:CC:DD:E\u{7}E:FF\u{202e}",
            "Mouse (EE:FF)",
        ),
    ] {
        assert_eq!(
            device_title(name, icon, address),
            expected,
            "{name:?} {icon} {address:?}"
        );
        assert_eq!(bt(name, icon, address).title(), expected, "{name:?}");
    }
}

#[test]
fn a_device_is_pairing_before_connected_before_connecting_and_only_the_moves_are_busy() {
    let mut d = bt(None, "", "");
    let mut states = vec![d.state()];
    d.connecting = true;
    states.push(d.state());
    d.connected = true;
    states.push(d.state());
    d.pairing = true;
    states.push(d.state());
    assert_eq!(
        states,
        [
            LinkState::NotConnected,
            LinkState::Connecting,
            LinkState::Connected,
            LinkState::Pairing
        ]
    );
    for (state, label, busy) in [
        (LinkState::Connected, "Connected", false),
        (LinkState::Connecting, "Connecting…", true),
        (LinkState::Pairing, "Pairing…", true),
        (LinkState::NotConnected, "Not Connected", false),
    ] {
        assert_eq!(state.label(), label);
        assert_eq!(state.busy(), busy, "{label}");
    }
}

#[test]
fn only_a_secured_network_that_is_not_saved_needs_a_passphrase() {
    for (security, saved, expected) in [
        ("wpa2", false, true),
        ("wep", false, true),
        ("open", false, false),
        ("open", true, false),
        ("wpa2", true, false),
    ] {
        assert_eq!(
            network(security, saved).needs_passphrase(),
            expected,
            "{security} saved {saved}"
        );
    }
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
    for (d, expected) in [
        (Some(device("wifi", "connected", "Home")), Some("Home")),
        (Some(device("wifi", "connected", "")), None),
        (Some(device("wifi", "disconnected", "Home")), None),
        (None, None),
    ] {
        let link = Link {
            enabled: true,
            device: d,
        };
        assert_eq!(link.network(), expected, "{:?}", link.device);
    }
}
