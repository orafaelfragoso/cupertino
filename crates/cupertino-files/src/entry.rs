//! Directory entries.

use crate::natural::NameKey;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    File,
    Directory,
    Symlink,
    Other,
}

/// One item of a directory listing. `name_key` is computed once so that sorting is a byte compare.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    /// Target is a directory (for symlinks); equals `kind == Directory` otherwise.
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub dev: u64,
    pub inode: u64,
    pub hidden: bool,
    pub tags: Vec<String>,
    pub mime: Option<String>,
    pub name_key: NameKey,
}

impl Entry {
    /// An entry without file system access (tests, layouts, previews of synthetic data).
    pub fn synthetic(name: &str, is_dir: bool, size: u64) -> Self {
        Entry {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind: if is_dir { Kind::Directory } else { Kind::File },
            is_dir,
            size,
            modified: None,
            created: None,
            accessed: None,
            dev: 0,
            inode: 0,
            hidden: name.starts_with('.'),
            tags: Vec::new(),
            mime: None,
            name_key: NameKey::new(name),
        }
    }

    /// Stat `path`. Symlinks are listed as symlinks; `is_dir` follows the link.
    pub fn from_path(path: PathBuf) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned());
        let lmeta = std::fs::symlink_metadata(&path)?;
        let ft = lmeta.file_type();
        let (kind, meta) = if ft.is_symlink() {
            (Kind::Symlink, std::fs::metadata(&path).unwrap_or_else(|_| lmeta.clone()))
        } else if ft.is_dir() {
            (Kind::Directory, lmeta.clone())
        } else if ft.is_file() {
            (Kind::File, lmeta.clone())
        } else {
            (Kind::Other, lmeta.clone())
        };
        Ok(Entry {
            hidden: name.starts_with('.'),
            name_key: NameKey::new(&name),
            name,
            is_dir: meta.is_dir(),
            kind,
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            created: meta.created().ok(),
            accessed: meta.accessed().ok(),
            dev: lmeta.dev(),
            inode: lmeta.ino(),
            tags: Vec::new(),
            mime: None,
            path,
        })
    }

    /// Extension without the dot (`None` for dot-files without another dot, folders and names ending in a dot).
    pub fn extension(&self) -> Option<&str> {
        if self.is_dir {
            return None;
        }
        extension_of(&self.name)
    }
}

pub fn extension_of(name: &str) -> Option<&str> {
    let idx = name.rfind('.')?;
    if idx == 0 || idx + 1 == name.len() {
        return None;
    }
    Some(&name[idx + 1..])
}
