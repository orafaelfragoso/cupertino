//! The properties udevd stored for a device node: the `E:` lines of `/run/udev/data/c<major>:<minor>`.
//!
//! The hardware database (`MOUSE_DPI`, `ID_INPUT_*` and the like) only reaches a device through udevd; sysfs has just the
//! kernel's own properties. The file is written by a privileged daemon but its lines come from rules and hardware
//! strings, so the reader bounds what it takes and skips what it does not understand.

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
    let Ok(file) = std::fs::File::open(path) else {
        return Properties::new();
    };
    let mut text = String::new();
    let read =
        std::io::Read::read_to_string(&mut std::io::Read::take(file, MAX_FILE_BYTES), &mut text);
    if read.is_err() {
        return Properties::new();
    }
    parse_udev_db(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = "S:input/by-id/usb-Example_Mouse-event-mouse\n\
I:16236341\n\
E:ID_INPUT=1\n\
E:ID_INPUT_MOUSE=1\n\
E:MOUSE_DPI=400@125 800@125 *1600@500\n\
E:BROKEN\n\
E:=empty key\n\
G:seat\n\
Q:seat\n\
V:1\n";

    #[test]
    fn the_environment_lines_of_a_database_file_are_the_properties() {
        let properties = parse_udev_db(DB);
        assert_eq!(properties.len(), 3);
        assert_eq!(
            properties.get("MOUSE_DPI").map(String::as_str),
            Some("400@125 800@125 *1600@500")
        );
        assert_eq!(
            properties.get("ID_INPUT_MOUSE").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn a_file_is_found_by_the_device_number() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("c13:67"), DB).unwrap();
        let found = read_udev_properties(dir.path(), DevT::new(13, 67));
        assert_eq!(found.get("ID_INPUT").map(String::as_str), Some("1"));
        assert!(read_udev_properties(dir.path(), DevT::new(13, 68)).is_empty());
    }

    #[test]
    fn oversized_entries_are_skipped() {
        let long = "x".repeat(MAX_ENTRY_BYTES + 1);
        let properties = parse_udev_db(&format!("E:LONG={long}\nE:SHORT=1\n"));
        assert_eq!(properties.keys().collect::<Vec<_>>(), ["SHORT"]);
    }
}
