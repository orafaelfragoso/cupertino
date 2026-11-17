//! The recently-used files: the bookmark file's entries, their times and reading the file.

mod common;

use common::write;
use cupertino_apps::recent::{
    parse_recent_files, parse_xbel, read_recent_files, xbel_path, MAX_READ_BYTES, XBEL_NAME,
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
fn the_bookmark_file_is_in_the_data_home() {
    assert_eq!(
        xbel_path(Path::new("/data")),
        Path::new("/data").join(XBEL_NAME)
    );
    assert_eq!(XBEL_NAME, "recently-used.xbel");
}

#[test]
fn local_files_are_listed_in_file_order() {
    let text = xbel(
        "<bookmark href=\"file:///b.txt\"/>\n<bookmark href=\"file:///a.txt\"/>\n<bookmark href=\"file:///c.txt\"/>\n",
    );
    assert_eq!(
        parse_xbel(&text),
        [PathBuf::from("/b.txt"), "/a.txt".into(), "/c.txt".into()]
    );
}

#[test]
fn paths_are_percent_decoded_and_their_entities_resolved() {
    let text = xbel(
        "<bookmark href=\"file:///home/u/a%20b%2Bc.txt\"/>\n<bookmark href=\"file:///x/a&amp;b&lt;c&gt;.txt\"/>\n",
    );
    assert_eq!(
        parse_xbel(&text),
        [PathBuf::from("/home/u/a b+c.txt"), "/x/a&b<c>.txt".into()]
    );
}

#[test]
fn a_name_that_is_not_utf8_keeps_its_bytes() {
    let paths = parse_xbel(&xbel("<bookmark href=\"file:///x/%FF.txt\"/>\n"));
    assert_eq!(paths[0].as_os_str().as_bytes(), b"/x/\xFF.txt");
}

#[test]
fn other_schemes_and_remote_hosts_are_left_out() {
    let text = xbel(
        "<bookmark href=\"https://example.com/a\"/>\n<bookmark href=\"file://server/share/a\"/>\n\
         <bookmark href=\"smb://host/a\"/>\n<bookmark href=\"file://localhost/tmp/ok.txt\"/>\n\
         <bookmark href=\"file://\"/>\n<bookmark/>\n",
    );
    assert_eq!(parse_xbel(&text), [PathBuf::from("/tmp/ok.txt")]);
}

#[test]
fn a_file_listed_twice_appears_once_at_its_first_place() {
    let text = xbel(
        "<bookmark href=\"file:///a\"/>\n<bookmark href=\"file:///b\"/>\n<bookmark href=\"file:///a\"/>\n",
    );
    assert_eq!(parse_xbel(&text), [PathBuf::from("/a"), "/b".into()]);
}

#[test]
fn a_file_is_used_at_the_latest_of_its_added_modified_and_visited_times() {
    let text = xbel(
        "<bookmark href=\"file:///a\" added=\"2026-10-01T09:00:00Z\" modified=\"2026-10-02T09:00:00Z\" \
         visited=\"2026-10-01T10:00:00Z\"/>\n",
    );
    assert_eq!(used(&text), [Some(1_790_931_600.0)]);
}

#[test]
fn the_times_of_every_bookmark_of_a_file_count() {
    let text = xbel(
        "<bookmark href=\"file:///a\" added=\"2026-10-01T09:00:00Z\"/>\n\
         <bookmark href=\"file:///a\" visited=\"2026-10-05T00:00:00Z\"/>\n\
         <bookmark href=\"file:///a\"/>\n",
    );
    assert_eq!(used(&text), [Some(1_791_158_400.0)]);
}

#[test]
fn a_file_without_a_parsable_time_has_none() {
    let text = xbel(
        "<bookmark href=\"file:///a\"/>\n<bookmark href=\"file:///b\" added=\"yesterday\"/>\n",
    );
    assert_eq!(used(&text), [None, None]);
}

#[test]
fn a_time_may_carry_a_fraction_and_an_offset_or_no_zone() {
    let at = |stamp: &str| {
        used(&xbel(&format!(
            "<bookmark href=\"file:///a\" added=\"{stamp}\"/>\n"
        )))[0]
    };
    assert_eq!(at("2000-01-01T00:00:00Z"), Some(946_684_800.0));
    assert_eq!(at("2000-01-01T00:00:00"), Some(946_684_800.0));
    assert_eq!(at("2000-01-01T00:00:00.5Z"), Some(946_684_800.5));
    assert_eq!(at("2000-01-01T01:00:00+01:00"), Some(946_684_800.0));
    assert_eq!(at("1999-12-31T23:00:00-01:00"), Some(946_684_800.0));
}

#[test]
fn a_time_out_of_range_or_malformed_reads_as_none() {
    let at = |stamp: &str| {
        used(&xbel(&format!(
            "<bookmark href=\"file:///a\" added=\"{stamp}\"/>\n"
        )))[0]
    };
    for bad in [
        "2000-13-01T00:00:00Z",
        "2000-01-32T00:00:00Z",
        "2000-01-01T24:00:00Z",
        "2000-01-01T00:60:00Z",
        "2000-01-01T00:00:00+15:00",
        "2000-01-01T00:00:00.xZ",
        "2000-01-01",
        "0000-01-01T00:00:00Z",
        "",
    ] {
        assert_eq!(at(bad), None, "{bad:?}");
    }
}

#[test]
fn an_attribute_named_like_a_time_attribute_is_not_one() {
    let text = xbel("<bookmark href=\"file:///a\" notadded=\"2026-10-01T09:00:00Z\"/>\n");
    assert_eq!(used(&text), [None]);
}

#[test]
fn text_without_bookmarks_lists_nothing() {
    assert!(parse_xbel("").is_empty());
    assert!(parse_xbel("not xml at all").is_empty());
    assert!(parse_xbel(&xbel("")).is_empty());
}

#[test]
fn a_missing_bookmark_file_means_nothing_was_used() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_recent_files(&xbel_path(dir.path()))
        .unwrap()
        .is_empty());
}

#[test]
fn a_bookmark_file_is_read_with_its_times() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        XBEL_NAME,
        &xbel("<bookmark href=\"file:///a\" added=\"2000-01-01T00:00:00Z\"/>\n"),
    );
    let files = read_recent_files(&path).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, Path::new("/a"));
    assert_eq!(files[0].used, Some(946_684_800.0));
}

#[test]
fn a_bookmark_file_over_the_limit_is_invalid_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(XBEL_NAME);
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_READ_BYTES + 1).unwrap();
    let err = read_recent_files(&path).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn a_bookmark_file_exactly_at_the_limit_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(XBEL_NAME);
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_READ_BYTES).unwrap();
    assert!(read_recent_files(&path).unwrap().is_empty());
}

#[test]
fn a_bookmark_file_that_cannot_be_read_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(XBEL_NAME);
    std::fs::create_dir(&path).unwrap();
    assert!(read_recent_files(&path).is_err());
}
