//! The journald sink: the native protocol's fields, the binary-safe form of a value with a newline, and the fallback
//! to standard error when there is no journal.

mod support;

use std::os::unix::net::UnixDatagram;
use std::path::Path;

use cupertino_diagnostic_events::journal::{self, Journal};
use cupertino_diagnostic_events::redact::{Policy, Salt};
use cupertino_diagnostic_events::{self as diag, Origin, Record, Severity, Value};

fn record() -> Record {
    let policy = Policy::new(
        Some(Path::new("/root")),
        Some("root"),
        Some("box"),
        Salt::from_bytes([5; 16]),
    );
    let origin = Origin {
        time: "2026-11-09T00:00:00.000Z".into(),
        boot_id: None,
        session: None,
        component: "cupertino-netd".into(),
        version: "0.1.0".into(),
        commit: None,
        pid: 77,
    };
    diag::event(
        Severity::Error,
        "network.backend.start_failed",
        "the network backend did not start\nat all",
    )
    .field("backend", Value::Public("iwd"))
    .to_record(&policy, &origin, 1)
}

fn field<'d>(fields: &'d [(String, Vec<u8>)], key: &str) -> &'d [u8] {
    &fields
        .iter()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("no {key}"))
        .1
}

#[test]
fn a_datagram_carries_the_fields_and_the_whole_line() {
    let record = record();
    let line = record.to_json();
    let datagram = journal::datagram(&record, &line, "cupertino-netd");
    let fields = journal::parse(&datagram).unwrap();
    let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "MESSAGE",
            "PRIORITY",
            "SYSLOG_IDENTIFIER",
            "CUPERTINO_DIAG_SCHEMA",
            "CUPERTINO_DIAG_ID",
            "CUPERTINO_DIAG_SEVERITY",
            "CUPERTINO_DIAG_JSON"
        ]
    );
    assert_eq!(
        field(&fields, "MESSAGE"),
        b"the network backend did not start\nat all"
    );
    assert_eq!(field(&fields, "PRIORITY"), b"3");
    assert_eq!(field(&fields, "SYSLOG_IDENTIFIER"), b"cupertino-netd");
    assert_eq!(field(&fields, "CUPERTINO_DIAG_SCHEMA"), b"1");
    assert_eq!(
        field(&fields, "CUPERTINO_DIAG_ID"),
        b"network.backend.start_failed"
    );
    assert_eq!(field(&fields, "CUPERTINO_DIAG_SEVERITY"), b"error");
    let json = std::str::from_utf8(field(&fields, "CUPERTINO_DIAG_JSON")).unwrap();
    assert_eq!(Record::parse(json).unwrap(), record);
    // the message holds a newline: the binary-safe form, the key, a newline, the 64-bit length, the value, a newline
    let message = b"the network backend did not start\nat all";
    let mut expected = b"MESSAGE\n".to_vec();
    expected.extend_from_slice(&(message.len() as u64).to_le_bytes());
    expected.extend_from_slice(message);
    expected.push(b'\n');
    assert!(datagram.starts_with(&expected));
    assert!(datagram.windows(11).any(|w| w == b"\nPRIORITY=3"));
    for (severity, priority) in [
        (Severity::Info, 6),
        (Severity::Warn, 4),
        (Severity::Error, 3),
        (Severity::Fatal, 2),
    ] {
        assert_eq!(severity.priority(), priority);
    }
}

#[test]
fn a_datagram_reaches_a_socket_named_by_path() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("socket");
    let socket = UnixDatagram::bind(&path).unwrap();
    let mut journal = Journal::new(&path);
    let record = record();
    let datagram = journal::datagram(&record, &record.to_json(), "cupertino-netd");
    journal.send(&datagram).unwrap();
    let mut buf = vec![0u8; 65536];
    let n = socket.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], &datagram[..]);
    // no socket: an error the emitter falls back on
    drop(socket);
    std::fs::remove_file(&path).unwrap();
    assert!(journal.send(&datagram).is_err());
}

#[test]
fn a_service_writes_to_the_journal_socket_the_environment_names() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let path = state.path().join("journal.socket");
        let socket = UnixDatagram::bind(&path).unwrap();
        let out = support::child(
            "a_service_writes_to_the_journal_socket_the_environment_names",
            state.path(),
            &[(journal::SOCKET_ENV, path.to_str().unwrap())],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        assert!(
            support::stderr(&out).is_empty(),
            "{}",
            support::stderr(&out)
        );
        socket.set_nonblocking(true).unwrap();
        let mut buf = vec![0u8; 65536];
        let n = socket.recv(&mut buf).unwrap();
        let fields = journal::parse(&buf[..n]).unwrap();
        assert_eq!(field(&fields, "CUPERTINO_DIAG_ID"), b"test.journal.sent");
        assert_eq!(
            field(&fields, "SYSLOG_IDENTIFIER"),
            b"cupertino-diagnostic-events"
        );
        let record =
            Record::parse(std::str::from_utf8(field(&fields, "CUPERTINO_DIAG_JSON")).unwrap())
                .unwrap();
        assert_eq!(
            record.fields.get("ssid").unwrap().as_str().unwrap().len(),
            "ssid#".len() + 8
        );
        // nothing went to a store
        assert!(!state.path().join("cupertino").exists());
        return;
    }
    diag::init(diag::component!(), "test", diag::Sink::Journal);
    diag::event(Severity::Warn, "test.journal.sent", "sent")
        .field("ssid", Value::Address(diag::AddressKind::Ssid, "Home"))
        .emit();
}

#[test]
fn without_a_journal_an_event_is_one_line_on_standard_error() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let missing = state.path().join("no-such-socket");
        let out = support::child(
            "without_a_journal_an_event_is_one_line_on_standard_error",
            state.path(),
            &[(journal::SOCKET_ENV, missing.to_str().unwrap())],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        let stderr = support::stderr(&out);
        let lines: Vec<&str> = stderr
            .lines()
            .filter(|l| l.contains("diagnostic event"))
            .collect();
        assert_eq!(lines.len(), 1, "{stderr}");
        let json = lines[0]
            .strip_prefix("cupertino-diagnostic-events: diagnostic event: ")
            .unwrap();
        assert_eq!(Record::parse(json).unwrap().id, "test.journal.missing");
        return;
    }
    diag::init(diag::component!(), "test", diag::Sink::Journal);
    diag::event(Severity::Error, "test.journal.missing", "no journal").emit();
}
