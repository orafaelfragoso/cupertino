//! Helpers the test files share: a temporary tree with a home trash and hand-made trash directories.

#![allow(dead_code)]

use cupertino_trash::Trash;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub fn uid() -> u32 {
    unsafe { libc::getuid() }
}

/// Whether permissions do not stop the user, so a test that relies on being refused cannot run.
pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// A temporary tree holding a home trash at `home/Trash`, a folder `data` for files to trash and `vol`, a mount point.
pub struct Tree {
    pub root: TempDir,
}

impl Tree {
    pub fn new() -> Tree {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("data")).unwrap();
        fs::create_dir_all(root.path().join("vol")).unwrap();
        Tree { root }
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    pub fn home_trash(&self) -> PathBuf {
        self.path("home/Trash")
    }

    pub fn vol(&self) -> PathBuf {
        self.path("vol")
    }

    /// A trash at the tree's home location that knows no volumes.
    pub fn trash(&self) -> Trash {
        Trash::with_home(self.home_trash())
    }

    /// A trash that takes the tree's `vol` as its one mount point.
    pub fn trash_with_volume(&self) -> Trash {
        self.trash().with_mounts(vec![self.vol()])
    }

    /// A file `data/<name>` with `content`.
    pub fn file(&self, name: &str, content: &str) -> PathBuf {
        let path = self.path("data").join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    /// The user's trash directory `.Trash-$uid` of the volume.
    pub fn volume_trash(&self) -> PathBuf {
        let dir = self.vol().join(format!(".Trash-{}", uid()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

/// An item in the trash directory `dir`: `files/<name>` with `content` and its record with `Path=` and the date.
pub fn put_item(dir: &Path, name: &str, content: &str, path: &str, date: &str) {
    fs::create_dir_all(dir.join("files")).unwrap();
    fs::create_dir_all(dir.join("info")).unwrap();
    fs::write(dir.join("files").join(name), content).unwrap();
    fs::write(
        dir.join("info").join(format!("{name}.trashinfo")),
        format!("[Trash Info]\nPath={path}\nDeletionDate={date}\n"),
    )
    .unwrap();
}

/// `dir` made unreadable to delete from: its entries cannot be removed until [`unlock`].
pub fn lock(dir: &Path) {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o500)).unwrap();
}

pub fn unlock(dir: &Path) {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
}

/// The names in `dir`, sorted.
pub fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}
