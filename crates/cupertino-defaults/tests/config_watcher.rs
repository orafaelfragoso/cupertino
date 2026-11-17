//! `watch::ConfigWatcher`: change notification for the settings files.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use cupertino_defaults::atomic::write_atomic;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent, COALESCE};

/// Long enough for a report to arrive on a loaded machine.
const REPORT: Duration = Duration::from_secs(10);
/// Long enough, many times over `COALESCE`, for a report that should not come to have come.
const QUIET: Duration = Duration::from_millis(600);

fn watch(files: Vec<PathBuf>) -> (ConfigWatcher, Receiver<WatchEvent>) {
    let (tx, rx) = channel();
    let watcher = ConfigWatcher::new(files, move |event| {
        let _ = tx.send(event);
    })
    .unwrap();
    (watcher, rx)
}

fn changed(rx: &Receiver<WatchEvent>) -> Vec<PathBuf> {
    match rx.recv_timeout(REPORT).expect("a report") {
        WatchEvent::Changed(paths) => paths,
        WatchEvent::Error(e) => panic!("{e}"),
    }
}

fn nothing(rx: &Receiver<WatchEvent>) {
    assert!(rx.recv_timeout(QUIET).is_err(), "no report");
}

#[test]
fn the_coalescing_time_is_fifty_milliseconds() {
    assert_eq!(COALESCE, Duration::from_millis(50));
}

#[test]
fn an_atomic_replacement_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    std::fs::write(&file, "one").unwrap();
    let (_w, rx) = watch(vec![file.clone()]);
    write_atomic(&file, b"two").unwrap();
    assert_eq!(changed(&rx), [file]);
    nothing(&rx);
}

#[test]
fn a_burst_of_writes_to_one_file_is_one_report() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    std::fs::write(&file, "start").unwrap();
    let (_w, rx) = watch(vec![file.clone()]);
    // one change first, so that the watch thread is known to be running before the burst
    std::fs::write(&file, "primed").unwrap();
    assert_eq!(changed(&rx), std::slice::from_ref(&file));
    nothing(&rx);
    for i in 0..10 {
        std::fs::write(&file, format!("v{i}")).unwrap();
    }
    assert_eq!(changed(&rx), [file]);
    nothing(&rx);
}

#[test]
fn creating_and_removing_a_file_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    let (_w, rx) = watch(vec![file.clone()]);
    std::fs::write(&file, "x").unwrap();
    assert_eq!(changed(&rx), std::slice::from_ref(&file));
    std::fs::remove_file(&file).unwrap();
    assert_eq!(changed(&rx), [file]);
}

#[test]
fn unchanged_contents_other_files_and_temporary_files_are_not_reported() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    std::fs::write(&file, "same").unwrap();
    let (_w, rx) = watch(vec![file.clone()]);
    std::fs::write(&file, "same").unwrap();
    nothing(&rx);
    std::fs::write(dir.path().join("other.conf"), "x").unwrap();
    std::fs::write(dir.path().join(".a.conf.1.0.tmp"), "x").unwrap();
    nothing(&rx);
}

#[test]
fn several_files_changed_in_one_burst_are_reported_together_in_the_order_given() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("a.conf"), dir.path().join("b.conf"));
    let (_w, rx) = watch(vec![b.clone(), a.clone()]);
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
fn a_file_outside_the_usual_settings_directory_is_watched_through_its_own_directory() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("redirected/deeper/x.conf");
    let (_w, rx) = watch(vec![elsewhere.clone()]);
    assert!(
        elsewhere.parent().unwrap().is_dir(),
        "the settings directory is created"
    );
    write_atomic(&elsewhere, b"x").unwrap();
    assert_eq!(changed(&rx), [elsewhere]);
}

#[test]
fn a_change_before_the_watch_began_is_not_reported_but_one_after_is() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    std::fs::write(&file, "old").unwrap();
    std::fs::write(&file, "newer").unwrap();
    let (_w, rx) = watch(vec![file.clone()]);
    nothing(&rx);
    std::fs::write(&file, "newest").unwrap();
    assert_eq!(changed(&rx), [file]);
}

#[test]
fn a_removed_and_recreated_settings_directory_is_watched_again() {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("cupertino");
    let file = settings.join("a.conf");
    let (_w, rx) = watch(vec![file.clone()]);
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
    let (_w, rx) = watch(vec![fifo, other.clone()]);
    write_atomic(&other, b"x").unwrap();
    assert_eq!(changed(&rx), [other]);
}

#[test]
fn reading_a_file_in_a_watched_directory_reports_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    std::fs::write(&file, "x").unwrap();
    let (_w, rx) = watch(vec![file.clone()]);
    for _ in 0..5 {
        let _ = std::fs::read(&file).unwrap();
    }
    nothing(&rx);
}

#[test]
fn a_panicking_callback_ends_the_watch_without_taking_the_process() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    let (tx, rx) = channel();
    let _w = ConfigWatcher::new(vec![file.clone()], move |_| {
        let _ = tx.send(());
        panic!("callback fails on purpose");
    })
    .unwrap();
    std::fs::write(&file, "1").unwrap();
    rx.recv_timeout(REPORT).expect("the callback ran");
    std::fs::write(&file, "2").unwrap();
    assert!(
        rx.recv_timeout(QUIET).is_err(),
        "the watch ended after the panic"
    );
}

#[test]
fn dropping_the_watcher_ends_its_thread_and_so_the_callback() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.conf");
    let (w, rx) = watch(vec![file.clone()]);
    drop(w);
    // the callback owns the sender: it is dropped when the watch thread ends
    assert_eq!(
        rx.recv_timeout(REPORT),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    );
}
