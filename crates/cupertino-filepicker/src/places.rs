//! The folders of a chooser's sidebar: the home folder and the user directories of `user-dirs.dirs`.

use std::path::{Path, PathBuf};

/// One sidebar row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub name: &'static str,
    pub path: PathBuf,
    /// The semantic name of the row's template symbol.
    pub symbol: &'static str,
}

/// The user directories listed, in sidebar order: the `user-dirs.dirs` key, the row name and the symbol.
const USER_DIRS: [(&str, &str, &str); 6] = [
    ("DESKTOP", "Desktop", "symbol.desktop"),
    ("DOCUMENTS", "Documents", "symbol.document"),
    ("DOWNLOAD", "Downloads", "symbol.download"),
    ("MUSIC", "Music", "symbol.music"),
    ("PICTURES", "Pictures", "symbol.photo"),
    ("VIDEOS", "Videos", "symbol.film"),
];

/// The `XDG_<NAME>_DIR` assignments of a `user-dirs.dirs` file as `(NAME, path)`, `$HOME` expanded against `home`.
/// Lines that are comments, malformed or relative without `$HOME/` are skipped, as xdg-user-dirs does.
pub fn parse_user_dirs(text: &str, home: &Path) -> Vec<(String, PathBuf)> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let name = key.trim().strip_prefix("XDG_")?.strip_suffix("_DIR")?;
            let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
            let path = if value == "$HOME" {
                home.to_path_buf()
            } else if let Some(rest) = value.strip_prefix("$HOME/") {
                home.join(rest)
            } else if value.starts_with('/') {
                PathBuf::from(value)
            } else {
                return None;
            };
            Some((name.to_string(), path))
        })
        .collect()
}

/// The home folder, then the user directories of `user_dirs` (the text of `user-dirs.dirs`) that exist; one that is
/// disabled (set to the home folder) is left out, and the desktop defaults to `~/Desktop`.
pub fn places(home: &Path, user_dirs: &str) -> Vec<Place> {
    let assigned = parse_user_dirs(user_dirs, home);
    let mut out = vec![Place {
        name: "Home",
        path: home.to_path_buf(),
        symbol: "symbol.house",
    }];
    for (key, name, symbol) in USER_DIRS {
        let path = match assigned.iter().rev().find(|(k, _)| k == key) {
            Some((_, p)) => p.clone(),
            None if key == "DESKTOP" => home.join("Desktop"),
            None => continue,
        };
        if path != home && path.is_dir() {
            out.push(Place { name, path, symbol });
        }
    }
    out
}

/// [`places`] of this session: `$XDG_CONFIG_HOME/user-dirs.dirs`, else `~/.config/user-dirs.dirs`.
pub fn session_places(home: &Path) -> Vec<Place> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let text = std::fs::read_to_string(config.join("user-dirs.dirs")).unwrap_or_default();
    places(home, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_are_home_then_the_user_directories_that_exist() {
        let home = tempfile::tempdir().unwrap();
        for dir in ["Desktop", "Docs", "Bilder"] {
            std::fs::create_dir(home.path().join(dir)).unwrap();
        }
        let text = "# written by xdg-user-dirs-update\n\
                    XDG_DOCUMENTS_DIR=\"$HOME/Docs\"\n\
                    XDG_PICTURES_DIR=\"$HOME/Bilder\"\n\
                    XDG_MUSIC_DIR=\"$HOME\"\n\
                    XDG_VIDEOS_DIR=\"$HOME/Missing\"\n\
                    XDG_DOWNLOAD_DIR=relative\n";
        let rows: Vec<(&str, PathBuf)> = places(home.path(), text)
            .into_iter()
            .map(|p| (p.name, p.path))
            .collect();
        assert_eq!(
            rows,
            [
                ("Home", home.path().to_path_buf()),
                ("Desktop", home.path().join("Desktop")),
                ("Documents", home.path().join("Docs")),
                ("Pictures", home.path().join("Bilder")),
            ]
        );
    }
}
