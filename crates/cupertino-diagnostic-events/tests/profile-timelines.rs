//! `profile`: the sinks `CUPERTINO_PROFILE` names and the trace file the Chrome layer writes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::profile::{self, Sink, ENV, FLUSH_DELAY};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

/// The events of a trace file `finish` closed.
fn events(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).expect("a closed array of events");
    value.as_array().unwrap().clone()
}

/// The complete events whose names start with `name`.
fn complete<'a>(events: &'a [serde_json::Value], name: &str) -> Vec<&'a serde_json::Value> {
    events
        .iter()
        .filter(|e| e["ph"] == "X" && e["name"].as_str().is_some_and(|n| n.starts_with(name)))
        .collect()
}

/// Where a complete event starts and ends, in microseconds.
fn extent(event: &serde_json::Value) -> (f64, f64) {
    let start = event["ts"].as_f64().unwrap();
    (start, start + event["dur"].as_f64().unwrap())
}

#[test]
fn a_profile_value_names_its_sinks_or_says_what_is_wrong() {
    let existing = tempfile::tempdir().unwrap();
    let existing_value = format!("chrome:{}", existing.path().display());
    let ok: [(&str, Vec<Sink>); 6] = [
        (
            "chrome:/tmp/trace.json",
            vec![Sink::Chrome("/tmp/trace.json".into())],
        ),
        // a directory gets a file for the program and its pid; an existing one needs no slash
        (
            "chrome:/tmp/traces/",
            vec![Sink::Chrome("/tmp/traces/dock-42.json".into())],
        ),
        (
            &existing_value,
            vec![Sink::Chrome(existing.path().join("dock-42.json"))],
        ),
        (
            "chrome:/t/, tracy ,",
            vec![Sink::Chrome("/t/dock-42.json".into()), Sink::Tracy],
        ),
        ("tracy", vec![Sink::Tracy]),
        ("", vec![]),
    ];
    for (value, sinks) in ok {
        assert_eq!(profile::parse(value, "dock", 42), Ok(sinks), "{value:?}");
    }
    for (value, message) in [
        ("flamegraph", "is not chrome:FILE, chrome:DIR/ or tracy"),
        ("chrome:", "needs a file"),
        ("chrome:/a.json,chrome:/b.json", "one chrome: sink at most"),
    ] {
        let err = profile::parse(value, "dock", 42).unwrap_err();
        assert!(err.contains(message), "{value}: {err}");
    }
    // the program's own name is safe in a file name
    let program = profile::program();
    assert!(!program.is_empty());
    assert!(program
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')));
}

#[test]
fn the_variable_gives_a_layer_only_when_it_names_a_sink_this_build_records() {
    std::env::remove_var(ENV);
    assert!(profile::layer::<Registry>().is_none(), "unset");
    profile::init();
    assert!(!tracing::dispatcher::has_been_set(), "unset: no subscriber");
    for value in ["", "nonsense"] {
        std::env::set_var(ENV, value);
        assert!(profile::layer::<Registry>().is_none(), "{value:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    std::env::set_var(ENV, format!("chrome:{}", path.display()));
    let layer = profile::layer::<Registry>().expect("a layer");
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        let _span = tracing::trace_span!(target: "cupertino_test", "frame").entered();
    });
    profile::finish();
    assert_eq!(complete(&events(&path), "frame").len(), 1);
}

#[test]
fn a_sink_that_cannot_record_gives_no_layer() {
    let dir = tempfile::tempdir().unwrap();
    let file: PathBuf = dir.path().join("a-file");
    std::fs::write(&file, "").unwrap();
    for sink in [
        // a directory of the path is a file
        Sink::Chrome(file.join("trace.json")),
        // this build is never made with `--cfg cupertino_tracy`
        Sink::Tracy,
    ] {
        assert!(
            profile::layer_of::<Registry>(std::slice::from_ref(&sink), "p").is_none(),
            "{sink:?}"
        );
    }
}

#[test]
fn a_trace_holds_the_workspaces_spans_and_intervals_as_complete_events_on_named_tracks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep/trace.json");
    let layer =
        profile::layer_of::<Registry>(&[Sink::Chrome(path.clone())], "test-program").unwrap();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        let paint = tracing::trace_span!(target: "cupertino_test", "paint", width = 640_u64, name = "main", dark = true);
        let _paint = paint.enter();
        std::thread::sleep(Duration::from_millis(5));
        {
            let _inner = tracing::trace_span!(target: "cupertino_test", "inner").entered();
            std::thread::sleep(Duration::from_millis(2));
        }
        let _label =
            tracing::trace_span!(target: "cupertino_test", "ui.message", label = "app::Message")
                .entered();
        let _other = tracing::trace_span!(target: "other_crate", "theirs").entered();
        tracing::trace!(target: "cupertino_test", "an event, not a span");
        let start = profile::now_ns();
        profile::interval("late start", start, start + 3_000_000);
    });
    profile::finish();
    let events = events(&path);
    let pid = u64::from(std::process::id());

    // the process by the program's name; the thread's track and the track of its waits
    let process = events.iter().find(|e| e["name"] == "process_name").unwrap();
    assert_eq!(
        (&process["ph"], &process["pid"], &process["args"]["name"]),
        (&"M".into(), &pid.into(), &"test-program".into())
    );
    let threads: Vec<&str> = events
        .iter()
        .filter(|e| e["name"] == "thread_name")
        .map(|e| e["args"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(threads.len(), 2, "{threads:?}");
    assert!(threads.iter().any(|t| t.ends_with(" waits")), "{threads:?}");

    // a span: one complete event, its target the category, its fields the arguments, in microseconds
    let paint = complete(&events, "paint");
    assert_eq!(paint.len(), 1);
    let paint = paint[0];
    assert_eq!(
        (&paint["cat"], &paint["pid"]),
        (&"cupertino_test".into(), &pid.into())
    );
    assert!(paint["dur"].as_f64().unwrap() >= 4000.0, "{}", paint["dur"]);
    assert_eq!(
        (
            &paint["args"]["width"],
            &paint["args"]["name"],
            &paint["args"]["dark"]
        ),
        (&640.into(), &"main".into(), &true.into())
    );
    // a nested span inside its parent on the same thread
    let inner = complete(&events, "inner")[0];
    assert_eq!(inner["tid"], paint["tid"]);
    let ((p_start, p_end), (i_start, i_end)) = (extent(paint), extent(inner));
    assert!(p_start <= i_start && i_end <= p_end + 1.0);
    // a `label` field goes into the name
    assert_eq!(complete(&events, "ui.message app::Message").len(), 1);
    // other crates' spans and plain events are not recorded
    assert!(complete(&events, "theirs").is_empty());
    assert!(events.iter().all(|e| e["name"] != "an event, not a span"));
    // an interval is a complete event on the waits track
    let wait = complete(&events, "late start");
    assert_eq!(wait.len(), 1);
    assert!((wait[0]["dur"].as_f64().unwrap() - 3000.0).abs() < 1.0);
    assert_ne!(wait[0]["tid"], paint["tid"]);
}

#[test]
fn an_unfinished_trace_is_readable_up_to_its_last_event_and_finish_closes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    let layer = profile::layer_of::<Registry>(&[Sink::Chrome(path.clone())], "p").unwrap();
    let writer = std::thread::spawn(profile::write_traces);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        let _s = tracing::trace_span!(target: "cupertino_test", "frame").entered();
    });
    // the writer writes within FLUSH_DELAY
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(&path).unwrap();
        if text.contains("\"frame\"") {
            assert!(
                text.starts_with("[\n") && !text.trim_end().ends_with(']'),
                "{text}"
            );
            break;
        }
        assert!(Instant::now() < deadline, "the span reached the file");
        std::thread::sleep(FLUSH_DELAY / 4);
    }
    profile::finish();
    writer.join().unwrap();
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .trim_end()
        .ends_with(']'));
}
