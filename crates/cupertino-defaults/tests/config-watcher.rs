//! `watch::ConfigWatcher`: change notification for the settings files.

use std::os::fd::{AsFd, AsRawFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver};
use std::time::{Duration, Instant};

use cupertino_defaults::atomic::write_atomic;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};

/// Long enough for a report to arrive on a loaded machine.
const REPORT: Duration = Duration::from_secs(10);
/// Long enough, many times over the watcher's coalescing time, for a report that should not come to have come.
const QUIET: Duration = Duration::from_millis(600);

/// The reports of a watch of `files` that a thread of the test's waits for.
fn watch(files: Vec<PathBuf>) -> Receiver<WatchEvent> {
    let (tx, rx) = channel();
    let mut watcher = ConfigWatcher::new(files).unwrap();
    std::thread::spawn(move || {
        while let Some(events) = watcher.wait() {
            for event in events {
                if tx.send(event).is_err() {
                    return;
                }
            }
        }
    });
    rx
}

fn changed(rx: &Receiver<WatchEvent>) -> Vec<PathBuf> {
    match rx.recv_timeout(REPORT).expect("a report") {
        WatchEvent::Changed(paths) => paths,
        WatchEvent::Error(e) => panic!("{e}"),
    }
}

fn nothing(rx: &Receiver<WatchEvent>, row: &str) {
    assert!(rx.recv_timeout(QUIET).is_err(), "no report: {row}");
}

#[test]
fn each_kind_of_change_to_a_watched_file_is_one_report() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    let rx = watch(vec![file.clone()]);
    let bursts: [(&str, &dyn Fn()); 4] = [
        ("created", &|| std::fs::write(&file, "start").unwrap()),
        ("replaced atomically", &|| {
            write_atomic(&file, b"two").unwrap()
        }),
        ("a burst of writes", &|| {
            for i in 0..10 {
                std::fs::write(&file, format!("v{i}")).unwrap();
            }
        }),
        ("removed", &|| std::fs::remove_file(&file).unwrap()),
    ];
    for (row, burst) in bursts {
        burst();
        assert_eq!(changed(&rx), std::slice::from_ref(&file), "{row}");
        nothing(&rx, row);
    }
}

#[test]
fn events_that_change_no_watched_file_are_not_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    std::fs::write(&file, "old").unwrap();
    std::fs::write(&file, "same").unwrap();
    let rx = watch(vec![file.clone()]);
    nothing(&rx, "a change before the watch began");
    let events: [(&str, &dyn Fn()); 4] = [
        ("unchanged contents", &|| {
            std::fs::write(&file, "same").unwrap()
        }),
        ("another file", &|| {
            std::fs::write(dir.path().join("other.conf"), "x").unwrap()
        }),
        ("a temporary file", &|| {
            std::fs::write(dir.path().join(".a.conf.1.0.tmp"), "x").unwrap()
        }),
        ("reads", &|| {
            for _ in 0..5 {
                let _ = std::fs::read(&file).unwrap();
            }
        }),
    ];
    for (row, event) in events {
        event();
        nothing(&rx, row);
    }
    std::fs::write(&file, "newest").unwrap();
    assert_eq!(changed(&rx), [file]);
}

#[test]
fn unrelated_activity_in_the_directory_does_not_hold_back_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cupertino/a.conf");
    let noise = dir.path().join("cupertino/other.conf");
    let rx = watch(vec![file.clone()]);
    let stop = AtomicBool::new(false);
    let (started_tx, started) = sync_channel(1);
    let received = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                std::fs::write(&noise, "unrelated").unwrap();
                std::fs::remove_file(&noise).unwrap();
                let _ = started_tx.try_send(());
            }
        });
        started.recv_timeout(REPORT).expect("the noise started");
        write_atomic(&file, b"x").unwrap();
        // the noise never pauses for as long as the coalescing time, so a report must not wait for quiet
        let received = rx.recv_timeout(QUIET);
        stop.store(true, Ordering::Relaxed);
        received
    });
    assert_eq!(received.ok(), Some(WatchEvent::Changed(vec![file])));
}

#[test]
fn several_files_changed_in_one_burst_are_all_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.conf"), dir.path().join("b.conf"));
    let rx = watch(vec![b.clone(), a.clone()]);
    std::fs::write(&a, "1").unwrap();
    std::fs::write(&b, "1").unwrap();
    let mut seen = changed(&rx);
    if seen.len() < 2 {
        seen.extend(changed(&rx));
    }
    assert_eq!(seen.len(), 2);
    assert!(seen.contains(&a) && seen.contains(&b));
}

#[test]
fn the_settings_directory_is_created_and_watched_again_when_recreated() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("redirected/cupertino");
    let file = settings.join("a.conf");
    let rx = watch(vec![file.clone()]);
    assert!(settings.is_dir(), "the settings directory is created");
    write_atomic(&file, b"1").unwrap();
    assert_eq!(changed(&rx), std::slice::from_ref(&file));
    std::fs::remove_dir_all(&settings).unwrap();
    assert_eq!(
        changed(&rx),
        std::slice::from_ref(&file),
        "the file went with the directory"
    );
    std::fs::create_dir(&settings).unwrap();
    write_atomic(&file, b"2").unwrap();
    assert_eq!(changed(&rx), [file]);
}

#[test]
fn a_fifo_at_a_watched_path_does_not_block_the_watch() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("a.conf");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // Safety: `c` is a valid C string for the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let other = dir.path().join("b.conf");
    let rx = watch(vec![fifo, other.clone()]);
    write_atomic(&other, b"x").unwrap();
    assert_eq!(changed(&rx), [other]);
}

#[test]
fn a_loop_that_polls_the_descriptor_reads_each_quiet_burst_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    let mut watcher = ConfigWatcher::new(vec![file.clone()]).unwrap();
    assert!(watcher.read(Instant::now()).is_empty(), "nothing changed");
    assert_eq!(watcher.quiet_at(), None, "no burst under way");
    std::fs::write(&file, "1").unwrap();
    let mut fd = libc::pollfd {
        fd: watcher.as_fd().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // Safety: `fd` is an initialised pollfd that outlives the call, and the count says it is one.
    let ready = unsafe { libc::poll(&mut fd, 1, REPORT.as_millis() as i32) };
    assert_eq!(ready, 1, "the descriptor is readable");
    let now = Instant::now();
    assert!(watcher.read(now).is_empty(), "the burst is not quiet yet");
    let quiet = watcher.quiet_at().expect("a burst under way");
    assert!(quiet > now);
    assert_eq!(
        watcher.read(quiet),
        [WatchEvent::Changed(vec![file.clone()])]
    );
    assert_eq!(watcher.quiet_at(), None);
    assert!(watcher.read(quiet).is_empty(), "reported once");
}

#[test]
fn a_loop_that_polls_futures_waits_for_a_quiet_burst_and_rereads_the_files_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    let mut watcher = ConfigWatcher::new(vec![file.clone()]).unwrap();
    // the files were read as the watch began: the first wait is over at once and the reread finds nothing new
    assert!(async_io::block_on(watcher.quiet()));
    assert!(watcher.reread().is_empty());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let quiet = async_io::block_on(watcher.quiet());
        // the reread on another thread than the loop's, as a pool runs it
        let events = std::thread::spawn(move || watcher.reread()).join().unwrap();
        let _ = tx.send((quiet, events));
    });
    std::fs::write(&file, "1").unwrap();
    let (quiet, events) = rx.recv_timeout(REPORT).expect("a quiet burst");
    assert!(quiet);
    assert_eq!(events, [WatchEvent::Changed(vec![file])]);
}
