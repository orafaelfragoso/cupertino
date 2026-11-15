use super::*;
use std::hint::black_box;
use std::time::Instant;

use serde_json::Value;

#[test]
fn the_variable_names_a_file_a_directory_of_one_file_per_process_or_tracy() {
    assert_eq!(
        parse("chrome:/tmp/t.json", "dock", 7),
        Ok(vec![Sink::Chrome("/tmp/t.json".into())])
    );
    assert_eq!(
        parse("chrome:/tmp/traces/", "cupertino-dock", 42),
        Ok(vec![Sink::Chrome(
            "/tmp/traces/cupertino-dock-42.json".into()
        )])
    );
    let dir = tempfile::tempdir().unwrap();
    let named = format!("chrome:{}", dir.path().display());
    assert_eq!(
        parse(&named, "files", 3),
        Ok(vec![Sink::Chrome(dir.path().join("files-3.json"))]),
        "an existing directory without the slash"
    );
    assert_eq!(
        parse(" chrome:/t/ , tracy ", "server", 1),
        Ok(vec![Sink::Chrome("/t/server-1.json".into()), Sink::Tracy])
    );
    assert_eq!(parse("", "x", 1), Ok(vec![]));
    assert!(parse("perf", "x", 1).is_err());
    assert!(parse("chrome:", "x", 1).is_err());
    assert!(parse("chrome:/a/,chrome:/b/", "x", 1).is_err());
}

/// The events of a trace file, which may lack its closing bracket.
fn events(path: &Path) -> Vec<Value> {
    let mut text = std::fs::read_to_string(path).unwrap();
    if !text.trim_end().ends_with(']') {
        text.push_str("\n]");
    }
    match serde_json::from_str(&text).unwrap() {
        Value::Array(events) => events,
        other => panic!("not an array: {other}"),
    }
}

fn complete(events: &[Value]) -> Vec<&Value> {
    events.iter().filter(|e| e["ph"] == "X").collect()
}

#[test]
fn spans_are_written_as_nested_complete_events_with_their_label_and_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    let layer =
        layer_of::<tracing_subscriber::Registry>(&[Sink::Chrome(path.clone())], "probe").unwrap();
    let subscriber = tracing_subscriber::registry().with(layer);
    tracing::subscriber::with_default(subscriber, || {
        let outer = tracing::trace_span!("cupertino.frame", window = 3_u64).entered();
        {
            let _update = tracing::trace_span!("cupertino.update", label = "Message").entered();
            std::thread::sleep(Duration::from_millis(2));
        }
        let paint = tracing::trace_span!("cupertino.paint", nodes = tracing::field::Empty);
        paint.record("nodes", 12_u64);
        paint.in_scope(|| std::thread::sleep(Duration::from_millis(1)));
        // another target's spans and plain events are not the timeline's
        tracing::trace_span!(target: "smithay", "smithay.render").in_scope(|| {});
        tracing::info!("not a span");
        let start = now_ns();
        interval("latch wait", start, start + 5_000);
        drop(outer);
    });
    finish();
    let events = events(&path);
    let process = events.iter().find(|e| e["name"] == "process_name").unwrap();
    assert_eq!(process["args"]["name"], "probe");
    assert_eq!(process["pid"], std::process::id());
    let slices = complete(&events);
    let names: Vec<&str> = slices.iter().map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "cupertino.update Message",
            "cupertino.paint",
            "latch wait",
            "cupertino.frame"
        ],
        "each written as it ends, the label after the name"
    );
    let by = |name: &str| *slices.iter().find(|e| e["name"] == name).unwrap();
    let (frame, update, paint) = (
        by("cupertino.frame"),
        by("cupertino.update Message"),
        by("cupertino.paint"),
    );
    assert_eq!(frame["args"]["window"], 3);
    assert_eq!(paint["args"]["nodes"], 12);
    assert!(update.get("args").is_none(), "the label is no argument");
    let span = |e: &Value| {
        let ts = e["ts"].as_f64().unwrap();
        (ts, ts + e["dur"].as_f64().unwrap())
    };
    let (f0, f1) = span(frame);
    for inner in [update, paint] {
        let (s0, s1) = span(inner);
        assert!(f0 <= s0 && s1 <= f1, "{inner} inside {frame}");
        assert_eq!(inner["tid"], frame["tid"]);
    }
    assert!(update["dur"].as_f64().unwrap() >= 2000.0, "{update}");
    let wait = by("latch wait");
    assert_eq!(wait["dur"].as_f64().unwrap(), 5.0);
    assert_ne!(wait["tid"], frame["tid"], "a wait has a track of its own");
    let threads: Vec<&Value> = events
        .iter()
        .filter(|e| e["name"] == "thread_name")
        .collect();
    assert_eq!(threads.len(), 2, "the thread's track and its waits'");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .trim_end()
        .ends_with(']'));
}

#[test]
fn a_trace_is_readable_while_the_process_runs_before_it_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    let layer =
        layer_of::<tracing_subscriber::Registry>(&[Sink::Chrome(path.clone())], "probe").unwrap();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        tracing::trace_span!("cupertino.turn").in_scope(|| {});
    });
    // the writer writes within its delay; a process killed now leaves this file
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = complete(&events(&path)).len();
        if found == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the span never reached the file");
        std::thread::sleep(FLUSH_DELAY);
    }
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .trim_end()
        .ends_with(']'));
}

/// The span callsite cost with no timeline: a loop entering a span each round against the same loop without. Prints the
/// figures; `scripts/test -p cupertino-profile -- --no-capture -E 'test(/callsite/)'` shows them. The bound is for this
/// crate's unoptimised test build, where the span's guard is a call or two; optimised (`cargo test --release`), the
/// check is a load and a compare, about a nanosecond.
#[test]
fn a_span_callsite_costs_nothing_measurable_with_no_timeline_recorded() {
    const ROUNDS: u32 = 2_000_000;
    let work = |i: u32| black_box(i.wrapping_mul(2_654_435_761));
    let measure = |spans: bool| {
        let began = Instant::now();
        for i in 0..ROUNDS {
            if spans {
                let _g = tracing::trace_span!("cupertino.bench", label = "x").entered();
                work(i);
            } else {
                work(i);
            }
        }
        began.elapsed().as_nanos() as f64 / f64::from(ROUNDS)
    };
    // the fastest of a few runs of each, so another process's moment on this core is not counted
    let best = |spans: bool| (0..5).map(|_| measure(spans)).fold(f64::MAX, f64::min);
    let (without, with) = (best(false), best(true));
    let cost = with - without;
    eprintln!("span callsite with no timeline: {cost:.2} ns ({with:.2} ns a round against {without:.2} ns)");
    assert!(cost < 100.0, "a disabled span costs {cost:.2} ns");

    // and recorded, for the figure per span
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trace.json");
    let layer = layer_of::<tracing_subscriber::Registry>(&[Sink::Chrome(path)], "probe").unwrap();
    let recorded =
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            const RECORDED: u32 = 20_000;
            let began = Instant::now();
            for i in 0..RECORDED {
                let _g = tracing::trace_span!("cupertino.bench", label = "x").entered();
                work(i);
            }
            began.elapsed().as_nanos() as f64 / f64::from(RECORDED)
        });
    eprintln!("span recorded to a trace file: {recorded:.0} ns");
}

/// A build without the client records nothing for `tracy` (and says so); `--all-features` alone is such a build.
#[cfg(not(all(feature = "tracy", cupertino_tracy)))]
#[test]
fn without_the_tracy_client_the_tracy_sink_records_nothing() {
    assert!(layer_of::<tracing_subscriber::Registry>(&[Sink::Tracy], "probe").is_none());
}

/// A build with the client (`--features tracy` and `--cfg cupertino_tracy`) gives the Tracy layer.
#[cfg(all(feature = "tracy", cupertino_tracy))]
#[test]
fn with_the_tracy_client_the_tracy_sink_gives_its_layer() {
    let layer = layer_of::<tracing_subscriber::Registry>(&[Sink::Tracy], "probe").unwrap();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        tracing::trace_span!("cupertino.frame").in_scope(|| {});
    });
}
