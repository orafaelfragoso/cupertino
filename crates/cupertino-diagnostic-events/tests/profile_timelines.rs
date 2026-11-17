//! `profile`: the sinks `CUPERTINO_PROFILE` names and the trace file the Chrome layer writes.

use std::path::{Path, PathBuf};

use cupertino_diagnostic_events::profile::{self, Sink, ENV, FLUSH_DELAY, INTERVAL_TARGET};
use tracing_subscriber::layer::SubscriberExt;

/// The events of a trace file, which `finish` closed.
fn events(path: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).expect("a closed array of events");
    value.as_array().unwrap().clone()
}

/// Run `body` with the Chrome layer writing to `path`, then close the trace.
fn record(path: &Path, body: impl FnOnce()) {
    let layer = profile::layer_of::<tracing_subscriber::Registry>(
        &[Sink::Chrome(path.to_path_buf())],
        "test-program",
    )
    .expect("a layer");
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), body);
    profile::finish();
}

fn complete<'a>(events: &'a [serde_json::Value], name_start: &str) -> Vec<&'a serde_json::Value> {
    events
        .iter()
        .filter(|e| {
            e["ph"] == "X"
                && e["name"]
                    .as_str()
                    .is_some_and(|n| n.starts_with(name_start))
        })
        .collect()
}

#[test]
fn the_variable_and_its_constants() {
    assert_eq!(ENV, "CUPERTINO_PROFILE");
    assert_eq!(FLUSH_DELAY, std::time::Duration::from_millis(100));
    assert_eq!(
        INTERVAL_TARGET,
        "cupertino_diagnostic_events::profile::interval"
    );
}

#[test]
fn a_file_value_names_the_trace_file() {
    assert_eq!(
        profile::parse("chrome:/tmp/trace.json", "dock", 7),
        Ok(vec![Sink::Chrome("/tmp/trace.json".into())])
    );
}

#[test]
fn a_directory_value_gets_a_file_for_the_program_and_its_pid() {
    assert_eq!(
        profile::parse("chrome:/tmp/traces/", "dock", 42),
        Ok(vec![Sink::Chrome("/tmp/traces/dock-42.json".into())])
    );
    let dir = tempfile::tempdir().unwrap();
    let value = format!("chrome:{}", dir.path().display());
    assert_eq!(
        profile::parse(&value, "dock", 42),
        Ok(vec![Sink::Chrome(dir.path().join("dock-42.json"))]),
        "an existing directory needs no slash"
    );
}

#[test]
fn several_sinks_are_comma_separated_and_spaces_are_ignored() {
    assert_eq!(
        profile::parse("chrome:/t/, tracy ,", "p", 1),
        Ok(vec![Sink::Chrome("/t/p-1.json".into()), Sink::Tracy])
    );
    assert_eq!(profile::parse("tracy", "p", 1), Ok(vec![Sink::Tracy]));
    assert_eq!(profile::parse("", "p", 1), Ok(vec![]));
}

#[test]
fn a_value_that_names_no_sink_is_an_error() {
    for (value, message) in [
        ("flamegraph", "is not chrome:FILE, chrome:DIR/ or tracy"),
        ("chrome:", "needs a file"),
        ("chrome:/a.json,chrome:/b.json", "one chrome: sink at most"),
    ] {
        let err = profile::parse(value, "p", 1).unwrap_err();
        assert!(err.contains(message), "{value}: {err}");
    }
}

#[test]
fn the_program_name_is_the_first_arguments_file_name_made_safe() {
    let name = profile::program();
    assert!(!name.is_empty());
    assert!(name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')));
}

#[test]
fn unset_there_is_no_layer_and_no_subscriber() {
    std::env::remove_var(ENV);
    assert!(profile::layer::<tracing_subscriber::Registry>().is_none());
    profile::init();
    assert!(!tracing::dispatcher::has_been_set());
    std::env::set_var(ENV, "");
    assert!(profile::layer::<tracing_subscriber::Registry>().is_none());
}

#[test]
fn a_value_that_names_nothing_this_build_can_record_gives_no_layer() {
    std::env::set_var(ENV, "nonsense");
    assert!(profile::layer::<tracing_subscriber::Registry>().is_none());
}

#[test]
fn the_variable_makes_a_layer_that_writes_the_named_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    std::env::set_var(ENV, format!("chrome:{}", path.display()));
    let layer = profile::layer::<tracing_subscriber::Registry>().expect("a layer");
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        let _span = tracing::trace_span!(target: "cupertino_test", "frame").entered();
    });
    profile::finish();
    assert_eq!(complete(&events(&path), "frame").len(), 1);
}

#[test]
fn a_trace_file_names_the_process_and_its_threads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep/trace.json");
    record(&path, || {
        let _span = tracing::trace_span!(target: "cupertino_test", "frame").entered();
    });
    let events = events(&path);
    let pid = u64::from(std::process::id());
    let process = events.iter().find(|e| e["name"] == "process_name").unwrap();
    assert_eq!(process["ph"], "M");
    assert_eq!(process["pid"], pid);
    assert_eq!(process["args"]["name"], "test-program");
    let threads: Vec<_> = events
        .iter()
        .filter(|e| e["name"] == "thread_name")
        .collect();
    assert_eq!(
        threads.len(),
        2,
        "the thread's track and the track of its waits"
    );
    assert!(threads
        .iter()
        .any(|t| t["args"]["name"].as_str().unwrap().ends_with(" waits")));
}

#[test]
fn a_span_is_one_complete_event_with_its_fields_as_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    record(&path, || {
        let span = tracing::trace_span!(target: "cupertino_test", "paint", width = 640_u64, name = "main", dark = true);
        let _entered = span.enter();
        std::thread::sleep(std::time::Duration::from_millis(5));
    });
    let events = events(&path);
    let spans = complete(&events, "paint");
    assert_eq!(spans.len(), 1);
    let e = spans[0];
    assert_eq!(e["cat"], "cupertino_test");
    assert_eq!(e["pid"], u64::from(std::process::id()));
    assert!(e["ts"].as_f64().unwrap() >= 0.0);
    assert!(
        e["dur"].as_f64().unwrap() >= 4000.0,
        "microseconds: {}",
        e["dur"]
    );
    assert_eq!(e["args"]["width"], 640);
    assert_eq!(e["args"]["name"], "main");
    assert_eq!(e["args"]["dark"], true);
}

#[test]
fn a_field_named_label_goes_into_the_events_name() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    record(&path, || {
        let _s =
            tracing::trace_span!(target: "cupertino_test", "ui.message", label = "app::Message")
                .entered();
    });
    let events = events(&path);
    assert_eq!(complete(&events, "ui.message app::Message").len(), 1);
}

#[test]
fn nested_spans_are_events_of_their_own_inside_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    record(&path, || {
        let _outer = tracing::trace_span!(target: "cupertino_test", "outer").entered();
        std::thread::sleep(std::time::Duration::from_millis(2));
        {
            let _inner = tracing::trace_span!(target: "cupertino_test", "inner").entered();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });
    let events = events(&path);
    let outer = complete(&events, "outer")[0];
    let inner = complete(&events, "inner")[0];
    assert_eq!(outer["tid"], inner["tid"]);
    let (o_start, o_end) = (
        outer["ts"].as_f64().unwrap(),
        outer["ts"].as_f64().unwrap() + outer["dur"].as_f64().unwrap(),
    );
    let (i_start, i_end) = (
        inner["ts"].as_f64().unwrap(),
        inner["ts"].as_f64().unwrap() + inner["dur"].as_f64().unwrap(),
    );
    assert!(o_start <= i_start && i_end <= o_end + 1.0);
}

#[test]
fn only_the_workspaces_spans_are_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    record(&path, || {
        let _own = tracing::trace_span!(target: "cupertino_test", "ours").entered();
        let _other = tracing::trace_span!(target: "other_crate", "theirs").entered();
        tracing::trace!(target: "cupertino_test", "an event, not a span");
    });
    let events = events(&path);
    assert_eq!(complete(&events, "ours").len(), 1);
    assert!(complete(&events, "theirs").is_empty());
    assert!(events.iter().all(|e| e["name"] != "an event, not a span"));
}

#[test]
fn an_interval_is_a_complete_event_on_a_track_beside_the_threads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    record(&path, || {
        let _s = tracing::trace_span!(target: "cupertino_test", "frame").entered();
        let start = profile::now_ns();
        profile::interval("late start", start, start + 3_000_000);
    });
    let events = events(&path);
    let wait = complete(&events, "late start");
    assert_eq!(wait.len(), 1);
    assert!((wait[0]["dur"].as_f64().unwrap() - 3000.0).abs() < 1.0);
    let frame = complete(&events, "frame")[0];
    assert_ne!(wait[0]["tid"], frame["tid"], "its own track");
}

#[test]
fn the_clock_is_monotonic_nanoseconds() {
    let a = profile::now_ns();
    let b = profile::now_ns();
    assert!(a > 0 && b >= a);
}

#[test]
fn a_file_that_cannot_be_made_gives_no_layer() {
    let dir = tempfile::tempdir().unwrap();
    let blocked: PathBuf = dir.path().join("a-file");
    std::fs::write(&blocked, "").unwrap();
    // a directory in the path is a file
    let layer = profile::layer_of::<tracing_subscriber::Registry>(
        &[Sink::Chrome(blocked.join("trace.json"))],
        "p",
    );
    assert!(layer.is_none());
}

#[test]
fn without_the_tracy_client_that_sink_gives_no_layer() {
    // this build is never made with `--cfg cupertino_tracy`
    assert!(profile::layer_of::<tracing_subscriber::Registry>(&[Sink::Tracy], "p").is_none());
}

#[test]
fn an_unfinished_trace_is_readable_up_to_its_last_event() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    let layer =
        profile::layer_of::<tracing_subscriber::Registry>(&[Sink::Chrome(path.clone())], "p")
            .unwrap();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        let _s = tracing::trace_span!(target: "cupertino_test", "frame").entered();
    });
    // the writer thread writes within FLUSH_DELAY; the array is not closed until `finish`
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let text = std::fs::read_to_string(&path).unwrap();
        if text.contains("\"frame\"") {
            assert!(text.starts_with("[\n"));
            assert!(!text.trim_end().ends_with(']'), "not closed yet");
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the span reached the file"
        );
        std::thread::sleep(FLUSH_DELAY / 4);
    }
    profile::finish();
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .trim_end()
        .ends_with(']'));
}
