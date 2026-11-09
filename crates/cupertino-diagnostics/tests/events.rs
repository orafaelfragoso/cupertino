//! Reading diagnostic events back: the user store, `session.log` and a fake journal, merged in time order, filtered,
//! with the state of each source, and the `events.recent` check.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cupertino_diagnostic_events::store::{Limits, Store};
use cupertino_diagnostic_events::{time, Fields, Record, Severity, SCHEMA};
use cupertino_diagnostics::events::{
    self, Filter, FollowFrom, JournalEntry, JournalOutput, JournalResult, SourceStatus,
};
use cupertino_diagnostics::{run, Area, Check, Sources, Status};

/// A home with a state directory, and what the fake journal answers.
struct Fixture {
    dir: tempfile::TempDir,
    journal: JournalResult,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("home/.local/state/cupertino")).unwrap();
        Fixture {
            dir,
            journal: JournalResult::Unavailable("journalctl is not installed".to_owned()),
        }
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }

    fn state(&self) -> PathBuf {
        self.home().join(".local/state/cupertino")
    }

    fn store_dir(&self) -> PathBuf {
        self.state().join("events")
    }

    fn store(&self) -> Store {
        Store::open(&self.store_dir(), Limits::default()).unwrap()
    }

    fn append(&self, records: &[Record]) {
        let store = self.store();
        for r in records {
            store
                .append(format!("{}\n", r.to_json()).as_bytes())
                .unwrap();
        }
    }

    fn session_log(&self, text: &str) {
        fs::write(self.state().join("session.log"), text).unwrap();
    }

    fn journal(mut self, journal: JournalResult) -> Self {
        self.journal = journal;
        self
    }

    fn sources(&self) -> Sources {
        let journal = self.journal.clone();
        Sources {
            sysfs: self.dir.path().join("sys"),
            root: self.dir.path().join("root"),
            env: HashMap::from([
                ("HOME".to_owned(), self.home().display().to_string()),
                ("USER".to_owned(), "alice".to_owned()),
            ]),
            unit_state: Box::new(|_, _| None),
            journal: Box::new(move |_| journal.clone()),
            journal_follow: Box::new(|_| {
                let mut command = std::process::Command::new("true");
                command.env("CUPERTINO_ASSET_SEARCH_PATH", "");
                command
            }),
        }
    }

    fn read(&self, filter: &Filter) -> events::EventsReport {
        events::read(&self.sources(), filter)
    }
}

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(seconds)
}

/// 2026-11-09T14:00:00Z and seconds after it.
fn t(offset: u64) -> SystemTime {
    at(1_794_232_800 + offset)
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
        message: "a fixed message".to_owned(),
        fields: Fields(vec![("n".to_owned(), 1.into())]),
        source: "cupertino-test:x.rs:1".to_owned(),
        redacted: Vec::new(),
        count: 1,
        suppressed: 0,
        truncated: false,
    }
}

/// `journalctl --output=json` lines of `records`, each with a cursor.
fn journal_lines(records: &[Record]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, r) in records.iter().enumerate() {
        let entry = serde_json::json!({
            "__CURSOR": format!("s=1;i={i}"),
            "PRIORITY": r.severity.priority().to_string(),
            "CUPERTINO_DIAG_SCHEMA": "1",
            "CUPERTINO_DIAG_JSON": r.to_json(),
        });
        out.extend(serde_json::to_vec(&entry).unwrap());
        out.push(b'\n');
    }
    out
}

fn journal_ok(stdout: Vec<u8>) -> JournalResult {
    JournalResult::Output(JournalOutput {
        stdout,
        stderr: Vec::new(),
        success: true,
        reader: Some(true),
    })
}

fn ids(report: &events::EventsReport) -> Vec<&str> {
    report.events.iter().map(|e| e.record.id.as_str()).collect()
}

fn status(report: &events::EventsReport, name: &str) -> SourceStatus {
    report.source(name).unwrap().status
}

#[test]
fn the_three_sources_merge_in_time_order() {
    let fixture = Fixture::new().journal(journal_ok(journal_lines(&[
        record(t(20), "cupertino-netd", "netd.c", Severity::Warn),
        record(t(40), "cupertino-btd", "btd.e", Severity::Error),
    ])));
    fixture.append(&[
        record(t(10), "cupertino-server", "server.b", Severity::Info),
        record(t(30), "cupertino-dock", "dock.d", Severity::Warn),
        // written later, but earlier: the merge goes by time
        record(t(0), "cupertino-server", "server.a", Severity::Info),
    ]);
    fixture.session_log(&format!("{} starting\n", events::local_time(t(35))));
    let report = fixture.read(&Filter::default());
    assert_eq!(
        ids(&report),
        [
            "server.a",
            "server.b",
            "netd.c",
            "dock.d",
            "session.start",
            "btd.e"
        ]
    );
    let sources: Vec<(&str, &str)> = report
        .events
        .iter()
        .map(|e| (e.record.id.as_str(), e.source))
        .collect();
    assert_eq!(sources[2], ("netd.c", "journal"));
    assert_eq!(sources[4], ("session.start", "session.log"));
    assert_eq!(sources[0], ("server.a", "store"));
    for name in ["store", "session.log", "journal"] {
        assert_eq!(status(&report, name), SourceStatus::Ok, "{name}");
    }
    let store = report.source("store").unwrap();
    assert_eq!((store.events, store.corrupt), (3, 0));
    assert_eq!(
        report.counts(),
        events::Counts {
            events: 6,
            info: 3,
            warn: 2,
            error: 1,
            fatal: 0
        }
    );
    assert_eq!(
        report.follow_from,
        Some(FollowFrom::Cursor("s=1;i=1".to_owned())),
        "a journal follow starts after the last entry read"
    );
}

#[test]
fn every_filter_narrows_every_source() {
    let fixture = Fixture::new().journal(journal_ok(journal_lines(&[
        record(t(20), "cupertino-netd", "netd.link.down", Severity::Warn),
        record(t(40), "cupertino-btd", "btd.adapter.lost", Severity::Fatal),
    ])));
    fixture.append(&[
        record(
            t(0),
            "cupertino-server",
            "server.gpu.reset",
            Severity::Error,
        ),
        record(
            t(10),
            "cupertino-dock",
            "dock.icon.load_failed",
            Severity::Info,
        ),
        record(
            t(30),
            "cupertino-server",
            "server.output.lost",
            Severity::Warn,
        ),
    ]);
    let read = |filter: Filter| ids(&fixture.read(&filter)).join(" ");

    let since = Filter {
        since: Some(t(20)),
        ..Filter::default()
    };
    assert_eq!(
        read(since),
        "netd.link.down server.output.lost btd.adapter.lost"
    );
    let components = Filter {
        components: vec!["cupertino-server".into(), "cupertino-btd".into()],
        ..Filter::default()
    };
    assert_eq!(
        read(components),
        "server.gpu.reset server.output.lost btd.adapter.lost"
    );
    let prefixes = Filter {
        ids: vec!["server.gpu".into(), "netd.".into()],
        ..Filter::default()
    };
    assert_eq!(read(prefixes), "server.gpu.reset netd.link.down");
    let severity = Filter {
        severity: Some(Severity::Error),
        ..Filter::default()
    };
    assert_eq!(read(severity), "server.gpu.reset btd.adapter.lost");
    let all = Filter {
        since: Some(t(5)),
        components: vec!["cupertino-server".into()],
        ids: vec!["server.".into()],
        severity: Some(Severity::Warn),
    };
    assert_eq!(read(all), "server.output.lost");
}

#[test]
fn since_takes_a_duration_back_from_now_or_an_rfc_3339_time() {
    let now = t(10 * 86_400);
    assert_eq!(
        events::parse_since("30s", now),
        Some(now - Duration::from_secs(30))
    );
    assert_eq!(
        events::parse_since("10m", now),
        Some(now - Duration::from_secs(600))
    );
    assert_eq!(
        events::parse_since("24h", now),
        Some(now - Duration::from_secs(86_400))
    );
    assert_eq!(
        events::parse_since("2d", now),
        Some(now - Duration::from_secs(172_800))
    );
    assert_eq!(events::parse_since("2026-11-09T14:00:00Z", now), Some(t(0)));
    assert_eq!(
        events::parse_since("2026-11-09T15:00:00+01:00", now),
        Some(t(0))
    );
    for bad in [
        "",
        "h",
        "24",
        "-5m",
        "1.5h",
        "10w",
        "yesterday",
        "2026-11-09",
    ] {
        assert_eq!(events::parse_since(bad, now), None, "{bad}");
    }
}

#[test]
fn a_store_that_is_missing_empty_or_corrupt_says_so() {
    let fixture = Fixture::new();
    let report = fixture.read(&Filter::default());
    assert_eq!(status(&report, "store"), SourceStatus::Unavailable);
    assert_eq!(
        report.source("store").unwrap().detail.as_deref(),
        Some("no event has been stored yet")
    );
    assert_eq!(status(&report, "session.log"), SourceStatus::Unavailable);
    assert_eq!(status(&report, "journal"), SourceStatus::Unavailable);
    assert_eq!(
        report.source("journal").unwrap().detail.as_deref(),
        Some("journalctl is not installed")
    );

    fixture.store();
    let report = fixture.read(&Filter::default());
    assert_eq!(status(&report, "store"), SourceStatus::Empty);

    fixture.append(&[record(t(0), "cupertino-dock", "dock.a", Severity::Info)]);
    let later = Filter {
        since: Some(t(60)),
        ..Filter::default()
    };
    assert_eq!(
        status(&fixture.read(&later), "store"),
        SourceStatus::Empty,
        "nothing the filter lets through"
    );

    let segment = fs::read_dir(fixture.store_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(&segment).unwrap();
    file.write_all(b"not json\n{\"schema\":\"cupertino.diag.v9\"}\n\xff\xfe\n")
        .unwrap();
    file.write_all(
        record(t(5), "cupertino-dock", "dock.b", Severity::Info)
            .to_json()
            .as_bytes(),
    )
    .unwrap();
    file.write_all(b"\n{\"schema\":\"cupertino.di").unwrap();
    let report = fixture.read(&Filter::default());
    let store = report.source("store").unwrap();
    assert_eq!(store.status, SourceStatus::Corrupt);
    assert_eq!((store.events, store.corrupt, store.partial), (2, 3, 1));
    assert_eq!(store.status_text(), "corrupt: 3");
    assert_eq!(ids(&report), ["dock.a", "dock.b"]);
}

#[test]
fn the_journal_is_ok_empty_corrupt_denied_or_unavailable() {
    let read = |journal: JournalResult| {
        let report = Fixture::new().journal(journal).read(&Filter::default());
        let source = report.source("journal").unwrap().clone();
        (source, report)
    };

    let (empty, report) = read(journal_ok(Vec::new()));
    assert_eq!(empty.status, SourceStatus::Empty);
    assert!(matches!(report.follow_from, Some(FollowFrom::Since(_))));

    // a value that is not UTF-8 or is large comes as an array of bytes; a field given twice as an array of values
    let line = record(t(0), "cupertino-netd", "netd.a", Severity::Warn).to_json();
    let bytes: Vec<u8> = line.bytes().collect();
    let mut stdout =
        serde_json::to_vec(&serde_json::json!({ "CUPERTINO_DIAG_JSON": bytes })).unwrap();
    stdout.push(b'\n');
    stdout.extend(
        serde_json::to_vec(&serde_json::json!({
            "CUPERTINO_DIAG_JSON": [record(t(1), "cupertino-netd", "netd.b", Severity::Warn).to_json(), "x"]
        }))
        .unwrap(),
    );
    stdout.push(b'\n');
    let (ok, report) = read(journal_ok(stdout.clone()));
    assert_eq!((ok.status, ok.events), (SourceStatus::Ok, 2));
    assert_eq!(ids(&report), ["netd.a", "netd.b"]);

    stdout
        .extend(b"not json\n{\"MESSAGE\":\"no event field\"}\n{\"CUPERTINO_DIAG_JSON\":\"{}\"}\n");
    stdout.extend(b"{\"CUPERTINO_DIAG_JSON\":null}\n");
    let (corrupt, _) = read(journal_ok(stdout));
    assert_eq!(
        (corrupt.status, corrupt.events, corrupt.corrupt),
        (SourceStatus::Corrupt, 2, 4)
    );
    assert_eq!(corrupt.status_text(), "corrupt: 4");

    // journalctl shows only the caller's own journal and says why
    let hint = JournalResult::Output(JournalOutput {
        stdout: Vec::new(),
        stderr:
            b"Hint: You are currently not seeing messages from other users and the system.\n      \
                  Users in groups 'adm', 'systemd-journal', 'wheel' can see all messages.\n"
                .to_vec(),
        success: true,
        reader: None,
    });
    let (denied, report) = read(hint);
    assert_eq!(denied.status, SourceStatus::Denied);
    assert!(denied
        .detail
        .as_deref()
        .unwrap()
        .contains("systemd-journal"));
    assert_eq!(report.follow_from, None, "a denied journal is not followed");

    // no journal file could be opened at all
    let none = JournalResult::Output(JournalOutput {
        stdout: Vec::new(),
        stderr: b"No journal files were opened due to insufficient permissions.\n".to_vec(),
        success: false,
        reader: None,
    });
    assert_eq!(read(none).0.status, SourceStatus::Denied);

    // without a notice: a caller outside the reader groups that sees nothing is denied
    let outside = JournalResult::Output(JournalOutput {
        reader: Some(false),
        ..JournalOutput::default()
    });
    assert_eq!(read(outside).0.status, SourceStatus::Denied);
    let member = JournalResult::Output(JournalOutput {
        success: true,
        reader: Some(true),
        ..JournalOutput::default()
    });
    assert_eq!(read(member).0.status, SourceStatus::Empty);

    let failed = JournalResult::Output(JournalOutput {
        stdout: Vec::new(),
        stderr: b"Failed to open journal: Too many open files\n".to_vec(),
        success: false,
        reader: Some(true),
    });
    let (unavailable, _) = read(failed);
    assert_eq!(unavailable.status, SourceStatus::Unavailable);
    assert_eq!(
        unavailable.detail.as_deref(),
        Some("Failed to open journal: Too many open files")
    );
    let missing = JournalResult::Unavailable("journalctl is not installed".to_owned());
    assert_eq!(read(missing).0.status, SourceStatus::Unavailable);
}

#[test]
fn the_journal_function_gets_the_filter() {
    let fixture = Fixture::new();
    let seen: Rc<RefCell<Vec<Filter>>> = Rc::default();
    let mut sources = fixture.sources();
    let log = seen.clone();
    sources.journal = Box::new(move |filter| {
        log.borrow_mut().push(filter.clone());
        journal_ok(Vec::new())
    });
    let filter = Filter {
        since: Some(t(0)),
        components: vec!["cupertino-netd".into()],
        ids: vec!["netd.".into()],
        severity: Some(Severity::Warn),
    };
    events::read(&sources, &filter);
    assert_eq!(*seen.borrow(), [filter]);
}

#[test]
fn a_journal_line_gives_its_record_and_cursor() {
    let r = record(t(0), "cupertino-netd", "netd.a", Severity::Warn);
    let line = serde_json::to_vec(
        &serde_json::json!({"__CURSOR": "c", "CUPERTINO_DIAG_JSON": r.to_json()}),
    )
    .unwrap();
    assert_eq!(
        events::journal_entry(&line),
        JournalEntry::Event(Box::new(r), Some("c".to_owned()))
    );
    assert_eq!(
        events::journal_entry(b"{\"__CURSOR\":\"d\"}"),
        JournalEntry::Corrupt(Some("d".to_owned()))
    );
    assert_eq!(events::journal_entry(b"[1,2]"), JournalEntry::Corrupt(None));
}

#[test]
fn session_log_lines_become_session_records_at_read_time() {
    let fixture = Fixture::new();
    let home = fixture.home().display().to_string();
    let stamp = |s: u64| events::local_time(t(s));
    fixture.session_log(&format!(
        "{} starting\n\
         {} the compositor ended normally after 600 s\n\
         {} starting with a limit of 30 s\n\
         {} locked the keyring (2 collections)\n\
         {} the compositor was stopped by the session (status 143) after 31 s\n\
         {} starting\n\
         {} the compositor was ended by signal 11 (status 139) after 5 s; see server.log\n\
         {} starting\n\
         {} the compositor ended with status 1 after 2 s: cannot open {home}/Pictures/wall.png for alice\n\
         {} the compositor did not stop in time and was ended by the watchdog (status 124) after 60 s\n\
         {} the compositor ended with status 3 after 4 s; see server.log\n\
         {} the compositor caught fire\n\
         not a line of the log\n",
        stamp(0),
        stamp(600),
        stamp(700),
        stamp(701),
        stamp(731),
        stamp(800),
        stamp(805),
        stamp(900),
        stamp(902),
        stamp(1000),
        stamp(1100),
        stamp(1200),
    ));
    let report = fixture.read(&Filter::default());
    assert_eq!(status(&report, "session.log"), SourceStatus::Ok);
    let records: Vec<&Record> = report.events.iter().map(|e| &e.record).collect();
    let summary: Vec<(&str, Severity, Option<&str>, u64)> = records
        .iter()
        .map(|r| {
            (
                r.id.as_str(),
                r.severity,
                r.fields.get("cause").and_then(|c| c.as_str()),
                r.count,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("session.start", Severity::Info, None, 1),
            ("session.end", Severity::Info, Some("normal"), 1),
            ("session.start", Severity::Info, None, 2),
            ("session.end", Severity::Info, Some("stopped"), 2),
            ("session.start", Severity::Info, None, 3),
            ("session.end_abnormal", Severity::Error, Some("signal"), 1),
            ("session.start", Severity::Info, None, 4),
            ("session.end_abnormal", Severity::Error, Some("reason"), 2),
            ("session.end_abnormal", Severity::Error, Some("watchdog"), 3),
            ("session.end_abnormal", Severity::Error, Some("status"), 4),
            ("session.end_abnormal", Severity::Error, Some("unknown"), 5),
        ]
    );
    for r in &records {
        assert_eq!(r.schema, SCHEMA);
        assert_eq!(r.component, "cupertino-session");
        assert_eq!(r.source, "session.log");
    }
    // local time without a zone, read with the zone of the machine
    assert_eq!(records[0].time, time::format(t(0)));
    assert_eq!(records[1].time, time::format(t(600)));
    assert_eq!(records[2].fields.get("limit_seconds"), Some(&30.into()));
    let end = records[1];
    assert_eq!(end.fields.get("status"), Some(&0.into()));
    assert_eq!(end.fields.get("seconds"), Some(&600.into()));
    assert_eq!(records[3].fields.get("status"), Some(&143.into()));
    assert_eq!(records[5].fields.get("signal"), Some(&11.into()));
    assert_eq!(records[5].fields.get("status"), Some(&139.into()));
    let reason = records[7].fields.get("reason").unwrap().as_str().unwrap();
    assert!(
        !reason.contains(&home) && !reason.contains("alice"),
        "{reason}"
    );
    assert!(reason.starts_with("cannot open ~/"), "{reason}");
    assert_eq!(records[7].redacted, ["reason"]);
    assert_eq!(
        records[10].fields.get("text"),
        Some(&"the compositor caught fire".into())
    );

    // they are made when read, never written to the store
    assert!(!fixture.store_dir().exists());
}

#[test]
fn local_times_round_trip_through_the_machine_zone() {
    for s in [0, 3600 * 7, 86_400 * 100, 86_400 * 200 + 1234] {
        let when = t(s);
        assert_eq!(events::local_to_utc(&events::local_time(when)), Some(when));
    }
    for bad in [
        "",
        "2026-11-09",
        "2026-11-09T14:00:00",
        "2026-13-09 14:00:00",
        "2026-11-09 25:00:00",
    ] {
        assert_eq!(events::local_to_utc(bad), None, "{bad}");
    }
}

#[test]
fn a_follow_gives_new_store_and_session_lines_only() {
    let fixture = Fixture::new();
    fixture.append(&[record(t(0), "cupertino-dock", "dock.old", Severity::Info)]);
    fixture.session_log(&format!("{} starting\n", events::local_time(t(1))));
    let sources = fixture.sources();
    let (report, mut follow) = events::start(
        &sources,
        &Filter {
            severity: Some(Severity::Info),
            ..Filter::default()
        },
    );
    assert_eq!(ids(&report), ["dock.old", "session.start"]);
    assert!(follow.poll().is_empty(), "nothing new yet");

    fixture.append(&[record(t(10), "cupertino-dock", "dock.new", Severity::Warn)]);
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(fixture.state().join("session.log"))
        .unwrap();
    writeln!(
        log,
        "{} the compositor ended normally after 9 s",
        events::local_time(t(11))
    )
    .unwrap();
    // a line still being written waits for its end
    write!(log, "{} the compositor ended", events::local_time(t(12))).unwrap();
    let new: Vec<(String, &str)> = follow
        .poll()
        .into_iter()
        .map(|e| (e.record.id, e.source))
        .collect();
    assert_eq!(
        new,
        [
            ("dock.new".to_owned(), "store"),
            ("session.end".to_owned(), "session.log")
        ]
    );
    writeln!(log, " normally after 1 s").unwrap();
    let ended = follow.poll();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].record.count, 2, "numbered on from the first read");

    // a store cleared and written again: the new segment is read from its start
    fixture.store().clear().unwrap();
    assert!(follow.poll().is_empty());
    fixture.append(&[record(
        t(20),
        "cupertino-dock",
        "dock.after_clear",
        Severity::Info,
    )]);
    let after: Vec<String> = follow.poll().into_iter().map(|e| e.record.id).collect();
    assert_eq!(after, ["dock.after_clear"]);
}

fn recent(fixture: &Fixture) -> Check {
    let checks = run(&fixture.sources(), &[Area::Events]).checks;
    assert_eq!(checks.len(), 1);
    checks.into_iter().next().unwrap()
}

fn facts<'a>(check: &'a Check, key: &str) -> Vec<&'a str> {
    check
        .facts
        .iter()
        .filter(|f| f.key == key)
        .map(|f| f.value.as_str())
        .collect()
}

#[test]
fn events_recent_warns_with_counts_per_id_for_errors_of_the_last_day() {
    let now = SystemTime::now();
    let ago = |h: u64| now - Duration::from_secs(h * 3600);
    let fixture = Fixture::new().journal(journal_ok(journal_lines(&[record(
        ago(3),
        "cupertino-netd",
        "netd.daemon.exit_error",
        Severity::Error,
    )])));
    fixture.append(&[
        record(ago(30), "cupertino-server", "server.panic", Severity::Fatal),
        record(
            ago(5),
            "cupertino-server",
            "server.gpu.reset",
            Severity::Error,
        ),
        record(
            ago(2),
            "cupertino-server",
            "server.gpu.reset",
            Severity::Error,
        ),
        record(ago(1), "cupertino-server", "server.panic", Severity::Fatal),
        record(
            ago(1),
            "cupertino-dock",
            "dock.icon.load_failed",
            Severity::Warn,
        ),
    ]);
    let check = recent(&fixture);
    assert_eq!(
        (check.id, check.area, check.status),
        ("events.recent", Area::Events, Status::Warn)
    );
    assert_eq!(check.summary, "3 error and 1 fatal events in the last 24 h");
    assert_eq!(facts(&check, "error"), ["3"]);
    assert_eq!(facts(&check, "fatal"), ["1"]);
    assert_eq!(
        facts(&check, "event"),
        [
            format!(
                "server.panic: 1 fatal, the last at {}",
                time::format(ago(1))
            ),
            format!(
                "netd.daemon.exit_error: 1 error, the last at {}",
                time::format(ago(3))
            ),
            format!(
                "server.gpu.reset: 2 error, the last at {}",
                time::format(ago(2))
            ),
        ]
    );
    assert_eq!(
        facts(&check, "source"),
        ["store: ok", "session.log: unavailable", "journal: ok"]
    );
    assert!(check
        .fix
        .as_deref()
        .unwrap()
        .contains("`cupertino diagnose events --since 24h`"));
    let json = serde_json::to_value(run(&fixture.sources(), &[Area::Events])).unwrap();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["checks"][0]["area"], "events");
}

#[test]
fn events_recent_is_ok_without_errors_and_skips_when_nothing_can_be_read() {
    let now = SystemTime::now();
    let fixture = Fixture::new();
    fixture.append(&[
        record(
            now - Duration::from_secs(3600),
            "cupertino-dock",
            "dock.a",
            Severity::Warn,
        ),
        // an error older than a day does not count
        record(
            now - Duration::from_secs(25 * 3600),
            "cupertino-dock",
            "dock.b",
            Severity::Error,
        ),
    ]);
    let ok = recent(&fixture);
    assert_eq!(ok.status, Status::Ok);
    assert_eq!(
        ok.summary,
        "no error or fatal event in the last 24 h (1 events)"
    );
    assert_eq!(ok.fix, None);

    // an abnormal end in session.log counts like any error
    fixture.session_log(&format!(
        "{} the compositor ended with status 1 after 2 s; see server.log\n",
        events::local_time(now - Duration::from_secs(60))
    ));
    let warned = recent(&fixture);
    assert_eq!(warned.status, Status::Warn);
    assert_eq!(facts(&warned, "event").len(), 1);
    assert!(facts(&warned, "event")[0].starts_with("session.end_abnormal: 1 error"));

    let nothing = Fixture::new();
    let skip = recent(&nothing);
    assert_eq!(skip.status, Status::Skip);
    assert_eq!(skip.summary, "no diagnostic events can be read");
    assert_eq!(
        facts(&skip, "source"),
        [
            "store: unavailable",
            "session.log: unavailable",
            "journal: unavailable"
        ]
    );

    let denied = Fixture::new().journal(JournalResult::Output(JournalOutput {
        reader: Some(false),
        ..JournalOutput::default()
    }));
    let skip = recent(&denied);
    assert_eq!(skip.status, Status::Skip);
    assert_eq!(facts(&skip, "source")[2], "journal: denied");
}

#[test]
fn every_area_still_runs_with_the_events_area_last() {
    let names: Vec<&str> = Area::ALL.iter().map(|a| a.name()).collect();
    assert_eq!(
        names,
        ["gpu", "session", "portals", "services", "crash", "logs", "events"]
    );
    assert_eq!(Area::from_name("events"), Some(Area::Events));
}
