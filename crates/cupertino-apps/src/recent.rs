//! The session's recently-used files: `$XDG_DATA_HOME/recently-used.xbel`, the desktop bookmark file GTK's recent
//! manager writes. Read only; the file belongs to the applications that record their opens in it.

use percent_encoding::percent_decode_str;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// The name of the bookmark file in the data home.
pub const XBEL_NAME: &str = "recently-used.xbel";

/// The most bytes read of a file another process writes; any application of the session can grow the bookmark file.
pub const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

/// The text of `path`, or `InvalidData` when it is longer than `max` bytes.
pub(crate) fn read_capped(path: &Path, max: u64) -> io::Result<String> {
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(max + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is larger than {max} bytes", path.display()),
        ));
    }
    Ok(text)
}

/// The bookmark file of the data home `data_home`.
pub fn xbel_path(data_home: &Path) -> PathBuf {
    data_home.join(XBEL_NAME)
}

/// A local file of the bookmark file and when it was last used.
#[derive(Clone, Debug, PartialEq)]
pub struct RecentFile {
    pub path: PathBuf,
    /// Unix seconds of the latest `added`, `modified` or `visited` time of its bookmarks; `None` when none parses.
    pub used: Option<f64>,
}

/// The five predefined XML entities, the only ones a bookmark file's attribute values use.
fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// The value of attribute `name` in the attribute text `tag`: the first `name="` at the start or after white space.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    tag.match_indices(&key)
        .find(|(at, _)| {
            tag[..*at]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
        })
        .and_then(|(at, _)| tag[at + key.len()..].split('"').next())
}

/// The local path of a `file://` URI; `None` for other schemes and remote hosts.
fn local_path(href: &str) -> Option<PathBuf> {
    let href = unescape_xml(href);
    let rest = href.strip_prefix("file://")?;
    let path = rest
        .strip_prefix("localhost")
        .unwrap_or(rest)
        .strip_prefix('/')?;
    // a path is bytes: a name that is not UTF-8 decodes to its own bytes, not to replacement characters
    let mut bytes = vec![b'/'];
    bytes.extend(percent_decode_str(path));
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

/// Days from 1970-01-01 to a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Unix seconds of an XML Schema date-time as bookmark files carry it (`2026-10-01T09:00:00Z`, optionally with a
/// fraction of a second and a `+hh:mm` or `-hh:mm` offset instead of `Z`; no zone reads as UTC).
fn parse_time(text: &str) -> Option<f64> {
    let (date, rest) = text.trim().split_once('T')?;
    let mut ymd = date.splitn(3, '-').map(|v| v.parse::<i64>().ok());
    let (y, m, d) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let zone_at = rest.find(['Z', '+', '-']).unwrap_or(rest.len());
    let (clock, zone) = rest.split_at(zone_at);
    let (hms, fraction) = clock.split_once('.').unwrap_or((clock, ""));
    let mut parts = hms.splitn(3, ':').map(|v| v.parse::<i64>().ok());
    let (h, min, s) = (parts.next()??, parts.next()??, parts.next()??);
    // the bounds keep the arithmetic below in range for any input
    if !(1..=9999).contains(&y)
        || !(1..=12).contains(&m)
        || !(1..=31).contains(&d)
        || !(0..=23).contains(&h)
        || !(0..=59).contains(&min)
        || !(0..=60).contains(&s)
    {
        return None;
    }
    let fraction = match fraction {
        "" => 0.0,
        f if f.bytes().all(|b| b.is_ascii_digit()) => format!("0.{f}").parse::<f64>().ok()?,
        _ => return None,
    };
    let offset = match zone {
        "" | "Z" => 0,
        _ => {
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            let (oh, om) = zone[1..].split_once(':')?;
            let (oh, om) = (oh.parse::<i64>().ok()?, om.parse::<i64>().ok()?);
            if !(0..=14).contains(&oh) || !(0..=59).contains(&om) {
                return None;
            }
            sign * (oh * 3600 + om * 60)
        }
    };
    let secs = days_from_civil(y, m, d) * 86400 + h * 3600 + min * 60 + s - offset;
    Some(secs as f64 + fraction)
}

/// The local files a bookmark file lists, in file order and each once, with the latest time any of their
/// `<bookmark>` elements gives. The `href` of each has its entities resolved and is percent-decoded; other schemes
/// and remote hosts are left out.
pub fn parse_recent_files(text: &str) -> Vec<RecentFile> {
    let mut out: Vec<RecentFile> = Vec::new();
    let mut index: HashMap<PathBuf, usize> = HashMap::new();
    for element in text.split("<bookmark ").skip(1) {
        let Some(tag) = element.split('>').next() else {
            continue;
        };
        let Some(path) = tag
            .split("href=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .and_then(local_path)
        else {
            continue;
        };
        let used = ["added", "modified", "visited"]
            .into_iter()
            .filter_map(|name| attribute(tag, name).and_then(parse_time))
            .reduce(f64::max);
        match index.get(&path) {
            Some(&i) => {
                let file = &mut out[i];
                file.used = match (file.used, used) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
            }
            None => {
                index.insert(path.clone(), out.len());
                out.push(RecentFile { path, used });
            }
        }
    }
    out
}

/// The local files a bookmark file lists, in file order and each once: the `file://` `href` of every `<bookmark>`
/// element, entities resolved and percent-decoded. Other schemes and remote hosts are left out.
pub fn parse_xbel(text: &str) -> Vec<PathBuf> {
    parse_recent_files(text)
        .into_iter()
        .map(|file| file.path)
        .collect()
}

/// The files of the bookmark file `xbel` as [`parse_recent_files`] gives them, whether or not they still exist. No
/// bookmark file yet means nothing was used; one larger than [`MAX_READ_BYTES`] is an `InvalidData` error.
pub fn read_recent_files(xbel: &Path) -> io::Result<Vec<RecentFile>> {
    match read_capped(xbel, MAX_READ_BYTES) {
        Ok(text) => Ok(parse_recent_files(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
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
    }

    #[test]
    fn a_file_is_used_at_the_latest_time_of_its_bookmarks() {
        let text = r#"<xbel>
  <bookmark href="file:///a" added="2026-10-01T09:00:00Z" modified="2026-10-05T10:30:00.250Z" visited="2026-10-02T09:00:00Z"/>
  <bookmark href="file:///b" added="2026-10-01T09:00:00+02:00"/>
  <bookmark href="file:///c" added="yesterday"/>
  <bookmark href="file:///b" modified="2026-09-01T00:00:00Z"/>
</xbel>"#;
        let day = |d: i64| days_from_civil(2026, 10, d) as f64 * 86400.0;
        assert_eq!(
            parse_recent_files(text),
            [
                RecentFile {
                    path: "/a".into(),
                    used: Some(day(5) + 10.5 * 3600.0 + 0.25)
                },
                RecentFile {
                    path: "/b".into(),
                    used: Some(day(1) + 7.0 * 3600.0)
                },
                RecentFile {
                    path: "/c".into(),
                    used: None
                },
            ]
        );
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_time("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn a_time_out_of_range_keeps_its_file_without_a_time() {
        let text = r#"<xbel>
  <bookmark href="file:///tmp/a" added="999999999999999-01-01T00:00:00Z"/>
  <bookmark href="file:///tmp/b" added="2026-10-01T09:00:00+999999999999999:00"/>
  <bookmark href="file:///tmp/c" added="2026-10-01T99999999999999:00:00Z"/>
</xbel>"#;
        assert_eq!(
            parse_xbel(text),
            [
                Path::new("/tmp/a"),
                Path::new("/tmp/b"),
                Path::new("/tmp/c")
            ]
        );
        assert!(parse_recent_files(text).iter().all(|f| f.used.is_none()));
        assert_eq!(
            parse_time("9999-12-31T23:59:60-14:00").map(|t| t > 0.0),
            Some(true)
        );
        assert_eq!(parse_time("10000-01-01T00:00:00Z"), None);
        assert_eq!(parse_time("2026-10-01T09:00:00+15:00"), None);
    }

    #[test]
    fn a_bookmark_file_over_the_limit_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let xbel = xbel_path(d.path());
        std::fs::write(&xbel, r#"<bookmark href="file:///tmp/a"/>"#).unwrap();
        assert_eq!(read_capped(&xbel, 64).unwrap().len(), 32);
        let error = read_capped(&xbel, 16).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn no_bookmark_file_is_an_empty_list() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_recent_files(&xbel_path(d.path())).unwrap().is_empty());
    }
}
