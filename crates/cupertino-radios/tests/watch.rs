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
fn the_services_have_their_names_and_paths() {
    assert_eq!(netd::NAME, "org.cupertino.Network1");
    assert_eq!(netd::PATH, "/org/cupertino/Network1");
    assert_eq!(btd::NAME, "org.cupertino.Bluetooth1");
    assert_eq!(btd::PATH, "/org/cupertino/Bluetooth1");
    assert_eq!(nm::NAME, "org.freedesktop.NetworkManager");
    assert_eq!(bluez::NAME, "org.bluez");
    assert_eq!(bluez::PATHS, "/org/bluez");
}

#[test]
fn every_rule_is_a_match_rule_the_bus_accepts() {
    let all = rules();
    assert!(!all.is_empty());
    for rule in &all {
        OwnedMatchRule::try_from(rule.as_str()).unwrap_or_else(|e| panic!("{rule}: {e}"));
        assert!(rule.starts_with("type='signal'"), "{rule}");
    }
}

#[test]
fn each_service_appearing_or_leaving_has_a_rule() {
    let all = rules();
    for name in [netd::NAME, nm::NAME, btd::NAME, bluez::NAME] {
        assert!(
            all.iter().any(|r| r.contains("member='NameOwnerChanged'")
                && r.contains(&format!("arg0='{name}'"))),
            "{name}"
        );
    }
}

#[test]
fn the_objects_the_readings_come_from_have_rules() {
    let all = rules();
    for (name, path) in [
        (netd::NAME, netd::PATH),
        (nm::NAME, nm::PATH),
        (nm::NAME, nm::SETTINGS_PATH),
        (btd::NAME, btd::PATH),
    ] {
        assert!(
            all.iter().any(|r| r.contains(&format!("sender='{name}'"))
                && r.contains(&format!("path='{path}'"))),
            "{name} {path}"
        );
    }
}

#[test]
fn the_bluetooth_stack_has_rules_for_its_objects_and_their_properties() {
    let all = rules();
    assert!(all.iter().any(|r| r.contains("sender='org.bluez'")
        && r.contains("path='/'")
        && r.contains(OBJECT_MANAGER)));
    assert!(all
        .iter()
        .any(|r| r.contains("path_namespace='/org/bluez'")
            && r.contains("member='PropertiesChanged'")));
}

#[test]
fn objects_are_told_to_their_radio() {
    assert_eq!(classify(netd::PATH, None), Some(Radio::Network));
    assert_eq!(classify(nm::PATH, None), Some(Radio::Network));
    assert_eq!(
        classify(nm::SETTINGS_PATH, Some("x.y")),
        Some(Radio::Network)
    );
    assert_eq!(classify(btd::PATH, None), Some(Radio::Bluetooth));
    assert_eq!(classify(DEVICE_PATH, None), Some(Radio::Bluetooth));
    assert_eq!(classify("/org/bluez", None), Some(Radio::Bluetooth));
}

#[test]
fn the_root_object_is_bluetooths_only_for_the_object_manager() {
    assert_eq!(classify("/", Some(OBJECT_MANAGER)), Some(Radio::Bluetooth));
    assert_eq!(classify("/", Some(PROPERTIES)), None);
    assert_eq!(classify("/", None), None);
}

#[test]
fn other_objects_belong_to_no_radio() {
    assert_eq!(
        classify("/org/freedesktop/NetworkManager/AccessPoint/1", None),
        None
    );
    assert_eq!(classify("/org/other", None), None);
    assert_eq!(classify("", None), None);
}

#[test]
fn only_the_properties_a_reading_shows_count_for_the_adapter_and_its_devices() {
    assert!(shown(bluez::ADAPTER, ["Powered"].into_iter()));
    assert!(shown(bluez::ADAPTER, ["Discovering"].into_iter()));
    assert!(!shown(bluez::ADAPTER, ["Name", "Class"].into_iter()));
    for property in ["Address", "Name", "Icon", "Paired", "Connected"] {
        assert!(shown(bluez::DEVICE, [property].into_iter()), "{property}");
    }
    assert!(!shown(
        bluez::DEVICE,
        ["RSSI", "ManufacturerData", "TxPower"].into_iter()
    ));
}

#[test]
fn one_shown_property_among_others_is_enough() {
    assert!(shown(
        bluez::DEVICE,
        ["RSSI", "Connected", "TxPower"].into_iter()
    ));
}

#[test]
fn nothing_changed_or_another_interface_shows_nothing() {
    assert!(!shown(bluez::DEVICE, std::iter::empty()));
    assert!(!shown(
        "org.bluez.MediaPlayer1",
        ["Position", "Connected"].into_iter()
    ));
    assert!(!shown("", ["Powered"].into_iter()));
}

#[test]
fn a_property_of_the_other_interface_does_not_count() {
    assert!(!shown(bluez::ADAPTER, ["Connected"].into_iter()));
    assert!(!shown(bluez::DEVICE, ["Powered"].into_iter()));
}

#[test]
fn a_service_of_the_network_appearing_or_leaving_makes_the_network_reading_out_of_date() {
    assert_eq!(
        concern(&name_owner_changed(netd::NAME)),
        Some(Radio::Network)
    );
    assert_eq!(concern(&name_owner_changed(nm::NAME)), Some(Radio::Network));
}

#[test]
fn a_service_of_bluetooth_appearing_or_leaving_makes_its_reading_out_of_date() {
    assert_eq!(
        concern(&name_owner_changed(btd::NAME)),
        Some(Radio::Bluetooth)
    );
    assert_eq!(
        concern(&name_owner_changed(bluez::NAME)),
        Some(Radio::Bluetooth)
    );
}

#[test]
fn another_name_changing_hands_changes_nothing() {
    assert_eq!(concern(&name_owner_changed("org.example.Other")), None);
}

#[test]
fn a_name_change_with_a_body_that_is_not_three_strings_changes_nothing() {
    let message = Message::signal(
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "NameOwnerChanged",
    )
    .unwrap()
    .build(&(7u32,))
    .unwrap();
    assert_eq!(concern(&message), None);
}

#[test]
fn any_signal_of_a_network_object_makes_the_network_reading_out_of_date() {
    assert_eq!(
        concern(&signal(netd::PATH, "org.cupertino.Network1", "Changed")),
        Some(Radio::Network)
    );
    assert_eq!(
        concern(&signal(nm::PATH, "x.y", "StateChanged")),
        Some(Radio::Network)
    );
    assert_eq!(
        concern(&signal(nm::SETTINGS_PATH, "x.y", "NewConnection")),
        Some(Radio::Network)
    );
}

#[test]
fn a_signal_of_the_bluetooth_daemon_makes_the_reading_out_of_date() {
    assert_eq!(
        concern(&signal(btd::PATH, "org.cupertino.Bluetooth1", "Changed")),
        Some(Radio::Bluetooth)
    );
}

#[test]
fn objects_of_the_bluetooth_stack_coming_and_going_make_the_reading_out_of_date() {
    assert_eq!(
        concern(&signal("/", OBJECT_MANAGER, "InterfacesAdded")),
        Some(Radio::Bluetooth)
    );
    assert_eq!(
        concern(&signal("/", OBJECT_MANAGER, "InterfacesRemoved")),
        Some(Radio::Bluetooth)
    );
}

#[test]
fn a_signal_of_another_object_changes_nothing() {
    assert_eq!(concern(&signal("/org/other", "x.y", "Z")), None);
    assert_eq!(concern(&signal("/", PROPERTIES, "PropertiesChanged")), None);
}

#[test]
fn a_shown_property_of_a_device_makes_the_bluetooth_reading_out_of_date() {
    let message = properties_changed(DEVICE_PATH, bluez::DEVICE, &["Connected"], &[]);
    assert_eq!(concern(&message), Some(Radio::Bluetooth));
    let adapter = properties_changed("/org/bluez/hci0", bluez::ADAPTER, &["Powered"], &[]);
    assert_eq!(concern(&adapter), Some(Radio::Bluetooth));
}

#[test]
fn an_invalidated_shown_property_counts_as_a_change() {
    let message = properties_changed(DEVICE_PATH, bluez::DEVICE, &[], &["Name"]);
    assert_eq!(concern(&message), Some(Radio::Bluetooth));
}

#[test]
fn signal_strength_and_advertisement_of_a_device_change_nothing() {
    let message = properties_changed(
        DEVICE_PATH,
        bluez::DEVICE,
        &["RSSI", "ManufacturerData"],
        &["TxPower"],
    );
    assert_eq!(concern(&message), None);
}

#[test]
fn the_properties_of_other_interfaces_of_the_stack_change_nothing() {
    let player = properties_changed(
        "/org/bluez/hci0/dev_X/player0",
        "org.bluez.MediaPlayer1",
        &["Position"],
        &[],
    );
    assert_eq!(concern(&player), None);
}

#[test]
fn a_properties_change_with_a_body_that_cannot_be_read_changes_nothing() {
    let message = Message::signal(DEVICE_PATH, PROPERTIES, "PropertiesChanged")
        .unwrap()
        .build(&("only one string",))
        .unwrap();
    assert_eq!(concern(&message), None);
}

#[test]
fn another_signal_of_a_bluetooth_object_makes_the_reading_out_of_date() {
    assert_eq!(
        concern(&signal(DEVICE_PATH, bluez::DEVICE, "Disconnected")),
        Some(Radio::Bluetooth)
    );
}
