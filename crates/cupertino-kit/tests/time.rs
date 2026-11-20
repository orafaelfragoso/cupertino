//! `kit::time`: a moment in the session's time zone and back, daylight saving time and day arithmetic included, Unix
//! seconds, and the time as the locale's text.

use cupertino_kit::time::{self, LocalTime};
use std::time::{Duration, SystemTime};

/// Central European Time with its summer time, named by rule so no zone database is needed. The C library reads the
/// zone once, so every test that reads local time sets this one zone first.
fn in_central_european_time() {
    std::env::set_var("TZ", "CET-1CEST,M3.5.0,M10.5.0/3");
}

fn at(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

#[test]
fn local_time_follows_the_zone_and_its_daylight_saving_time_both_ways() {
    in_central_european_time();
    let local = |year, month, day, weekday, hour, minute, second, utc_offset| LocalTime {
        year,
        month,
        day,
        weekday,
        hour,
        minute,
        second,
        utc_offset,
    };
    for (seconds, expected) in [
        (0, local(1970, 1, 1, 4, 1, 0, 0, 3600)),
        // 2026-07-01 12:00:00 UTC, in summer time
        (1_782_907_200, local(2026, 7, 1, 3, 14, 0, 0, 7200)),
        // 2026-12-31 23:30:00 UTC is already the new year here
        (1_798_759_800, local(2027, 1, 1, 5, 0, 30, 0, 3600)),
        // a second before the epoch
        (-1, local(1970, 1, 1, 4, 0, 59, 59, 3600)),
    ] {
        assert_eq!(time::local(at(seconds)), Some(expected), "{seconds}");
        let back = time::from_local(
            expected.year,
            expected.month,
            i32::from(expected.day),
            expected.hour,
            expected.minute,
            expected.second,
        );
        assert_eq!(back, Some(at(seconds)), "{seconds}");
    }
    // a day past the end of the month runs into the next, and one before the first back into the last
    assert_eq!(
        time::from_local(2026, 1, 32, 9, 0, 0),
        time::from_local(2026, 2, 1, 9, 0, 0)
    );
    assert_eq!(
        time::from_local(2026, 3, 0, 9, 0, 0),
        time::from_local(2026, 2, 28, 9, 0, 0)
    );
    // across the change to summer time a day is 23 hours long
    let before = time::from_local(2026, 3, 28, 12, 0, 0).unwrap();
    let after = time::from_local(2026, 3, 29, 12, 0, 0).unwrap();
    assert_eq!(
        after.duration_since(before).unwrap(),
        Duration::from_secs(23 * 3600)
    );
    let now = time::now().unwrap();
    assert!(
        (1..=12).contains(&now.month) && (1..=31).contains(&now.day),
        "{now:?}"
    );
}

#[test]
fn unix_seconds_go_both_ways_and_count_a_part_second_before_the_epoch_as_the_earlier_one() {
    for seconds in [0, 1, -1, 1_782_907_200, -1_782_907_200] {
        assert_eq!(time::from_unix(seconds), at(seconds), "{seconds}");
        assert_eq!(
            time::to_unix(time::from_unix(seconds)),
            Some(seconds),
            "{seconds}"
        );
    }
    let half = Duration::from_millis(500);
    assert_eq!(time::to_unix(SystemTime::UNIX_EPOCH + half), Some(0));
    assert_eq!(time::to_unix(SystemTime::UNIX_EPOCH - half), Some(-1));
}

#[test]
fn a_moment_is_formatted_in_the_zone_by_the_strftime_pattern() {
    in_central_european_time();
    // 2026-07-01 12:00:00 UTC is 14:00 in summer time; the process runs in the C locale, whose names are English
    let summer = at(1_782_907_200);
    assert_eq!(time::format(summer, c"%a %b %-d").unwrap(), "Wed Jul 1");
    assert_eq!(time::format(summer, c"%H:%M").unwrap(), "14:00");
    assert_eq!(time::format(summer, c"%-I:%M").unwrap(), "2:00");
    // 2026-12-31 23:30:00 UTC is already the new year here
    assert_eq!(
        time::format(at(1_798_759_800), c"%Y-%m-%d %H:%M").unwrap(),
        "2027-01-01 00:30"
    );
    let empty = time::format(summer, c"").unwrap_err();
    assert_eq!(empty.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn the_c_locale_counts_hours_to_24() {
    // no program of the toolkit's tests calls `setlocale`, so `LC_TIME` is the C locale's
    assert_eq!(time::locale_time_format(), "%H:%M:%S");
}
