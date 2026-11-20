//! Who calls the control interface: the caller the bus names is read from its process.

use cupertino_controlcenter::control::{judge, BUS_NAME, OBJECT_PATH};
use cupertino_peer::{Caller, Relation};
use zbus::fdo::ConnectionCredentials;

#[test]
fn the_interface_keeps_the_names_its_callers_use() {
    assert_eq!(BUS_NAME, "org.cupertino.ControlPanel1");
    assert_eq!(OBJECT_PATH, "/org/cupertino/ControlPanel1");
}

#[test]
fn a_caller_the_bus_gives_no_process_for_is_anonymous_and_refused() {
    let caller = judge(None);
    assert!(matches!(caller, Caller::Anonymous));
    assert!(caller
        .require_relation(std::process::id(), 1, &[Relation::Itself, Relation::Other])
        .is_err());
}

#[test]
fn a_caller_named_by_its_process_is_classified_by_it() {
    let own = std::process::id();
    let credentials = ConnectionCredentials::default().set_process_id(own);
    let caller = judge(Some(&credentials));
    assert_eq!(caller.pid(), Some(own));
    assert_eq!(caller.require_relation(own, 1, &[Relation::Itself]), Ok(()));
    assert!(caller
        .require_relation(own, 1, &[Relation::Parent])
        .is_err());
}
