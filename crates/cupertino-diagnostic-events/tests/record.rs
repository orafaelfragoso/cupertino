//! A record of `cupertino.diag.v1`: the line it is written as, what the code gives it, reading a line back, and the
//! size limit.

#[allow(dead_code)]
mod support;

use std::path::Path;
use std::time::Duration;

use cupertino_diagnostic_events::{
    self as diag, AddressKind, Identity, Record, Severity, Value, EVENT_BYTES, FIELD_TEXT_LIMIT,
};

/// An event with a field of every value type, as `cupertino-server` would record it.
fn every_value_type() -> Record {
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
        .to_record(&support::policy(), &support::origin(), 1);
    record.source = "cupertino-server:gpu_reset.rs:120".into();
    record
}

#[test]
fn a_record_is_written_as_the_golden_v1_line_and_reads_back_equal() {
    let line = every_value_type().to_json() + "\n";
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/goldens/event-v1.jsonl");
    if std::env::var_os("DIAG_BLESS").is_some() {
        std::fs::write(&golden, &line).unwrap();
    }
    // the golden line fixes the key order, each value type's form and the redacted keys in field order
    assert_eq!(line, std::fs::read_to_string(golden).unwrap());
    assert_eq!(Record::parse(line.trim_end()).unwrap(), every_value_type());
    // a reader of v1 ignores keys a later writer adds
    let later = line
        .trim_end()
        .replacen('{', "{\"added_later\":{\"x\":1},", 1);
    assert_eq!(Record::parse(&later).unwrap(), every_value_type());
}

#[test]
fn a_line_that_is_not_a_v1_record_does_not_read() {
    for line in [
        "",
        "garbage",
        "{\"schema\": 3}",
        "{\"schema\":\"cupertino.diag.v2\"}",
        "{\"schema\":\"cupertino.diag.v1\"}",
    ] {
        assert!(Record::parse(line).is_err(), "{line:?}");
    }
}

#[test]
fn a_record_keeps_the_codes_message_unformatted_and_names_its_source_file_and_line() {
    let before = line!();
    let record = diag::event(
        Severity::Info,
        "test.message.fixed",
        "count {count} of {} at /home/alex/x",
    )
    .field("count", Value::Count(3))
    .to_record(&support::policy(), &support::origin(), 1);
    let after = line!();
    assert_eq!(record.message, "count {count} of {} at /home/alex/x");
    assert_eq!(record.fields.get("count").unwrap(), 3);
    // the package and the file name, no directories
    let line: u32 = record
        .source
        .strip_prefix("cupertino-diagnostic-events:record.rs:")
        .unwrap_or_else(|| panic!("{}", record.source))
        .parse()
        .unwrap();
    assert!(before < line && line < after, "{}", record.source);
}

#[test]
fn an_oversize_record_is_cut_to_the_limit_field_text_first_then_the_last_fields() {
    let long = "x".repeat(10_000);
    let parts: Vec<String> = (0..40)
        .map(|i| format!("part {i} {}", "y".repeat(250)))
        .collect();
    let mut many = diag::event(Severity::Error, "test.size.many", "many");
    for part in &parts {
        many = many.field("k", Value::Public(part));
    }
    let rows = [
        (
            "one long field",
            diag::event(Severity::Error, "test.size.big", "big").field("a", Value::Public(&long)),
        ),
        ("many fields", many),
        (
            "small",
            diag::event(Severity::Info, "test.size.small", "small"),
        ),
    ];
    for (row, event) in rows {
        let mut record = event.to_record(&support::policy(), &support::origin(), 1);
        let line = record.fit(EVENT_BYTES);
        assert!(
            line.len() <= EVENT_BYTES && line.ends_with('\n'),
            "{row}: {}",
            line.len()
        );
        let read = Record::parse(line.trim_end()).unwrap();
        let first = read.fields.0.first().map(|(_, v)| v.as_str().unwrap());
        match row {
            "one long field" => {
                assert!(read.truncated, "{row}");
                // the kept characters and the ellipsis
                assert_eq!(
                    first.unwrap().chars().count(),
                    FIELD_TEXT_LIMIT + 1,
                    "{row}"
                );
            }
            "many fields" => {
                assert!(read.truncated, "{row}");
                assert!((1..40).contains(&read.fields.0.len()), "{row}");
                assert!(first.unwrap().starts_with("part 0 "), "{row}");
            }
            _ => assert!(!read.truncated, "{row}"),
        }
    }
}
