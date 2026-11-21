//! The journald sink: the datagram of the native protocol, sending it to a socket, and a service's events reaching the
//! journal reduced, or standard error when there is none.

use crate::support;

use std::os::unix::net::UnixDatagram;

use cupertino_diagnostic_events::journal::{self, Journal};
use cupertino_diagnostic_events::{self as diag, AddressKind, Record, Severity, Value};

const MESSAGE: &str = "the network backend did not start\nat all";

fn record(severity: Severity) -> Record {
    diag::event(severity, "network.backend.start_failed", MESSAGE)
        .field("backend", Value::Public("iwd"))
        .to_record(&support::policy(), &support::origin(), 1)
}

fn field<'d>(fields: &'d [(String, Vec<u8>)], key: &str) -> &'d [u8] {
    &fields
        .iter()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("no {key}"))
        .1
}

#[test]
fn a_datagram_carries_the_journal_fields_and_the_whole_record() {
    for (severity, priority) in [
        (Severity::Info, "6"),
        (Severity::Warn, "4"),
        (Severity::Error, "3"),
        (Severity::Fatal, "2"),
    ] {
        let record = record(severity);
        let datagram = journal::datagram(&record, &record.to_json(), "cupertino-netd");
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
            field(&fields, "PRIORITY"),
            priority.as_bytes(),
            "{severity:?}"
        );
        assert_eq!(field(&fields, "SYSLOG_IDENTIFIER"), b"cupertino-netd");
        assert_eq!(field(&fields, "CUPERTINO_DIAG_SCHEMA"), b"1");
        assert_eq!(
            field(&fields, "CUPERTINO_DIAG_ID"),
            b"network.backend.start_failed"
        );
        assert_eq!(
            field(&fields, "CUPERTINO_DIAG_SEVERITY"),
            severity.name().as_bytes()
        );
        let json = std::str::from_utf8(field(&fields, "CUPERTINO_DIAG_JSON")).unwrap();
        assert_eq!(Record::parse(json).unwrap(), record);
        // a value with a newline takes the binary-safe form: key, newline, 64-bit little-endian length, value, newline
        let mut message = b"MESSAGE\n".to_vec();
        message.extend_from_slice(&(MESSAGE.len() as u64).to_le_bytes());
        message.extend_from_slice(MESSAGE.as_bytes());
        message.push(b'\n');
        assert!(datagram.starts_with(&message));
        assert_eq!(field(&fields, "MESSAGE"), MESSAGE.as_bytes());
        // one cut short does not parse
        assert!(journal::parse(&datagram[..datagram.len() - 3]).is_none());
    }
}

#[test]
fn a_journal_sends_the_datagram_whole_and_fails_when_its_socket_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("socket");
    let socket = UnixDatagram::bind(&path).unwrap();
    let mut journal = Journal::new(&path);
    let record = record(Severity::Error);
    let datagram = journal::datagram(&record, &record.to_json(), "cupertino-netd");
    journal.send(&datagram).unwrap();
    let mut buf = vec![0u8; 65536];
    let n = socket.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], &datagram[..]);
    drop(socket);
    std::fs::remove_file(&path).unwrap();
    assert!(journal.send(&datagram).is_err());
}

#[test]
fn a_services_events_reach_the_journal_socket_the_environment_names_reduced() {
    let state = tempfile::tempdir().unwrap();
    support::isolate(state.path());
    let path = state.path().join("journal.socket");
    let socket = UnixDatagram::bind(&path).unwrap();
    std::env::set_var(journal::SOCKET_ENV, &path);
    diag::init(diag::component!(), "test", diag::Sink::Journal);
    let secret = "s3cr3t-Pa55w0rd-NEVER";
    diag::event(Severity::Warn, "test.journal.sent", "sent")
        .field("ssid", Value::Address(AddressKind::Ssid, "Home"))
        .field("password", Value::Secret(secret))
        .field("error", Value::Public(&format!("token: {secret}")))
        .emit();
    socket.set_nonblocking(true).unwrap();
    let mut buf = vec![0u8; 65536];
    let n = socket.recv(&mut buf).unwrap();
    let datagram = &buf[..n];
    assert!(!support::contains(datagram, b"Pa55w0rd"));
    let fields = journal::parse(datagram).unwrap();
    assert_eq!(field(&fields, "CUPERTINO_DIAG_ID"), b"test.journal.sent");
    assert_eq!(
        field(&fields, "SYSLOG_IDENTIFIER"),
        b"cupertino-diagnostic-events"
    );
    let record =
        Record::parse(std::str::from_utf8(field(&fields, "CUPERTINO_DIAG_JSON")).unwrap()).unwrap();
    let ssid = record.fields.get("ssid").unwrap().as_str().unwrap();
    assert!(
        ssid.starts_with("ssid#") && ssid.len() == "ssid#".len() + 8,
        "{ssid}"
    );
    // nothing went to a store
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn a_service_without_a_journal_goes_on_and_writes_no_store() {
    let state = tempfile::tempdir().unwrap();
    // the socket the environment names is not there: the emitter falls back to standard error
    support::isolate(state.path());
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Journal);
    diag::event(Severity::Error, "test.journal.missing", "no journal").emit();
    assert_eq!(emitter.recent().len(), 1);
    assert!(!state.path().join("cupertino").exists());
}
