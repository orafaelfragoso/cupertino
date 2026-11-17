//! The signals that tell a reading is out of date: rules, objects and the changes a reading shows.

#![cfg(feature = "dbus")]

use cupertino_radios::watch::{classify, concern, rules, shown, Radio};
use cupertino_radios::{bluez, btd, netd, nm};
use std::collections::HashMap;
use zbus::zvariant::Value;
use zbus::{Message, OwnedMatchRule};

const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const DEVICE_PATH: &str = "/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF";

fn name_owner_changed(name: &str) -> Message {
    Message::signal(
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameOwnerChanged",
    )
    .unwrap()
    .sender("org.freedesktop.DBus")
    .unwrap()
    .build(&(name, "", ":1.9"))
    .unwrap()
}

fn signal(path: &str, interface: &str, member: &str) -> Message {
    Message::signal(path, interface, member)
        .unwrap()
        .build(&())
        .unwrap()
}

fn properties_changed(
    path: &str,
    interface: &str,
    changed: &[&str],
    invalidated: &[&str],
) -> Message {
    let changed: HashMap<&str, Value<'_>> =
        changed.iter().map(|k| (*k, Value::from(true))).collect();
    Message::signal(path, PROPERTIES, "PropertiesChanged")
        .unwrap()
        .build(&(interface, changed, invalidated.to_vec()))
        .unwrap()
}

#[test]
fn the_rules_are_signal_match_rules_for_every_service_and_object_a_reading_comes_from() {
    let all = rules();
    for rule in &all {
        OwnedMatchRule::try_from(rule.as_str()).unwrap_or_else(|e| panic!("{rule}: {e}"));
        assert!(rule.starts_with("type='signal'"), "{rule}");
    }
    let has = |parts: &[String]| all.iter().any(|r| parts.iter().all(|p| r.contains(p)));
    let mut wanted: Vec<Vec<String>> = [netd::NAME, nm::NAME, btd::NAME, bluez::NAME]
        .iter()
        .map(|name| vec!["member='NameOwnerChanged'".into(), format!("arg0='{name}'")])
        .collect();
    for (name, path) in [
        (netd::NAME, netd::PATH),
        (nm::NAME, nm::PATH),
        (nm::NAME, nm::SETTINGS_PATH),
        (btd::NAME, btd::PATH),
    ] {
        wanted.push(vec![format!("sender='{name}'"), format!("path='{path}'")]);
    }
    wanted.push(vec![
        "sender='org.bluez'".into(),
        "path='/'".into(),
        OBJECT_MANAGER.into(),
    ]);
    wanted.push(vec![
        "path_namespace='/org/bluez'".into(),
        "member='PropertiesChanged'".into(),
    ]);
    for parts in &wanted {
        assert!(has(parts), "{parts:?}");
    }
}

#[test]
fn objects_are_told_to_their_radio() {
    for (path, interface, expected) in [
        (netd::PATH, None, Some(Radio::Network)),
        (nm::PATH, None, Some(Radio::Network)),
        (nm::SETTINGS_PATH, Some("x.y"), Some(Radio::Network)),
        (btd::PATH, None, Some(Radio::Bluetooth)),
        (DEVICE_PATH, None, Some(Radio::Bluetooth)),
        ("/org/bluez", None, Some(Radio::Bluetooth)),
        // the root object is Bluetooth's only for the object manager
        ("/", Some(OBJECT_MANAGER), Some(Radio::Bluetooth)),
        ("/", Some(PROPERTIES), None),
        ("/", None, None),
        ("/org/freedesktop/NetworkManager/AccessPoint/1", None, None),
        ("/org/other", None, None),
        ("", None, None),
    ] {
        assert_eq!(classify(path, interface), expected, "{path} {interface:?}");
    }
}

#[test]
fn only_the_properties_a_reading_shows_count_for_the_adapter_and_its_devices() {
    for (interface, properties, expected) in [
        (bluez::ADAPTER, &["Powered"][..], true),
        (bluez::ADAPTER, &["Discovering"], true),
        (bluez::ADAPTER, &["Name", "Class"], false),
        (bluez::ADAPTER, &["Connected"], false),
        (bluez::DEVICE, &["Address"], true),
        (bluez::DEVICE, &["Name"], true),
        (bluez::DEVICE, &["Icon"], true),
        (bluez::DEVICE, &["Paired"], true),
        (bluez::DEVICE, &["Connected"], true),
        (
            bluez::DEVICE,
            &["RSSI", "ManufacturerData", "TxPower"],
            false,
        ),
        (bluez::DEVICE, &["RSSI", "Connected", "TxPower"], true),
        (bluez::DEVICE, &["Powered"], false),
        (bluez::DEVICE, &[], false),
        ("org.bluez.MediaPlayer1", &["Position", "Connected"], false),
        ("", &["Powered"], false),
    ] {
        assert_eq!(
            shown(interface, properties.iter().copied()),
            expected,
            "{interface} {properties:?}"
        );
    }
}

#[test]
fn a_signal_makes_the_reading_of_its_radio_out_of_date() {
    let unreadable_name_change = Message::signal(
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameOwnerChanged",
    )
    .unwrap()
    .build(&(7u32,))
    .unwrap();
    let unreadable_properties = Message::signal(DEVICE_PATH, PROPERTIES, "PropertiesChanged")
        .unwrap()
        .build(&("only one string",))
        .unwrap();
    for (row, message, expected) in [
        (
            "netd appears",
            name_owner_changed(netd::NAME),
            Some(Radio::Network),
        ),
        (
            "nm appears",
            name_owner_changed(nm::NAME),
            Some(Radio::Network),
        ),
        (
            "btd appears",
            name_owner_changed(btd::NAME),
            Some(Radio::Bluetooth),
        ),
        (
            "bluez appears",
            name_owner_changed(bluez::NAME),
            Some(Radio::Bluetooth),
        ),
        (
            "another name",
            name_owner_changed("org.example.Other"),
            None,
        ),
        (
            "a name change that is not three strings",
            unreadable_name_change,
            None,
        ),
        (
            "netd signal",
            signal(netd::PATH, "org.cupertino.Network1", "Changed"),
            Some(Radio::Network),
        ),
        (
            "nm signal",
            signal(nm::PATH, "x.y", "StateChanged"),
            Some(Radio::Network),
        ),
        (
            "nm settings signal",
            signal(nm::SETTINGS_PATH, "x.y", "NewConnection"),
            Some(Radio::Network),
        ),
        (
            "btd signal",
            signal(btd::PATH, "org.cupertino.Bluetooth1", "Changed"),
            Some(Radio::Bluetooth),
        ),
        (
            "interfaces added",
            signal("/", OBJECT_MANAGER, "InterfacesAdded"),
            Some(Radio::Bluetooth),
        ),
        (
            "interfaces removed",
            signal("/", OBJECT_MANAGER, "InterfacesRemoved"),
            Some(Radio::Bluetooth),
        ),
        ("another object", signal("/org/other", "x.y", "Z"), None),
        (
            "root properties",
            signal("/", PROPERTIES, "PropertiesChanged"),
            None,
        ),
        (
            "another signal of a device",
            signal(DEVICE_PATH, bluez::DEVICE, "Disconnected"),
            Some(Radio::Bluetooth),
        ),
        (
            "a shown device property",
            properties_changed(DEVICE_PATH, bluez::DEVICE, &["Connected"], &[]),
            Some(Radio::Bluetooth),
        ),
        (
            "a shown adapter property",
            properties_changed("/org/bluez/hci0", bluez::ADAPTER, &["Powered"], &[]),
            Some(Radio::Bluetooth),
        ),
        (
            "an invalidated shown property",
            properties_changed(DEVICE_PATH, bluez::DEVICE, &[], &["Name"]),
            Some(Radio::Bluetooth),
        ),
        (
            "signal strength and advertisement",
            properties_changed(
                DEVICE_PATH,
                bluez::DEVICE,
                &["RSSI", "ManufacturerData"],
                &["TxPower"],
            ),
            None,
        ),
        (
            "another interface of the stack",
            properties_changed(
                "/org/bluez/hci0/dev_X/player0",
                "org.bluez.MediaPlayer1",
                &["Position"],
                &[],
            ),
            None,
        ),
        (
            "an unreadable properties change",
            unreadable_properties,
            None,
        ),
    ] {
        assert_eq!(concern(&message), expected, "{row}");
    }
}
