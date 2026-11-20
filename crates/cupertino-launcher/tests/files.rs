//! The file index (`files::FileIndex`) a host builds off the UI thread, and the folder Reveal opens.

#![expect(
    clippy::disallowed_methods,
    reason = "the test makes and removes its own files; it has no UI thread"
)]

use std::path::{Path, PathBuf};

use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::Launcher;

#[test]
fn the_index_holds_the_files_under_its_root_but_hidden_ones() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("launcher-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("Documents/.secret")).unwrap();
    std::fs::write(root.join("Documents/report.txt"), "text").unwrap();
    std::fs::write(root.join("Documents/.secret/key.txt"), "text").unwrap();
    let index = FileIndex::build(FileSearchConfig::new(root.clone()));
    let names: Vec<&str> = index.entries().iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"Documents"), "{names:?}");
    assert!(names.contains(&"report.txt"), "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| n.contains("secret") || *n == "key.txt"),
        "{names:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn reveal_opens_a_folder_itself_and_a_files_folder() {
    let file = Path::new("/home/user/Documents/report.txt");
    assert_eq!(
        Launcher::reveal_folder(file, false),
        Path::new("/home/user/Documents")
    );
    let folder = Path::new("/home/user/Documents");
    assert_eq!(Launcher::reveal_folder(folder, true), folder);
}
