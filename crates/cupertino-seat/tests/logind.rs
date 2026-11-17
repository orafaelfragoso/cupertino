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
const SESSION: &str = "org.freedesktop.login1.Session";

fn signal(interface: &str, member: &str, body: &(impl Serialize + DynamicType)) -> Message {
    Message::signal(SESSION_PATH, interface, member)
        .unwrap()
        .build(body)
        .unwrap()
}

fn session_signal(member: &str, body: &(impl Serialize + DynamicType)) -> Message {
    signal(SESSION, member, body)
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

fn active(value: Value<'_>) -> Message {
    properties_changed(SESSION, HashMap::from([("Active", value)]), Vec::new())
}

#[test]
fn the_session_id_of_the_environment_even_empty_wins_over_the_process_id() {
    for (id, expected) in [
        (Some("32"), SessionLookup::ById("32".to_owned())),
        (Some(""), SessionLookup::ById(String::new())),
        (None, SessionLookup::ByPid(4242)),
    ] {
        assert_eq!(
            SessionLookup::choose(id.map(str::to_owned), 4242),
            expected,
            "{id:?}"
        );
    }
}

#[test]
fn the_session_signals_decode_to_inputs_and_the_rest_to_nothing() {
    let null = zbus::zvariant::OwnedFd::from(std::os::fd::OwnedFd::from(
        std::fs::File::open("/dev/null").unwrap(),
    ));
    let pause = |kind| {
        Some(Input::PauseDevice {
            dev: DevT::new(226, 0),
            kind,
        })
    };
    for (name, message, expected) in [
        (
            "pause",
            session_signal("PauseDevice", &(226u32, 0u32, "pause")),
            pause(PauseKind::Pause),
        ),
        (
            "force",
            session_signal("PauseDevice", &(226u32, 0u32, "force")),
            pause(PauseKind::Force),
        ),
        (
            "gone",
            session_signal("PauseDevice", &(226u32, 0u32, "gone")),
            pause(PauseKind::Gone),
        ),
        (
            "resume, the descriptor dropped",
            session_signal("ResumeDevice", &(13u32, 64u32, null)),
            Some(Input::ResumeDevice {
                dev: DevT::new(13, 64),
            }),
        ),
        (
            "active",
            active(Value::Bool(true)),
            Some(Input::ActiveChanged(true)),
        ),
        (
            "inactive",
            active(Value::Bool(false)),
            Some(Input::ActiveChanged(false)),
        ),
        (
            "another property",
            properties_changed(
                SESSION,
                HashMap::from([("IdleHint", Value::Bool(true))]),
                Vec::new(),
            ),
            None,
        ),
        (
            "another property invalidated",
            properties_changed(SESSION, HashMap::new(), vec!["IdleHint"]),
            None,
        ),
        (
            "the seat's properties",
            properties_changed(
                "org.freedesktop.login1.Seat",
                HashMap::from([("Active", Value::Bool(true))]),
                Vec::new(),
            ),
            None,
        ),
        ("lock", session_signal("Lock", &()), None),
        ("unlock", session_signal("Unlock", &()), None),
        (
            "another interface",
            signal("org.example.Other", "PauseDevice", &(1u32, 2u32, "pause")),
            None,
        ),
    ] {
        assert_eq!(decode(&message).unwrap(), expected, "{name}");
    }
}

#[test]
fn a_session_signal_that_cannot_be_trusted_is_an_error_instead_of_a_guess() {
    for (name, message) in [
        (
            "a pause type logind does not define",
            session_signal("PauseDevice", &(226u32, 0u32, "sleep")),
        ),
        (
            "a pause type in the wrong case",
            session_signal("PauseDevice", &(226u32, 0u32, "Pause")),
        ),
        (
            "a pause body of the wrong shape",
            session_signal("PauseDevice", &("not", "the", "right types")),
        ),
        ("an active that is not a boolean", active(Value::U32(1))),
        (
            "an invalidated active",
            properties_changed(SESSION, HashMap::new(), vec!["Active"]),
        ),
    ] {
        let error = decode(&message).unwrap_err();
        let kind_matches = match name {
            "a pause type logind does not define" => {
                matches!(&error, Error::UnknownPauseType(kind) if kind == "sleep")
            }
            "a pause type in the wrong case" => {
                matches!(&error, Error::UnknownPauseType(kind) if kind == "Pause")
            }
            "a pause body of the wrong shape" => matches!(
                &error,
                Error::Bus {
                    what: "decoding a session signal",
                    ..
                }
            ),
            "an active that is not a boolean" => matches!(&error, Error::Bus { .. }),
            _ => matches!(&error, Error::ActiveInvalidated),
        };
        assert!(kind_matches, "{name}: {error:?}");
    }
}
