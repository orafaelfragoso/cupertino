//! The write state of hovered folders: unknown allows, a read refuses, a stale state is asked for again.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_files::space::{read_each, MountedCache, MOUNT_STATE_LIFETIME};

#[test]
fn an_unknown_folder_is_writable_and_asked_for_once_until_it_is_read() {
    let cache = MountedCache::default();
    let now = Instant::now();
    let dir = Path::new("/media/stick");
    assert!(!cache.read_only(dir, now));
    assert!(!cache.read_only(dir, now));
    assert_eq!(cache.take_wanted(), vec![dir.to_path_buf()]);
    assert!(cache.take_wanted().is_empty());
    cache.record(dir.to_path_buf(), true, now);
    assert!(cache.read_only(dir, now));
    assert!(cache.take_wanted().is_empty());
}

#[test]
fn a_state_older_than_its_lifetime_still_answers_and_is_asked_for_again() {
    let cache = MountedCache::default();
    let now = Instant::now();
    let dir = PathBuf::from("/media/stick");
    cache.record(dir.clone(), true, now);
    let later = now + MOUNT_STATE_LIFETIME + Duration::from_secs(1);
    assert!(cache.read_only(&dir, later));
    assert_eq!(cache.take_wanted(), vec![dir]);
}

#[test]
fn folders_are_read_off_the_cache_and_a_missing_one_counts_as_writable() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let read = read_each(vec![root.path().to_path_buf(), missing.clone()]);
    assert_eq!(
        read,
        vec![(root.path().to_path_buf(), false), (missing, false)]
    );
}
