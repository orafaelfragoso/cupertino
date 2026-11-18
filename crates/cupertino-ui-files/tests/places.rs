//! The places of a file chooser's sidebar.

use std::path::{Path, PathBuf};

use cupertino_ui_files::places::chooser_places;

#[test]
fn a_chooser_lists_home_then_the_user_directories_that_exist() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("avery");
    for folder in [
        "Desktop",
        "Documents",
        "Downloads",
        "Music",
        "Elsewhere/Pictures",
    ] {
        std::fs::create_dir_all(home.join(folder)).unwrap();
    }
    let assign = |pairs: &[(&str, &Path)]| -> Vec<(String, PathBuf)> {
        pairs
            .iter()
            .map(|(name, path)| (name.to_string(), path.to_path_buf()))
            .collect()
    };
    let pictures = home.join("Elsewhere/Pictures");
    let home_row = ("Home", "symbol.house", home.clone());
    for (row, assigned, expected) in [
        (
            "nothing assigned: home and the desktop, which defaults to Desktop",
            assign(&[]),
            vec![
                home_row.clone(),
                ("Desktop", "symbol.desktop", home.join("Desktop")),
            ],
        ),
        (
            "every directory assigned, in the chooser's order; Videos does not exist",
            assign(&[
                ("VIDEOS", &home.join("Videos")),
                ("PICTURES", &pictures),
                ("MUSIC", &home.join("Music")),
                ("DOWNLOAD", &home.join("Downloads")),
                ("DOCUMENTS", &home.join("Documents")),
                ("DESKTOP", &home.join("Desktop")),
            ]),
            vec![
                home_row.clone(),
                ("Desktop", "symbol.desktop", home.join("Desktop")),
                ("Documents", "symbol.document", home.join("Documents")),
                ("Downloads", "symbol.download", home.join("Downloads")),
                ("Music", "symbol.music", home.join("Music")),
                ("Pictures", "symbol.photo", pictures.clone()),
            ],
        ),
        (
            "a directory set to the home folder is disabled; the last assignment of a name wins",
            assign(&[
                ("DESKTOP", &home),
                ("DOCUMENTS", &home.join("Downloads")),
                ("DOCUMENTS", &home.join("Documents")),
            ]),
            vec![
                home_row.clone(),
                ("Documents", "symbol.document", home.join("Documents")),
            ],
        ),
    ] {
        let rows: Vec<(String, &str, PathBuf)> = chooser_places(&home, &assigned)
            .into_iter()
            .map(|p| (p.name, p.kind.symbol(), p.path))
            .collect();
        let expected: Vec<(String, &str, PathBuf)> = expected
            .into_iter()
            .map(|(name, symbol, path)| (name.to_string(), symbol, path))
            .collect();
        assert_eq!(rows, expected, "{row}");
    }
}
