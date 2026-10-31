//! The Settings backend on a private bus, called as xdg-desktop-portal calls it: `ReadAll` and `Read` give the session's
//! appearance, and a change of appearance emits `SettingChanged` for each key it changes.

use std::collections::HashMap;

use cupertino_portal::settings::{Appearance, Service};
use cupertino_portal::{BUS_NAME, PORTAL_PATH};
use cupertino_test_bus::{missing_tool, PrivateBus, WAIT};
use zbus::blocking::{Connection, MessageIterator};
use zbus::zvariant::OwnedValue;
use zbus::MatchRule;

const INTERFACE: &str = "org.freedesktop.impl.portal.Settings";

struct Running {
    _bus: PrivateBus,
    portal: Service,
    _served: Connection,
    caller: Connection,
}

fn start(appearance: Appearance) -> Option<Running> {
    let bus = PrivateBus::start()?;
    let served = bus.connect();
    let portal = Service::serve(&served, appearance).unwrap();
    served.request_name(BUS_NAME).unwrap();
    let caller = bus.connect();
    Some(Running {
        _bus: bus,
        portal,
        _served: served,
        caller,
    })
}

type All = HashMap<String, HashMap<String, OwnedValue>>;

fn read_all(caller: &Connection, namespaces: &[&str]) -> All {
    caller
        .call_method(
            Some(BUS_NAME),
            PORTAL_PATH,
            Some(INTERFACE),
            "ReadAll",
            &(namespaces,),
        )
        .unwrap()
        .body()
        .deserialize()
        .unwrap()
}

fn values(all: &All) -> Vec<(String, u32)> {
    let mut values: Vec<_> = all["org.freedesktop.appearance"]
        .iter()
        .map(|(k, v)| (k.clone(), u32::try_from(v).unwrap()))
        .collect();
    values.sort();
    values
}

#[test]
fn read_all_gives_the_appearance_to_patterns_that_match_its_namespace() {
    let Some(r) = start(Appearance {
        dark: true,
        increase_contrast: true,
        reduce_motion: false,
    }) else {
        return missing_tool("dbus-daemon");
    };
    let expected = [
        ("color-scheme".to_owned(), 1),
        ("contrast".to_owned(), 1),
        ("reduced-motion".to_owned(), 0),
    ];
    assert_eq!(values(&read_all(&r.caller, &[])), expected);
    assert_eq!(
        values(&read_all(&r.caller, &["org.freedesktop.*"])),
        expected
    );
    assert!(read_all(&r.caller, &["org.gnome.desktop.interface"]).is_empty());
}

#[test]
fn read_gives_one_key_and_refuses_an_unknown_one() {
    let Some(r) = start(Appearance::default()) else {
        return missing_tool("dbus-daemon");
    };
    let read = |namespace: &str, key: &str| {
        r.caller.call_method(
            Some(BUS_NAME),
            PORTAL_PATH,
            Some(INTERFACE),
            "Read",
            &(namespace, key),
        )
    };
    let light: OwnedValue = read("org.freedesktop.appearance", "color-scheme")
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(u32::try_from(light).unwrap(), 2, "prefer light");
    assert!(read("org.freedesktop.appearance", "accent-color").is_err());
    assert!(read("org.gnome.desktop.interface", "color-scheme").is_err());
}

#[test]
fn a_change_of_appearance_emits_setting_changed_for_each_key_it_changes() {
    let Some(r) = start(Appearance::default()) else {
        return missing_tool("dbus-daemon");
    };
    let rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(INTERFACE)
        .unwrap()
        .member("SettingChanged")
        .unwrap()
        .build();
    let mut signals = MessageIterator::for_match_rule(rule, &r.caller, None).unwrap();
    let dark = Appearance {
        dark: true,
        ..Appearance::default()
    };
    r.portal.set(dark).unwrap();
    r.portal.set(dark).unwrap();
    r.portal
        .set(Appearance {
            reduce_motion: true,
            ..dark
        })
        .unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for message in signals.by_ref().take(2) {
            let (namespace, key, value): (String, String, OwnedValue) =
                message.unwrap().body().deserialize().unwrap();
            tx.send((namespace, key, u32::try_from(value).unwrap()))
                .unwrap();
        }
    });
    let changed: Vec<_> = (0..2).map(|_| rx.recv_timeout(WAIT).unwrap()).collect();
    assert_eq!(
        changed,
        [
            (
                "org.freedesktop.appearance".to_owned(),
                "color-scheme".to_owned(),
                1
            ),
            (
                "org.freedesktop.appearance".to_owned(),
                "reduced-motion".to_owned(),
                1
            ),
        ],
        "the same appearance again changes nothing"
    );
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(200))
            .is_err(),
        "no other signal"
    );
}

#[test]
fn the_portal_file_names_the_bus_name_and_the_settings_interface() {
    let file = include_str!("../data/cupertino.portal");
    assert!(file.lines().any(|l| l == format!("DBusName={BUS_NAME}")));
    let interfaces = file
        .lines()
        .find_map(|l| l.strip_prefix("Interfaces="))
        .unwrap();
    assert!(interfaces.split(';').any(|i| i == INTERFACE));
}
