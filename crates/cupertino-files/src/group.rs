//! The date buckets a date is worded in relative to now: Today, Yesterday, Previous 7 Days, Previous 30 Days, then
//! the month for an earlier date of this year, else the year.

use std::time::SystemTime;

/// Seconds-resolution calendar needed for the date buckets (local time through `localtime_r`).
fn local_ymd(t: SystemTime) -> (i32, u32, u32) {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let tt = secs as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&tt, &mut tm);
    }
    (tm.tm_year + 1900, tm.tm_mon as u32 + 1, tm.tm_mday as u32)
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn day_number(y: i32, m: u32, d: u32) -> i64 {
    // days from civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Bucket title and sort rank (lower = nearer to now) of a date relative to `now`.
pub fn date_bucket(t: Option<SystemTime>, now: SystemTime) -> (String, i64) {
    let Some(t) = t else {
        return ("Unknown".into(), i64::MAX);
    };
    let (ty, tm, td) = local_ymd(t);
    let (ny, nm, nd) = local_ymd(now);
    let age = day_number(ny, nm, nd) - day_number(ty, tm, td);
    match age {
        i64::MIN..=0 => ("Today".into(), 0),
        1 => ("Yesterday".into(), 1),
        2..=7 => ("Previous 7 Days".into(), 2),
        8..=30 => ("Previous 30 Days".into(), 3),
        _ if ty == ny => (
            MONTHS[tm as usize - 1].to_string(),
            100 + (nm as i64 - tm as i64),
        ),
        _ => (ty.to_string(), 1000 + (ny - ty) as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// `now - days`.
    fn days_ago(now: SystemTime, days: u64) -> SystemTime {
        now - Duration::from_secs(days * 86_400)
    }

    fn now() -> SystemTime {
        // 2026-10-22 12:00 UTC
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_670_400)
    }

    #[test]
    fn date_buckets() {
        let n = now();
        assert_eq!(date_bucket(Some(n), n).0, "Today");
        assert_eq!(date_bucket(Some(days_ago(n, 1)), n).0, "Yesterday");
        assert_eq!(date_bucket(Some(days_ago(n, 5)), n).0, "Previous 7 Days");
        assert_eq!(date_bucket(Some(days_ago(n, 20)), n).0, "Previous 30 Days");
        assert_eq!(date_bucket(Some(days_ago(n, 60)), n).0, "August");
        assert_eq!(date_bucket(Some(days_ago(n, 400)), n).0, "2025");
        assert_eq!(date_bucket(None, n).0, "Unknown");
    }
}
