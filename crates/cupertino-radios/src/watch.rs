//! The signals that tell a reading of the radios is out of date: a service starting or stopping, and the services'
//! signals on the objects the readings come from. An access point's strength, a device's statistics or any other
//! object of these services sends nothing here.

use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;

use zbus::export::futures_core::Stream;
use zbus::{MessageStream, OwnedMatchRule};

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

/// Call `on` after every signal of [`rules`] on `connection` (the system bus), from tasks of the connection's own
/// executor, until the connection closes: a reading taken after the call sees what the signal announced. `on` must
/// not wait; a burst of signals calls it once per signal, so it should only note that a reading is due.
pub async fn follow(
    connection: &zbus::Connection,
    on: Arc<dyn Fn() + Send + Sync>,
) -> zbus::Result<()> {
    for rule in rules() {
        let rule = OwnedMatchRule::try_from(rule.as_str())?;
        let mut stream = MessageStream::for_match_rule(rule, connection, Some(QUEUE)).await?;
        let on = on.clone();
        connection
            .executor()
            .spawn(
                async move {
                    while poll_fn(|cx| Pin::new(&mut stream).poll_next(cx))
                        .await
                        .is_some()
                    {
                        on();
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
