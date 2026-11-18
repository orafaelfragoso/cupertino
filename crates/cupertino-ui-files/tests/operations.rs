//! The file operations that are not a transfer: a rename that never replaces, a new folder with the first free name,
//! a folder made around items, an alias, and what a drop does by the keys held.

#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use cupertino_ui_files::operations::{
    drop_action, make_alias, new_folder, new_folder_with, rename_item, same_volume, DropAction,
};
use scratch::Scratch;

fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_rename_moves_the_item_in_its_folder_and_never_replaces_another() {
    let scratch = Scratch::new("ops-rename");
    let file = scratch.file("a.txt", b"first");
    let folder = scratch.folder("pictures");
    scratch.file("pictures/one.png", b"png");
    scratch.file("taken.txt", b"other");

    let renamed = rename_item(&file, "b.txt").unwrap();
    assert_eq!(renamed, scratch.path().join("b.txt"));
    assert_eq!(fs::read(&renamed).unwrap(), b"first");
    assert!(!file.exists());

    let folder_renamed = rename_item(&folder, "photos").unwrap();
    assert_eq!(listing(&folder_renamed), ["one.png"]);

    let clash = rename_item(&renamed, "taken.txt").unwrap_err();
    assert_eq!(clash.kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&renamed).unwrap(), b"first");
    assert_eq!(
        fs::read(scratch.path().join("taken.txt")).unwrap(),
        b"other"
    );

    let missing = rename_item(&scratch.path().join("gone"), "x").unwrap_err();
    assert_eq!(missing.kind(), ErrorKind::NotFound);
}

#[test]
fn a_new_folder_takes_the_first_free_numbered_name() {
    let scratch = Scratch::new("ops-new-folder");
    let made: Vec<PathBuf> = (0..2)
        .map(|_| new_folder(scratch.path()).unwrap())
        .collect();
    assert_eq!(
        made,
        [
            scratch.path().join("untitled folder"),
            scratch.path().join("untitled folder 2")
        ]
    );
    scratch.folder("untitled folder 4");
    assert_eq!(
        new_folder(scratch.path()).unwrap(),
        scratch.path().join("untitled folder 3")
    );
    assert!(made.iter().all(|folder| folder.is_dir()));
    assert!(new_folder(&scratch.path().join("missing")).is_err());
}

#[test]
fn a_folder_made_around_items_holds_them_and_an_item_that_fails_is_reported() {
    let scratch = Scratch::new("ops-folder-with");
    let a = scratch.file("a.txt", b"a");
    let b = scratch.file("b.txt", b"b");
    let folder = new_folder_with(scratch.path(), &[a.clone(), b.clone()]).unwrap();
    assert_eq!(folder, scratch.path().join("untitled folder"));
    assert_eq!(listing(&folder), ["a.txt", "b.txt"]);
    assert_eq!(listing(scratch.path()), ["untitled folder"]);

    let c = scratch.file("c.txt", b"c");
    let gone = scratch.path().join("gone.txt");
    let failed = new_folder_with(scratch.path(), &[c, gone]).unwrap_err();
    assert_eq!(failed.kind(), ErrorKind::NotFound);
    assert_eq!(
        listing(&scratch.path().join("untitled folder 2")),
        ["c.txt"]
    );
}

#[test]
fn an_alias_is_a_link_by_absolute_path_named_after_its_item_and_the_first_free_number() {
    let scratch = Scratch::new("ops-alias");
    let file = scratch.file("deep/notes.txt", b"notes");
    let folder = scratch.folder("deep/inner");
    scratch.file("deep/inner/x", b"x");

    let first = make_alias(&file).unwrap();
    let second = make_alias(&file).unwrap();
    assert_eq!(first, scratch.path().join("deep/notes.txt alias"));
    assert_eq!(second, scratch.path().join("deep/notes.txt alias 2"));
    assert_eq!(
        fs::read_link(&first).unwrap(),
        fs::canonicalize(&file).unwrap()
    );
    assert_eq!(fs::read(&second).unwrap(), b"notes");

    let of_folder = make_alias(&folder).unwrap();
    assert_eq!(listing(&of_folder), ["x"]);

    // a link made of a link reaches the item itself
    let of_link = make_alias(&first).unwrap();
    assert_eq!(
        fs::read_link(&of_link).unwrap(),
        fs::canonicalize(&file).unwrap()
    );

    assert_eq!(
        make_alias(&scratch.path().join("gone")).unwrap_err().kind(),
        ErrorKind::NotFound
    );
    assert_eq!(
        make_alias(std::path::Path::new("/")).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
}

#[test]
fn a_drop_copies_with_alt_moves_with_super_and_else_moves_within_a_volume_only() {
    let rows = [
        (true, false, false, DropAction::Move),
        (false, false, false, DropAction::Copy),
        (true, true, false, DropAction::Copy),
        (false, true, false, DropAction::Copy),
        (false, false, true, DropAction::Move),
        (true, false, true, DropAction::Move),
        (true, true, true, DropAction::Copy),
    ];
    for (same, alt, super_key, expected) in rows {
        assert_eq!(
            drop_action(same, alt, super_key),
            expected,
            "same volume {same}, alt {alt}, super {super_key}"
        );
    }
}

#[test]
fn two_paths_of_one_folder_are_on_one_volume_and_a_missing_one_is_an_error() {
    let scratch = Scratch::new("ops-volume");
    let file = scratch.file("deep/a.txt", b"a");
    assert!(same_volume(&file, scratch.path()).unwrap());
    assert!(same_volume(&file, &scratch.path().join("gone")).is_err());
}
