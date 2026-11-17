//! Moving items to the trash, listing, restoring, deleting and the helpers of the trashinfo records.

mod common;

use common::{put_item, Tree};
use cupertino_trash::{
    decode_path, encode_path, failure_message, forget, format_local_time, trashed_item, TrashError,
};
use std::fs;
use std::path::Path;

const DATE: &str = "2026-01-01T00:00:00";

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn assert_local_time_shape(text: &str) {
    assert_eq!(text.len(), 19, "{text}");
    assert_eq!(&text[4..5], "-", "{text}");
    assert_eq!(&text[10..11], "T", "{text}");
    assert_eq!(&text[13..14], ":", "{text}");
}

#[test]
fn a_file_moves_into_the_home_trash_with_a_record_of_its_encoded_path_and_local_date() {
    let t = Tree::new();
    let file = t.file("a b#c%d.txt", "hello");
    let item = t.trash().trash(&file).unwrap();
    assert_eq!(item.name, "a b#c%d.txt");
    assert_eq!(item.trash_dir, t.home_trash());
    assert_eq!(item.original_path, file);
    assert!(!file.exists());
    assert_eq!(item.file_path(), t.home_trash().join("files/a b#c%d.txt"));
    assert_eq!(fs::read_to_string(item.file_path()).unwrap(), "hello");
    assert_eq!(
        item.info_path(),
        t.home_trash().join("info/a b#c%d.txt.trashinfo")
    );
    let info = fs::read_to_string(item.info_path()).unwrap();
    let lines: Vec<&str> = info.lines().collect();
    assert_eq!(lines[0], "[Trash Info]");
    let path = lines[1].strip_prefix("Path=").unwrap();
    assert!(
        path.starts_with('/') && path.ends_with("/a%20b%23c%25d.txt"),
        "{path}"
    );
    let date = lines[2].strip_prefix("DeletionDate=").unwrap();
    assert_eq!(date, item.deletion_date);
    assert_local_time_shape(date);
}

#[test]
fn a_name_that_is_taken_gets_a_counter_before_its_extension_or_at_the_end() {
    let t = Tree::new();
    let trash = t.trash();
    // a file without a record still holds its name
    fs::create_dir_all(t.home_trash().join("files")).unwrap();
    fs::write(t.home_trash().join("files/a.txt"), "old").unwrap();
    let mut got = Vec::new();
    for dir in ["one", "two", "three"] {
        for name in ["a.tar.gz", "README", ".profile", "a.txt"] {
            got.push(
                trash
                    .trash(&t.file(&format!("{dir}/{name}"), ""))
                    .unwrap()
                    .name,
            );
        }
    }
    assert_eq!(
        got,
        [
            "a.tar.gz",
            "README",
            ".profile",
            "a 2.txt",
            "a.tar 2.gz",
            "README 2",
            ".profile 2",
            "a 3.txt",
            "a.tar 3.gz",
            "README 3",
            ".profile 3",
            "a 4.txt"
        ]
    );
    assert_eq!(
        fs::read_to_string(t.home_trash().join("files/a.txt")).unwrap(),
        "old"
    );
    assert!(!t.home_trash().join("info/a.txt.trashinfo").exists());
}

#[test]
fn a_folder_is_moved_whole_and_its_size_is_recorded_under_its_encoded_name() {
    let t = Tree::new();
    t.trash().trash(&t.file("a.txt", "x")).unwrap();
    assert!(
        !t.home_trash().join("directorysizes").exists(),
        "a plain file adds no size line"
    );
    t.file("my dir/a.bin", "12345");
    t.file("my dir/sub/b.bin", "123");
    let item = t.trash().trash(&t.path("data/my dir")).unwrap();
    assert_eq!(
        fs::read_to_string(item.file_path().join("sub/b.bin")).unwrap(),
        "123"
    );
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert!(sizes.ends_with('\n'));
    let fields: Vec<&str> = sizes.trim_end().split(' ').collect();
    assert_eq!(fields[0], "8");
    assert!(fields[1].parse::<i64>().is_ok());
    assert_eq!(fields[2], "my%20dir");
}

#[test]
fn the_size_of_a_folder_can_be_left_for_later() {
    let t = Tree::new();
    let trash = t.trash();
    let (_, pending) = trash.trash_unsized(&t.file("a.txt", "x")).unwrap();
    assert!(pending.is_none(), "a file has no size to record");

    t.file("dir/a.bin", "1234");
    let (item, pending) = trash.trash_unsized(&t.path("data/dir")).unwrap();
    assert!(item.file_path().is_dir());
    assert!(!t.home_trash().join("directorysizes").exists());
    pending.unwrap().record().unwrap();
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert!(sizes.starts_with("4 "), "{sizes}");

    // a folder that left the trash before its size was recorded gets no line
    t.file("gone/a.bin", "1234");
    let (item, pending) = trash.trash_unsized(&t.path("data/gone")).unwrap();
    trash.restore(&item).unwrap();
    pending.unwrap().record().unwrap();
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert_eq!(sizes.lines().count(), 1, "{sizes}");
}

#[test]
fn a_symbolic_link_is_trashed_and_deleted_itself() {
    let t = Tree::new();
    let trash = t.trash();
    let target = t.file("target.txt", "keep");
    let link = t.path("data/link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let item = trash.trash(&link).unwrap();
    assert!(fs::symlink_metadata(item.file_path())
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(fs::symlink_metadata(&link).is_err());
    trash.delete(&item).unwrap();
    assert!(target.exists());
}

#[test]
fn what_cannot_be_trashed_is_refused_by_kind() {
    let t = Tree::new();
    let trash = t.trash();
    match trash.trash(&t.path("data/missing")) {
        Err(TrashError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
    let item = trash.trash(&t.file("a.txt", "")).unwrap();
    assert!(matches!(
        trash.trash(&item.file_path()),
        Err(TrashError::AlreadyTrashed)
    ));
    assert!(item.file_path().exists());
}

#[test]
fn the_list_has_every_record_by_name_with_its_decoded_path_and_date() {
    let t = Tree::new();
    assert!(t.trash().list().unwrap().is_empty(), "a missing trash");
    let dir = t.home_trash();
    fs::create_dir_all(dir.join("info")).unwrap();
    assert!(t.trash().list().unwrap().is_empty(), "an empty trash");
    put_item(
        &dir,
        "b.txt",
        "",
        "/home/u/b%20x.txt",
        "2026-01-02T03:04:05",
    );
    put_item(&dir, "a.txt", "", "/home/u/a.txt", DATE);
    // files that are not records are skipped
    fs::write(dir.join("info/notes.txt"), "not a record").unwrap();
    fs::write(dir.join("info/noext"), "not a record").unwrap();
    // a record without a date lists with an empty one
    fs::write(dir.join("info/c.trashinfo"), "[Trash Info]\nPath=/x/c\n").unwrap();
    let items = t.trash().list().unwrap();
    let listed: Vec<(&str, &Path, &str)> = items
        .iter()
        .map(|i| {
            (
                i.name.as_str(),
                i.original_path.as_path(),
                i.deletion_date.as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("a.txt", Path::new("/home/u/a.txt"), DATE),
            ("b.txt", Path::new("/home/u/b x.txt"), "2026-01-02T03:04:05"),
            ("c", Path::new("/x/c"), ""),
        ]
    );
    assert!(items.iter().all(|i| i.trash_dir == dir));
}

#[test]
fn a_record_without_a_path_or_a_header_is_a_bad_info_error() {
    let t = Tree::new();
    fs::create_dir_all(t.home_trash().join("info")).unwrap();
    let record = t.home_trash().join("info/a.trashinfo");
    fs::write(&record, "[Trash Info]\nDeletionDate=2026-01-01T00:00:00\n").unwrap();
    match t.trash().list() {
        Err(TrashError::BadInfo(why)) => assert!(why.contains("Path")),
        other => panic!("{other:?}"),
    }
    fs::write(&record, "Path=/x\n").unwrap();
    assert!(matches!(t.trash().list(), Err(TrashError::BadInfo(_))));
}

#[test]
fn a_volume_trash_directory_lists_paths_relative_to_its_mount_point() {
    let t = Tree::new();
    let own = t.volume_trash();
    put_item(&own, "a.txt", "", "docs/a.txt", DATE);
    put_item(&own, "abs.txt", "", "/elsewhere/abs.txt", DATE);
    let items = t.trash().list_dir(&own).unwrap();
    assert_eq!(items[0].original_path, t.vol().join("docs/a.txt"));
    assert_eq!(items[1].original_path, Path::new("/elsewhere/abs.txt"));
    // a shared trash is relative to the folder above `.Trash`
    let shared = t.vol().join(".Trash").join("1000");
    put_item(&shared, "a.txt", "", "docs/a.txt", DATE);
    let items = t.trash().list_dir(&shared).unwrap();
    assert_eq!(items[0].original_path, t.vol().join("docs/a.txt"));
}

#[test]
fn restoring_puts_the_item_back_and_forgets_its_record_and_size() {
    let t = Tree::new();
    let trash = t.trash();
    let file = t.file("deep/er/a.txt", "content");
    let item = trash.trash(&file).unwrap();
    // the folders that are gone are made again
    fs::remove_dir_all(t.path("data/deep")).unwrap();
    assert_eq!(trash.restore(&item).unwrap(), file);
    assert_eq!(fs::read_to_string(&file).unwrap(), "content");
    assert!(!item.info_path().exists());
    assert!(!item.file_path().exists());
    assert!(trash.list().unwrap().is_empty());

    t.file("dir/a", "xx");
    t.file("other/b", "yy");
    let dir = trash.trash(&t.path("data/dir")).unwrap();
    trash.trash(&t.path("data/other")).unwrap();
    trash.restore(&dir).unwrap();
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert_eq!(sizes.lines().count(), 1);
    assert!(sizes.trim_end().ends_with(" other"));
}

#[test]
fn restoring_onto_an_occupied_location_fails_and_changes_nothing() {
    let t = Tree::new();
    let trash = t.trash();
    let file = t.file("a.txt", "old");
    let item = trash.trash(&file).unwrap();
    fs::write(&file, "new").unwrap();
    match trash.restore(&item) {
        Err(TrashError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists),
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(&file).unwrap(), "new");
    assert!(item.file_path().exists());
    assert!(item.info_path().exists());
    // a dangling link in the way blocks it too
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink("/nonexistent/target", &file).unwrap();
    assert!(trash.restore(&item).is_err());
    assert!(item.file_path().exists());
}

#[test]
fn deleting_an_item_removes_it_its_record_and_its_size_and_may_be_repeated() {
    let t = Tree::new();
    let trash = t.trash();
    t.file("dir/a", "x");
    let dir = trash.trash(&t.path("data/dir")).unwrap();
    let file = trash.trash(&t.file("a.txt", "x")).unwrap();
    trash.delete(&dir).unwrap();
    trash.delete(&file).unwrap();
    trash.delete(&file).unwrap();
    assert!(trash.list().unwrap().is_empty());
    assert!(common::names(&t.home_trash().join("files")).is_empty());
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert!(sizes.is_empty());
}

#[test]
fn an_item_is_found_only_from_a_file_with_a_good_record_in_a_trash() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "a b.txt", "", "/home/u/a%20b.txt", DATE);
    let item = trashed_item(&dir.join("files/a b.txt")).unwrap();
    assert_eq!(item.name, "a b.txt");
    assert_eq!(item.trash_dir, dir);
    assert_eq!(item.original_path, Path::new("/home/u/a b.txt"));
    assert_eq!(item.deletion_date, DATE);

    fs::write(dir.join("files/orphan"), "").unwrap();
    fs::write(dir.join("info/bad.trashinfo"), "garbage").unwrap();
    fs::write(dir.join("files/bad"), "").unwrap();
    for path in [
        dir.join("info/a b.txt.trashinfo"),
        t.file("a.txt", ""),
        dir.join("files/orphan"),
        dir.join("files/bad"),
        "/".into(),
    ] {
        assert!(trashed_item(&path).is_none(), "{}", path.display());
    }
}

#[test]
fn forgetting_an_item_drops_its_record_and_size_line_only_and_is_quiet() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "a", "", "/x/a", DATE);
    put_item(&dir, "b", "", "/x/b", DATE);
    fs::write(dir.join("directorysizes"), "10 1 a\n20 2 b\n").unwrap();
    let item = trashed_item(&dir.join("files/a")).unwrap();
    fs::remove_file(item.file_path()).unwrap();
    forget(&item);
    forget(&item);
    assert!(!item.info_path().exists());
    assert!(dir.join("info/b.trashinfo").exists());
    assert_eq!(
        fs::read_to_string(dir.join("directorysizes")).unwrap(),
        "20 2 b\n"
    );

    let bare = Tree::new();
    put_item(&bare.home_trash(), "a", "", "/x/a", DATE);
    forget(&trashed_item(&bare.home_trash().join("files/a")).unwrap());
    assert!(!bare.home_trash().join("directorysizes").exists());
}

#[test]
fn items_are_purged_only_after_more_than_the_given_days() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "old", "", "/x/old", "2000-01-01T00:00:00");
    put_item(&dir, "bad-date", "", "/x/bad", "yesterday");
    let recent = t.trash().trash(&t.file("recent.txt", "")).unwrap();
    let trash = t.trash();
    assert_eq!(trash.purge_older_than(30, now()).unwrap(), 1);
    let left: Vec<String> = trash.list().unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(left, ["bad-date", "recent.txt"]);
    assert!(recent.file_path().exists());
    assert!(!dir.join("files/old").exists());
    assert_eq!(trash.purge_older_than(1, now()).unwrap(), 0);
    assert_eq!(trash.purge_older_than(1, now() + 2 * 86_400).unwrap(), 1);
    assert!(!recent.file_path().exists());
}

#[test]
fn a_path_is_percent_encoded_except_unreserved_characters_and_slashes_and_decodes_back() {
    for (text, encoded) in [
        ("/a/b-c_d.e~f", "/a/b-c_d.e~f"),
        ("a b", "a%20b"),
        ("100%", "100%25"),
        ("é", "%C3%A9"),
        ("a\nb", "a%0Ab"),
        ("#?[]", "%23%3F%5B%5D"),
        ("", ""),
        ("tab\there", "tab%09here"),
    ] {
        assert_eq!(encode_path(text), encoded, "{text:?}");
        assert_eq!(decode_path(encoded), text, "{encoded}");
    }
    assert_eq!(decode_path("a%FFb"), "a\u{fffd}b");
}

#[test]
fn a_local_time_has_the_shape_the_record_needs() {
    let text = format_local_time(1_700_000_000);
    assert_local_time_shape(&text);
    assert!(text.starts_with("2023-11-1"), "{text}");
}

#[test]
fn the_failure_message_names_one_failure_or_counts_several() {
    let reason = |path: &'static str, why: &str| (Path::new(path), why.to_string());
    for (failures, expected) in [
        (vec![], None),
        (
            vec![reason("/trash/files/a.txt", "Permission denied")],
            Some("\u{201c}a.txt\u{201d} could not be deleted: Permission denied."),
        ),
        (
            vec![
                reason("/t/files/a", "first reason"),
                reason("/t/files/b", "second reason"),
                reason("/t/files/c", "third reason"),
            ],
            Some("3 items could not be deleted. \u{201c}a\u{201d}: first reason."),
        ),
        // a path without a name shows the path
        (
            vec![reason("/", "busy")],
            Some("\u{201c}/\u{201d} could not be deleted: busy."),
        ),
    ] {
        assert_eq!(
            failure_message(failures.clone()).as_deref(),
            expected,
            "{failures:?}"
        );
    }
}
