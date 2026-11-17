//! The names the record and the store use: severities, address kinds, why a store is unavailable, the limits and how a
//! line is read as a record.

use std::path::PathBuf;

use cupertino_diagnostic_events::store::{self, Limits, Unavailable};
use cupertino_diagnostic_events::{
    self as diag, AddressKind, Corrupt, Record, Severity, EVENT_BYTES, FIELD_TEXT_LIMIT,
    RATE_PER_ID, RATE_WINDOW, RETENTION_DAYS, RING_BYTES, RING_EVENTS, SCHEMA, SEGMENT_BYTES,
    STORE_BYTES,
};

#[test]
fn severities_have_names_that_read_back_and_a_syslog_priority() {
    assert_eq!(
        Severity::ALL.map(|s| s.name()),
        ["info", "warn", "error", "fatal"]
    );
    for s in Severity::ALL {
        assert_eq!(Severity::from_name(s.name()), Some(s));
        assert_eq!(s.to_string(), s.name());
    }
    assert_eq!(Severity::from_name("debug"), None);
    assert_eq!(Severity::ALL.map(|s| s.priority()), [6, 4, 3, 2]);
}

#[test]
fn address_kinds_have_the_prefixes_of_the_readme() {
    assert_eq!(
        [
            AddressKind::Ip,
            AddressKind::Mac,
            AddressKind::Bluetooth,
            AddressKind::Ssid,
            AddressKind::DeviceName
        ]
        .map(|k| k.name()),
        ["ip", "mac", "bluetooth", "ssid", "device"]
    );
}

#[test]
fn the_limits_are_the_documented_ones() {
    assert_eq!(SCHEMA, "cupertino.diag.v1");
    assert_eq!(SEGMENT_BYTES, 1 << 20);
    assert_eq!(STORE_BYTES, 8 << 20);
    assert_eq!(RETENTION_DAYS, 14);
    assert_eq!(EVENT_BYTES, 4096);
    assert_eq!(FIELD_TEXT_LIMIT, 256);
    assert_eq!((RING_EVENTS, RING_BYTES), (256, 256 << 10));
    assert_eq!(
        (RATE_PER_ID, RATE_WINDOW),
        (10, std::time::Duration::from_secs(10))
    );
    let default = Limits::default();
    assert_eq!(default.segment_bytes, SEGMENT_BYTES);
    assert_eq!(default.store_bytes, STORE_BYTES);
    assert_eq!(default.event_bytes, EVENT_BYTES);
    assert_eq!(
        default.retention,
        std::time::Duration::from_secs(RETENTION_DAYS * 86_400)
    );
}

#[test]
fn every_reason_a_store_is_unavailable_has_a_stable_code_and_a_description() {
    let all = [
        (Unavailable::NoHome, "no_home"),
        (Unavailable::Symlink, "symlink"),
        (Unavailable::NotADirectory, "not_a_directory"),
        (Unavailable::NotOwned, "not_owned"),
        (Unavailable::Writable, "writable_by_others"),
        (Unavailable::ReadOnly, "read_only"),
        (Unavailable::NoSpace, "no_space"),
        (Unavailable::Denied, "denied"),
        (Unavailable::Io, "io_error"),
    ];
    for (reason, code) in all {
        assert_eq!(reason.code(), code);
        assert!(!reason.describe().is_empty());
    }
}

#[test]
fn an_io_error_maps_to_its_reason() {
    use std::io::{Error, ErrorKind};
    for (kind, reason) in [
        (ErrorKind::ReadOnlyFilesystem, Unavailable::ReadOnly),
        (ErrorKind::StorageFull, Unavailable::NoSpace),
        (ErrorKind::PermissionDenied, Unavailable::Denied),
        (ErrorKind::NotADirectory, Unavailable::NotADirectory),
        (ErrorKind::Other, Unavailable::Io),
    ] {
        assert_eq!(Unavailable::of(&Error::from(kind)), reason);
    }
}

#[test]
fn the_user_store_is_under_the_state_home_else_under_home() {
    let var = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| std::ffi::OsString::from(v))
        }
    };
    assert_eq!(
        store::user_dir(var(&[("XDG_STATE_HOME", "/state"), ("HOME", "/home/u")])),
        Some(PathBuf::from("/state/cupertino/events"))
    );
    assert_eq!(
        store::user_dir(var(&[("HOME", "/home/u")])),
        Some(PathBuf::from("/home/u/.local/state/cupertino/events"))
    );
    assert_eq!(store::user_dir(var(&[])), None);
}

#[test]
fn a_line_is_a_record_of_this_schema_or_corrupt() {
    assert_eq!(Record::parse("garbage"), Err(Corrupt::Malformed));
    assert_eq!(Record::parse("{\"schema\": 3}"), Err(Corrupt::Malformed));
    assert_eq!(
        Record::parse("{\"schema\":\"cupertino.diag.v2\"}"),
        Err(Corrupt::Schema("cupertino.diag.v2".into()))
    );
    assert_eq!(
        Record::parse("{\"schema\":\"cupertino.diag.v1\"}"),
        Err(Corrupt::Malformed)
    );
}

#[test]
fn a_record_ignores_fields_it_does_not_know() {
    let policy =
        diag::redact::Policy::new(None, None, None, diag::redact::Salt::from_bytes([1; 16]));
    let origin = diag::Origin {
        time: "2026-11-09T14:03:27.512Z".into(),
        boot_id: None,
        session: None,
        component: "c".into(),
        version: "1".into(),
        commit: None,
        pid: 1,
    };
    let record = diag::event(Severity::Info, "test.vocab.id", "m").to_record(&policy, &origin, 1);
    let mut value: serde_json::Value = serde_json::from_str(&record.to_json()).unwrap();
    value["added_later"] = serde_json::json!({"x": 1});
    let read = Record::parse(&value.to_string()).unwrap();
    assert_eq!(read, record);
}
