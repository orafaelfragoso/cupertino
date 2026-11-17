//! `lock::FileLock`: the advisory lock beside a settings file.

use std::os::unix::fs::{symlink, MetadataExt};
use std::time::{Duration, Instant};

use cupertino_defaults::lock::{lock_path, FileLock, LOCK_WAIT};

#[test]
fn the_lock_file_sits_beside_the_settings_file_as_dot_name_dot_lock() {
    assert_eq!(
        lock_path(std::path::Path::new("/a/b/dock.conf")).unwrap(),
        std::path::Path::new("/a/b/.dock.conf.lock")
    );
    assert!(lock_path(std::path::Path::new("/")).is_err());
}

#[test]
fn taking_the_lock_makes_the_directory_and_a_lock_file_of_mode_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new/dir/x.conf");
    let _lock = FileLock::acquire(&path).unwrap();
    let lock = lock_path(&path).unwrap();
    assert_eq!(std::fs::metadata(lock).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn a_second_taker_waits_until_the_first_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    let first = FileLock::acquire(&path).unwrap();
    let waiter = {
        let path = path.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            FileLock::acquire_within(&path, Duration::from_secs(10)).map(|_| start.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(150));
    drop(first);
    let waited = waiter.join().unwrap().unwrap();
    assert!(waited >= Duration::from_millis(100), "{waited:?}");
}

#[test]
fn a_holder_that_never_lets_go_makes_the_taker_time_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    let _held = FileLock::acquire(&path).unwrap();
    let start = Instant::now();
    let err = FileLock::acquire_within(&path, Duration::from_millis(100)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() >= Duration::from_millis(100));
}

#[test]
fn the_wait_is_two_seconds() {
    assert_eq!(LOCK_WAIT, Duration::from_secs(2));
}

#[test]
fn the_lock_is_free_again_after_it_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    drop(FileLock::acquire(&path).unwrap());
    FileLock::acquire_within(&path, Duration::from_millis(50)).unwrap();
}

#[test]
fn a_link_at_the_lock_path_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    let elsewhere = dir.path().join("elsewhere");
    std::fs::write(&elsewhere, "").unwrap();
    symlink(&elsewhere, lock_path(&path).unwrap()).unwrap();
    assert!(FileLock::acquire(&path).is_err());
}

#[test]
fn a_fifo_at_the_lock_path_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    let lock = lock_path(&path).unwrap();
    let c = std::ffi::CString::new(lock.to_str().unwrap()).unwrap();
    // Safety: `c` is a valid C string for the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(FileLock::acquire(&path).is_err());
}
