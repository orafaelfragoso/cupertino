//! Recents: the files of the session's recently-used list, `$XDG_DATA_HOME/recently-used.xbel` (the desktop
//! bookmark file GTK's recent manager writes), shown as the virtual folder `recent:/// (the GIO
//! scheme for the same list). The design's Recents is a saved search over last-opened dates; its
//! layout and order is not known, so the folder is listed in the window's sort order (approximate).

use crate::entry::Entry;
use percent_encoding::percent_decode_str;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// Where the Recents place points: not a directory, the browser opens it from the bookmark file.
pub const RECENTS: &str = "recent:///";

/// The name of the bookmark file in the data home.
pub const XBEL_NAME: &str = "recently-used.xbel";

/// [`RECENTS`] as a path.
pub fn location() -> &'static Path {
    Path::new(RECENTS)
}

/// The five predefined XML entities, the only ones a bookmark file's attribute values use.
fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The local files a bookmark file lists, in file order and each once: the `file://` `href` of every `<bookmark>`
/// element, entities resolved and percent-decoded. Other schemes and remote hosts are left out.
pub fn parse_xbel(text: &str) -> Vec<PathBuf> {
    let (mut out, mut seen): (Vec<PathBuf>, HashSet<PathBuf>) = (Vec::new(), HashSet::new());
    for element in text.split("<bookmark ").skip(1) {
        let Some(tag) = element.split('>').next() else {
            continue;
        };
        let Some(href) = tag
            .split("href=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
        else {
            continue;
        };
        let href = unescape_xml(href);
        let Some(rest) = href.strip_prefix("file://") else {
            continue;
        };
        let Some(path) = rest
            .strip_prefix("localhost")
            .unwrap_or(rest)
            .strip_prefix('/')
        else {
            continue;
        };
        // a path is bytes: a name that is not UTF-8 decodes to its own bytes, not to replacement characters
        let mut bytes = vec![b'/'];
        bytes.extend(percent_decode_str(path));
        let path = PathBuf::from(OsString::from_vec(bytes));
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

/// The listed files that still exist, as entries, in file order. No bookmark file yet means nothing was used.
pub fn recent_entries(xbel: &Path) -> io::Result<Vec<Entry>> {
    let text = match std::fs::read_to_string(xbel) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    // a file removed since it was used drops out of the list
    Ok(parse_xbel(&text)
        .into_iter()
        .filter_map(|path| Entry::from_path(path).ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmarks_give_their_local_files_once_in_file_order() {
        let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0" xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks">
  <bookmark href="file:///home/ada/Notes%20%26%20Plans.md" added="2026-10-01T09:00:00Z" modified="2026-10-01T09:00:00Z" visited="2026-10-01T09:00:00Z">
    <info/>
  </bookmark>
  <bookmark href="https://example.org/page" added="2026-10-01T09:00:00Z"/>
  <bookmark href="file://server/share/x.txt" added="2026-10-01T09:00:00Z"/>
  <bookmark href="file://localhost/tmp/a&amp;b.txt" added="2026-10-02T09:00:00Z"/>
  <bookmark href="file:///home/ada/Notes%20%26%20Plans.md" added="2026-10-03T09:00:00Z"/>
</xbel>"#;
        assert_eq!(
            parse_xbel(text),
            [
                PathBuf::from("/home/ada/Notes & Plans.md"),
                PathBuf::from("/tmp/a&b.txt")
            ]
        );
    }

    #[test]
    fn a_name_that_is_not_utf8_keeps_its_bytes() {
        use std::os::unix::ffi::OsStrExt;
        // "café" in Latin-1: the 0xE9 byte alone is not UTF-8
        let paths = parse_xbel(
            r#"<xbel><bookmark href="file:///tmp/caf%E9.txt" added="2026-10-01T09:00:00Z"/></xbel>"#,
        );
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].as_os_str().as_bytes(), b"/tmp/caf\xe9.txt");
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
