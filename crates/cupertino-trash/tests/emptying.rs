//! Emptying the trash: what goes, what stays and what is reported.

mod common;

use common::{is_root, lock, names, put_item, uid, unlock, Tree};
use cupertino_trash::Emptied;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{symlink, PermissionsExt};

fn sticky(dir: &std::path::Path) {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o1777)).unwrap();
}

#[test]
fn emptying_deletes_every_item_and_leaves_an_empty_trash() {
    let t = Tree::new();
    let trash = t.trash();
    t.file("dir/a", "x");
    t.file("dir/sub/b", "y");
    trash.trash(&t.path("data/dir")).unwrap();
    trash.trash(&t.file("a.txt", "x")).unwrap();
    let emptied = trash.empty();
    assert_eq!(emptied.deleted, 2);
    assert!(emptied.failures.is_empty());
    assert_eq!(emptied.failure_message(), None);
    assert_eq!(names(&t.home_trash()), ["files", "info"]);
    assert!(names(&t.home_trash().join("files")).is_empty());
    assert!(names(&t.home_trash().join("info")).is_empty());
    assert!(trash.list().unwrap().is_empty());
    assert!(!trash.holds_items());
}

#[test]
fn emptying_a_trash_that_is_not_there_does_nothing() {
    let t = Tree::new();
    let emptied = t.trash().empty();
    assert_eq!(emptied.deleted, 0);
    assert!(emptied.failures.is_empty());
    assert!(!t.home_trash().exists());
}

#[test]
fn emptying_an_empty_trash_counts_nothing() {
    let t = Tree::new();
    let trash = t.trash();
    trash.trash(&t.file("a.txt", "")).unwrap();
    assert_eq!(trash.empty().deleted, 1);
    assert_eq!(trash.empty().deleted, 0);
}

#[test]
fn an_entry_without_a_record_goes_too() {
    let t = Tree::new();
    fs::create_dir_all(t.home_trash().join("files")).unwrap();
    fs::write(t.home_trash().join("files/orphan"), "x").unwrap();
    let emptied = t.trash().empty();
    assert_eq!(emptied.deleted, 1);
    assert!(names(&t.home_trash().join("files")).is_empty());
}

#[test]
fn emptying_forgets_the_folder_sizes() {
    let t = Tree::new();
    let trash = t.trash();
    t.file("dir/a", "x");
    trash.trash(&t.path("data/dir")).unwrap();
    assert!(t.home_trash().join("directorysizes").exists());
    trash.empty();
    assert!(!t.home_trash().join("directorysizes").exists());
}

#[test]
fn emptying_leaves_no_working_folder_behind() {
    let t = Tree::new();
    let trash = t.trash();
    trash.trash(&t.file("a.txt", "")).unwrap();
    trash.empty();
    assert!(names(&t.home_trash())
        .iter()
        .all(|n| !n.starts_with(".expunged")));
}

#[test]
fn a_link_in_the_trash_is_deleted_without_following_it() {
    let t = Tree::new();
    let target = t.file("outside/keep.txt", "keep");
    let link = t.path("data/link");
    symlink(t.path("data/outside"), &link).unwrap();
    let trash = t.trash();
    trash.trash(&link).unwrap();
    let emptied = trash.empty();
    assert_eq!(emptied.deleted, 1);
    assert!(target.exists());
}

#[test]
fn a_files_folder_that_is_a_link_is_taken_away_not_followed() {
    let t = Tree::new();
    let target = t.file("elsewhere/keep.txt", "keep");
    fs::create_dir_all(t.home_trash()).unwrap();
    symlink(t.path("data/elsewhere"), t.home_trash().join("files")).unwrap();
    let emptied = t.trash().empty();
    assert!(emptied.failures.is_empty());
    assert!(target.exists());
    assert!(fs::symlink_metadata(t.home_trash().join("files"))
        .unwrap()
        .is_dir());
}

#[test]
fn an_info_folder_that_is_a_link_is_not_followed() {
    let t = Tree::new();
    let record = t.file("elsewhere/a.trashinfo", "keep");
    fs::create_dir_all(t.home_trash().join("files")).unwrap();
    fs::write(t.home_trash().join("files/a"), "").unwrap();
    symlink(t.path("data/elsewhere"), t.home_trash().join("info")).unwrap();
    let emptied = t.trash().empty();
    assert_eq!(emptied.deleted, 1);
    assert!(record.exists());
}

#[test]
fn what_cannot_be_deleted_stays_in_the_trash_and_is_reported() {
    if is_root() {
        return;
    }
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "fine.txt", "x", "/x/fine.txt", "2026-01-01T00:00:00");
    put_item(&dir, "locked", "", "/x/locked", "2026-01-01T00:00:00");
    fs::remove_file(dir.join("files/locked")).unwrap();
    fs::create_dir_all(dir.join("files/locked/sub")).unwrap();
    fs::write(dir.join("files/locked/sub/inner"), "x").unwrap();
    lock(&dir.join("files/locked/sub"));
    let emptied = t.trash().empty();
    assert_eq!(emptied.deleted, 1);
    assert_eq!(emptied.failures.len(), 1);
    assert_eq!(emptied.failures[0].path, dir.join("files/locked"));
    assert_eq!(
        emptied.failures[0].error.kind(),
        ErrorKind::PermissionDenied
    );
    let message = emptied.failure_message().unwrap();
    assert!(
        message.starts_with("\u{201c}locked\u{201d} could not be deleted: "),
        "{message}"
    );
    // the item is back in the trash with its record, so it can be restored or emptied later
    assert_eq!(names(&dir.join("files")), ["locked"]);
    assert_eq!(names(&dir.join("info")), ["locked.trashinfo"]);
    let items = t.trash().list().unwrap();
    assert_eq!(items.len(), 1);
    unlock(&dir.join("files/locked/sub"));
    let again = t.trash().empty();
    assert_eq!(again.deleted, 1);
    assert!(again.failures.is_empty());
    assert!(names(&dir.join("files")).is_empty());
}

#[test]
fn the_failure_message_counts_what_stayed() {
    let emptied = Emptied {
        deleted: 0,
        failures: vec![
            cupertino_trash::Failure {
                path: "/t/files/a".into(),
                error: std::io::Error::new(ErrorKind::PermissionDenied, "denied"),
            },
            cupertino_trash::Failure {
                path: "/t/files/b".into(),
                error: std::io::Error::new(ErrorKind::PermissionDenied, "denied"),
            },
        ],
    };
    assert_eq!(
        emptied.failure_message().as_deref(),
        Some("2 items could not be deleted. \u{201c}a\u{201d}: denied.")
    );
}

#[test]
fn a_volumes_own_trash_is_emptied_with_the_home_trash() {
    let t = Tree::new();
    let trash = t.trash_with_volume();
    trash.trash(&t.file("a.txt", "")).unwrap();
    let own = t.volume_trash();
    put_item(&own, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    let emptied = trash.empty();
    assert_eq!(emptied.deleted, 2);
    assert!(emptied.failures.is_empty());
    assert!(names(&own.join("files")).is_empty());
    assert!(names(&own.join("info")).is_empty());
}

#[test]
fn a_shared_trash_counts_only_as_a_real_sticky_folder() {
    let t = Tree::new();
    let shared = t.vol().join(".Trash");
    let mine = shared.join(uid().to_string());
    put_item(&mine, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    let trash = t.trash_with_volume();
    // without the sticky bit it is not used
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(trash.volume_trash_dirs().is_empty());
    assert_eq!(trash.empty().deleted, 0);
    sticky(&shared);
    assert_eq!(trash.volume_trash_dirs(), std::slice::from_ref(&mine));
    assert_eq!(trash.empty().deleted, 1);
}

#[test]
fn a_shared_trash_that_is_a_link_is_not_used() {
    let t = Tree::new();
    let real = t.path("data/real-shared");
    let mine = real.join(uid().to_string());
    put_item(&mine, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    sticky(&real);
    symlink(&real, t.vol().join(".Trash")).unwrap();
    let trash = t.trash_with_volume();
    assert!(trash.volume_trash_dirs().is_empty());
    assert_eq!(trash.empty().deleted, 0);
    assert!(mine.join("files/v.txt").exists());
}

#[test]
fn a_users_trash_that_is_a_link_is_not_used() {
    let t = Tree::new();
    let elsewhere = t.path("data/elsewhere");
    put_item(
        &elsewhere,
        "v.txt",
        "x",
        "docs/v.txt",
        "2026-01-01T00:00:00",
    );
    symlink(&elsewhere, t.vol().join(format!(".Trash-{}", uid()))).unwrap();
    let trash = t.trash_with_volume();
    assert!(trash.volume_trash_dirs().is_empty());
    assert_eq!(trash.empty().deleted, 0);
    assert!(elsewhere.join("files/v.txt").exists());
}

#[test]
fn a_trash_of_another_user_is_not_used() {
    let t = Tree::new();
    let other = t.vol().join(format!(".Trash-{}", uid() + 1));
    put_item(&other, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    let trash = t.trash_with_volume();
    assert!(trash.volume_trash_dirs().is_empty());
    assert_eq!(trash.empty().deleted, 0);
}

#[test]
fn a_volume_trash_that_cannot_be_written_is_not_used() {
    if is_root() {
        return;
    }
    let t = Tree::new();
    let own = t.volume_trash();
    put_item(&own, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    lock(&own);
    let trash = t.trash_with_volume();
    assert!(trash.volume_trash_dirs().is_empty());
    unlock(&own);
    assert_eq!(trash.volume_trash_dirs(), [own]);
}

#[test]
fn a_trash_without_mount_points_knows_no_volumes() {
    let t = Tree::new();
    let own = t.volume_trash();
    put_item(&own, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    let trash = t.trash();
    assert!(trash.volume_trash_dirs().is_empty());
    assert!(trash.silent_volumes().is_empty());
    assert_eq!(trash.trash_dirs(), [t.home_trash()]);
    assert_eq!(trash.empty().deleted, 0);
    assert!(own.join("files/v.txt").exists());
}

#[test]
fn the_trash_directories_are_the_home_trash_then_the_volumes_shared_and_own() {
    let t = Tree::new();
    let shared = t.vol().join(".Trash");
    fs::create_dir_all(shared.join(uid().to_string())).unwrap();
    sticky(&shared);
    let own = t.volume_trash();
    let trash = t.trash_with_volume();
    assert_eq!(
        trash.trash_dirs(),
        [t.home_trash(), shared.join(uid().to_string()), own]
    );
}

#[test]
fn a_mount_point_listed_twice_counts_once_and_a_missing_one_holds_nothing() {
    let t = Tree::new();
    let own = t.volume_trash();
    put_item(&own, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    let trash = t
        .trash()
        .with_mounts(vec![t.vol(), t.vol(), t.path("not-mounted")]);
    assert_eq!(trash.volume_trash_dirs(), [own]);
    assert_eq!(trash.empty().deleted, 1);
}

#[test]
fn the_home_trash_is_not_taken_a_second_time_as_a_volumes() {
    let t = Tree::new();
    let home = t.vol().join(format!(".Trash-{}", uid()));
    put_item(&home, "v.txt", "x", "docs/v.txt", "2026-01-01T00:00:00");
    let trash = cupertino_trash::Trash::with_home(&home).with_mounts(vec![t.vol()]);
    assert!(trash.volume_trash_dirs().is_empty());
    assert_eq!(trash.empty().deleted, 1);
}
