//! The signals that tell a reading of the radios is out of date: a service starting or stopping, and the services'
//! signals on the objects the readings come from. An access point's strength, a device's statistics or any other
//! object of these services sends nothing here, and a change of a Bluetooth object that no reading shows (a device's
//! signal strength and advertisement while the adapter discovers, a player's position) is passed over ([`concern`]).

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;

use zbus::export::futures_core::Stream;
use zbus::zvariant::Value;
use zbus::{Message, MessageStream, OwnedMatchRule};

use crate::{bluez, btd, netd, nm};

/// The bus daemon's own name, which sends `NameOwnerChanged`.
const BUS_DAEMON: &str = "org.freedesktop.DBus";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// How many signals of one rule may wait for [`follow`]'s task; each only says "read again", so more would tell
/// nothing new.
const QUEUE: usize = 8;

/// Which radio a signal is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Radio {
    Network,
    Bluetooth,
}

/// The match rules of the system bus for the radios: each service starting or stopping, every signal of the network
/// services' objects (their own interfaces tell their changes in signals of their own), and the Bluetooth stack's
/// objects coming, going and changing.
pub fn rules() -> Vec<String> {
    let owner = |name: &str| {
        format!(
            "type='signal',sender='{BUS_DAEMON}',interface='{BUS_DAEMON}',member='NameOwnerChanged',arg0='{name}'"
        )
    };
    let object = |name: &str, path: &str| format!("type='signal',sender='{name}',path='{path}'");
    vec![
        owner(netd::NAME),
        object(netd::NAME, netd::PATH),
        owner(nm::NAME),
        object(nm::NAME, nm::PATH),
        object(nm::NAME, nm::SETTINGS_PATH),
        owner(btd::NAME),
        object(btd::NAME, btd::PATH),
        owner(bluez::NAME),
        format!(
            "type='signal',sender='{}',path='/',interface='{OBJECT_MANAGER}'",
            bluez::NAME
        ),
        format!(
            "type='signal',sender='{}',path_namespace='{}',interface='{PROPERTIES}',member='PropertiesChanged'",
            bluez::NAME,
            bluez::PATHS
        ),
    ]
}

/// Which radio a service's signal that came through [`rules`] is about, by its object. A message's sender is a unique
/// name, so the services are told apart by their objects: the rules let only theirs through.
pub fn classify(path: &str, interface: Option<&str>) -> Option<Radio> {
    match path {
        netd::PATH | nm::PATH | nm::SETTINGS_PATH => Some(Radio::Network),
        btd::PATH => Some(Radio::Bluetooth),
        "/" if interface == Some(OBJECT_MANAGER) => Some(Radio::Bluetooth),
        p if p.starts_with(bluez::PATHS) => Some(Radio::Bluetooth),
        _ => None,
    }
}

/// The properties of the Bluetooth stack's adapter and devices that a reading shows.
const ADAPTER_SHOWN: [&str; 2] = ["Powered", "Discovering"];
const DEVICE_SHOWN: [&str; 5] = ["Address", "Name", "Icon", "Paired", "Connected"];

/// Whether a change of `properties` of the Bluetooth stack's `interface` changes a reading. A device's signal
/// strength and advertisement, which change all the time while the adapter discovers, and the objects of its other
/// interfaces do not.
pub fn shown<'a>(interface: &str, mut properties: impl Iterator<Item = &'a str>) -> bool {
    let shown: &[&str] = match interface {
        bluez::ADAPTER => &ADAPTER_SHOWN,
        bluez::DEVICE => &DEVICE_SHOWN,
        _ => return false,
    };
    properties.any(|property| shown.contains(&property))
}

/// Which radio's reading a signal that came through [`rules`] makes out of date; `None` for one that changes nothing
/// a reading shows.
pub fn concern(message: &Message) -> Option<Radio> {
    let header = message.header();
    let interface = header.interface().map(|i| i.as_str());
    let member = header.member().map(|m| m.as_str());
    if interface == Some(BUS_DAEMON) && member == Some("NameOwnerChanged") {
        let body = message.body();
        let (name, _, _): (&str, &str, &str) = body.deserialize().ok()?;
        return match name {
            netd::NAME | nm::NAME => Some(Radio::Network),
            btd::NAME | bluez::NAME => Some(Radio::Bluetooth),
            _ => None,
        };
    }
    let path = header.path()?.as_str();
    let radio = classify(path, interface)?;
    if path.starts_with(bluez::PATHS)
        && interface == Some(PROPERTIES)
        && member == Some("PropertiesChanged")
    {
        let body = message.body();
        let (of, changed, invalidated): (&str, HashMap<&str, Value<'_>>, Vec<&str>) =
            body.deserialize().ok()?;
        let properties = changed.keys().copied().chain(invalidated.iter().copied());
        return shown(of, properties).then_some(radio);
    }
    Some(radio)
}

/// Call `on` with the radio of every signal of [`rules`] on `connection` (the system bus) that makes a reading out of
/// date ([`concern`]), from tasks of the connection's own executor, until the connection closes: a reading taken
/// after the call sees what the signal announced. `on` must not wait; a burst of signals calls it once per signal, so
/// it should only note that a reading is due.
pub async fn follow(
    connection: &zbus::Connection,
    on: Arc<dyn Fn(Radio) + Send + Sync>,
) -> zbus::Result<()> {
    for rule in rules() {
        let rule = OwnedMatchRule::try_from(rule.as_str())?;
        let mut stream = MessageStream::for_match_rule(rule, connection, Some(QUEUE)).await?;
        let on = on.clone();
        connection
            .executor()
            .spawn(
                async move {
                    while let Some(message) =
                        poll_fn(|cx| Pin::new(&mut stream).poll_next(cx)).await
                    {
                        if let Some(radio) = message.ok().as_ref().and_then(concern) {
                            on(radio);
                        }
                    }
                },
                "cupertino-radios-follow",
            )
            .detach();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_parses_names_its_sender_and_one_object_or_name() {
        let rules = rules();
        for rule in &rules {
            zbus::MatchRule::try_from(rule.as_str()).unwrap_or_else(|e| panic!("{rule}: {e}"));
            assert!(rule.starts_with("type='signal'"), "signals only: {rule}");
            assert!(
                rule.contains("sender='"),
                "no rule takes every signal of the bus: {rule}"
            );
            assert!(
                rule.contains("arg0='")
                    || rule.contains("path='")
                    || rule.contains("path_namespace='"),
                "a service's signals only on the objects read: {rule}"
            );
        }
        for name in [netd::NAME, nm::NAME, btd::NAME, bluez::NAME] {
            assert!(
                rules.iter().any(|r| r.ends_with(&format!("arg0='{name}'"))),
                "{name} starting"
            );
            assert!(
                rules
                    .iter()
                    .any(|r| r.contains(&format!("sender='{name}',path"))),
                "{name}'s signals"
            );
        }
        assert!(
            !rules
                .iter()
                .any(|r| r.contains("AccessPoint") || r.contains("/Devices")),
            "an access point's strength and a device's statistics send nothing"
        );
    }

    #[test]
    fn only_what_a_reading_shows_of_the_bluetooth_stack_makes_it_out_of_date() {
        let properties = |path: &str, of: &str, changed: &[&str], invalidated: &[&str]| {
            let changed: HashMap<&str, Value<'_>> =
                changed.iter().map(|k| (*k, Value::from(-60i16))).collect();
            let message = Message::signal(path, PROPERTIES, "PropertiesChanged")
                .unwrap()
                .build(&(of, changed, invalidated.to_vec()))
                .unwrap();
            concern(&message)
        };
        let device = "/org/bluez/hci0/dev_00";
        for advertised in ["RSSI", "TxPower", "ManufacturerData", "ServiceData"] {
            assert_eq!(
                properties(device, bluez::DEVICE, &[advertised], &[]),
                None,
                "{advertised}"
            );
        }
        assert_eq!(
            properties(device, bluez::DEVICE, &[], &["RSSI"]),
            None,
            "a strength that is no longer known"
        );
        assert_eq!(
            properties(device, bluez::DEVICE, &["RSSI", "Connected"], &[]),
            Some(Radio::Bluetooth)
        );
        assert_eq!(
            properties(device, bluez::DEVICE, &["Name"], &[]),
            Some(Radio::Bluetooth)
        );
        assert_eq!(
            properties("/org/bluez/hci0", bluez::ADAPTER, &["Discovering"], &[]),
            Some(Radio::Bluetooth)
        );
        assert_eq!(
            properties(
                "/org/bluez/hci0/dev_00/player0",
                "org.bluez.MediaPlayer1",
                &["Position"],
                &[]
            ),
            None
        );
        // the network services' objects tell every change
        assert_eq!(
            properties(nm::PATH, nm::NAME, &["Anything"], &[]),
            Some(Radio::Network)
        );

        let owner = |name: &str| {
            let message = Message::signal("/org/freedesktop/DBus", BUS_DAEMON, "NameOwnerChanged")
                .unwrap()
                .build(&(name, "", ":1.5"))
                .unwrap();
            concern(&message)
        };
        assert_eq!(owner(nm::NAME), Some(Radio::Network));
        assert_eq!(owner(netd::NAME), Some(Radio::Network));
        assert_eq!(owner(bluez::NAME), Some(Radio::Bluetooth));
        assert_eq!(owner(btd::NAME), Some(Radio::Bluetooth));
        assert_eq!(owner("org.example.Other"), None);
    }

    #[test]
    fn a_signal_names_its_radio_by_its_object() {
        assert_eq!(classify(netd::PATH, Some(netd::NAME)), Some(Radio::Network));
        assert_eq!(classify(nm::PATH, Some(PROPERTIES)), Some(Radio::Network));
        assert_eq!(classify(nm::SETTINGS_PATH, Some("x")), Some(Radio::Network));
        assert_eq!(
            classify(
                "/org/freedesktop/NetworkManager/AccessPoint/3",
                Some(PROPERTIES)
            ),
            None
        );
        assert_eq!(classify(btd::PATH, Some(btd::NAME)), Some(Radio::Bluetooth));
        assert_eq!(
            classify("/org/bluez/hci0/dev_00", Some(PROPERTIES)),
            Some(Radio::Bluetooth)
        );
        assert_eq!(classify("/", Some(OBJECT_MANAGER)), Some(Radio::Bluetooth));
        assert_eq!(classify("/", Some(PROPERTIES)), None);
        assert_eq!(classify("/elsewhere", None), None);
    }
}
