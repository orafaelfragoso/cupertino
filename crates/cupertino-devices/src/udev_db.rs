//! The properties udevd stored for a device node: the `E:` lines of `/run/udev/data/c<major>:<minor>`.
//!
//! The hardware database (`MOUSE_DPI`, `ID_INPUT_*` and the like) only reaches a device through udevd; sysfs has just the
//! kernel's own properties. The file is written by a privileged daemon but its lines come from rules and hardware
//! strings, so the reader bounds what it takes and skips what it does not understand.

use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::dev_t::DevT;
use crate::uevent::Properties;

/// Where udevd keeps its database.
pub const UDEV_DATA_DIR: &str = "/run/udev/data";
/// Larger files are not read.
const MAX_FILE_BYTES: u64 = 1 << 20;
/// Longer keys and values are skipped.
const MAX_ENTRY_BYTES: usize = 4096;

/// The `E:KEY=VALUE` lines of a database file.
pub fn parse_udev_db(text: &str) -> Properties {
    text.lines()
        .filter_map(|line| line.strip_prefix("E:"))
        .filter_map(|entry| entry.split_once('='))
        .filter(|(key, value)| {
            !key.is_empty() && key.len() <= MAX_ENTRY_BYTES && value.len() <= MAX_ENTRY_BYTES
        })
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// The properties udevd holds for the character device `devnum`, read from `dir`; empty when udevd has no record.
pub fn read_udev_properties(dir: &Path, devnum: DevT) -> Properties {
    let path = dir.join(format!("c{devnum}"));
    // not through a link, and without blocking on a pipe someone put there
    let flags = rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK;
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags.bits() as i32)
        .open(path)
    else {
        return Properties::new();
    };
    if !file.metadata().is_ok_and(|meta| meta.is_file()) {
        return Properties::new();
    }
    let mut bytes = Vec::new();
    if std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_FILE_BYTES), &mut bytes)
        .is_err()
    {
        return Properties::new();
    }
    parse_udev_db(&String::from_utf8_lossy(&bytes))
}
