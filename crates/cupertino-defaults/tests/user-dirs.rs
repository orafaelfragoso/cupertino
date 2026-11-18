//! The user directories of `user-dirs.dirs`: the assignments a text makes, and the file the session reads them from.

use std::path::{Path, PathBuf};

use cupertino_defaults::user_dirs::{parse, read};

fn assigned(pairs: &[(&str, &str)]) -> Vec<(String, PathBuf)> {
    pairs
        .iter()
        .map(|(name, path)| (name.to_string(), PathBuf::from(path)))
        .collect()
}

#[test]
fn a_text_assigns_the_directories_xdg_user_dirs_reads() {
    let home = Path::new("/home/avery");
    for (row, text, expected) in [
        (
            "under the home folder",
            "XDG_DOCUMENTS_DIR=\"$HOME/Documents\"",
            vec![("DOCUMENTS", "/home/avery/Documents")],
        ),
        (
            "an absolute path",
            "XDG_MUSIC_DIR=\"/srv/music\"",
            vec![("MUSIC", "/srv/music")],
        ),
        (
            "the home folder itself",
            "XDG_DESKTOP_DIR=\"$HOME\"",
            vec![("DESKTOP", "/home/avery")],
        ),
        (
            "white space around the line and the sign",
            "  XDG_VIDEOS_DIR = \"$HOME/Videos\"  ",
            vec![("VIDEOS", "/home/avery/Videos")],
        ),
        (
            "a comment, a relative path, no quotes, another key",
            "# XDG_MUSIC_DIR=\"$HOME/Music\"\nXDG_MUSIC_DIR=\"Music\"\nXDG_MUSIC_DIR=$HOME/Music\nMUSIC_DIR=\"/m\"",
            vec![],
        ),
        (
            "every line, in order, repeated keys kept",
            "XDG_DOWNLOAD_DIR=\"$HOME/a\"\n\nXDG_DOWNLOAD_DIR=\"$HOME/b\"",
            vec![
                ("DOWNLOAD", "/home/avery/a"),
                ("DOWNLOAD", "/home/avery/b"),
            ],
        ),
    ] {
        assert_eq!(parse(text, home), assigned(&expected), "{row}");
    }
}

#[test]
fn the_session_file_is_in_an_absolute_xdg_config_home_else_in_the_home_folders_config() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let xdg = dir.path().join("xdg");
    for folder in [home.join(".config"), xdg.clone()] {
        std::fs::create_dir_all(&folder).unwrap();
    }
    let in_home = home.join("Home documents");
    let in_xdg = home.join("Xdg documents");
    std::fs::write(
        home.join(".config/user-dirs.dirs"),
        "XDG_DOCUMENTS_DIR=\"$HOME/Home documents\"\n",
    )
    .unwrap();
    std::fs::write(
        xdg.join("user-dirs.dirs"),
        "XDG_DOCUMENTS_DIR=\"$HOME/Xdg documents\"\n",
    )
    .unwrap();
    let unread = dir.path().join("unread");
    // a folder where the file should be: there, and it cannot be read
    std::fs::create_dir_all(unread.join("user-dirs.dirs")).unwrap();
    let documents = |path: &Path| Some(vec![("DOCUMENTS".to_string(), path.to_path_buf())]);
    for (row, xdg_config_home, expected) in [
        (
            "an absolute XDG_CONFIG_HOME",
            Some(xdg.clone()),
            documents(&in_xdg),
        ),
        ("no XDG_CONFIG_HOME", None, documents(&in_home)),
        (
            "an empty XDG_CONFIG_HOME",
            Some(PathBuf::new()),
            documents(&in_home),
        ),
        (
            "a relative XDG_CONFIG_HOME is passed over",
            Some(PathBuf::from("xdg")),
            documents(&in_home),
        ),
        (
            "a missing file assigns nothing",
            Some(dir.path().join("nowhere")),
            Some(Vec::new()),
        ),
        (
            "a file that cannot be read is an error",
            Some(unread.clone()),
            None,
        ),
    ] {
        match &xdg_config_home {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        assert_eq!(read(&home).ok(), expected, "{row}");
    }
}
