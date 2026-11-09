//! `cupertino diagnose events`, `diagnose events clear` and `diagnose export` on fixture sources: a store in a
//! temporary home, a `session.log`, and a fake journal (a function, and `sh` for `journalctl --follow`).

use cupertino_cli::{Context, Output, Runner, StopFlag, REGISTRY};
use cupertino_diagnostic_events::store::{Limits, Store};
use cupertino_diagnostic_events::{time, Fields, Record, Severity, SCHEMA};
use cupertino_diagnostics::events::{FollowFrom, JournalOutput, JournalResult};
use cupertino_diagnostics::Sources;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a test waits for output before it fails.
const DEADLINE: Duration = Duration::from_secs(20);

/// How soon a stop must end a follow: one follow step and some slack.
const PROMPT: Duration = Duration::from_secs(2);

struct Fixture {
    dir: tempfile::TempDir,
    journal: JournalResult,
    /// The script `sh -c` runs as `journalctl --follow`.
    follow_script: String,
    follow_from: Rc<RefCell<Vec<FollowFrom>>>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home/.local/state/cupertino")).unwrap();
        Fixture {
            dir,
            journal: journal(Vec::new()),
            follow_script: "exec sleep 600".to_owned(),
            follow_from: Rc::default(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn home(&self) -> PathBuf {
        self.root().join("home")
    }

    fn state(&self) -> PathBuf {
        self.home().join(".local/state/cupertino")
    }

    fn store_dir(&self) -> PathBuf {
        self.state().join("events")
    }

    fn append(&self, records: &[Record]) {
        let store = Store::open(&self.store_dir(), Limits::default()).unwrap();
        for r in records {
            store
                .append(format!("{}\n", r.to_json()).as_bytes())
                .unwrap();
        }
    }

    fn sources(&self) -> Sources {
        let journal = self.journal.clone();
        let script = self.follow_script.clone();
        let pidfile = self.root().join("journal.pid");
        let seen = self.follow_from.clone();
        Sources {
            sysfs: self.root().join("sys"),
            root: self.root().join("root"),
            env: HashMap::from([
                ("HOME".to_owned(), self.home().display().to_string()),
                ("USER".to_owned(), "alice".to_owned()),
            ]),
            unit_state: Box::new(|_, _| None),
            journal: Box::new(move |_| journal.clone()),
            journal_follow: Box::new(move |from| {
                seen.borrow_mut().push(from.clone());
                let mut command = std::process::Command::new("sh");
                command.env("CUPERTINO_ASSET_SEARCH_PATH", "");
                command.env("PIDFILE", &pidfile);
                command.args(["-c", &format!("echo $$ > \"$PIDFILE\"; {script}")]);
                command
            }),
        }
    }

    fn run(&self, words: &[&str]) -> Output {
        let sources = self.sources();
        let cx = Context::isolated(self.root()).provide(&sources);
        Runner::new(REGISTRY).context(cx).run(&args(words), "r1")
    }
}

fn args(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

fn journal(stdout: Vec<u8>) -> JournalResult {
    JournalResult::Output(JournalOutput {
        stdout,
        stderr: Vec::new(),
        success: true,
        reader: Some(true),
    })
}

/// 2026-11-09T14:00:00Z and seconds after it.
fn t(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_794_232_800 + offset)
}

fn record(when: SystemTime, component: &str, id: &str, severity: Severity) -> Record {
    Record {
        schema: SCHEMA.to_owned(),
        time: time::format(when),
        boot_id: None,
        session: None,
        component: component.to_owned(),
        version: "0.1.0".to_owned(),
        commit: None,
        pid: 42,
        id: id.to_owned(),
        severity,
        message: "it failed".to_owned(),
        fields: Fields(vec![
            ("error".to_owned(), "no such device".into()),
            ("n".to_owned(), 3.into()),
        ]),
        source: "cupertino-test:x.rs:1".to_owned(),
        redacted: Vec::new(),
        count: 1,
        suppressed: 0,
        truncated: false,
    }
}

fn journal_line(r: &Record, cursor: &str) -> String {
    json!({"__CURSOR": cursor, "CUPERTINO_DIAG_JSON": r.to_json()}).to_string()
}

fn lines(out: &Output) -> Vec<Value> {
    out.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// A store with two events and a journal with one between them.
fn three_events() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.append(&[
        record(
            t(0),
            "cupertino-server",
            "server.gpu.reset",
            Severity::Error,
        ),
        record(
            t(20),
            "cupertino-dock",
            "dock.icon.load_failed",
            Severity::Warn,
        ),
    ]);
    fixture.journal = journal(
        format!(
            "{}\n",
            journal_line(
                &record(t(10), "cupertino-netd", "netd.link.lost", Severity::Info),
                "c1"
            )
        )
        .into_bytes(),
    );
    fixture
}

#[test]
fn events_in_human_form_are_one_line_each_then_the_sources() {
    let out = three_events().run(&["diagnose", "events"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stderr, "");
    let text: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(
        text[..3],
        [
            "2026-11-09T14:00:00.000Z  error  cupertino-server  server.gpu.reset  it failed error=\"no such device\" n=3",
            "2026-11-09T14:00:10.000Z  info   cupertino-netd  netd.link.lost  it failed error=\"no such device\" n=3",
            "2026-11-09T14:00:20.000Z  warn   cupertino-dock  dock.icon.load_failed  it failed error=\"no such device\" n=3",
        ]
    );
    assert_eq!(text[3], "3 events: 0 fatal, 1 error, 1 warn, 1 info");
    assert_eq!(text[4], "store: ok");
    assert_eq!(text[5], "session.log: unavailable (no session has run yet)");
    assert_eq!(text[6], "journal: ok");
}

#[test]
fn events_in_json_form_carry_the_records_the_sources_and_the_counts() {
    let fixture = three_events();
    let out = fixture.run(&["--format", "json", "diagnose", "events"]);
    assert_eq!(out.code, 0);
    let v: Value = serde_json::from_str(out.stdout.trim_end()).unwrap();
    assert_eq!(v["schema"], "cupertino.cli.v1");
    assert_eq!(v["command"], "diagnose events");
    let data = &v["data"];
    let events = data["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["event"], "event");
    assert_eq!(
        events[0]["data"],
        serde_json::to_value(record(
            t(0),
            "cupertino-server",
            "server.gpu.reset",
            Severity::Error
        ))
        .unwrap(),
        "an event's data is its cupertino.diag.v1 record"
    );
    assert_eq!(events[1]["data"]["id"], "netd.link.lost");
    assert_eq!(
        data["counts"],
        json!({"events": 3, "info": 1, "warn": 1, "error": 1, "fatal": 0})
    );
    let sources = data["sources"].as_array().unwrap();
    let names: Vec<(&str, &str)> = sources
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["status"].as_str().unwrap()))
        .collect();
    assert_eq!(
        names,
        [
            ("store", "ok"),
            ("session.log", "unavailable"),
            ("journal", "ok")
        ]
    );
    assert_eq!(sources[0]["events"], 2);
    assert_eq!(
        sources[0]["path"],
        fixture.store_dir().display().to_string()
    );
    assert_eq!(data["warnings"], json!([]));
}

#[test]
fn events_in_json_lines_are_event_records_then_result_and_end() {
    let out = three_events().run(&[
        "--format",
        "jsonl",
        "diagnose",
        "events",
        "--severity",
        "warn",
    ]);
    assert_eq!(out.code, 0);
    assert_eq!(out.stderr, "");
    let records = lines(&out);
    let kinds: Vec<&str> = records
        .iter()
        .map(|r| r["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["event", "event", "result", "end"]);
    for (seq, r) in records.iter().enumerate() {
        assert_eq!(r["schema"], "cupertino.cli.stream.v1");
        assert_eq!(r["command"], "diagnose events");
        assert_eq!(r["seq"], seq);
    }
    assert_eq!(records[0]["event"], "event");
    assert_eq!(records[0]["data"]["id"], "server.gpu.reset");
    assert_eq!(records[1]["data"]["id"], "dock.icon.load_failed");
    assert_eq!(records[2]["data"]["counts"]["events"], 2);
    assert_eq!(
        records[3],
        json!({
            "schema": "cupertino.cli.stream.v1", "command": "diagnose events", "request_id": "r1", "seq": 3,
            "type": "end", "ok": true, "exit_code": 0
        })
    );
}

#[test]
fn the_filters_reach_every_source() {
    let fixture = three_events();
    let ids = |words: &[&str]| -> Vec<String> {
        let mut all = vec!["--format", "jsonl", "diagnose", "events"];
        all.extend_from_slice(words);
        lines(&fixture.run(&all))
            .iter()
            .filter(|r| r["type"] == "event")
            .map(|r| r["data"]["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(ids(&["--severity", "error"]), ["server.gpu.reset"]);
    assert_eq!(
        ids(&[
            "--component",
            "cupertino-netd",
            "--component",
            "cupertino-dock"
        ]),
        ["netd.link.lost", "dock.icon.load_failed"]
    );
    assert_eq!(
        ids(&["--id", "netd.", "--id", "server.gpu"]),
        ["server.gpu.reset", "netd.link.lost"]
    );
    assert_eq!(
        ids(&["--since", "2026-11-09T14:00:15Z"]),
        ["dock.icon.load_failed"]
    );
    assert_eq!(
        ids(&["--since", "2026-11-09T15:00:00+01:00"]),
        [
            "server.gpu.reset",
            "netd.link.lost",
            "dock.icon.load_failed"
        ]
    );
    assert_eq!(
        ids(&["--since", "2026-11-09T14:00:21Z"]),
        Vec::<String>::new()
    );

    let bad = fixture.run(&[
        "--format",
        "json",
        "diagnose",
        "events",
        "--since",
        "yesterday",
    ]);
    assert_eq!(bad.code, 2);
    let v: Value = serde_json::from_str(bad.stdout.trim_end()).unwrap();
    assert_eq!(v["error"]["kind"], "invalid_usage");
    assert_eq!(v["error"]["code"], "invalid_value");
    let bad = fixture.run(&[
        "--format",
        "json",
        "diagnose",
        "events",
        "--severity",
        "debug",
    ]);
    assert_eq!(bad.code, 2);
}

#[test]
fn a_denied_journal_and_corrupt_lines_are_warnings() {
    let mut fixture = three_events();
    fixture.journal = JournalResult::Output(JournalOutput {
        stdout: Vec::new(),
        stderr: b"No journal files were opened due to insufficient permissions.\n".to_vec(),
        success: false,
        reader: Some(false),
    });
    let segment = fs::read_dir(fixture.store_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(segment)
        .unwrap()
        .write_all(b"garbage\n")
        .unwrap();
    let out = fixture.run(&["--format", "jsonl", "diagnose", "events"]);
    assert_eq!(out.code, 0);
    let errors: Vec<(String, String)> = lines(&out)
        .iter()
        .filter(|r| r["type"] == "error")
        .map(|r| {
            (
                r["error"]["kind"].as_str().unwrap().to_owned(),
                r["error"]["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        errors,
        [
            ("validation".to_owned(), "corrupt_lines".to_owned()),
            ("denied".to_owned(), "journal_denied".to_owned()),
        ]
    );
    let human = fixture.run(&["diagnose", "events"]);
    assert!(
        human
            .stderr
            .contains("cupertino: warning: the system journal cannot be read"),
        "{}",
        human.stderr
    );
    assert!(
        human.stdout.contains("\nstore: corrupt: 1\n"),
        "{}",
        human.stdout
    );
    assert!(
        human.stdout.contains("\njournal: denied ("),
        "{}",
        human.stdout
    );
}

/// Standard output shared with the test thread: it can wait for text and break the pipe.
#[derive(Clone, Default)]
struct Shared(Arc<(Mutex<Vec<u8>>, Condvar, AtomicBool)>);

impl Shared {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0 .0.lock().unwrap()).into_owned()
    }

    /// Waits until the output holds `n` occurrences of `needle`.
    fn wait_for(&self, needle: &str, n: usize) {
        let (buffer, changed, _) = &*self.0;
        let deadline = Instant::now() + DEADLINE;
        let mut guard = buffer.lock().unwrap();
        while String::from_utf8_lossy(&guard).matches(needle).count() < n {
            let left = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| {
                    panic!("no {n} × {needle:?} in {}", String::from_utf8_lossy(&guard))
                });
            guard = changed.wait_timeout(guard, left).unwrap().0;
        }
    }

    fn close(&self) {
        self.0 .2.store(true, Ordering::SeqCst);
    }
}

impl Write for Shared {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let (buffer, changed, closed) = &*self.0;
        if closed.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        buffer.lock().unwrap().extend_from_slice(data);
        changed.notify_all();
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `words` on the fixture in this thread while `meanwhile` runs in another with the output and the stop flag;
/// gives the exit code, stdout, stderr, and when `meanwhile` returned.
fn follow(
    fixture: &Fixture,
    words: &[&str],
    meanwhile: impl FnOnce(Shared, &StopFlag) + Send + 'static,
) -> (u8, String, String, Instant) {
    let stop = Arc::new(StopFlag::new());
    let out = Shared::default();
    let helper = {
        let (out, stop) = (out.clone(), stop.clone());
        std::thread::spawn(move || {
            meanwhile(out, &stop);
            Instant::now()
        })
    };
    let sources = fixture.sources();
    let cx = Context::isolated(fixture.root()).provide(&sources);
    let mut err = Vec::new();
    let code = Runner::new(REGISTRY).context(cx).stop(&stop).run_to(
        &args(words),
        "r1",
        &mut out.clone(),
        &mut err,
    );
    let done = helper.join().unwrap();
    (code, out.text(), String::from_utf8(err).unwrap(), done)
}

/// The pid the fake `journalctl --follow` wrote, once it has.
fn journal_pid(fixture: &Fixture) -> u32 {
    let path = fixture.root().join("journal.pid");
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(pid) = fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            return pid;
        }
        assert!(Instant::now() < deadline, "the fake journal never started");
        std::thread::yield_now();
    }
}

#[test]
fn a_follow_prints_new_store_and_journal_lines_and_a_stop_ends_it_promptly() {
    let mut fixture = three_events();
    let late = record(t(30), "cupertino-btd", "btd.adapter.lost", Severity::Error);
    fixture.follow_script = format!(
        "printf '%s\\n' '{}'; exec sleep 600",
        journal_line(&late, "c2")
    );
    let store_dir = fixture.store_dir();
    let (code, stdout, stderr, stopped) = follow(
        &fixture,
        &["--format", "jsonl", "diagnose", "events", "--follow"],
        move |out, stop| {
            out.wait_for("\"type\":\"event\"", 3);
            // the journal child's line
            out.wait_for("btd.adapter.lost", 1);
            let store = Store::open(&store_dir, Limits::default()).unwrap();
            let new = record(t(40), "cupertino-dock", "dock.new", Severity::Warn);
            store
                .append(format!("{}\n", new.to_json()).as_bytes())
                .unwrap();
            out.wait_for("dock.new", 1);
            // nothing more comes: the stop alone ends it
            stop.set();
        },
    );
    let elapsed = stopped.elapsed();
    assert!(
        elapsed < PROMPT,
        "a stop took {elapsed:?} to end the follow"
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    let records: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let ids: Vec<&str> = records
        .iter()
        .filter(|r| r["type"] == "event")
        .map(|r| r["data"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "server.gpu.reset",
            "netd.link.lost",
            "dock.icon.load_failed",
            "btd.adapter.lost",
            "dock.new"
        ]
    );
    let end = records.last().unwrap();
    assert_eq!(
        (&end["type"], &end["ok"], &end["reason"]),
        (&json!("end"), &json!(true), &json!("stopped"))
    );
    assert_eq!(records[records.len() - 2]["data"]["counts"]["events"], 5);
    assert_eq!(
        *fixture.follow_from.borrow(),
        [FollowFrom::Cursor("c1".to_owned())],
        "the journal is followed after the last entry read"
    );
    // the child was killed and reaped
    let pid = journal_pid(&fixture);
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "journal child {pid} is still there"
    );
}

#[test]
fn a_stop_ends_a_follow_in_human_form_while_no_event_arrives() {
    let fixture = Fixture::new();
    let root = fixture.root().to_path_buf();
    let (code, stdout, _, stopped) = follow(
        &fixture,
        &["diagnose", "events", "--follow"],
        move |_, stop| {
            // the fake journal runs: the follow is waiting on it
            let path = root.join("journal.pid");
            let deadline = Instant::now() + DEADLINE;
            while !path.exists() {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            stop.set();
        },
    );
    let elapsed = stopped.elapsed();
    assert!(elapsed < PROMPT, "a stop took {elapsed:?}");
    assert_eq!(code, 0);
    assert!(stdout.starts_with("0 events: "), "{stdout}");
    assert_eq!(fixture.follow_from.borrow().len(), 1);
    assert!(matches!(
        fixture.follow_from.borrow()[0],
        FollowFrom::Since(_)
    ));
    let pid = journal_pid(&fixture);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn a_closed_output_ends_a_follow_with_exit_0() {
    let fixture = three_events();
    let store_dir = fixture.store_dir();
    let (code, stdout, stderr, _) = follow(
        &fixture,
        &["--format", "jsonl", "diagnose", "events", "--follow"],
        move |out, _| {
            out.wait_for("\"type\":\"event\"", 3);
            // the reader goes away; the next event finds the pipe closed
            out.close();
            let store = Store::open(&store_dir, Limits::default()).unwrap();
            let new = record(t(40), "cupertino-dock", "dock.new", Severity::Warn);
            store
                .append(format!("{}\n", new.to_json()).as_bytes())
                .unwrap();
        },
    );
    assert_eq!((code, stderr.as_str()), (0, ""));
    assert!(
        !stdout.contains("dock.new") && !stdout.contains("\"end\""),
        "{stdout}"
    );
    let pid = journal_pid(&fixture);
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
}

#[test]
fn clear_asks_for_yes_then_deletes_the_segments_and_keeps_the_salt() {
    let fixture = three_events();
    let salt = fs::read(fixture.store_dir().join("salt")).unwrap_or_else(|_| {
        let store = Store::open(&fixture.store_dir(), Limits::default()).unwrap();
        store.salt().unwrap();
        fs::read(fixture.store_dir().join("salt")).unwrap()
    });
    let segments = || {
        fs::read_dir(fixture.store_dir())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "jsonl")
            })
            .count()
    };
    assert_eq!(segments(), 1);

    let refused = fixture.run(&["--format", "json", "diagnose", "events", "clear"]);
    assert_eq!(refused.code, 2);
    let v: Value = serde_json::from_str(refused.stdout.trim_end()).unwrap();
    assert_eq!(v["error"]["kind"], "invalid_usage");
    assert_eq!(v["error"]["code"], "confirmation_required");
    assert_eq!(segments(), 1, "nothing deleted without --yes");

    let cleared = fixture.run(&["--format", "json", "diagnose", "events", "clear", "--yes"]);
    assert_eq!(cleared.code, 0, "{}", cleared.stdout);
    let v: Value = serde_json::from_str(cleared.stdout.trim_end()).unwrap();
    assert_eq!(v["data"]["deleted"], 1);
    assert_eq!(segments(), 0);
    assert_eq!(fs::read(fixture.store_dir().join("salt")).unwrap(), salt);
    let after = fixture.run(&["--format", "json", "diagnose", "events"]);
    let v: Value = serde_json::from_str(after.stdout.trim_end()).unwrap();
    assert_eq!(v["data"]["sources"][0]["status"], "empty");

    // no store at all: nothing to delete
    let empty = Fixture::new().run(&["diagnose", "events", "clear", "--yes"]);
    assert_eq!(empty.code, 0);
    assert!(
        empty.stdout.starts_with("deleted 0 segments of "),
        "{}",
        empty.stdout
    );
}

#[test]
fn export_writes_one_private_document_and_never_overwrites() {
    let fixture = three_events();
    fs::create_dir_all(fixture.root().join("root/etc")).unwrap();
    fs::write(
        fixture.root().join("root/etc/os-release"),
        "NAME=\"Cupertino OS\"\nID=cupertino\nIMAGE_VERSION=7\n",
    )
    .unwrap();
    fs::create_dir_all(fixture.root().join("root/proc/sys/kernel")).unwrap();
    fs::write(
        fixture.root().join("root/proc/sys/kernel/osrelease"),
        "6.12.1-arch1\n",
    )
    .unwrap();
    let out = fixture.run(&[
        "--format",
        "json",
        "diagnose",
        "export",
        "--output",
        "report.json",
    ]);
    assert_eq!(out.code, 0, "{}", out.stdout);
    let path = fixture.root().join("report.json");
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let doc: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut keys: Vec<&str> = doc
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["created", "events", "report", "schema", "sources", "versions"]
    );
    assert_eq!(doc["schema"], "cupertino.diag.export.v1");
    assert!(time::parse(doc["created"].as_str().unwrap()).is_some());
    assert_eq!(doc["versions"]["cli"], env!("CARGO_PKG_VERSION"));
    assert!(doc["versions"].as_object().unwrap().contains_key("commit"));
    assert_eq!(doc["versions"]["os_image"]["id"], "cupertino");
    assert_eq!(doc["versions"]["os_image"]["image_version"], "7");
    assert_eq!(doc["versions"]["kernel"], "6.12.1-arch1");
    assert_eq!(doc["report"]["schema"], 1);
    let areas: Vec<&str> = doc["report"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["area"].as_str().unwrap())
        .collect();
    assert!(
        areas.contains(&"gpu") && areas.contains(&"events"),
        "{areas:?}"
    );
    assert_eq!(doc["events"].as_array().unwrap().len(), 3);
    assert_eq!(doc["events"][0]["id"], "server.gpu.reset");
    assert_eq!(doc["sources"].as_array().unwrap().len(), 3);
    assert_eq!(doc["sources"][0]["path"], "~/.local/state/cupertino/events");
    let v: Value = serde_json::from_str(out.stdout.trim_end()).unwrap();
    assert_eq!(v["data"]["events"], 3);

    let again = fixture.run(&[
        "--format",
        "json",
        "diagnose",
        "export",
        "--output",
        "report.json",
    ]);
    assert_eq!(again.code, 5);
    let v: Value = serde_json::from_str(again.stdout.trim_end()).unwrap();
    assert_eq!(v["error"]["kind"], "validation");
    assert_eq!(v["error"]["code"], "file_exists");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        doc,
        "the file is untouched"
    );

    let missing = fixture.run(&["diagnose", "export"]);
    assert_eq!(missing.code, 2);
}

#[test]
fn export_with_logs_scrubs_every_line() {
    let fixture = three_events();
    let home = fixture.home().display().to_string();
    let token = "0123456789abcdef0123456789abcdef01234567";
    fs::write(
        fixture.state().join("server.log"),
        format!(
            "loading /home/alice/Projects/secret plan.txt failed\n\
             session of alice started\n\
             wrote {home}/Documents/notes.md\n\
             sent token={token} to the portal\n\
             the GPU was reset\n"
        ),
    )
    .unwrap();
    fs::write(
        fixture.state().join("session.log"),
        format!(
            "{} starting\n",
            cupertino_diagnostics::events::local_time(t(5))
        ),
    )
    .unwrap();
    let out = fixture.run(&[
        "--format",
        "json",
        "diagnose",
        "export",
        "--output",
        "with-logs.json",
        "--include-logs",
    ]);
    assert_eq!(out.code, 0, "{}", out.stdout);
    let text = fs::read_to_string(fixture.root().join("with-logs.json")).unwrap();
    for secret in [
        "alice",
        "/home/alice",
        "secret plan",
        home.as_str(),
        token,
        "notes.md",
    ] {
        assert!(!text.contains(secret), "the export holds {secret:?}");
    }
    let doc: Value = serde_json::from_str(&text).unwrap();
    let logs = doc["logs"].as_object().unwrap();
    let names: Vec<&str> = logs.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        ["server.log", "session.log"],
        "server.log.1 does not exist"
    );
    let server: Vec<&str> = logs["server.log"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert_eq!(server.len(), 5);
    assert_eq!(server[1], "session of [user] started");
    assert_eq!(server[3], "sent token=[redacted] to the portal");
    assert_eq!(server[4], "the GPU was reset");
    assert!(server[2].starts_with("wrote ~/…#"), "{}", server[2]);
    let v: Value = serde_json::from_str(out.stdout.trim_end()).unwrap();
    assert_eq!(v["data"]["logs"], json!(["server.log", "session.log"]));
}
