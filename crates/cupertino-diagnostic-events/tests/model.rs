//! The event model: the record's fields and their order, every value type, the rate limit, the size limit, and that a
//! message is never formatted.

mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::rate::RateLimiter;
use cupertino_diagnostic_events::redact::{Policy, Salt};
use cupertino_diagnostic_events::{
    self as diag, AddressKind, Identity, Origin, Record, Severity, Value, EVENT_BYTES, SCHEMA,
};

fn policy() -> Policy {
    Policy::new(
        Some(Path::new("/home/alex")),
        Some("alex"),
        Some("studio"),
        Salt::from_bytes([7; 16]),
    )
}

fn origin() -> Origin {
    Origin {
        time: "2026-11-09T14:03:27.512Z".into(),
        boot_id: Some("0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0".into()),
        session: Some("3".into()),
        component: "cupertino-server".into(),
        version: "0.1.0".into(),
        commit: Some("0123456789abcdef0123456789abcdef01234567".into()),
        pid: 4242,
    }
}

/// An event with one field of each type.
fn every_type() -> Record {
    let mut record = diag::event(Severity::Warn, "server.gpu.reset", "the GPU was reset")
        .field("reason", Value::Public("another program caused it"))
        .field("remade", Value::Bool(true))
        .field("resets", Value::Count(2))
        .field("status", Value::Int(-11))
        .field("took", Value::Duration(Duration::from_millis(1500)))
        .field(
            "log",
            Value::Path(Path::new("/home/alex/.local/state/cupertino/server.log")),
        )
        .field(
            "file",
            Value::Path(Path::new("/home/alex/Documents/taxes.pdf")),
        )
        .field("device", Value::Path(Path::new("/dev/dri/card1")))
        .field("title", Value::UserText("Bank statement"))
        .field("password", Value::Secret("hunter2"))
        .field("peer", Value::Address(AddressKind::Ip, "192.168.1.20"))
        .field("user", Value::Identifier(Identity::User, "alex"))
        .to_record(&policy(), &origin(), 1);
    record.source = "cupertino-server:gpu_reset.rs:120".into();
    record
}

#[test]
fn a_record_keeps_its_fields_in_the_v1_order_as_the_golden_line() {
    let line = every_type().to_json() + "\n";
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/event-v1.jsonl");
    if std::env::var_os("DIAG_BLESS").is_some() {
        std::fs::write(&golden, &line).unwrap();
    }
    assert_eq!(line, std::fs::read_to_string(golden).unwrap());
    let value: serde_json::Value = serde_json::from_str(&line).unwrap();
    let keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    // the order is the line's, not the parser's
    let order: Vec<&str> = line
        .split("\":")
        .filter_map(|part| part.rsplit(['{', ',']).next())
        .map(|k| k.trim_start_matches('"'))
        .filter(|k| sorted.binary_search(k).is_ok())
        .take(17)
        .collect();
    assert_eq!(
        &order[..12],
        [
            "schema",
            "time",
            "boot_id",
            "session",
            "component",
            "version",
            "commit",
            "pid",
            "id",
            "severity",
            "message",
            "fields"
        ]
    );
    assert_eq!(Record::parse(&line).unwrap(), every_type());
}

#[test]
fn every_value_type_serialises_as_its_class_says() {
    let record = every_type();
    let field = |k: &str| record.fields.get(k).unwrap().clone();
    assert_eq!(field("reason"), "another program caused it");
    assert_eq!(field("remade"), true);
    assert_eq!(field("resets"), 2);
    assert_eq!(field("status"), -11);
    assert_eq!(field("took"), 1500);
    assert_eq!(field("log"), "~/.local/state/cupertino/server.log");
    let file = field("file");
    let file = file.as_str().unwrap();
    assert!(
        file.starts_with("~/…#") && file.len() == "~/…#".len() + 8,
        "{file}"
    );
    assert_eq!(field("device"), "/dev/dri/card1");
    assert_eq!(field("title"), "[redacted]");
    assert_eq!(field("password"), "[redacted]");
    let peer = field("peer");
    let peer = peer.as_str().unwrap();
    assert!(peer.starts_with("ip#") && peer.len() == 11, "{peer}");
    assert_eq!(field("user"), "[user]");
    assert_eq!(
        record.redacted,
        ["log", "file", "title", "password", "peer", "user"]
    );
    assert_eq!(record.schema, SCHEMA);
}

#[test]
fn an_event_says_where_it_comes_from_without_directories() {
    let before = line!();
    let record =
        diag::event(Severity::Info, "test.source.here", "here").to_record(&policy(), &origin(), 1);
    let after = line!();
    let line: u32 = record
        .source
        .strip_prefix("cupertino-diagnostic-events:model.rs:")
        .unwrap()
        .parse()
        .unwrap();
    assert!(before < line && line < after, "{}", record.source);
}

#[test]
fn a_message_is_written_as_the_code_gives_it_and_never_formatted() {
    let record = diag::event(
        Severity::Info,
        "test.message.fixed",
        "count {count} of {} at /home/alex/x",
    )
    .field("count", Value::Count(3))
    .to_record(&policy(), &origin(), 1);
    assert_eq!(record.message, "count {count} of {} at /home/alex/x");
    assert_eq!(record.fields.get("count").unwrap(), 3);
}

#[test]
fn eleven_events_in_a_window_give_ten_and_one_summary_of_one_when_it_closes() {
    let start = Instant::now();
    let mut rate = RateLimiter::default();
    let mut written = Vec::new();
    for n in 1..=11u64 {
        let record = diag::event(Severity::Warn, "test.rate.burst", "burst")
            .field("n", Value::Count(n))
            .to_record(&policy(), &origin(), n);
        written.extend(rate.offer(record, start + Duration::from_millis(n * 100)));
    }
    assert_eq!(written.len(), 10);
    assert!(written.iter().all(|r| r.suppressed == 0));
    assert_eq!(
        rate.next_due(),
        Some(start + Duration::from_millis(100) + diag::RATE_WINDOW)
    );
    assert!(rate.due(start + Duration::from_secs(5)).is_empty());
    let summary = rate.due(start + Duration::from_millis(100) + diag::RATE_WINDOW);
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].suppressed, 1);
    assert_eq!(summary[0].count, 11);
    assert_eq!(summary[0].fields.get("n").unwrap(), 11);
    assert!(rate.due(start + Duration::from_secs(60)).is_empty());
    // the next window starts with room again
    let record =
        diag::event(Severity::Warn, "test.rate.burst", "burst").to_record(&policy(), &origin(), 12);
    assert_eq!(rate.offer(record, start + Duration::from_secs(30)).len(), 1);
    // another id has its own window, and a summary held at exit is drained
    for n in 0..12 {
        let record = diag::event(Severity::Info, "test.rate.other", "other").to_record(
            &policy(),
            &origin(),
            n,
        );
        rate.offer(record, start + Duration::from_secs(30));
    }
    let drained = rate.drain();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].suppressed, 2);
}

#[test]
fn a_summary_is_written_first_when_the_next_event_opens_a_new_window() {
    let start = Instant::now();
    let mut rate = RateLimiter::new(1, Duration::from_secs(10));
    let make = |n| {
        diag::event(Severity::Info, "test.rate.next", "next").to_record(&policy(), &origin(), n)
    };
    assert_eq!(rate.offer(make(1), start).len(), 1);
    assert!(rate
        .offer(make(2), start + Duration::from_secs(1))
        .is_empty());
    let out = rate.offer(make(3), start + Duration::from_secs(11));
    assert_eq!(
        out.iter()
            .map(|r| (r.count, r.suppressed))
            .collect::<Vec<_>>(),
        [(2, 1), (3, 0)]
    );
}

#[test]
fn an_oversize_event_loses_field_text_then_fields_and_says_so() {
    let long = "x".repeat(10_000);
    let mut record = diag::event(Severity::Error, "test.size.big", "big")
        .field("a", Value::Public(&long))
        .to_record(&policy(), &origin(), 1);
    let line = record.fit(EVENT_BYTES);
    assert!(
        line.len() <= EVENT_BYTES && line.ends_with('\n'),
        "{}",
        line.len()
    );
    let read = Record::parse(line.trim_end()).unwrap();
    assert!(read.truncated);
    assert_eq!(
        read.fields
            .get("a")
            .unwrap()
            .as_str()
            .unwrap()
            .chars()
            .count(),
        diag::FIELD_TEXT_LIMIT + 1
    );
    // many fields: the last ones go
    let parts: Vec<String> = (0..40)
        .map(|i| format!("part {i} {}", "y".repeat(250)))
        .collect();
    let mut event = diag::event(Severity::Error, "test.size.many", "many");
    for part in &parts {
        event = event.field("k", Value::Public(part));
    }
    let mut record = event.to_record(&policy(), &origin(), 1);
    let line = record.fit(EVENT_BYTES);
    assert!(line.len() <= EVENT_BYTES);
    let read = Record::parse(line.trim_end()).unwrap();
    assert!(read.truncated && !read.fields.0.is_empty() && read.fields.0.len() < 40);
    assert!(read.fields.0[0].1.as_str().unwrap().starts_with("part 0 "));
    // a small one is untouched
    let mut small =
        diag::event(Severity::Info, "test.size.small", "small").to_record(&policy(), &origin(), 1);
    assert!(
        !Record::parse(small.fit(EVENT_BYTES).trim_end())
            .unwrap()
            .truncated
    );
}

#[test]
fn an_event_before_the_emitter_is_installed_is_dropped_and_counted() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "an_event_before_the_emitter_is_installed_is_dropped_and_counted",
            state.path(),
            &[],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        assert!(!state.path().join("cupertino/events").exists());
        return;
    }
    assert_eq!(diag::dropped_before_install(), 0);
    diag::event(Severity::Info, "test.early.event", "too early").emit();
    diag::exit_error(&"too early");
    assert_eq!(diag::dropped_before_install(), 2);
    assert!(diag::recent().is_empty());
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Memory);
    diag::event(Severity::Info, "test.late.event", "in time").emit();
    assert_eq!(diag::dropped_before_install(), 2);
    assert_eq!(emitter.recent().len(), 1);
    // installing again keeps the first
    let again = diag::Emitter::install(diag::component!(), "other", diag::Sink::UserStore);
    assert!(std::ptr::eq(again.unwrap_err(), emitter));
}

#[test]
fn the_panic_hook_records_the_panic_and_calls_the_hook_before_it() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "the_panic_hook_records_the_panic_and_calls_the_hook_before_it",
            state.path(),
            &[],
        );
        let stderr = support::stderr(&out);
        assert!(!out.status.success());
        assert!(
            stderr.contains("prior hook saw: the bad thing at /home/"),
            "{stderr}"
        );
        let contents = diag::store::read(&state.path().join("cupertino/events")).unwrap();
        assert_eq!(contents.records.len(), 1, "{contents:?}");
        let record = &contents.records[0].record;
        assert_eq!(record.id, "test.panic");
        assert_eq!(record.severity, Severity::Fatal);
        assert_eq!(record.message, "the process panicked");
        let panic = record.fields.get("panic").unwrap().as_str().unwrap();
        assert!(panic.starts_with("the bad thing at /home/…#"), "{panic}");
        assert!(!panic.contains("secretfile"));
        let location = record.fields.get("location").unwrap().as_str().unwrap();
        assert!(location.starts_with("model.rs:"), "{location}");
        assert!(record
            .source
            .starts_with("cupertino-diagnostic-events:model.rs:"));
        assert_eq!(record.redacted, ["panic"]);
        return;
    }
    std::panic::set_hook(Box::new(|info| {
        let payload = info
            .payload()
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        eprintln!("prior hook saw: {payload}");
    }));
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    let user = "someone";
    panic!("the bad thing at /home/{user}/secretfile");
}

#[test]
fn a_test_can_make_a_process_panic_right_after_init() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let name = "a_test_can_make_a_process_panic_right_after_init";
        let out = support::child(name, state.path(), &[(diag::TEST_PANIC_ENV, "test")]);
        assert_eq!(out.status.code(), Some(101), "{}", support::stderr(&out));
        let records = diag::store::read(&state.path().join("cupertino/events"))
            .unwrap()
            .records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].record.id, "test.panic");
        assert!(records[0]
            .record
            .fields
            .get("location")
            .unwrap()
            .as_str()
            .unwrap()
            .starts_with("emitter.rs:"));
        // another area is not asked
        let state = tempfile::tempdir().unwrap();
        let out = support::child(name, state.path(), &[(diag::TEST_PANIC_ENV, "dock")]);
        assert!(out.status.success(), "{}", support::stderr(&out));
        return;
    }
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
}
