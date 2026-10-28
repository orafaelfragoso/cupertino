//! Recents: the files of the session's recently-used list, `$XDG_DATA_HOME/recently-used.xbel` (the desktop
//! bookmark file GTK's recent manager writes), shown as the virtual folder `recent:/// (the GIO
//! scheme for the same list). The design's Recents is a saved search over last-opened dates; its
//! layout and order is not known, so the folder is listed in the window's sort order (approximate).

use crate::entry::Entry;
use cupertino_apps::recent::read_recent_files;
pub use cupertino_apps::recent::{parse_xbel, XBEL_NAME};
use std::io;
use std::path::Path;

/// Where the Recents place points: not a directory, the browser opens it from the bookmark file.
pub const RECENTS: &str = "recent:///";

/// [`RECENTS`] as a path.
pub fn location() -> &'static Path {
    Path::new(RECENTS)
}

/// The listed files that still exist, as entries, in file order. No bookmark file yet means nothing was used.
pub fn recent_entries(xbel: &Path) -> io::Result<Vec<Entry>> {
    // a file removed since it was used drops out of the list
    Ok(read_recent_files(xbel)?
        .into_iter()
        .filter_map(|file| Entry::from_path(file.path).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_that_is_not_utf8_is_found() {
        use std::os::unix::ffi::OsStrExt;
        // "café" in Latin-1: the 0xE9 byte alone is not UTF-8
        let d = tempfile::tempdir().unwrap();
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
        std::fs::write(d.path().join(name), b"x").unwrap();
        let xbel = d.path().join(XBEL_NAME);
        let mut href = b"<xbel><bookmark href=\"file://".to_vec();
        href.extend(
            percent_encoding::percent_encode(
                d.path().join(name).as_os_str().as_bytes(),
                percent_encoding::NON_ALPHANUMERIC,
            )
            .to_string()
            .replace("%2F", "/")
            .into_bytes(),
        );
        href.extend(b"\" added=\"2026-10-01T09:00:00Z\"/></xbel>");
        std::fs::write(&xbel, href).unwrap();
        let entries = recent_entries(&xbel).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| e.path.file_name().unwrap().as_bytes())
                .collect::<Vec<_>>(),
            [b"caf\xe9.txt".as_slice()],
            "the file is found"
        );
    }

    #[test]
    fn only_existing_files_are_listed_and_no_file_is_an_empty_list() {
        let d = tempfile::tempdir().unwrap();
        let xbel = d.path().join(XBEL_NAME);
        assert!(recent_entries(&xbel).unwrap().is_empty());
        std::fs::write(d.path().join("kept.txt"), b"x").unwrap();
        let href = |name: &str| {
            format!(
                "<bookmark href=\"file://{}\" added=\"2026-10-01T09:00:00Z\"/>",
                d.path().join(name).display()
            )
        };
        std::fs::write(
            &xbel,
            format!("<xbel>{}{}</xbel>", href("gone.txt"), href("kept.txt")),
        )
        .unwrap();
        assert_eq!(
            recent_entries(&xbel)
                .unwrap()
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            ["kept.txt"]
        );
    }
}
