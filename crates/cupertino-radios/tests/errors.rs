//! Telling an unanswered call from a refusal, and the text a refusal carries.

#![cfg(feature = "dbus")]

use cupertino_radios::{refusal, unanswered, CALL_TIMEOUT, LINK_TIMEOUT};
use std::sync::Arc;
use std::time::Duration;
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

#[test]
fn a_reading_waits_three_seconds_and_a_link_call_ninety() {
    assert_eq!(CALL_TIMEOUT, Duration::from_secs(3));
    assert_eq!(LINK_TIMEOUT, Duration::from_secs(90));
}

#[test]
fn the_buss_no_reply_and_timeout_errors_say_the_service_did_not_answer() {
    for error in [
        zbus::fdo::Error::NoReply("x".into()),
        zbus::fdo::Error::Timeout("x".into()),
        zbus::fdo::Error::TimedOut("x".into()),
    ] {
        assert!(unanswered(&zbus::Error::from(error)));
    }
}

#[test]
fn the_same_errors_named_by_a_service_say_so_too() {
    for name in [
        "org.freedesktop.DBus.Error.NoReply",
        "org.freedesktop.DBus.Error.Timeout",
        "org.freedesktop.DBus.Error.TimedOut",
    ] {
        assert!(unanswered(&method_error(name, None)), "{name}");
    }
}

#[test]
fn a_timed_out_socket_says_so() {
    let error =
        zbus::Error::InputOutput(Arc::new(std::io::Error::from(std::io::ErrorKind::TimedOut)));
    assert!(unanswered(&error));
}

#[test]
fn other_errors_are_not_a_missing_answer() {
    assert!(!unanswered(&zbus::Error::from(
        zbus::fdo::Error::AccessDenied("x".into())
    )));
    assert!(!unanswered(&method_error(
        "org.freedesktop.DBus.Error.AccessDenied",
        None
    )));
    assert!(!unanswered(&zbus::Error::InputOutput(Arc::new(
        std::io::Error::from(std::io::ErrorKind::BrokenPipe)
    ))));
    assert!(!unanswered(&zbus::Error::Failure("x".into())));
}

#[test]
fn a_refusal_is_the_text_the_service_sent() {
    let error = method_error("org.example.Error.Failed", Some("The passphrase is wrong."));
    assert_eq!(refusal(&error), "The passphrase is wrong.");
}

#[test]
fn a_refusal_without_text_is_the_error_as_the_bus_words_it() {
    let silent = method_error("org.example.Error.Failed", None);
    assert_eq!(refusal(&silent), silent.to_string());
    let empty = method_error("org.example.Error.Failed", Some(""));
    assert_eq!(refusal(&empty), empty.to_string());
}

#[test]
fn a_refusal_of_the_buss_own_error_is_its_description() {
    let error = zbus::Error::from(zbus::fdo::Error::AccessDenied("Not authorized.".into()));
    assert_eq!(refusal(&error), "Not authorized.");
}

#[test]
fn any_other_error_is_worded_as_it_is() {
    let error = zbus::Error::Failure("something broke".into());
    assert_eq!(refusal(&error), error.to_string());
    assert!(refusal(&error).contains("something broke"));
}
