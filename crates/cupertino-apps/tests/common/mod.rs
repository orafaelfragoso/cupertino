//! Helpers the test files share: desktop entries, files in a temporary tree and the visibility used everywhere.

#![allow(dead_code)]

use cupertino_apps::entry::{parse_desktop_entry, DesktopEntry, Locale, Visibility};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Visibility on a desktop called `Cupertino` that does not check programs.
pub fn vis() -> Visibility {
    Visibility {
        current_desktops: vec!["Cupertino".into()],
        path_dirs: Vec::new(),
        skip_try_exec: true,
    }
}

/// Write `text` to `root/rel`, creating the folders above it.
pub fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

/// An executable file at `root/rel`.
pub fn executable(root: &Path, rel: &str, text: &str) -> PathBuf {
    let path = write(root, rel, text);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// A desktop file with the given extra lines under `[Desktop Entry]`.
pub fn desktop(name: &str, exec: &str, extra: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\n{extra}\n")
}

/// The entry of `text`, which must be visible.
pub fn entry(id: &str, text: &str) -> DesktopEntry {
    parse_desktop_entry(id, text, &Locale::default(), &vis()).unwrap()
}
