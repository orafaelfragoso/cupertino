//! `time`: RFC 3339 UTC times with milliseconds.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cupertino_diagnostic_events::time::{format, format_into, parse, LEN};

fn at(secs: u64, millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(secs * 1000 + millis)
}

#[test]
fn the_epoch_is_written_as_midnight_1970() {
    assert_eq!(format(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    assert_eq!(LEN, 24);
}

#[test]
fn a_time_is_written_with_milliseconds_in_utc() {
    // 2026-11-09T14:03:27.512Z
    assert_eq!(format(at(1_794_233_007, 512)), "2026-11-09T14:03:27.512Z");
}

#[test]
fn leap_days_and_year_ends_are_right() {
    assert_eq!(format(at(951_782_400, 0)), "2000-02-29T00:00:00.000Z");
    assert_eq!(format(at(1_709_164_800, 999)), "2024-02-29T00:00:00.999Z");
    assert_eq!(format(at(1_767_225_599, 0)), "2025-12-31T23:59:59.000Z");
}

#[test]
fn format_into_fills_the_buffer_like_format() {
    let mut buf = [0u8; LEN];
    format_into(at(1_794_233_007, 5), &mut buf);
    assert_eq!(
        std::str::from_utf8(&buf).unwrap(),
        format(at(1_794_233_007, 5))
    );
}

#[test]
fn a_written_time_parses_back() {
    for (secs, millis) in [
        (0, 0),
        (1_794_233_007, 512),
        (951_782_400, 1),
        (4_102_444_799, 999),
    ] {
        let t = at(secs, millis);
        assert_eq!(parse(&format(t)), Some(t), "{secs}.{millis}");
    }
}

#[test]
fn a_time_without_fraction_or_with_more_digits_parses() {
    assert_eq!(parse("1970-01-01T00:00:01Z"), Some(at(1, 0)));
    assert_eq!(parse("1970-01-01T00:00:01.5Z"), Some(at(1, 500)));
    assert_eq!(
        parse("1970-01-01T00:00:01.123456Z"),
        Some(at(1, 123) + Duration::from_micros(456))
    );
}

#[test]
fn text_that_is_not_a_time_does_not_parse() {
    for bad in [
        "",
        "yesterday",
        "1970-13-01T00:00:00Z",
        "1970-00-01T00:00:00Z",
        "1970-01-32T00:00:00Z",
        "1970-01-01T24:00:00Z",
        "1970-01-01T00:60:00Z",
        "1970-01-01T00:00:00.Z",
        "1970/01/01T00:00:00Z",
        "1970-01-01T00-00-00Z",
        "19a0-01-01T00:00:00Z",
    ] {
        assert_eq!(parse(bad), None, "{bad:?}");
    }
}
