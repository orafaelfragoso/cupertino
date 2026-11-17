//! Who sent a signal, and the lookup connection, without a bus.

use cupertino_peer::lookup::Lookup;
use cupertino_peer::signal::{sent_by_bus, Owner, BUS_DAEMON};
use std::str::FromStr;
use zbus::names::UniqueName;
use zbus::Message;

fn signal_from(sender: Option<&str>) -> Message {
    let mut builder = Message::signal("/org/example", "org.example.Interface", "Changed").unwrap();
    if let Some(sender) = sender {
        builder = builder.sender(sender).unwrap();
    }
    builder.build(&()).unwrap()
}

#[test]
fn only_the_bus_daemon_sends_the_bus_daemons_signals() {
    for (sender, expected) in [
        (Some(BUS_DAEMON), true),
        (Some(":1.42"), false),
        (None, false),
    ] {
        assert_eq!(sent_by_bus(&signal_from(sender)), expected, "{sender:?}");
    }
}

#[test]
fn nobody_owns_a_name_before_the_owner_is_known() {
    let owner = Owner::default();
    let sender = UniqueName::try_from(":1.7").unwrap();
    assert!(!owner.is(Some(&sender)));
    assert!(!owner.is(None));
}

#[test]
fn a_lookup_fails_to_connect_to_a_bus_that_is_not_there() {
    let dir = tempfile::tempdir().unwrap();
    let address =
        zbus::Address::from_str(&format!("unix:path={}/no-bus", dir.path().display())).unwrap();
    assert!(Lookup::connect(address).is_err());
}
