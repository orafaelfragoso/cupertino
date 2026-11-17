//! `kit::time`: a moment in the session's time zone and back, daylight saving time and day arithmetic included.

use cupertino_kit::time::{self, LocalTime};
use std::time::{Duration, SystemTime};

fn at(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

#[test]
fn local_time_follows_the_zone_and_its_daylight_saving_time_both_ways() {
    // Central European Time with its summer time, named by rule so no zone database is needed; the C library reads
    // the zone once, so the one test of the process sets it first
    std::env::set_var("TZ", "CET-1CEST,M3.5.0,M10.5.0/3");
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
