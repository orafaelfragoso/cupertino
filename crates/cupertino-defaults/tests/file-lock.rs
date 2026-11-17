//! `lock::FileLock`: the advisory lock beside a settings file.

use std::os::unix::fs::{symlink, MetadataExt};
use std::path::Path;
use std::time::{Duration, Instant};

use cupertino_defaults::lock::{lock_path, FileLock};

#[test]
fn the_lock_file_sits_beside_the_settings_file_with_mode_0600() {
    assert_eq!(
        lock_path(Path::new("/a/b/dock.conf")).unwrap(),
        Path::new("/a/b/.dock.conf.lock")
    );
    assert!(lock_path(Path::new("/")).is_err());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new/dir/x.conf");
    let _lock = FileLock::acquire(&path).unwrap();
    assert_eq!(
        std::fs::metadata(lock_path(&path).unwrap()).unwrap().mode() & 0o777,
        0o600
    );
}

#[test]
fn the_lock_is_exclusive_until_dropped_and_a_taker_times_out_while_it_is_held() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    let first = FileLock::acquire(&path).unwrap();
    let start = Instant::now();
    let err = FileLock::acquire_within(&path, Duration::from_millis(100)).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    assert!(start.elapsed() >= Duration::from_millis(100));
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
    FileLock::acquire_within(&path, Duration::from_millis(50)).unwrap();
}

#[test]
fn a_link_or_a_fifo_at_the_lock_path_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let linked = dir.path().join("linked.conf");
    let elsewhere = dir.path().join("elsewhere");
    std::fs::write(&elsewhere, "").unwrap();
    symlink(&elsewhere, lock_path(&linked).unwrap()).unwrap();
    assert!(FileLock::acquire(&linked).is_err(), "link");

    let fifo = dir.path().join("fifo.conf");
    let c = std::ffi::CString::new(lock_path(&fifo).unwrap().to_str().unwrap()).unwrap();
    // Safety: `c` is a valid C string for the call.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(FileLock::acquire(&fifo).is_err(), "fifo");
}
