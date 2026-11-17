//! Moving items to the trash, listing, restoring, deleting and the helpers of the trashinfo records.

mod common;

use common::{put_item, Tree};
use cupertino_trash::{
    decode_path, encode_path, failure_message, forget, format_local_time, trashed_item, TrashError,
};
use std::fs;
use std::path::Path;

#[test]
fn a_file_moves_into_the_home_trash_with_a_record() {
    let t = Tree::new();
    let file = t.file("note.txt", "hello");
    let item = t.trash().trash(&file).unwrap();
    assert_eq!(item.name, "note.txt");
    assert_eq!(item.trash_dir, t.home_trash());
    assert_eq!(item.original_path, file);
    assert!(!file.exists());
    assert_eq!(fs::read_to_string(item.file_path()).unwrap(), "hello");
    assert_eq!(item.file_path(), t.home_trash().join("files/note.txt"));
    assert_eq!(
        item.info_path(),
        t.home_trash().join("info/note.txt.trashinfo")
    );
}

#[test]
fn the_record_has_the_encoded_path_and_a_local_deletion_date() {
    let t = Tree::new();
    let file = t.file("a b#c%d.txt", "");
    let item = t.trash().trash(&file).unwrap();
    let info = fs::read_to_string(item.info_path()).unwrap();
    let lines: Vec<&str> = info.lines().collect();
    assert_eq!(lines[0], "[Trash Info]");
    let path = lines[1].strip_prefix("Path=").unwrap();
    assert!(path.ends_with("/a%20b%23c%25d.txt"), "{path}");
    assert!(path.starts_with('/'));
    let date = lines[2].strip_prefix("DeletionDate=").unwrap();
    assert_eq!(date, item.deletion_date);
    assert_eq!(date.len(), 19);
    assert_eq!(&date[4..5], "-");
    assert_eq!(&date[10..11], "T");
    assert_eq!(&date[13..14], ":");
}

#[test]
fn a_name_that_is_taken_gets_a_counter_before_its_extension() {
    let t = Tree::new();
    let trash = t.trash();
    let names: Vec<String> = ["one", "two", "three"]
        .iter()
        .map(|dir| {
            let file = t.file(&format!("{dir}/a.tar.gz"), "");
            trash.trash(&file).unwrap().name
        })
        .collect();
    assert_eq!(names, ["a.tar.gz", "a.tar 2.gz", "a.tar 3.gz"]);
}

#[test]
fn a_name_without_an_extension_or_with_a_leading_dot_gets_the_counter_at_the_end() {
    let t = Tree::new();
    let trash = t.trash();
    let mut names = Vec::new();
    for dir in ["one", "two"] {
        for name in ["README", ".profile"] {
            let file = t.file(&format!("{dir}/{name}"), "");
            names.push(trash.trash(&file).unwrap().name);
        }
    }
    assert_eq!(names, ["README", ".profile", "README 2", ".profile 2"]);
}

#[test]
fn a_name_whose_file_remains_in_the_trash_is_not_reused() {
    let t = Tree::new();
    // a file without a record still holds its name
    fs::create_dir_all(t.home_trash().join("files")).unwrap();
    fs::write(t.home_trash().join("files/a.txt"), "old").unwrap();
    let item = t.trash().trash(&t.file("a.txt", "new")).unwrap();
    assert_eq!(item.name, "a 2.txt");
    assert_eq!(
        fs::read_to_string(t.home_trash().join("files/a.txt")).unwrap(),
        "old"
    );
    assert!(!t.home_trash().join("info/a.txt.trashinfo").exists());
}

#[test]
fn a_folder_is_moved_whole_and_its_size_is_recorded() {
    let t = Tree::new();
    t.file("dir/a.bin", "12345");
    t.file("dir/sub/b.bin", "123");
    let item = t.trash().trash(&t.path("data/dir")).unwrap();
    assert_eq!(
        fs::read_to_string(item.file_path().join("sub/b.bin")).unwrap(),
        "123"
    );
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    let fields: Vec<&str> = sizes.trim_end().split(' ').collect();
    assert_eq!(fields[0], "8");
    assert_eq!(fields[2], "dir");
    assert!(fields[1].parse::<i64>().is_ok());
    assert!(sizes.ends_with('\n'));
}

#[test]
fn a_recorded_size_names_the_folder_percent_encoded() {
    let t = Tree::new();
    t.file("my dir/a", "xy");
    t.trash().trash(&t.path("data/my dir")).unwrap();
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert!(sizes.starts_with("2 "), "{sizes}");
    assert!(sizes.trim_end().ends_with(" my%20dir"), "{sizes}");
}

#[test]
fn a_plain_file_adds_no_size_line() {
    let t = Tree::new();
    t.trash().trash(&t.file("a.txt", "x")).unwrap();
    assert!(!t.home_trash().join("directorysizes").exists());
}

#[test]
fn the_size_of_a_folder_can_be_left_for_later() {
    let t = Tree::new();
    t.file("dir/a.bin", "1234");
    let (item, pending) = t.trash().trash_unsized(&t.path("data/dir")).unwrap();
    assert!(!t.home_trash().join("directorysizes").exists());
    pending.unwrap().record().unwrap();
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert!(sizes.starts_with("4 "), "{sizes}");
    assert!(item.file_path().is_dir());
}

#[test]
fn a_file_trashed_unsized_has_no_pending_size() {
    let t = Tree::new();
    let (_, pending) = t.trash().trash_unsized(&t.file("a.txt", "x")).unwrap();
    assert!(pending.is_none());
}

#[test]
fn a_pending_size_of_a_folder_that_left_the_trash_writes_nothing() {
    let t = Tree::new();
    t.file("dir/a.bin", "1234");
    let trash = t.trash();
    let (item, pending) = trash.trash_unsized(&t.path("data/dir")).unwrap();
    trash.restore(&item).unwrap();
    pending.unwrap().record().unwrap();
    assert!(!t.home_trash().join("directorysizes").exists());
}

#[test]
fn a_symbolic_link_is_trashed_itself() {
    let t = Tree::new();
    let target = t.file("target.txt", "keep");
    let link = t.path("data/link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let item = t.trash().trash(&link).unwrap();
    assert!(target.exists());
    assert!(fs::symlink_metadata(item.file_path())
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(fs::symlink_metadata(&link).is_err());
}

#[test]
fn a_path_that_does_not_exist_is_an_io_error() {
    let t = Tree::new();
    match t.trash().trash(&t.path("data/missing")) {
        Err(TrashError::Io(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_item_already_in_the_trash_is_refused() {
    let t = Tree::new();
    let trash = t.trash();
    let item = trash.trash(&t.file("a.txt", "")).unwrap();
    assert!(matches!(
        trash.trash(&item.file_path()),
        Err(TrashError::AlreadyTrashed)
    ));
    assert!(item.file_path().exists());
}

#[test]
fn the_list_has_every_item_by_name_with_its_original_path_and_date() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(
        &dir,
        "b.txt",
        "",
        "/home/u/b%20x.txt",
        "2026-01-02T03:04:05",
    );
    put_item(&dir, "a.txt", "", "/home/u/a.txt", "2026-01-01T00:00:00");
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
            ("a.txt", Path::new("/home/u/a.txt"), "2026-01-01T00:00:00"),
            ("b.txt", Path::new("/home/u/b x.txt"), "2026-01-02T03:04:05"),
        ]
    );
    assert!(items.iter().all(|i| i.trash_dir == dir));
}

#[test]
fn an_empty_or_missing_trash_lists_nothing() {
    let t = Tree::new();
    assert!(t.trash().list().unwrap().is_empty());
    fs::create_dir_all(t.home_trash().join("info")).unwrap();
    assert!(t.trash().list().unwrap().is_empty());
}

#[test]
fn files_in_the_info_folder_that_are_not_records_are_skipped() {
    let t = Tree::new();
    put_item(
        &t.home_trash(),
        "a.txt",
        "",
        "/x/a.txt",
        "2026-01-01T00:00:00",
    );
    fs::write(t.home_trash().join("info/notes.txt"), "not a record").unwrap();
    fs::write(t.home_trash().join("info/noext"), "not a record").unwrap();
    assert_eq!(t.trash().list().unwrap().len(), 1);
}

#[test]
fn a_record_without_a_path_is_a_bad_info_error() {
    let t = Tree::new();
    fs::create_dir_all(t.home_trash().join("info")).unwrap();
    fs::write(
        t.home_trash().join("info/a.trashinfo"),
        "[Trash Info]\nDeletionDate=2026-01-01T00:00:00\n",
    )
    .unwrap();
    match t.trash().list() {
        Err(TrashError::BadInfo(why)) => assert!(why.contains("Path")),
        other => panic!("{other:?}"),
    }
    fs::write(t.home_trash().join("info/a.trashinfo"), "Path=/x\n").unwrap();
    assert!(matches!(t.trash().list(), Err(TrashError::BadInfo(_))));
}

#[test]
fn a_record_without_a_date_lists_with_an_empty_date() {
    let t = Tree::new();
    fs::create_dir_all(t.home_trash().join("info")).unwrap();
    fs::write(
        t.home_trash().join("info/a.trashinfo"),
        "[Trash Info]\nPath=/x/a\n",
    )
    .unwrap();
    let items = t.trash().list().unwrap();
    assert_eq!(items[0].deletion_date, "");
}

#[test]
fn a_volume_trash_directory_lists_paths_relative_to_its_mount_point() {
    let t = Tree::new();
    let own = t.volume_trash();
    put_item(&own, "a.txt", "", "docs/a.txt", "2026-01-01T00:00:00");
    put_item(
        &own,
        "abs.txt",
        "",
        "/elsewhere/abs.txt",
        "2026-01-01T00:00:00",
    );
    let items = t.trash().list_dir(&own).unwrap();
    assert_eq!(items[0].original_path, t.vol().join("docs/a.txt"));
    assert_eq!(items[1].original_path, Path::new("/elsewhere/abs.txt"));
}

#[test]
fn a_shared_volume_trash_directory_is_relative_to_the_folder_above_dot_trash() {
    let t = Tree::new();
    let shared = t.vol().join(".Trash").join("1000");
    put_item(&shared, "a.txt", "", "docs/a.txt", "2026-01-01T00:00:00");
    let items = t.trash().list_dir(&shared).unwrap();
    assert_eq!(items[0].original_path, t.vol().join("docs/a.txt"));
}

#[test]
fn restoring_puts_the_item_back_and_forgets_its_record() {
    let t = Tree::new();
    let trash = t.trash();
    let file = t.file("a.txt", "content");
    let item = trash.trash(&file).unwrap();
    assert_eq!(trash.restore(&item).unwrap(), file);
    assert_eq!(fs::read_to_string(&file).unwrap(), "content");
    assert!(!item.info_path().exists());
    assert!(!item.file_path().exists());
    assert!(trash.list().unwrap().is_empty());
}

#[test]
fn restoring_a_folder_drops_its_size_line() {
    let t = Tree::new();
    let trash = t.trash();
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
}

#[test]
fn a_link_in_the_way_also_blocks_a_restore() {
    let t = Tree::new();
    let trash = t.trash();
    let file = t.file("a.txt", "old");
    let item = trash.trash(&file).unwrap();
    std::os::unix::fs::symlink("/nonexistent/target", &file).unwrap();
    assert!(trash.restore(&item).is_err());
}

#[test]
fn restoring_makes_the_folders_that_are_gone() {
    let t = Tree::new();
    let trash = t.trash();
    let file = t.file("deep/er/a.txt", "x");
    let item = trash.trash(&file).unwrap();
    fs::remove_dir_all(t.path("data/deep")).unwrap();
    trash.restore(&item).unwrap();
    assert!(file.exists());
}

#[test]
fn deleting_an_item_removes_it_and_its_record() {
    let t = Tree::new();
    let trash = t.trash();
    t.file("dir/a", "x");
    let dir = trash.trash(&t.path("data/dir")).unwrap();
    let file = trash.trash(&t.file("a.txt", "x")).unwrap();
    trash.delete(&dir).unwrap();
    trash.delete(&file).unwrap();
    assert!(trash.list().unwrap().is_empty());
    assert!(common::names(&t.home_trash().join("files")).is_empty());
    let sizes = fs::read_to_string(t.home_trash().join("directorysizes")).unwrap();
    assert!(sizes.is_empty());
}

#[test]
fn deleting_an_item_that_is_gone_is_not_an_error() {
    let t = Tree::new();
    let trash = t.trash();
    let item = trash.trash(&t.file("a.txt", "x")).unwrap();
    trash.delete(&item).unwrap();
    trash.delete(&item).unwrap();
}

#[test]
fn deleting_a_link_leaves_what_it_points_at() {
    let t = Tree::new();
    let trash = t.trash();
    let target = t.file("target.txt", "keep");
    let link = t.path("data/link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let item = trash.trash(&link).unwrap();
    trash.delete(&item).unwrap();
    assert!(target.exists());
}

#[test]
fn an_item_is_found_from_its_place_in_a_trash() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(
        &dir,
        "a b.txt",
        "",
        "/home/u/a%20b.txt",
        "2026-01-01T00:00:00",
    );
    let item = trashed_item(&dir.join("files/a b.txt")).unwrap();
    assert_eq!(item.name, "a b.txt");
    assert_eq!(item.trash_dir, dir);
    assert_eq!(item.original_path, Path::new("/home/u/a b.txt"));
    assert_eq!(item.deletion_date, "2026-01-01T00:00:00");
}

#[test]
fn a_path_outside_a_files_folder_or_without_a_record_is_no_trashed_item() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "a.txt", "", "/x/a.txt", "2026-01-01T00:00:00");
    assert!(trashed_item(&dir.join("info/a.txt.trashinfo")).is_none());
    assert!(trashed_item(&t.file("a.txt", "")).is_none());
    fs::write(dir.join("files/orphan"), "").unwrap();
    assert!(trashed_item(&dir.join("files/orphan")).is_none());
    assert!(trashed_item(Path::new("/")).is_none());
    fs::write(dir.join("info/bad.trashinfo"), "garbage").unwrap();
    fs::write(dir.join("files/bad"), "").unwrap();
    assert!(trashed_item(&dir.join("files/bad")).is_none());
}

#[test]
fn forgetting_an_item_drops_its_record_and_size_line_only() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "a", "", "/x/a", "2026-01-01T00:00:00");
    put_item(&dir, "b", "", "/x/b", "2026-01-01T00:00:00");
    fs::write(dir.join("directorysizes"), "10 1 a\n20 2 b\n").unwrap();
    let item = trashed_item(&dir.join("files/a")).unwrap();
    fs::remove_file(item.file_path()).unwrap();
    forget(&item);
    assert!(!item.info_path().exists());
    assert!(dir.join("info/b.trashinfo").exists());
    assert_eq!(
        fs::read_to_string(dir.join("directorysizes")).unwrap(),
        "20 2 b\n"
    );
}

#[test]
fn forgetting_an_item_of_a_trash_without_sizes_is_quiet() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "a", "", "/x/a", "2026-01-01T00:00:00");
    let item = trashed_item(&dir.join("files/a")).unwrap();
    forget(&item);
    forget(&item);
    assert!(!dir.join("directorysizes").exists());
}

#[test]
fn old_items_are_purged_and_recent_ones_stay() {
    let t = Tree::new();
    let dir = t.home_trash();
    put_item(&dir, "old", "", "/x/old", "2000-01-01T00:00:00");
    put_item(&dir, "bad-date", "", "/x/bad", "yesterday");
    let recent = t.trash().trash(&t.file("recent.txt", "")).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let trash = t.trash();
    assert_eq!(trash.purge_older_than(30, now).unwrap(), 1);
    let left: Vec<String> = trash.list().unwrap().into_iter().map(|i| i.name).collect();
    assert_eq!(left, ["bad-date", "recent.txt"]);
    assert!(recent.file_path().exists());
    assert!(!dir.join("files/old").exists());
}

#[test]
fn an_item_is_purged_only_after_more_than_the_given_days() {
    let t = Tree::new();
    let trash = t.trash();
    trash.trash(&t.file("a.txt", "")).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert_eq!(trash.purge_older_than(1, now).unwrap(), 0);
    assert_eq!(trash.purge_older_than(1, now + 2 * 86_400).unwrap(), 1);
    assert!(trash.list().unwrap().is_empty());
}

#[test]
fn a_path_is_percent_encoded_except_unreserved_characters_and_slashes() {
    assert_eq!(encode_path("/a/b-c_d.e~f"), "/a/b-c_d.e~f");
    assert_eq!(encode_path("a b"), "a%20b");
    assert_eq!(encode_path("100%"), "100%25");
    assert_eq!(encode_path("é"), "%C3%A9");
    assert_eq!(encode_path("a\nb"), "a%0Ab");
    assert_eq!(encode_path("#?[]"), "%23%3F%5B%5D");
}

#[test]
fn decoding_undoes_encoding() {
    for text in ["/a b/é/100%/x#y", "plain", "", "tab\there"] {
        assert_eq!(decode_path(&encode_path(text)), text);
    }
}

#[test]
fn decoding_replaces_bytes_that_are_not_utf8() {
    assert_eq!(decode_path("a%FFb"), "a\u{fffd}b");
}

#[test]
fn a_local_time_has_the_shape_the_record_needs() {
    let text = format_local_time(1_700_000_000);
    assert_eq!(text.len(), 19);
    assert!(text.starts_with("2023-11-1"), "{text}");
    assert_eq!(&text[4..5], "-");
    assert_eq!(&text[10..11], "T");
}

#[test]
fn nothing_failed_gives_no_message() {
    assert_eq!(failure_message(std::iter::empty()), None);
}

#[test]
fn one_failure_names_the_item_and_the_reason() {
    let message = failure_message([(
        Path::new("/trash/files/a.txt"),
        "Permission denied".to_string(),
    )]);
    assert_eq!(
        message.as_deref(),
        Some("\u{201c}a.txt\u{201d} could not be deleted: Permission denied.")
    );
}

#[test]
fn several_failures_give_their_count_and_the_first_reason() {
    let message = failure_message([
        (Path::new("/t/files/a"), "first reason".to_string()),
        (Path::new("/t/files/b"), "second reason".to_string()),
        (Path::new("/t/files/c"), "third reason".to_string()),
    ]);
    assert_eq!(
        message.as_deref(),
        Some("3 items could not be deleted. \u{201c}a\u{201d}: first reason.")
    );
}

#[test]
fn a_failure_of_a_path_without_a_name_shows_the_path() {
    let message = failure_message([(Path::new("/"), "busy".to_string())]);
    assert_eq!(
        message.as_deref(),
        Some("\u{201c}/\u{201d} could not be deleted: busy.")
    );
}
