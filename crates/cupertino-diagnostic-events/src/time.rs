//! Times as events write them: RFC 3339 in UTC with milliseconds, `2026-11-09T14:03:27.512Z`, and back.
//!
//! The formatting allocates nothing and calls nothing but arithmetic, so a signal handler may use [`format_into`].

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The length of a formatted time: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub const LEN: usize = 24;

/// `time` as RFC 3339 UTC with milliseconds. A time before 1970 is written as the epoch.
pub fn format(time: SystemTime) -> String {
    let mut buf = [0u8; LEN];
    format_into(time, &mut buf);
    // only ASCII digits and separators are written
    String::from_utf8_lossy(&buf).into_owned()
}

/// Writes `time` into `out`; async-signal-safe.
pub fn format_into(time: SystemTime, out: &mut [u8; LEN]) {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let secs = since.as_secs();
    let millis = since.subsec_millis();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rest = secs % 86_400;
    let mut put = |at: usize, value: u64, width: usize| {
        let mut v = value;
        for i in (0..width).rev() {
            out[at + i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
    };
    put(0, year.clamp(0, 9999) as u64, 4);
    put(5, month as u64, 2);
    put(8, day as u64, 2);
    put(11, rest / 3600, 2);
    put(14, rest / 60 % 60, 2);
    put(17, rest % 60, 2);
    put(20, millis as u64, 3);
    for (at, sep) in [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ] {
        out[at] = sep;
    }
}

/// Parses what [`format`] writes, and also RFC 3339 times with another number of fraction digits, without a fraction,
/// or with a `+HH:MM`/`-HH:MM` offset. `None` for anything else.
pub fn parse(text: &str) -> Option<SystemTime> {
    let b = text.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<u64> {
        let s = text.get(from..to)?;
        s.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| s.parse().ok())?
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if b[13] != b':' || b[16] != b':' {
        return None;
    }
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut at = 19;
    let mut nanos = 0u64;
    if b.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while b.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let digits = &text[start..at];
        if digits.is_empty() {
            return None;
        }
        let kept = &digits[..digits.len().min(9)];
        nanos = kept.parse::<u64>().ok()? * 10u64.pow(9 - kept.len() as u32);
    }
    let offset: i64 = match &text[at..] {
        "Z" | "z" => 0,
        zone if zone.len() == 6
            && matches!(zone.as_bytes()[0], b'+' | b'-')
            && zone.as_bytes()[3] == b':' =>
        {
            let h = num(at + 1, at + 3)? as i64;
            let m = num(at + 4, at + 6)? as i64;
            let sign = if zone.starts_with('-') { -1 } else { 1 };
            sign * (h * 3600 + m * 60)
        }
        _ => return None,
    };
    let days = days_from_civil(year as i64, month as u32, day as u32);
    let secs = days * 86_400 + (hour * 3600 + minute * 60 + second) as i64 - offset;
    let secs = u64::try_from(secs).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(secs, nanos as u32))
}

/// The proleptic Gregorian date of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The day count since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_utc_with_milliseconds_and_read_back() {
        let t = UNIX_EPOCH + Duration::from_millis(1_793_022_207_512);
        assert_eq!(format(t), "2026-10-26T13:43:27.512Z");
        assert_eq!(parse("2026-10-26T13:43:27.512Z"), Some(t));
        assert_eq!(format(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format(UNIX_EPOCH + Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00.000Z"
        );
        assert_eq!(parse("2026-10-26T15:43:27.512+02:00"), Some(t));
        assert_eq!(
            parse("2026-10-26T13:43:27Z"),
            Some(t - Duration::from_millis(512))
        );
        for bad in [
            "",
            "2026-10-26",
            "2026-13-01T00:00:00Z",
            "2026-10-26T13:43:27.Z",
            "2026-10-26T13:43:27",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn every_day_of_four_centuries_round_trips() {
        for day in -1000..146_097 * 2 {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
    }
}
