//! The emitter of a process: what it adds to every event, the exit-error event, the summaries of the rate limit at the
//! end of a window, and the lines a fatal signal handler writes.

#[allow(dead_code)]
mod support;

use std::io::Write;
use std::time::{Duration, Instant, SystemTime};

use cupertino_diagnostic_events::{self as diag, store, Record, Severity, Value};

fn state() -> tempfile::TempDir {
    let state = tempfile::tempdir().unwrap();
    support::isolate(state.path());
    state
}

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
    let state = state();
    std::env::set_var("XDG_SESSION_ID", "c7");
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::event(Severity::Info, "test.emitter.context", "context").emit();
    diag::set_context("backend", Value::Public("winit"));
    diag::set_context("renderer", Value::Public("software"));
    diag::set_context("backend", Value::Public("udev"));
    diag::event(Severity::Info, "test.emitter.context", "context")
        .field("renderer", Value::Public("gles"))
        .emit();
    let records = stored(state.path());
    assert_eq!(records.len(), 2);
    let first = &records[0];
    assert_eq!(first.component, "cupertino-diagnostic-events");
    assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(first.commit.as_deref(), diag::build_info::COMMIT);
    assert_eq!(first.session.as_deref(), Some("c7"));
    assert_eq!(first.boot_id, diag::boot_id());
    assert_eq!(first.pid, std::process::id());
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
}

#[test]
fn an_exit_with_an_error_is_recorded_with_the_scrubbed_error() {
    let state = state();
    let emitter = diag::init(diag::component!(), "dock", diag::Sink::UserStore);
    assert_eq!(emitter.exit_error_id(), "dock.exit_error");
    assert_eq!(emitter.panic_id(), "dock.panic");
    diag::exit_error(&"cannot open /home/someoneelse/plan.odt: refused");
    let records = stored(state.path());
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.id, "dock.exit_error");
    assert_eq!(record.severity, Severity::Error);
    assert_eq!(record.message, "the process ended with an error");
    let error = record.fields.get("error").unwrap().as_str().unwrap();
    assert!(error.starts_with("cannot open /home/…#"), "{error}");
    assert!(
        error.ends_with(": refused") && !error.contains("plan"),
        "{error}"
    );
    assert_eq!(record.redacted, ["error"]);
    assert!(
        record
            .source
            .starts_with("cupertino-diagnostic-events:emitter.rs:"),
        "{}",
        record.source
    );
}

#[test]
fn the_summary_of_a_window_is_written_when_it_closes() {
    let _state = state();
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
fn a_fatal_signal_writes_a_line_made_ready_before() {
    let state = state();
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
    assert_eq!(fatal.render(2, &mut buf), 0, "no value at that index");
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
    drop(file);

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
}

#[test]
fn a_process_that_records_nothing_leaves_no_store_behind() {
    let state = state();
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::set_context("backend", Value::Public("udev"));
    diag::flush();
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn an_explicit_store_keeps_the_events_in_its_own_directory() {
    let state = state();
    let dir = state.path().join("apart");
    diag::init(diag::component!(), "test", diag::Sink::Store(dir.clone()));
    diag::event(Severity::Info, "test.emitter.apart", "apart").emit();
    let records = store::read(&dir).unwrap().records;
    assert_eq!(records.len(), 1);
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn the_memory_sink_keeps_events_in_the_ring_and_nowhere_else() {
    let state = state();
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Memory);
    diag::event(Severity::Info, "test.emitter.memory", "in memory").emit();
    assert_eq!(emitter.recent().len(), 1);
    assert_eq!(diag::recent(), emitter.recent());
    assert!(!state.path().join("cupertino").exists());
}
