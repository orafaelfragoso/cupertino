//! The emitter of a process: installing it, what it adds to every event, the sinks, the memory ring, the rate limit's
//! summaries, an unavailable store, and what an exit with an error, a panic and a fatal signal leave behind. Each test
//! installs the emitter of its own process.

#[allow(dead_code)]
mod support;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use cupertino_diagnostic_events::store::{self, Unavailable};
use cupertino_diagnostic_events::{self as diag, Record, Severity, Value};

fn isolated() -> tempfile::TempDir {
    let state = tempfile::tempdir().unwrap();
    support::isolate(state.path());
    state
}

/// How long ago a record's time was.
fn age(record: &Record) -> Duration {
    SystemTime::now()
        .duration_since(diag::time::parse(&record.time).unwrap())
        .unwrap()
}

#[test]
fn an_event_before_the_emitter_is_installed_is_dropped_and_counted_and_the_first_install_stays() {
    let state = isolated();
    diag::event(Severity::Info, "test.early.event", "too early").emit();
    diag::exit_error(&"too early");
    assert_eq!(diag::dropped_before_install(), 2);
    assert!(diag::recent().is_empty());
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Memory);
    diag::event(Severity::Info, "test.late.event", "in time").emit();
    assert_eq!(diag::dropped_before_install(), 2);
    assert_eq!(emitter.recent().len(), 1);
    let again = diag::Emitter::install(diag::component!(), "other", diag::Sink::UserStore);
    assert!(std::ptr::eq(again.unwrap_err(), emitter));
    assert!(std::ptr::eq(diag::Emitter::get().unwrap(), emitter));
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn an_event_carries_the_process_its_build_its_session_and_the_context_set_before_it() {
    let state = isolated();
    std::env::set_var("XDG_SESSION_ID", "c7");
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::event(Severity::Info, "test.emitter.context", "context").emit();
    diag::set_context("backend", Value::Public("winit"));
    diag::set_context("renderer", Value::Public("software"));
    diag::set_context("backend", Value::Public("udev"));
    diag::event(Severity::Info, "test.emitter.context", "context")
        .field("renderer", Value::Public("gles"))
        .emit();
    let records = support::stored(state.path());
    assert_eq!(records.len(), 2);
    let first = &records[0];
    assert_eq!(first.component, "cupertino-diagnostic-events");
    assert_eq!(first.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(first.commit.as_deref(), diag::build_info::COMMIT);
    assert_eq!(first.session.as_deref(), Some("c7"));
    assert_eq!(first.boot_id, diag::boot_id());
    assert_eq!(first.pid, std::process::id());
    assert!(age(first) < Duration::from_secs(120));
    assert_eq!(first.fields.get("backend"), None);
    // the count goes on per id; the latest context follows the event's own fields, which win
    let second = &records[1];
    assert_eq!(second.count, 2);
    let fields: Vec<(&str, &serde_json::Value)> = second
        .fields
        .0
        .iter()
        .map(|(k, v)| (k.as_str(), v))
        .collect();
    assert_eq!(
        fields,
        [("renderer", &"gles".into()), ("backend", &"udev".into())]
    );
}

#[test]
fn a_process_that_records_nothing_leaves_no_store_behind() {
    let state = isolated();
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    diag::set_context("backend", Value::Public("udev"));
    diag::flush();
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn a_store_sink_keeps_the_events_in_its_own_directory() {
    let state = isolated();
    let dir = state.path().join("apart");
    diag::init(diag::component!(), "test", diag::Sink::Store(dir.clone()));
    diag::event(Severity::Info, "test.emitter.apart", "apart").emit();
    assert_eq!(store::read(&dir).unwrap().records.len(), 1);
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn the_memory_ring_keeps_the_last_events_within_its_count_and_bytes_and_the_memory_sink_nothing_else(
) {
    let state = isolated();
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Memory);
    // ids of their own, so the rate limit lets every one through
    let ids: Vec<&'static str> = (0..300)
        .map(|i| &*Box::leak(format!("test.ring.n{i}").into_boxed_str()))
        .collect();
    for id in &ids {
        diag::event(Severity::Info, id, "ring").emit();
    }
    let recent = emitter.recent();
    assert_eq!(recent.len(), diag::RING_EVENTS);
    assert!(recent.last().unwrap().contains("\"test.ring.n299\""));
    assert_eq!(diag::recent(), recent);
    let big = "y".repeat(3000);
    for id in &ids[..200] {
        diag::event(Severity::Info, id, "big")
            .field("big", Value::Public(&big))
            .emit();
    }
    let recent = emitter.recent();
    let bytes: usize = recent.iter().map(String::len).sum();
    assert!(
        bytes <= diag::RING_BYTES && recent.len() < diag::RING_EVENTS,
        "{bytes} bytes in {}",
        recent.len()
    );
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn the_summary_of_a_rate_limited_id_is_written_when_its_window_closes() {
    let _state = isolated();
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Memory);
    let sent = diag::RATE_PER_ID as u64 + 3;
    for n in 0..sent {
        diag::event(Severity::Warn, "test.emitter.burst", "burst")
            .field("n", Value::Count(n))
            .emit();
    }
    assert_eq!(emitter.recent().len(), diag::RATE_PER_ID);
    // the writer thread writes it with no later event to carry it
    let until = Instant::now() + diag::RATE_WINDOW + Duration::from_secs(20);
    let summary = loop {
        if let Some(line) = emitter.recent().get(diag::RATE_PER_ID) {
            break line.clone();
        }
        assert!(Instant::now() < until, "no summary");
        std::thread::sleep(Duration::from_millis(50));
    };
    let record = Record::parse(summary.trim_end()).unwrap();
    assert_eq!((record.count, record.suppressed), (sent, 3));
    assert_eq!(record.fields.get("n").unwrap(), sent - 1);
}

#[test]
fn an_unavailable_store_keeps_events_in_memory_and_says_so_once_it_works_again() {
    let state = isolated();
    let dir = state.path().join("cupertino/events");
    std::fs::create_dir_all(&dir).unwrap();
    let set_mode =
        |mode| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode)).unwrap();
    set_mode(0o777);
    let emitter = diag::init(diag::component!(), "test", diag::Sink::UserStore);
    for n in 0..3 {
        diag::event(Severity::Warn, "test.store.before", "while unavailable")
            .field("n", Value::Count(n))
            .emit();
    }
    assert_eq!(emitter.recent().len(), 3);
    assert_eq!(emitter.store_unavailable(), Some(Unavailable::Writable));
    assert!(support::stored(state.path()).is_empty());
    set_mode(0o700);
    diag::event(Severity::Info, "test.store.after", "available again").emit();
    assert_eq!(emitter.store_unavailable(), None);
    assert_eq!(emitter.recent().len(), 5);
    let records = support::stored(state.path());
    let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["test.store.after", diag::STORE_UNAVAILABLE]);
    assert_eq!(
        records[1].fields.get("reason").unwrap(),
        "writable_by_others"
    );
    assert_eq!(records[1].fields.get("kept_in_memory").unwrap(), 3);
}

#[test]
fn an_exit_with_an_error_is_recorded_under_the_area_with_the_error_scrubbed() {
    let state = isolated();
    diag::init(diag::component!(), "dock", diag::Sink::UserStore);
    diag::exit_error(&"cannot open /home/someoneelse/plan.odt: refused");
    let records = support::stored(state.path());
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.id, "dock.exit_error");
    assert_eq!(record.severity, Severity::Error);
    let error = record.fields.get("error").unwrap().as_str().unwrap();
    assert!(
        error.starts_with("cannot open /home/…#")
            && error.ends_with(": refused")
            && !error.contains("plan"),
        "{error}"
    );
    assert_eq!(record.redacted, ["error"]);
}

#[test]
fn a_panic_is_recorded_scrubbed_and_the_hook_set_before_still_runs() {
    let state = isolated();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let prior = seen.clone();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        *prior.lock().unwrap() = payload;
    }));
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    let user = "someone";
    let caught = std::panic::catch_unwind(|| panic!("the bad thing at /home/{user}/secretfile"));
    assert!(caught.is_err());
    assert_eq!(
        *seen.lock().unwrap(),
        "the bad thing at /home/someone/secretfile"
    );
    let records = support::stored(state.path());
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(
        (record.id.as_str(), record.severity),
        ("test.panic", Severity::Fatal)
    );
    let panic = record.fields.get("panic").unwrap().as_str().unwrap();
    assert!(
        panic.starts_with("the bad thing at /home/…#") && !panic.contains("secretfile"),
        "{panic}"
    );
    let location = record.fields.get("location").unwrap().as_str().unwrap();
    assert!(location.starts_with("emitter.rs:"), "{location}");
    assert_eq!(record.redacted, ["panic"]);
}

#[test]
fn a_thread_in_a_scope_records_its_panic_exit_and_context_as_the_scope_and_others_as_the_process() {
    let state = isolated();
    std::panic::set_hook(Box::new(|_| {}));
    diag::init(diag::component!(), "shell", diag::Sink::UserStore);
    diag::set_context("backend", Value::Public("udev"));
    diag::set_context("output", Value::Public("DP-1"));
    let dock = diag::Scope::new("dock");
    dock.set_context("output", Value::Public("HDMI-A-1"));
    std::thread::scope(|threads| {
        threads.spawn(|| {
            let _in_dock = dock.enter();
            assert_eq!(diag::Scope::current().map(|s| s.area()), Some("dock"));
            diag::event(Severity::Info, "dock.icon.drawn", "drawn").emit();
            diag::exit_error(&"gone");
            let caught = std::panic::catch_unwind(|| panic!("in the dock"));
            assert!(caught.is_err());
        });
    });
    assert!(diag::Scope::current().is_none());
    diag::event(Severity::Info, "shell.started", "started").emit();
    let outer = dock.enter();
    {
        let _inner = diag::Scope::new("menubar").enter();
        diag::exit_error(&"nested");
    }
    diag::exit_error(&"back in the dock");
    drop(outer);
    diag::exit_error(&"the process");
    let records: Vec<(String, Option<String>, Option<String>)> = support::stored(state.path())
        .into_iter()
        .map(|r| {
            let field = |k: &str| r.fields.get(k).and_then(|v| v.as_str()).map(str::to_owned);
            (r.id.clone(), field("output"), field("backend"))
        })
        .collect();
    let row = |id: &str, output: &str| {
        (
            id.to_owned(),
            Some(output.to_owned()),
            Some("udev".to_owned()),
        )
    };
    assert_eq!(
        records,
        [
            row("dock.icon.drawn", "HDMI-A-1"),
            row("dock.exit_error", "HDMI-A-1"),
            row("dock.panic", "HDMI-A-1"),
            row("shell.started", "DP-1"),
            row("menubar.exit_error", "DP-1"),
            row("dock.exit_error", "HDMI-A-1"),
            row("shell.exit_error", "DP-1"),
        ]
    );
}

#[test]
fn the_test_variable_makes_the_process_of_its_area_panic_right_after_init() {
    let state = isolated();
    std::panic::set_hook(Box::new(|_| {}));
    std::env::set_var(diag::TEST_PANIC_ENV, "test");
    let caught = std::panic::catch_unwind(|| {
        diag::init(diag::component!(), "test", diag::Sink::UserStore);
    });
    assert!(caught.is_err());
    let ids: Vec<String> = support::stored(state.path())
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, ["test.panic"]);
}

#[test]
fn the_test_variable_leaves_a_process_of_another_area_alone() {
    let state = isolated();
    std::env::set_var(diag::TEST_PANIC_ENV, "dock");
    diag::init(diag::component!(), "test", diag::Sink::UserStore);
    assert!(!state.path().join("cupertino").exists());
}

#[test]
fn a_fatal_signal_writes_a_line_made_ready_before_into_a_fatal_segment() {
    let state = isolated();
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
    // what the signal handler does with `open`, `write` and `close`
    let path = Path::new(std::str::from_utf8(fatal.path().to_bytes()).unwrap());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .unwrap();
    file.write_all(&buf[..len]).unwrap();
    drop(file);
    let segments = store::segments(&state.path().join("cupertino/events")).unwrap();
    let names: Vec<String> = segments
        .iter()
        .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.len() == 1 && names[0].starts_with("fatal-") && names[0].ends_with(".jsonl"),
        "{names:?}"
    );
    let records = support::stored(state.path());
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(
        (record.id.as_str(), record.severity),
        ("test.fatal_signal", Severity::Fatal)
    );
    assert_eq!(record.fields.get("signal").unwrap(), "SIGSEGV");
    assert_eq!(record.fields.get("backend").unwrap(), "udev");
    assert!(age(record) < Duration::from_secs(120), "{}", record.time);
}
