//! The bounded store: where it is and who may read it, rotation, the total and age limits, many writers, the
//! directories it refuses, a failed write, reading back what is broken, and clearing.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use cupertino_diagnostic_events::store::{self, Limits, Segment, Store, Unavailable};
use cupertino_diagnostic_events::{SEGMENT_BYTES, STORE_BYTES};

/// A record line of `len` bytes, newline included, of process `pid` with count `n`.
fn line(pid: u32, n: u64, len: usize) -> Vec<u8> {
    let head = format!(
        "{{\"schema\":\"cupertino.diag.v1\",\"time\":\"2026-11-09T00:00:00.000Z\",\"boot_id\":null,\"session\":null,\
         \"component\":\"test\",\"version\":\"0\",\"commit\":null,\"pid\":{pid},\"id\":\"test.store.line\",\
         \"severity\":\"info\",\"message\":\"m\",\"fields\":{{\"pad\":\""
    );
    let tail = format!("\"}},\"source\":\"t\",\"redacted\":[],\"count\":{n},\"suppressed\":0}}\n");
    let pad = len
        .checked_sub(head.len() + tail.len())
        .expect("a longer line");
    format!("{head}{}{tail}", "p".repeat(pad)).into_bytes()
}

fn limits(segment_bytes: u64, store_bytes: u64) -> Limits {
    Limits {
        segment_bytes,
        store_bytes,
        ..Limits::default()
    }
}

/// The segments' file names and lengths, in the store's order.
fn listing(dir: &Path) -> Vec<(String, u64)> {
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

/// The `(pid, count)` of every record the store reads back.
fn read_back(dir: &Path) -> Vec<(u32, u64)> {
    store::read(dir)
        .unwrap()
        .records
        .iter()
        .map(|r| (r.record.pid, r.record.count))
        .collect()
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn set_modified(path: &Path, time: SystemTime) {
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn running_as_root() -> bool {
    fs::metadata("/proc/self")
        .map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0)
        .unwrap_or(false)
}

#[test]
fn the_user_store_is_under_the_state_home_and_only_its_owner_may_read_it() {
    let var = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| std::ffi::OsString::from(v))
        }
    };
    for (env, dir) in [
        (
            &[("XDG_STATE_HOME", "/state"), ("HOME", "/home/u")][..],
            Some("/state/cupertino/events"),
        ),
        (
            &[("HOME", "/home/u")][..],
            Some("/home/u/.local/state/cupertino/events"),
        ),
        (&[][..], None),
    ] {
        assert_eq!(store::user_dir(var(env)), dir.map(PathBuf::from), "{env:?}");
    }
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("state/cupertino/events");
    let store = Store::open(&dir, Limits::default()).unwrap();
    store.append(&line(1, 1, 300)).unwrap();
    store.salt().unwrap();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        assert_eq!(mode(&path), 0o600, "{}", path.display());
    }
}

#[test]
fn a_segment_closes_before_the_line_that_would_pass_its_size_and_numbers_go_on_across_boots() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), limits(3000, 100_000))
        .unwrap()
        .with_boot("0f1e2d3c-4b5a");
    for n in 0..10 {
        store.append(&line(1, n, 300)).unwrap();
    }
    assert_eq!(
        listing(tmp.path()),
        [("events-0f1e2d3c-00000001.jsonl".to_owned(), 3000)]
    );
    store.append(&line(1, 10, 300)).unwrap();
    // a line longer than a segment goes in alone
    store.append(&line(1, 11, 4500)).unwrap();
    store.append(&line(1, 12, 300)).unwrap();
    let sizes: Vec<u64> = listing(tmp.path())
        .into_iter()
        .map(|(_, len)| len)
        .collect();
    assert_eq!(sizes, [3000, 300, 4500, 300]);
    let later_boot = Store::open(tmp.path(), limits(3000, 100_000))
        .unwrap()
        .with_boot("aaaabbbb");
    later_boot.append(&line(1, 13, 3000)).unwrap();
    assert_eq!(
        listing(tmp.path()).last().unwrap().0,
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
        let total: u64 = listing(tmp.path()).iter().map(|(_, len)| len).sum();
        assert!(total <= 9000, "{total} after line {n}");
    }
    assert_eq!(deleted, 7);
    let counts: Vec<u64> = read_back(tmp.path()).into_iter().map(|(_, n)| n).collect();
    assert_eq!(counts, (70..100).collect::<Vec<_>>());
}

/// A clock fifteen days ahead.
fn fifteen_days_later() -> SystemTime {
    SystemTime::now() + Duration::from_secs(15 * 86_400)
}

#[test]
fn segments_last_written_before_the_retention_go() {
    let day = Duration::from_secs(86_400);
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), limits(600, 100_000)).unwrap();
    for n in 0..4 {
        store.append(&line(1, n, 450)).unwrap();
    }
    // fifteen days on, every segment is past fourteen days old but the one just written
    let first = store::segments(tmp.path()).unwrap()[0].path.clone();
    set_modified(&first, SystemTime::now() - 13 * day);
    let ahead = store.with_clock(fifteen_days_later);
    assert_eq!(ahead.append(&line(1, 4, 450)).unwrap().deleted, 4);
    assert_eq!(listing(tmp.path()).len(), 1);
    // with the real clock: thirteen days stays, fourteen and a minute goes
    let store = Store::open(tmp.path(), limits(600, 100_000)).unwrap();
    store.append(&line(1, 5, 450)).unwrap();
    let old = store::segments(tmp.path()).unwrap()[0].path.clone();
    for (age, deleted) in [(13 * day, 0), (14 * day + Duration::from_secs(60), 1)] {
        set_modified(&old, SystemTime::now() - age);
        assert_eq!(
            store.append(&line(1, 6, 450)).unwrap().deleted,
            deleted,
            "{age:?}"
        );
    }
}

const PER_WRITER: u64 = 10_000;

#[test]
fn eight_writers_with_stores_of_their_own_write_whole_lines_within_the_limits() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("events");
    std::thread::scope(|scope| {
        for writer in 0..8u32 {
            let dir = &dir;
            scope.spawn(move || {
                // each its own store, so each holds its own description of the lock file
                let store = Store::open(dir, Limits::default()).unwrap();
                for n in 0..PER_WRITER {
                    // lengths vary, so writers cross segment ends at different places
                    let len = 300 + (n as usize * 7 + writer as usize * 13) % 200;
                    store.append(&line(writer, n, len)).unwrap();
                }
            });
        }
    });
    let segments = store::segments(&dir).unwrap();
    assert!(segments.iter().map(|s| s.len).sum::<u64>() <= STORE_BYTES);
    assert!(segments.iter().all(|s| s.len <= SEGMENT_BYTES));
    let contents = store::read(&dir).unwrap();
    assert_eq!((contents.corrupt, contents.partial), (0, 0));
    // whole segments go, oldest first: what is left of each writer is its last lines without a gap
    let mut per_writer: std::collections::BTreeMap<u32, Vec<u64>> = Default::default();
    for (pid, n) in read_back(&dir) {
        per_writer.entry(pid).or_default().push(n);
    }
    assert_eq!(per_writer.len(), 8);
    let mut kept = 0;
    for (writer, mut counts) in per_writer {
        counts.sort_unstable();
        assert_eq!(
            counts,
            (counts[0]..PER_WRITER).collect::<Vec<_>>(),
            "writer {writer}"
        );
        kept += counts.len() as u64;
    }
    // at least seven full segments of the longest lines stay
    assert!(kept * 500 >= STORE_BYTES - SEGMENT_BYTES, "{kept}");
}

static DISK_FULL: AtomicBool = AtomicBool::new(false);

/// While [`DISK_FULL`] is set, writes half the line and fails as a full disk does.
fn filling_disk(file: &File, line: &[u8]) -> io::Result<usize> {
    let mut file = file;
    if DISK_FULL.load(Ordering::SeqCst) {
        file.write_all(&line[..line.len() / 2])?;
        return Err(io::ErrorKind::StorageFull.into());
    }
    file.write(line)
}

#[test]
fn a_full_disk_is_unavailable_leaves_no_half_line_and_the_next_write_works() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), Limits::default())
        .unwrap()
        .with_writer(filling_disk);
    store.append(&line(1, 0, 300)).unwrap();
    DISK_FULL.store(true, Ordering::SeqCst);
    assert_eq!(store.append(&line(1, 1, 300)), Err(Unavailable::NoSpace));
    DISK_FULL.store(false, Ordering::SeqCst);
    store.append(&line(1, 2, 300)).unwrap();
    let contents = store::read(tmp.path()).unwrap();
    assert_eq!((contents.corrupt, contents.partial), (0, 0));
    assert_eq!(read_back(tmp.path()), [(1, 0), (1, 2)]);
}

#[test]
fn a_read_only_directory_is_unavailable_until_it_is_writable_again() {
    if running_as_root() {
        // root writes a read-only directory anyway
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    // segments of one line: the second line needs a new file in the directory
    let store = Store::open(tmp.path(), limits(300, 100_000)).unwrap();
    store.append(&line(1, 0, 300)).unwrap();
    set_mode(tmp.path(), 0o500);
    assert_eq!(store.append(&line(1, 1, 300)), Err(Unavailable::Denied));
    set_mode(tmp.path(), 0o700);
    store.append(&line(1, 1, 300)).unwrap();
    assert_eq!(read_back(tmp.path()), [(1, 0), (1, 1)]);
}

#[test]
fn a_directory_others_could_tamper_with_is_refused_at_the_open_and_at_every_write() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real");
    fs::create_dir(&real).unwrap();
    let link = tmp.path().join("link");
    symlink(&real, &link).unwrap();
    let file = tmp.path().join("file");
    fs::write(&file, "").unwrap();
    for (dir, mode, refusal) in [
        (&link, 0o700, Unavailable::Symlink),
        (&real, 0o777, Unavailable::Writable),
        (&real, 0o770, Unavailable::Writable),
        (&file, 0o700, Unavailable::NotADirectory),
    ] {
        set_mode(&real, mode);
        assert_eq!(
            Store::open(dir, Limits::default()).err(),
            Some(refusal),
            "{} {mode:o}",
            dir.display()
        );
    }
    set_mode(&real, 0o700);
    let store = Store::open(&real, Limits::default()).unwrap();
    store.append(&line(1, 0, 300)).unwrap();
    set_mode(&real, 0o777);
    assert_eq!(store.append(&line(1, 1, 300)), Err(Unavailable::Writable));
    set_mode(&real, 0o700);
    store.append(&line(1, 2, 300)).unwrap();
    // only root may give a directory away
    if running_as_root() {
        let theirs = tmp.path().join("theirs");
        fs::create_dir(&theirs).unwrap();
        set_mode(&theirs, 0o700);
        std::os::unix::fs::chown(&theirs, Some(65534), Some(65534)).unwrap();
        assert_eq!(
            Store::open(&theirs, Limits::default()).err(),
            Some(Unavailable::NotOwned)
        );
    }
}

#[test]
fn reading_skips_and_counts_what_is_broken_cut_off_or_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let whole = line(1, 1, 300);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"garbage that is no json\n");
    bytes.extend_from_slice(&whole);
    bytes.extend_from_slice(b"{\"schema\":\"cupertino.diag.v0\",\"id\":\"x\"}\n");
    bytes.extend_from_slice(b"\xff\xfe not utf-8 \xc3\n");
    bytes.extend_from_slice(&line(1, 2, 300));
    bytes.extend_from_slice(&whole[..120]);
    fs::write(tmp.path().join("events-0f1e2d3c-00000001.jsonl"), &bytes).unwrap();
    // a file of another name is no segment
    fs::write(tmp.path().join("notes.txt"), "x").unwrap();
    let contents = store::read(tmp.path()).unwrap();
    assert_eq!(
        (contents.records.len(), contents.corrupt, contents.partial),
        (2, 3, 1)
    );
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
fn the_line_after_one_a_crash_cut_off_starts_a_line_of_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), Limits::default()).unwrap();
    store.append(&line(1, 1, 300)).unwrap();
    let segment = store::segments(tmp.path()).unwrap()[0].path.clone();
    // a process ended in the middle of its write
    fs::OpenOptions::new()
        .append(true)
        .open(&segment)
        .unwrap()
        .write_all(&line(1, 2, 300)[..100])
        .unwrap();
    store.append(&line(2, 1, 300)).unwrap();
    let contents = store::read(tmp.path()).unwrap();
    assert_eq!((contents.corrupt, contents.partial), (1, 0));
    assert_eq!(read_back(tmp.path()), [(1, 1), (2, 1)]);
}

#[test]
fn segments_are_listed_fatal_ones_first_then_by_number_whatever_their_times_say() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let make = |name: String, secs: u64| {
        let path = dir.join(name);
        fs::write(&path, b"").unwrap();
        set_modified(&path, SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
    };
    // a clock that went back between writes: numbers and times disagree
    for (seq, secs) in [(1, 300), (2, 100), (3, 200)] {
        make(format!("events-0f1e2d3c-{seq:08}.jsonl"), secs);
    }
    make("fatal-0f1e2d3c-77.jsonl".into(), 150);
    let names: Vec<String> = listing(dir).into_iter().map(|(name, _)| name).collect();
    assert_eq!(
        names,
        [
            "fatal-0f1e2d3c-77.jsonl",
            "events-0f1e2d3c-00000001.jsonl",
            "events-0f1e2d3c-00000002.jsonl",
            "events-0f1e2d3c-00000003.jsonl",
        ]
    );
    // many of both with scattered times still sort, by number (a sort given no consistent order may panic)
    for seq in 4..40u64 {
        make(
            format!("events-0f1e2d3c-{seq:08}.jsonl"),
            (seq * 7919) % 500,
        );
        make(format!("fatal-0f1e2d3c-{seq}.jsonl"), (seq * 104_729) % 500);
    }
    let all = store::segments(dir).unwrap();
    let seqs: Vec<u64> = all.iter().filter_map(|s| s.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    assert_eq!(all.len(), 4 + 36 * 2);
}

#[test]
fn clearing_deletes_every_segment_and_keeps_the_salt() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(tmp.path(), limits(400, 100_000)).unwrap();
    for n in 0..5 {
        store.append(&line(1, n, 300)).unwrap();
    }
    let salt = store.salt().unwrap();
    assert_eq!(store.clear().unwrap(), 5);
    assert!(read_back(tmp.path()).is_empty());
    assert_eq!(store.salt().unwrap(), salt);
}
