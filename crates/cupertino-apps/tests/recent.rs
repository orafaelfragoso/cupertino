//! The recently-used files: the bookmark file's entries, their times and reading the file.

use crate::common;

use common::write;
use cupertino_apps::recent::{
    parse_recent_files, parse_xbel, read_recent_files, xbel_path, MAX_READ_BYTES,
};
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

fn xbel(bookmarks: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xbel version=\"1.0\">\n{bookmarks}</xbel>\n"
    )
}

fn used(text: &str) -> Vec<Option<f64>> {
    parse_recent_files(text)
        .into_iter()
        .map(|f| f.used)
        .collect()
}

#[test]
fn local_files_are_listed_once_in_file_order_decoded() {
    for (bookmarks, expected) in [
        (
            "<bookmark href=\"file:///b.txt\"/>\n<bookmark href=\"file:///a.txt\"/>\n<bookmark href=\"file:///c.txt\"/>\n",
            &["/b.txt", "/a.txt", "/c.txt"][..],
        ),
        (
            "<bookmark href=\"file:///home/u/a%20b%2Bc.txt\"/>\n<bookmark href=\"file:///x/a&amp;b&lt;c&gt;.txt\"/>\n",
            &["/home/u/a b+c.txt", "/x/a&b<c>.txt"],
        ),
        (
            "<bookmark href=\"https://example.com/a\"/>\n<bookmark href=\"file://server/share/a\"/>\n\
             <bookmark href=\"smb://host/a\"/>\n<bookmark href=\"file://localhost/tmp/ok.txt\"/>\n\
             <bookmark href=\"file://\"/>\n<bookmark/>\n",
            &["/tmp/ok.txt"],
        ),
        (
            "<bookmark href=\"file:///a\"/>\n<bookmark href=\"file:///b\"/>\n<bookmark href=\"file:///a\"/>\n",
            &["/a", "/b"],
        ),
        ("", &[]),
    ] {
        let expected: Vec<PathBuf> = expected.iter().map(PathBuf::from).collect();
        assert_eq!(parse_xbel(&xbel(bookmarks)), expected, "{bookmarks}");
    }
    assert!(parse_xbel("").is_empty());
    assert!(parse_xbel("not xml at all").is_empty());
    let raw = parse_xbel(&xbel("<bookmark href=\"file:///x/%FF.txt\"/>\n"));
    assert_eq!(raw[0].as_os_str().as_bytes(), b"/x/\xFF.txt");
}

#[test]
fn a_file_is_used_at_the_latest_time_of_any_of_its_bookmarks() {
    for (bookmarks, expected) in [
        (
            "<bookmark href=\"file:///a\" added=\"2026-10-01T09:00:00Z\" modified=\"2026-10-02T09:00:00Z\" \
             visited=\"2026-10-01T10:00:00Z\"/>\n",
            &[Some(1_790_931_600.0)][..],
        ),
        (
            "<bookmark href=\"file:///a\" added=\"2026-10-01T09:00:00Z\"/>\n\
             <bookmark href=\"file:///a\" visited=\"2026-10-05T00:00:00Z\"/>\n\
             <bookmark href=\"file:///a\"/>\n",
            &[Some(1_791_158_400.0)],
        ),
        (
            "<bookmark href=\"file:///a\"/>\n<bookmark href=\"file:///b\" added=\"yesterday\"/>\n",
            &[None, None],
        ),
        (
            "<bookmark href=\"file:///a\" notadded=\"2026-10-01T09:00:00Z\"/>\n",
            &[None],
        ),
    ] {
        assert_eq!(used(&xbel(bookmarks)), expected, "{bookmarks}");
    }
}

#[test]
fn a_time_is_read_with_fraction_and_offset_and_out_of_range_ones_are_none() {
    for (stamp, expected) in [
        ("2000-01-01T00:00:00Z", Some(946_684_800.0)),
        ("2000-01-01T00:00:00", Some(946_684_800.0)),
        ("2000-01-01T00:00:00.5Z", Some(946_684_800.5)),
        ("2000-01-01T01:00:00+01:00", Some(946_684_800.0)),
        ("1999-12-31T23:00:00-01:00", Some(946_684_800.0)),
        ("2000-13-01T00:00:00Z", None),
        ("2000-01-32T00:00:00Z", None),
        ("2000-01-01T24:00:00Z", None),
        ("2000-01-01T00:60:00Z", None),
        ("2000-01-01T00:00:00+15:00", None),
        ("2000-01-01T00:00:00.xZ", None),
        ("2000-01-01", None),
        ("0000-01-01T00:00:00Z", None),
        ("", None),
    ] {
        let text = xbel(&format!(
            "<bookmark href=\"file:///a\" added=\"{stamp}\"/>\n"
        ));
        assert_eq!(used(&text), [expected], "{stamp:?}");
    }
}

#[test]
fn the_bookmark_file_in_the_data_home_is_read_with_its_times() {
    let dir = tempfile::tempdir().unwrap();
    let path = xbel_path(dir.path());
    assert!(read_recent_files(&path).unwrap().is_empty(), "missing file");
    write(
        path.parent().unwrap(),
        path.file_name().unwrap().to_str().unwrap(),
        &xbel("<bookmark href=\"file:///a\" added=\"2000-01-01T00:00:00Z\"/>\n"),
    );
    let files = read_recent_files(&path).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, Path::new("/a"));
    assert_eq!(files[0].used, Some(946_684_800.0));
}

#[test]
fn a_bookmark_file_over_the_limit_or_unreadable_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recent.xbel");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_READ_BYTES).unwrap();
    assert!(read_recent_files(&path).unwrap().is_empty(), "at the limit");
    file.set_len(MAX_READ_BYTES + 1).unwrap();
    assert_eq!(
        read_recent_files(&path).unwrap_err().kind(),
        ErrorKind::InvalidData
    );

    let folder = dir.path().join("folder.xbel");
    std::fs::create_dir(&folder).unwrap();
    assert!(read_recent_files(&folder).is_err());
}
