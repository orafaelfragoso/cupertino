//! Who sent a signal, and the limits of the lookup connection, without a bus.

use cupertino_peer::lookup::{Lookup, ANSWER_DEADLINE, MAX_IN_FLIGHT};
use cupertino_peer::signal::{sent_by_bus, Owner, BUS_DAEMON};
use std::str::FromStr;
use std::time::Duration;
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
fn the_bus_daemon_is_the_sender_of_its_own_signals() {
    assert_eq!(BUS_DAEMON, "org.freedesktop.DBus");
    assert!(sent_by_bus(&signal_from(Some(BUS_DAEMON))));
}

#[test]
fn a_connection_is_not_the_bus_daemon() {
    assert!(!sent_by_bus(&signal_from(Some(":1.42"))));
}

#[test]
fn a_message_without_a_sender_is_not_the_bus_daemons() {
    assert!(!sent_by_bus(&signal_from(None)));
}

#[test]
fn nobody_owns_a_name_before_the_owner_is_known() {
    let owner = Owner::default();
    let sender = UniqueName::try_from(":1.7").unwrap();
    assert!(!owner.is(Some(&sender)));
    assert!(!owner.is(None));
}

#[test]
fn a_clone_of_an_owner_follows_the_same_name() {
    let owner = Owner::default();
    let copy = owner.clone();
    let sender = UniqueName::try_from(":1.7").unwrap();
    assert_eq!(owner.is(Some(&sender)), copy.is(Some(&sender)));
}

#[test]
fn questions_are_limited_in_number_and_in_time() {
    assert_eq!(MAX_IN_FLIGHT, 8);
    assert_eq!(ANSWER_DEADLINE, Duration::from_secs(5));
}

#[test]
fn a_lookup_fails_to_connect_to_a_bus_that_is_not_there() {
    let dir = tempfile::tempdir().unwrap();
    let address =
        zbus::Address::from_str(&format!("unix:path={}/no-bus", dir.path().display())).unwrap();
    assert!(Lookup::connect(address).is_err());
}
