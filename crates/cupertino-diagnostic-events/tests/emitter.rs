//! The emitter of a process: what it adds to every event, the exit-error event, the summaries of the rate limit at the
//! end of a window and at exit, and the lines a fatal signal handler writes.

use crate::support;

use std::io::Write;
use std::time::{Duration, Instant, SystemTime};

use cupertino_diagnostic_events::{self as diag, store, Record, Severity, Value};

fn stored(state: &std::path::Path) -> Vec<Record> {
    store::read(&state.join("cupertino/events"))
        .unwrap()
        .records
        .into_iter()
        .map(|r| r.record)
        .collect()
}

#[test]
fn an_event_carries_the_process_build_and_session() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "an_event_carries_the_process_build_and_session",
            state.path(),
            &[("XDG_SESSION_ID", "c7")],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        let records = stored(state.path());
        assert_eq!(records.len(), 2);
        let first = &records[0];
        assert_eq!(first.component, "cupertino-diagnostic-events");
        assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(first.commit.as_deref(), cupertino_build_info::COMMIT);
        assert_eq!(first.session.as_deref(), Some("c7"));
        assert_eq!(first.boot_id, diag::boot_id());
        assert_ne!(first.pid, std::process::id());
        let age = SystemTime::now()
            .duration_since(diag::time::parse(&first.time).unwrap())
            .unwrap();
        assert!(age < Duration::from_secs(120));
        assert_eq!(first.count, 1);
        // the context the process set after the first event, on the second only; an event's own field wins
        assert_eq!(first.fields.get("backend"), None);
        let second = &records[1];
        assert_eq!(second.fields.get("backend").unwrap(), "udev");
        assert_eq!(second.fields.get("renderer").unwrap(), "gles");
        assert_eq!(second.count, 2);
        let keys: Vec<&str> = second.fields.0.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["renderer", "backend"]);
        return;
    }
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::event(Severity::Info, "test.emitter.context", "context").emit();
    diag::set_context("backend", Value::Public("winit"));
    diag::set_context("renderer", Value::Public("software"));
    diag::set_context("backend", Value::Public("udev"));
    diag::event(Severity::Info, "test.emitter.context", "context")
        .field("renderer", Value::Public("gles"))
        .emit();
}

#[test]
fn an_exit_with_an_error_is_recorded_with_the_scrubbed_error() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "an_exit_with_an_error_is_recorded_with_the_scrubbed_error",
            state.path(),
            &[],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        let records = stored(state.path());
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.id, "dock.exit_error");
        assert_eq!(record.severity, Severity::Error);
        assert_eq!(record.message, "the process ended with an error");
        assert_eq!(
            record.fields.get("error").unwrap(),
            "cannot connect to the display: [user]@[host] refused"
        );
        assert_eq!(record.redacted, ["error"]);
        assert!(
            record
                .source
                .starts_with("cupertino-diagnostic-events:emitter.rs:"),
            "{}",
            record.source
        );
        return;
    }
    let emitter = diag::init(diag::component!(), "dock", diag::Sink::UserStore);
    assert_eq!(emitter.exit_error_id(), "dock.exit_error");
    assert_eq!(emitter.panic_id(), "dock.panic");
    diag::exit_error(&"cannot connect to the display: alice@desk refused");
}

#[test]
fn the_summary_of_a_window_is_written_when_it_closes() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "the_summary_of_a_window_is_written_when_it_closes",
            state.path(),
            &[],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        return;
    }
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Memory);
    for n in 0..13 {
        diag::event(Severity::Warn, "test.emitter.burst", "burst")
            .field("n", Value::Count(n))
            .emit();
    }
    assert_eq!(emitter.recent().len(), 10);
    // the writer thread writes it when the window closes, with no other event to carry it
    let until = Instant::now() + diag::RATE_WINDOW + Duration::from_secs(20);
    let summary = loop {
        let recent = emitter.recent();
        if recent.len() > 10 {
            break recent[10].clone();
        }
        assert!(Instant::now() < until, "no summary");
        std::thread::sleep(Duration::from_millis(50));
    };
    let record = Record::parse(summary.trim_end()).unwrap();
    assert_eq!((record.count, record.suppressed), (13, 3));
    assert_eq!(record.fields.get("n").unwrap(), 12);
}

#[test]
fn the_open_summaries_are_written_when_the_process_exits() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "the_open_summaries_are_written_when_the_process_exits",
            state.path(),
            &[],
        );
        assert_eq!(out.status.code(), Some(3), "{}", support::stderr(&out));
        let records = stored(state.path());
        assert_eq!(records.len(), 11);
        assert_eq!((records[10].count, records[10].suppressed), (12, 2));
        return;
    }
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    for _ in 0..12 {
        diag::event(Severity::Warn, "test.emitter.exit", "at exit").emit();
    }
    let _ = std::io::stdout().flush();
    std::process::exit(3);
}

#[test]
fn a_fatal_signal_writes_a_line_made_ready_before() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "a_fatal_signal_writes_a_line_made_ready_before",
            state.path(),
            &[],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        let dir = state.path().join("cupertino/events");
        let segments = store::segments(&dir).unwrap();
        assert_eq!(segments.len(), 1);
        let name = segments[0]
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("fatal-") && name.ends_with(".jsonl"),
            "{name}"
        );
        let records = stored(state.path());
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.id, "test.fatal_signal");
        assert_eq!(record.severity, Severity::Fatal);
        assert_eq!(record.fields.get("signal").unwrap(), "SIGSEGV");
        assert_eq!(record.fields.get("backend").unwrap(), "udev");
        let age = SystemTime::now()
            .duration_since(diag::time::parse(&record.time).unwrap())
            .unwrap();
        assert!(age < Duration::from_secs(120), "{}", record.time);
        return;
    }
    let emitter = diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::set_context("backend", Value::Public("udev"));
    let fatal = emitter
        .prepare_fatal(
            "test.fatal_signal",
            "a fatal signal ended the process",
            "signal",
            &["SIGABRT", "SIGSEGV"],
        )
        .unwrap();
    let mut buf = [0u8; diag::FATAL_LINE];
    assert_eq!(fatal.render(2, &mut buf), 0);
    let len = fatal.render(1, &mut buf);
    // what the handler does with `open`, `write` and `close`
    let path = std::path::Path::new(std::ffi::OsStr::new(
        std::str::from_utf8(fatal.path().to_bytes()).unwrap(),
    ));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap();
    file.write_all(&buf[..len]).unwrap();
    // without a store directory there is nothing to make ready
}

#[test]
fn a_process_that_records_nothing_leaves_no_store_behind() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "a_process_that_records_nothing_leaves_no_store_behind",
            state.path(),
            &[],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        assert!(!state.path().join("cupertino").exists());
        return;
    }
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::set_context("backend", Value::Public("udev"));
    diag::flush();
}
