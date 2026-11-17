//! Choosing the session to ask logind about, and decoding the signals it sends, without a bus.

use cupertino_devices::DevT;
use cupertino_seat::logind::decode;
use cupertino_seat::state::{Input, PauseKind};
use cupertino_seat::{Error, SessionLookup};
use std::collections::HashMap;
use zbus::export::serde::Serialize;
use zbus::zvariant::{DynamicType, Value};
use zbus::Message;

const SESSION_PATH: &str = "/org/freedesktop/login1/session/_32";

fn signal(interface: &str, member: &str, body: &(impl Serialize + DynamicType)) -> Message {
    Message::signal(SESSION_PATH, interface, member)
        .unwrap()
        .build(body)
        .unwrap()
}

fn session_signal(member: &str, body: &(impl Serialize + DynamicType)) -> Message {
    signal("org.freedesktop.login1.Session", member, body)
}

fn properties_changed(
    interface: &str,
    changed: HashMap<&str, Value<'_>>,
    invalidated: Vec<&str>,
) -> Message {
    signal(
        "org.freedesktop.DBus.Properties",
        "PropertiesChanged",
        &(interface, changed, invalidated),
    )
}

#[test]
fn the_session_id_of_the_environment_wins_over_the_process_id() {
    assert_eq!(
        SessionLookup::choose(Some("32".to_owned()), 4242),
        SessionLookup::ById("32".to_owned())
    );
    assert_eq!(
        SessionLookup::choose(None, 4242),
        SessionLookup::ByPid(4242)
    );
}

#[test]
fn an_empty_session_id_is_still_the_id_asked_for() {
    assert_eq!(
        SessionLookup::choose(Some(String::new()), 1),
        SessionLookup::ById(String::new())
    );
}

#[test]
fn pause_device_decodes_to_the_device_and_its_kind() {
    for (wire, kind) in [
        ("pause", PauseKind::Pause),
        ("force", PauseKind::Force),
        ("gone", PauseKind::Gone),
    ] {
        let message = session_signal("PauseDevice", &(226u32, 0u32, wire));
        assert_eq!(
            decode(&message).unwrap(),
            Some(Input::PauseDevice {
                dev: DevT::new(226, 0),
                kind
            })
        );
    }
}

#[test]
fn a_pause_type_logind_does_not_define_is_an_error() {
    let message = session_signal("PauseDevice", &(226u32, 0u32, "sleep"));
    assert!(matches!(
        decode(&message),
        Err(Error::UnknownPauseType(kind)) if kind == "sleep"
    ));
}

#[test]
fn a_pause_device_body_of_the_wrong_shape_names_the_step() {
    let message = session_signal("PauseDevice", &("not", "the", "right types"));
    let error = decode(&message).unwrap_err();
    assert!(matches!(
        &error,
        Error::Bus {
            what: "decoding a session signal",
            ..
        }
    ));
    assert!(error.to_string().starts_with("decoding a session signal: "));
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn resume_device_decodes_to_the_device_and_drops_the_descriptor() {
    let null = zbus::zvariant::OwnedFd::from(std::os::fd::OwnedFd::from(
        std::fs::File::open("/dev/null").unwrap(),
    ));
    let message = session_signal("ResumeDevice", &(13u32, 64u32, null));
    assert_eq!(
        decode(&message).unwrap(),
        Some(Input::ResumeDevice {
            dev: DevT::new(13, 64)
        })
    );
}

#[test]
fn the_active_property_decodes_to_its_value() {
    for active in [true, false] {
        let message = properties_changed(
            "org.freedesktop.login1.Session",
            HashMap::from([("Active", Value::Bool(active))]),
            Vec::new(),
        );
        assert_eq!(
            decode(&message).unwrap(),
            Some(Input::ActiveChanged(active))
        );
    }
}

#[test]
fn other_properties_and_other_interfaces_are_not_inputs() {
    let other_property = properties_changed(
        "org.freedesktop.login1.Session",
        HashMap::from([("IdleHint", Value::Bool(true))]),
        Vec::new(),
    );
    assert_eq!(decode(&other_property).unwrap(), None);
    let other_interface = properties_changed(
        "org.freedesktop.login1.Seat",
        HashMap::from([("Active", Value::Bool(true))]),
        Vec::new(),
    );
    assert_eq!(decode(&other_interface).unwrap(), None);
}

#[test]
fn an_active_property_that_is_not_a_boolean_is_an_error() {
    let message = properties_changed(
        "org.freedesktop.login1.Session",
        HashMap::from([("Active", Value::U32(1))]),
        Vec::new(),
    );
    assert!(matches!(decode(&message), Err(Error::Bus { .. })));
}

#[test]
fn an_invalidated_active_property_is_an_error_instead_of_a_guess() {
    let message = properties_changed(
        "org.freedesktop.login1.Session",
        HashMap::new(),
        vec!["Active"],
    );
    assert!(matches!(decode(&message), Err(Error::ActiveInvalidated)));
    let other = properties_changed(
        "org.freedesktop.login1.Session",
        HashMap::new(),
        vec!["IdleHint"],
    );
    assert_eq!(decode(&other).unwrap(), None);
}

#[test]
fn signals_the_session_does_not_use_are_not_errors() {
    assert_eq!(decode(&session_signal("Lock", &())).unwrap(), None);
    assert_eq!(decode(&session_signal("Unlock", &())).unwrap(), None);
    let other_interface = signal("org.example.Other", "PauseDevice", &(1u32, 2u32, "pause"));
    assert_eq!(decode(&other_interface).unwrap(), None);
}
