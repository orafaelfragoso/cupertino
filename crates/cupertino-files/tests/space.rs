//! What a folder's file system says, as the program reads it on an I/O task.

use cupertino_files::space::{mounted_read_only, nearest_existing_ancestor, read, Space};
use cupertino_kit::files::{applications, recents};

#[test]
fn a_folder_of_the_file_system_tells_its_room_and_whether_it_is_mounted_read_only() {
    let root = tempfile::tempdir().unwrap();
    let space = read(root.path()).unwrap();
    assert!(space.available.is_some_and(|bytes| bytes > 0));
    assert!(!space.read_only);
    assert!(!mounted_read_only(root.path()).unwrap());
    assert!(read(&root.path().join("missing")).is_err());
    assert!(mounted_read_only(&root.path().join("missing")).is_err());
}

#[test]
fn a_virtual_folder_has_no_room_and_applications_takes_nothing_new() {
    assert_eq!(
        read(recents::location()),
        Ok(Space {
            available: None,
            read_only: false
        })
    );
    assert_eq!(
        read(applications::location()),
        Ok(Space {
            available: None,
            read_only: true
        })
    );
}

#[test]
fn the_nearest_existing_ancestor_skips_the_folders_that_are_gone() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("a")).unwrap();
    let gone = root.path().join("a/b/c");
    assert_eq!(
        nearest_existing_ancestor(&gone),
        Some(root.path().join("a"))
    );
    // a folder that exists is not its own ancestor
    assert_eq!(
        nearest_existing_ancestor(&root.path().join("a")),
        Some(root.path().to_path_buf())
    );
    assert_eq!(nearest_existing_ancestor(std::path::Path::new("/")), None);
}
