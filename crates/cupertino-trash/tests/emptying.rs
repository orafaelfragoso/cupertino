//! Emptying the trash: what goes, what stays and what is reported.

use crate::common;

use common::{is_root, lock, names, put_item, uid, unlock, Tree};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;

fn sticky(dir: &std::path::Path) {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o1777)).unwrap();
}

const DATE: &str = "2026-01-01T00:00:00";

#[test]
fn emptying_deletes_every_entry_and_leaves_an_empty_trash() {
    let t = Tree::new();
    let trash = t.trash();
    let nothing = trash.empty();
    assert_eq!(nothing.deleted, 0);
    assert!(nothing.failures.is_empty());
    assert!(
        !t.home_trash().exists(),
        "a trash that is not there stays so"
    );

    t.file("dir/a", "x");
    t.file("dir/sub/b", "y");
    trash.trash(&t.path("data/dir")).unwrap();
    trash.trash(&t.file("a.txt", "x")).unwrap();
    assert!(t.home_trash().join("directorysizes").exists());
    // an entry without a record goes too
    fs::write(t.home_trash().join("files/orphan"), "x").unwrap();
    let emptied = trash.empty();
    assert_eq!(emptied.deleted, 3);
    assert!(emptied.failures.is_empty());
    assert_eq!(emptied.failure_message(), None);
    // no folder sizes and no working folder are left behind
    assert_eq!(names(&t.home_trash()), ["files", "info"]);
    assert!(names(&t.home_trash().join("files")).is_empty());
    assert!(names(&t.home_trash().join("info")).is_empty());
    assert!(trash.list().unwrap().is_empty());
    assert!(!trash.holds_items());
    assert_eq!(trash.empty().deleted, 0);
}

#[test]
fn emptying_never_follows_a_link_out_of_the_trash() {
    // a link trashed as an item
    let t = Tree::new();
    let target = t.file("outside/keep.txt", "keep");
    let link = t.path("data/link");
    symlink(t.path("data/outside"), &link).unwrap();
    let trash = t.trash();
    trash.trash(&link).unwrap();
    assert_eq!(trash.empty().deleted, 1);
    assert!(target.exists());

    // a files folder that is a link is taken away and replaced by a real one
    let t = Tree::new();
    let target = t.file("elsewhere/keep.txt", "keep");
    fs::create_dir_all(t.home_trash()).unwrap();
    symlink(t.path("data/elsewhere"), t.home_trash().join("files")).unwrap();
    assert!(t.trash().empty().failures.is_empty());
    assert!(target.exists());
    assert!(fs::symlink_metadata(t.home_trash().join("files"))
        .unwrap()
        .is_dir());

    // an info folder that is a link
    let t = Tree::new();
    let record = t.file("elsewhere/a.trashinfo", "keep");
    fs::create_dir_all(t.home_trash().join("files")).unwrap();
    fs::write(t.home_trash().join("files/a"), "").unwrap();
    symlink(t.path("data/elsewhere"), t.home_trash().join("info")).unwrap();
    assert_eq!(t.trash().empty().deleted, 1);
    assert!(record.exists());
}

#[test]
fn what_cannot_be_deleted_stays_in_the_trash_and_is_reported() {
    if is_root() {
        return;
    }
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "fine.txt", "x", "/x/fine.txt", DATE);
    put_item(&dir, "locked", "", "/x/locked", DATE);
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
    assert_eq!(t.trash().list().unwrap().len(), 1);
    unlock(&dir.join("files/locked/sub"));
    let again = t.trash().empty();
    assert_eq!(again.deleted, 1);
    assert!(again.failures.is_empty());
    assert!(names(&dir.join("files")).is_empty());
}

#[test]
fn a_volumes_own_trash_is_emptied_with_the_home_trash() {
    let t = Tree::new();
    let trash = t.trash_with_volume();
    trash.trash(&t.file("a.txt", "")).unwrap();
    let own = t.volume_trash();
    put_item(&own, "v.txt", "x", "docs/v.txt", DATE);
    let emptied = trash.empty();
    assert_eq!(emptied.deleted, 2);
    assert!(emptied.failures.is_empty());
    assert!(names(&own.join("files")).is_empty());
    assert!(names(&own.join("info")).is_empty());
}

/// A volume trash the user's trash must not take: its folder holds an item that must stay.
fn refused(t: &Tree, row: &str) -> PathBuf {
    match row {
        "a shared trash without the sticky bit" => {
            let shared = t.vol().join(".Trash");
            let mine = shared.join(uid().to_string());
            put_item(&mine, "v.txt", "x", "docs/v.txt", DATE);
            fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
            mine
        }
        "a shared trash that is a link" => {
            let real = t.path("data/real-shared");
            let mine = real.join(uid().to_string());
            put_item(&mine, "v.txt", "x", "docs/v.txt", DATE);
            sticky(&real);
            symlink(&real, t.vol().join(".Trash")).unwrap();
            mine
        }
        "a user's trash that is a link" => {
            let elsewhere = t.path("data/elsewhere");
            put_item(&elsewhere, "v.txt", "x", "docs/v.txt", DATE);
            symlink(&elsewhere, t.vol().join(format!(".Trash-{}", uid()))).unwrap();
            elsewhere
        }
        "another user's trash" => {
            let other = t.vol().join(format!(".Trash-{}", uid() + 1));
            put_item(&other, "v.txt", "x", "docs/v.txt", DATE);
            other
        }
        _ => unreachable!("{row}"),
    }
}

#[test]
fn a_volume_trash_counts_only_as_the_users_own_real_and_writable_folder() {
    for row in [
        "a shared trash without the sticky bit",
        "a shared trash that is a link",
        "a user's trash that is a link",
        "another user's trash",
    ] {
        let t = Tree::new();
        let dir = refused(&t, row);
        let trash = t.trash_with_volume();
        assert!(trash.volume_trash_dirs().is_empty(), "{row}");
        assert_eq!(trash.empty().deleted, 0, "{row}");
        assert!(dir.join("files/v.txt").exists(), "{row}");
    }

    // a shared trash counts once it is a real sticky folder
    let t = Tree::new();
    let shared = t.vol().join(".Trash");
    let mine = refused(&t, "a shared trash without the sticky bit");
    sticky(&shared);
    let trash = t.trash_with_volume();
    assert_eq!(trash.volume_trash_dirs(), std::slice::from_ref(&mine));
    assert_eq!(trash.empty().deleted, 1);

    // the user's own trash counts only while it can be written
    if !is_root() {
        let t = Tree::new();
        let own = t.volume_trash();
        put_item(&own, "v.txt", "x", "docs/v.txt", DATE);
        lock(&own);
        let trash = t.trash_with_volume();
        assert!(trash.volume_trash_dirs().is_empty());
        unlock(&own);
        assert_eq!(trash.volume_trash_dirs(), [own]);
    }
}

#[test]
fn the_trash_directories_are_the_home_trash_then_each_volumes_shared_and_own_once() {
    let t = Tree::new();
    let own = t.volume_trash();
    put_item(&own, "v.txt", "x", "docs/v.txt", DATE);

    // without mount points no volume is known
    let trash = t.trash();
    assert!(trash.volume_trash_dirs().is_empty());
    assert!(trash.silent_volumes().is_empty());
    assert_eq!(trash.trash_dirs(), [t.home_trash()]);
    assert_eq!(trash.empty().deleted, 0);
    assert!(own.join("files/v.txt").exists());

    let shared = t.vol().join(".Trash");
    fs::create_dir_all(shared.join(uid().to_string())).unwrap();
    sticky(&shared);
    assert_eq!(
        t.trash_with_volume().trash_dirs(),
        [t.home_trash(), shared.join(uid().to_string()), own.clone()]
    );

    // a mount point listed twice counts once, and a missing one holds nothing
    let trash = t
        .trash()
        .with_mounts(vec![t.vol(), t.vol(), t.path("not-mounted")]);
    assert_eq!(
        trash.volume_trash_dirs(),
        [shared.join(uid().to_string()), own.clone()]
    );
    assert_eq!(trash.empty().deleted, 1);

    // the home trash is not taken a second time as a volume's
    put_item(&own, "w.txt", "x", "docs/w.txt", DATE);
    fs::remove_dir_all(&shared).unwrap();
    let trash = cupertino_trash::Trash::with_home(&own).with_mounts(vec![t.vol()]);
    assert!(trash.volume_trash_dirs().is_empty());
    assert_eq!(trash.empty().deleted, 1);
}
