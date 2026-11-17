//! Telling an unanswered call from a refusal, and the text a refusal carries.

#![cfg(feature = "dbus")]

use cupertino_radios::{refusal, unanswered};
use std::sync::Arc;
use zbus::names::OwnedErrorName;
use zbus::Message;

fn method_error(name: &str, text: Option<&str>) -> zbus::Error {
    let message = Message::method_call("/org/example", "Method")
        .unwrap()
        .build(&())
        .unwrap();
    zbus::Error::MethodError(
        OwnedErrorName::try_from(name).unwrap(),
        text.map(str::to_owned),
        message,
    )
}

fn io(kind: std::io::ErrorKind) -> zbus::Error {
    zbus::Error::InputOutput(Arc::new(std::io::Error::from(kind)))
}

#[test]
fn only_no_reply_and_timeouts_are_a_missing_answer() {
    for (error, expected) in [
        (zbus::fdo::Error::NoReply("x".into()).into(), true),
        (zbus::fdo::Error::Timeout("x".into()).into(), true),
        (zbus::fdo::Error::TimedOut("x".into()).into(), true),
        (
            method_error("org.freedesktop.DBus.Error.NoReply", None),
            true,
        ),
        (
            method_error("org.freedesktop.DBus.Error.Timeout", None),
            true,
        ),
        (
            method_error("org.freedesktop.DBus.Error.TimedOut", None),
            true,
        ),
        (io(std::io::ErrorKind::TimedOut), true),
        (zbus::fdo::Error::AccessDenied("x".into()).into(), false),
        (
            method_error("org.freedesktop.DBus.Error.AccessDenied", None),
            false,
        ),
        (io(std::io::ErrorKind::BrokenPipe), false),
        (zbus::Error::Failure("x".into()), false),
    ] {
        assert_eq!(unanswered(&error), expected, "{error}");
    }
}

#[test]
fn a_refusal_is_the_text_the_service_sent_or_else_the_error_as_worded() {
    let worded = |error: &zbus::Error| error.to_string();
    for (error, expected) in [
        (
            method_error("org.example.Error.Failed", Some("The passphrase is wrong.")),
            Some("The passphrase is wrong."),
        ),
        (method_error("org.example.Error.Failed", None), None),
        (method_error("org.example.Error.Failed", Some("")), None),
        (
            zbus::fdo::Error::AccessDenied("Not authorized.".into()).into(),
            Some("Not authorized."),
        ),
        (zbus::Error::Failure("something broke".into()), None),
    ] {
        let expected = expected.map_or_else(|| worded(&error), str::to_owned);
        assert_eq!(refusal(&error), expected, "{error}");
    }
}
