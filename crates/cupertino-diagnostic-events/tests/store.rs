//! The bounded store: rotation, the total cap, age expiry, many writers, the unavailable store and its recovery,
//! and reading back what is broken.

mod support;

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use cupertino_diagnostic_events::store::{self, Limits, Segment, Store, Unavailable};
use cupertino_diagnostic_events::{self as diag, Severity, Value};

/// A record line of `len` bytes (newline included) for writer `writer`, number `n`.
fn line(writer: u32, n: u64, len: usize) -> Vec<u8> {
    let head = format!(
        "{{\"schema\":\"cupertino.diag.v1\",\"time\":\"2026-11-09T00:00:00.000Z\",\"boot_id\":null,\"session\":null,\
         \"component\":\"test\",\"version\":\"0\",\"commit\":null,\"pid\":{writer},\"id\":\"test.store.line\",\
         \"severity\":\"info\",\"message\":\"m\",\"fields\":{{\"pad\":\""
    );
    let tail = format!("\"}},\"source\":\"t\",\"redacted\":[],\"count\":{n},\"suppressed\":0}}\n");
    let pad = len
        .checked_sub(head.len() + tail.len())
        .expect("a longer line");
    format!("{head}{}{tail}", "p".repeat(pad)).into_bytes()
}

fn limits(segment: u64, total: u64) -> Limits {
    Limits {
        segment_bytes: segment,
        store_bytes: total,
        ..Limits::default()
    }
}

fn names(dir: &Path) -> Vec<(String, u64)> {
    store::segments(dir)
        .unwrap()
        .into_iter()
        .map(|s| {
            (
                s.path.file_name().unwrap().to_string_lossy().into_owned(),
                s.len,
            )
        })
        .collect()
}

#[test]
fn the_store_directory_and_its_files_are_private() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("state/cupertino/events");
    let store = Store::open(&dir, Limits::default()).unwrap();
    store.append(&line(1, 1, 300)).unwrap();
    store.salt().unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    for entry in fs::read_dir(&dir).unwrap() {
        assert_eq!(mode(&entry.unwrap().path()), 0o600);
    }
    assert_eq!(
        store::user_dir(|k| (k == "HOME").then(|| "/h".into())),
        Some("/h/.local/state/cupertino/events".into())
    );
    assert_eq!(
        store::user_dir(|k| match k {
            "HOME" => Some("/h".into()),
            "XDG_STATE_HOME" => Some("/s".into()),
            _ => None,
        }),
        Some("/s/cupertino/events".into())
    );
    assert_eq!(store::user_dir(|_| None), None);
}

#[test]
fn a_segment_is_closed_when_the_next_line_would_pass_its_size() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), limits(3000, 100_000))
        .unwrap()
        .with_boot("0f1e2d3c-4b5a");
    for n in 0..10 {
        store.append(&line(1, n, 300)).unwrap();
    }
    assert_eq!(
        names(tmp.path()),
        [("events-0f1e2d3c-00000001.jsonl".to_owned(), 3000)]
    );
    store.append(&line(1, 10, 300)).unwrap();
    assert_eq!(
        names(tmp.path()),
        [
            ("events-0f1e2d3c-00000001.jsonl".to_owned(), 3000),
            ("events-0f1e2d3c-00000002.jsonl".to_owned(), 300)
        ]
    );
    // a line longer than a segment still goes in, alone in a fresh one
    store.append(&line(1, 11, 4500)).unwrap();
    store.append(&line(1, 12, 300)).unwrap();
    let sizes: Vec<u64> = names(tmp.path()).into_iter().map(|(_, len)| len).collect();
    assert_eq!(sizes, [3000, 300, 4500, 300]);
    // a later boot goes on numbering
    let next = Store::open(tmp.path(), limits(3000, 100_000))
        .unwrap()
        .with_boot("aaaabbbb");
    next.append(&line(1, 13, 3000)).unwrap();
    assert_eq!(
        names(tmp.path()).last().unwrap().0,
        "events-aaaabbbb-00000005.jsonl"
    );
}

#[test]
fn the_oldest_segments_go_while_the_store_is_over_its_total() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), limits(3000, 9000)).unwrap();
    let mut deleted = 0;
    for n in 0..100 {
        deleted += store.append(&line(1, n, 300)).unwrap().deleted;
        let total: u64 = names(tmp.path()).iter().map(|(_, l)| l).sum();
        assert!(total <= 9000, "{total} after {n}");
    }
    assert_eq!(deleted, 7);
    let contents = store::read(tmp.path()).unwrap();
    let counts: Vec<u64> = contents.records.iter().map(|r| r.record.count).collect();
    assert_eq!(counts, (70..100).collect::<Vec<_>>());
}

/// The clock of [`segments_older_than_the_retention_go`]: fifteen days from now.
fn fifteen_days_later() -> SystemTime {
    SystemTime::now() + Duration::from_secs(15 * 86_400)
}

#[test]
fn segments_older_than_the_retention_go() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), limits(600, 100_000)).unwrap();
    for n in 0..4 {
        store.append(&line(1, n, 450)).unwrap();
    }
    assert_eq!(names(tmp.path()).len(), 4);
    // one segment was last written thirteen days ago, the others now: thirteen plus fifteen is past fourteen for all
    let first = store::segments(tmp.path()).unwrap()[0].path.clone();
    File::options()
        .write(true)
        .open(&first)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(13 * 86_400))
        .unwrap();
    let store = store.with_clock(fifteen_days_later);
    let appended = store.append(&line(1, 4, 450)).unwrap();
    // every older segment expired; the one written now stays
    assert_eq!(appended.deleted, 4);
    assert_eq!(names(tmp.path()).len(), 1);
    // with the real clock, a thirteen-day-old segment stays
    let store = Store::open(tmp.path(), limits(600, 100_000)).unwrap();
    store.append(&line(1, 5, 450)).unwrap();
    let old = store::segments(tmp.path()).unwrap()[0].path.clone();
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(13 * 86_400))
        .unwrap();
    assert_eq!(store.append(&line(1, 6, 450)).unwrap().deleted, 0);
    File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(14 * 86_400 + 60))
        .unwrap();
    assert_eq!(store.append(&line(1, 7, 450)).unwrap().deleted, 1);
}

const WRITERS_DIR: &str = "DIAG_TEST_WRITERS_DIR";
const WRITER_ID: &str = "DIAG_TEST_WRITER_ID";
const PER_WRITER: u64 = 10_000;

fn write_many(dir: &Path, writer: u32) {
    let store = Store::open(dir, Limits::default()).unwrap();
    for n in 0..PER_WRITER {
        // lengths vary, so lines of different writers cross segment ends at different places
        let len = 300 + (n as usize * 7 + writer as usize * 13) % 200;
        store.append(&line(writer, n, len)).unwrap();
    }
}

#[test]
fn concurrent_writer_child() {
    if let (Some(dir), Some(id)) = (std::env::var_os(WRITERS_DIR), std::env::var_os(WRITER_ID)) {
        write_many(Path::new(&dir), id.to_str().unwrap().parse().unwrap());
    }
}

#[test]
fn eight_threads_and_four_processes_write_whole_lines_within_the_cap() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("events");
    Store::open(&dir, Limits::default()).unwrap();
    let children: Vec<_> = (100..104)
        .map(|id| {
            std::process::Command::new(std::env::current_exe().unwrap())
                .env("CUPERTINO_ASSET_SEARCH_PATH", "")
                .args(["--exact", "concurrent_writer_child", "--test-threads", "1"])
                .env(WRITERS_DIR, &dir)
                .env(WRITER_ID, id.to_string())
                .spawn()
                .unwrap()
        })
        .collect();
    std::thread::scope(|scope| {
        for writer in 0..8 {
            let dir = &dir;
            scope.spawn(move || write_many(dir, writer));
        }
    });
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let segments = store::segments(&dir).unwrap();
    let total: u64 = segments.iter().map(|s| s.len).sum();
    assert!(total <= diag::STORE_BYTES, "{total}");
    assert!(segments.iter().all(|s| s.len <= diag::SEGMENT_BYTES));
    let contents = store::read(&dir).unwrap();
    assert_eq!((contents.corrupt, contents.partial), (0, 0));
    // deletion takes whole segments, oldest first: what is left of each writer is its last events, without a gap
    let mut per_writer: std::collections::BTreeMap<u32, Vec<u64>> = Default::default();
    for read in &contents.records {
        per_writer
            .entry(read.record.pid)
            .or_default()
            .push(read.record.count);
    }
    assert_eq!(per_writer.len(), 12, "{:?}", per_writer.keys());
    let mut kept = 0;
    for (writer, mut counts) in per_writer {
        counts.sort_unstable();
        let first = counts[0];
        assert_eq!(
            counts,
            (first..PER_WRITER).collect::<Vec<_>>(),
            "writer {writer}"
        );
        kept += counts.len();
    }
    // the cap keeps at least what fits in seven full segments of the longest lines
    assert!(
        kept as u64 * 500 >= diag::STORE_BYTES - diag::SEGMENT_BYTES,
        "{kept}"
    );
}

static FAIL: AtomicBool = AtomicBool::new(false);

/// Writes half the line, then fails as a full disk does, while [`FAIL`] is set.
fn full_disk(file: &File, line: &[u8]) -> io::Result<usize> {
    if FAIL.load(Ordering::SeqCst) {
        let mut f = file;
        f.write_all(&line[..line.len() / 2])?;
        return Err(io::Error::from_raw_os_error(28));
    }
    let mut f = file;
    f.write(line)
}

#[test]
fn a_full_disk_leaves_no_half_line_and_the_store_works_again_after() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), Limits::default())
        .unwrap()
        .with_writer(full_disk);
    store.append(&line(1, 0, 300)).unwrap();
    FAIL.store(true, Ordering::SeqCst);
    assert_eq!(store.append(&line(1, 1, 300)), Err(Unavailable::NoSpace));
    FAIL.store(false, Ordering::SeqCst);
    store.append(&line(1, 2, 300)).unwrap();
    let contents = store::read(tmp.path()).unwrap();
    assert_eq!((contents.corrupt, contents.partial), (0, 0));
    let counts: Vec<u64> = contents.records.iter().map(|r| r.record.count).collect();
    assert_eq!(counts, [0, 2]);
}

fn running_as_root() -> bool {
    fs::metadata("/proc/self")
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0)
        .unwrap_or(false)
}

#[test]
fn a_read_only_directory_is_unavailable_until_it_is_writable_again() {
    if running_as_root() {
        // root writes a read-only directory anyway; the other refusals are covered below
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), Limits::default()).unwrap();
    store.append(&line(1, 0, 300)).unwrap();
    // the segment exists: rotation into a new one needs the directory
    let small = Store::open(tmp.path(), limits(300, 100_000)).unwrap();
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o500)).unwrap();
    assert_eq!(small.append(&line(1, 1, 300)), Err(Unavailable::Denied));
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    small.append(&line(1, 1, 300)).unwrap();
    assert_eq!(store::read(tmp.path()).unwrap().records.len(), 2);
}

#[test]
fn a_directory_that_is_a_link_or_writable_by_others_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let link = tmp.path().join("link");
    symlink(&real, &link).unwrap();
    assert_eq!(
        Store::open(&link, Limits::default()).err(),
        Some(Unavailable::Symlink)
    );
    fs::set_permissions(&real, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        Store::open(&real, Limits::default()).err(),
        Some(Unavailable::Writable)
    );
    fs::set_permissions(&real, fs::Permissions::from_mode(0o770)).unwrap();
    assert_eq!(
        Store::open(&real, Limits::default()).err(),
        Some(Unavailable::Writable)
    );
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    let store = Store::open(&real, Limits::default()).unwrap();
    store.append(&line(1, 0, 300)).unwrap();
    // checked at every write, not only at the open
    fs::set_permissions(&real, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(store.append(&line(1, 1, 300)), Err(Unavailable::Writable));
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).unwrap();
    store.append(&line(1, 2, 300)).unwrap();
    let file = tmp.path().join("file");
    fs::write(&file, "").unwrap();
    assert_eq!(
        Store::open(&file, Limits::default()).err(),
        Some(Unavailable::NotADirectory)
    );
}

#[test]
fn a_directory_of_another_user_is_refused() {
    if !running_as_root() {
        // only root may give a directory away (CAP_CHOWN); the link and mode refusals run for everyone
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("events");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::chown(&dir, Some(65534), Some(65534)).unwrap();
    assert_eq!(
        Store::open(&dir, Limits::default()).err(),
        Some(Unavailable::NotOwned)
    );
}

#[test]
fn a_broken_segment_is_read_around_with_counts() {
    let tmp = tempfile::tempdir().unwrap();
    let good = line(1, 1, 300);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"garbage that is no json\n");
    bytes.extend_from_slice(&good);
    bytes.extend_from_slice(b"{\"schema\":\"cupertino.diag.v0\",\"id\":\"x\"}\n");
    bytes.extend_from_slice(b"\xff\xfe not utf-8 \xc3\n");
    bytes.extend_from_slice(&line(1, 2, 300));
    bytes.extend_from_slice(&good[..120]);
    fs::write(tmp.path().join("events-0f1e2d3c-00000001.jsonl"), &bytes).unwrap();
    // a file of another name is not a segment
    fs::write(tmp.path().join("notes.txt"), "x").unwrap();
    let contents = store::read(tmp.path()).unwrap();
    assert_eq!(contents.records.len(), 2);
    assert_eq!(contents.corrupt, 3);
    assert_eq!(contents.partial, 1);
    // a segment removed between the listing and the read
    let gone = Segment {
        path: tmp.path().join("events-0f1e2d3c-00000002.jsonl"),
        seq: Some(2),
        len: 10,
        modified: SystemTime::now(),
    };
    let contents = store::read_segments(&[gone]).unwrap();
    assert_eq!((contents.records.len(), contents.vanished), (0, 1));
}

#[test]
fn clearing_deletes_the_segments_and_keeps_the_salt() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), limits(400, 100_000)).unwrap();
    for n in 0..5 {
        store.append(&line(1, n, 300)).unwrap();
    }
    let salt = store.salt().unwrap();
    assert_eq!(store.clear().unwrap(), 5);
    assert!(store::read(tmp.path()).unwrap().records.is_empty());
    assert_eq!(store.salt().unwrap(), salt);
    assert!(tmp.path().join("lock").exists());
}

#[test]
fn an_unavailable_store_keeps_events_in_memory_and_says_so_once_it_works_again() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let dir = state.path().join("cupertino/events");
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        let out = support::child(
            "an_unavailable_store_keeps_events_in_memory_and_says_so_once_it_works_again",
            state.path(),
            &[],
        );
        let stderr = support::stderr(&out);
        assert!(out.status.success(), "{stderr}");
        let warnings: Vec<&str> = stderr
            .lines()
            .filter(|l| l.contains("kept in memory only"))
            .collect();
        assert_eq!(
            warnings,
            ["cupertino-diagnostic-events: warning: diagnostic events are kept in memory only: others may write the store directory"],
            "{stderr}"
        );
        let contents = store::read(&dir).unwrap();
        let ids: Vec<&str> = contents
            .records
            .iter()
            .map(|r| r.record.id.as_str())
            .collect();
        assert_eq!(ids, ["test.store.after", "diag.store.unavailable"]);
        let back = &contents.records[1].record;
        assert_eq!(back.fields.get("reason").unwrap(), "writable_by_others");
        assert_eq!(back.fields.get("kept_in_memory").unwrap(), 3);
        return;
    }
    let dir = Path::new(&std::env::var_os("XDG_STATE_HOME").unwrap()).join("cupertino/events");
    let emitter = diag::init(diag::component!(), "test", diag::Sink::UserStore);
    for n in 0..3 {
        diag::event(Severity::Warn, "test.store.before", "while unavailable")
            .field("n", Value::Count(n))
            .emit();
    }
    assert_eq!(emitter.recent().len(), 3);
    assert_eq!(emitter.store_unavailable(), Some(Unavailable::Writable));
    assert!(store::read(&dir).unwrap().records.is_empty());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    diag::event(Severity::Info, "test.store.after", "available again").emit();
    assert_eq!(emitter.store_unavailable(), None);
    assert_eq!(emitter.recent().len(), 5);
}

#[test]
fn the_memory_ring_keeps_the_last_events_within_its_limits() {
    if !support::is_child() {
        let state = tempfile::tempdir().unwrap();
        let out = support::child(
            "the_memory_ring_keeps_the_last_events_within_its_limits",
            state.path(),
            &[],
        );
        assert!(out.status.success(), "{}", support::stderr(&out));
        return;
    }
    let emitter = diag::init(diag::component!(), "test", diag::Sink::Memory);
    // distinct ids, so the rate limit lets each through
    const IDS: [&str; 4] = ["test.ring.a", "test.ring.b", "test.ring.c", "test.ring.d"];
    for n in 0..300u64 {
        let id = IDS[(n % 4) as usize];
        // the limit is per id per window; a fresh window is not waited for, so only ten of each go in
        diag::event(Severity::Info, id, "ring")
            .field("n", Value::Count(n))
            .emit();
    }
    assert_eq!(emitter.recent().len(), 40);
    // past the event count: distinct ids
    let names: Vec<&'static str> = (0..300)
        .map(|i| &*Box::leak(format!("test.ring.n{i}").into_boxed_str()))
        .collect();
    for id in &names {
        diag::event(Severity::Info, id, "ring").emit();
    }
    let recent = emitter.recent();
    assert_eq!(recent.len(), diag::RING_EVENTS);
    assert!(recent.last().unwrap().contains("\"test.ring.n299\""));
    // past the byte count: big events
    let big = "y".repeat(3000);
    for id in &names[..200] {
        diag::event(Severity::Info, id, "big")
            .field("big", Value::Public(&big))
            .emit();
    }
    let recent = emitter.recent();
    let bytes: usize = recent.iter().map(String::len).sum();
    assert!(
        bytes <= diag::RING_BYTES && recent.len() < diag::RING_EVENTS,
        "{bytes} {}",
        recent.len()
    );
}

#[test]
fn a_write_to_an_open_lock_file_does_not_block_another_store() {
    // two stores on one directory in one process: each holds its own open file description of the lock
    let tmp = tempfile::tempdir().unwrap();
    let a = Store::open(tmp.path(), Limits::default()).unwrap();
    let b = Store::open(tmp.path(), Limits::default()).unwrap();
    a.append(&line(1, 0, 300)).unwrap();
    b.append(&line(2, 0, 300)).unwrap();
    assert_eq!(store::read(tmp.path()).unwrap().records.len(), 2);
}
