//! Small sysfs file helpers that attach the path to every failure.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{Error, ParseError, Result};

/// `<root>/devices`, canonical, so paths from resolved class symlinks compare with it.
pub(crate) fn devices_dir(sysfs_root: &Path) -> Result<PathBuf> {
    canonicalize(&sysfs_root.join("devices"))
}

pub(crate) fn canonicalize(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|source| Error::Io { path: path.to_owned(), source })
}

/// Reads a sysfs attribute; a missing file is an error.
pub(crate) fn read_attr(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| Error::Io { path: path.to_owned(), source })
}

/// Reads a sysfs attribute that may legitimately be absent; every other failure is an error.
pub(crate) fn read_attr_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::Io { path: path.to_owned(), source }),
    }
}

/// Reads a hexadecimal `u16` attribute such as `id/bustype`.
pub(crate) fn read_hex_u16(path: &Path, key: &'static str) -> Result<u16> {
    let text = read_attr(path)?;
    u16::from_str_radix(text.trim(), 16).map_err(|_| Error::Parse {
        origin: path.display().to_string(),
        source: ParseError::InvalidNumber { key, value: text.trim().to_owned() },
    })
}

/// The directories above `syspath` that are still inside `<root>/devices`, nearest first.
pub(crate) fn ancestor_dirs(syspath: &Path, devices_dir: &Path) -> Vec<PathBuf> {
    syspath
        .ancestors()
        .skip(1)
        .take_while(|dir| *dir != devices_dir && dir.starts_with(devices_dir))
        .map(Path::to_owned)
        .collect()
}
