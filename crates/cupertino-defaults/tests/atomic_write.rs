//! `atomic::write_atomic` and `write_atomic_private`.

use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};

use cupertino_defaults::atomic::{write_atomic, write_atomic_private};

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn a_write_replaces_the_file_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.conf");
    write_atomic(&path, b"one").unwrap();
    write_atomic(&path, b"two").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"two");
    assert_eq!(names(dir.path()), ["a.conf"]);
}

#[test]
fn a_write_creates_the_missing_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x/y/a.conf");
    write_atomic(&path, b"hi").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"hi");
}

#[test]
fn an_existing_file_keeps_its_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.conf");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    write_atomic(&path, b"new").unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);
}

#[test]
fn a_symlinked_settings_file_stays_a_link_and_its_target_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let real_dir = dir.path().join("dotfiles");
    fs::create_dir(&real_dir).unwrap();
    let target = real_dir.join("a.conf");
    fs::write(&target, "old").unwrap();
    let link = dir.path().join("a.conf");
    symlink(&target, &link).unwrap();
    write_atomic(&link, b"new").unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&target).unwrap(), b"new");
    assert_eq!(
        names(&real_dir),
        ["a.conf"],
        "the temporary file was in the target's directory"
    );
}

#[test]
fn a_leftover_temporary_name_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.conf");
    // the names a crash of this process id could have left behind
    for n in 0..10 {
        fs::write(
            dir.path()
                .join(format!(".a.conf.{}.{n}.tmp", std::process::id())),
            "stale",
        )
        .unwrap();
    }
    write_atomic(&path, b"fresh").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"fresh");
    assert_eq!(
        fs::read(
            dir.path()
                .join(format!(".a.conf.{}.0.tmp", std::process::id()))
        )
        .unwrap(),
        b"stale",
        "the leftover is not touched"
    );
}

#[test]
fn a_failed_write_removes_its_temporary_file_and_keeps_the_old_file() {
    let dir = tempfile::tempdir().unwrap();
    // a directory at the path makes the final rename fail after the temporary file was written
    let path = dir.path().join("a.conf");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("inner"), "x").unwrap();
    assert!(write_atomic(&path, b"new").is_err());
    assert!(path.is_dir());
    assert_eq!(names(dir.path()), ["a.conf"]);
}

#[test]
fn a_path_without_a_file_name_is_an_error() {
    let err = write_atomic(std::path::Path::new("/"), b"x").unwrap_err();
    assert!(
        err.kind() == std::io::ErrorKind::InvalidInput || err.raw_os_error().is_some(),
        "{err}"
    );
}

#[test]
fn a_private_write_gives_mode_0600_whatever_the_old_file_had() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret.conf");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    write_atomic_private(&path, b"new").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn a_private_write_creates_a_file_only_its_owner_can_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub/secret.conf");
    write_atomic_private(&path, b"x").unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
}

#[test]
fn a_private_write_replaces_a_link_rather_than_following_it() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere");
    fs::write(&elsewhere, "untouched").unwrap();
    let link = dir.path().join("secret.conf");
    symlink(&elsewhere, &link).unwrap();
    write_atomic_private(&link, b"private").unwrap();
    assert!(!fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read(&link).unwrap(), b"private");
    assert_eq!(fs::read(&elsewhere).unwrap(), b"untouched");
}
