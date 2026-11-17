//! `time`: RFC 3339 UTC times with milliseconds, written and read back.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cupertino_diagnostic_events::time::{format, format_into, parse, LEN};

fn at(secs: u64, millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(secs * 1000 + millis)
}

#[test]
fn a_time_is_written_in_utc_with_milliseconds_and_reads_back() {
    for (time, text) in [
        (UNIX_EPOCH, "1970-01-01T00:00:00.000Z"),
        (at(1_794_233_007, 512), "2026-11-09T14:03:27.512Z"),
        (at(951_782_400, 0), "2000-02-29T00:00:00.000Z"),
        (at(1_709_164_800, 999), "2024-02-29T00:00:00.999Z"),
        (at(1_767_225_599, 0), "2025-12-31T23:59:59.000Z"),
        (at(4_102_444_799, 999), "2099-12-31T23:59:59.999Z"),
    ] {
        assert_eq!(format(time), text);
        let mut buf = [0u8; LEN];
        format_into(time, &mut buf);
        assert_eq!(std::str::from_utf8(&buf).unwrap(), text, "format_into");
        assert_eq!(parse(text), Some(time), "{text}");
    }
}

#[test]
fn parse_takes_any_fraction_and_refuses_what_is_not_a_time() {
    for (text, time) in [
        ("1970-01-01T00:00:01Z", Some(at(1, 0))),
        ("1970-01-01T00:00:01.5Z", Some(at(1, 500))),
        (
            "1970-01-01T00:00:01.123456Z",
            Some(at(1, 123) + Duration::from_micros(456)),
        ),
        ("", None),
        ("yesterday", None),
        ("1970-13-01T00:00:00Z", None),
        ("1970-00-01T00:00:00Z", None),
        ("1970-01-32T00:00:00Z", None),
        ("1970-01-01T24:00:00Z", None),
        ("1970-01-01T00:60:00Z", None),
        ("1970-01-01T00:00:00.Z", None),
        ("1970/01/01T00:00:00Z", None),
        ("1970-01-01T00-00-00Z", None),
        ("19a0-01-01T00:00:00Z", None),
    ] {
        assert_eq!(parse(text), time, "{text:?}");
    }
}
