//! Local time: the wall clock in the session's time zone, its daylight saving time included, through the C library
//! (`localtime_r` and `mktime`, which read the zone once, from `TZ` or `/etc/localtime`).

use std::time::{Duration, SystemTime};

/// A moment as the session's calendar and clock name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    /// 1 = January … 12.
    pub month: u8,
    /// 1 … 31.
    pub day: u8,
    /// 0 = Sunday … 6.
    pub weekday: u8,
    /// 0 … 23.
    pub hour: u8,
    pub minute: u8,
    /// 0 … 60: 60 is a leap second.
    pub second: u8,
    /// Seconds east of UTC, daylight saving time included.
    pub utc_offset: i32,
}

/// `at` in the session's time zone; `None` for a moment the C library cannot represent.
pub fn local(at: SystemTime) -> Option<LocalTime> {
    let seconds = match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok()?,
        Err(before) => {
            let before = before.duration();
            // a moment between two whole seconds before the epoch belongs to the earlier one
            let whole = i64::try_from(before.as_secs()).ok()?;
            -whole - i64::from(before.subsec_nanos() > 0)
        }
    };
    let t = libc::time_t::try_from(seconds).ok()?;
    // SAFETY: an all-zero `tm` is a valid value; `localtime_r` reads `t` and writes only `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    Some(LocalTime {
        year: tm.tm_year.checked_add(1900)?,
        month: u8::try_from(tm.tm_mon + 1).ok()?,
        day: u8::try_from(tm.tm_mday).ok()?,
        weekday: u8::try_from(tm.tm_wday).ok()?,
        hour: u8::try_from(tm.tm_hour).ok()?,
        minute: u8::try_from(tm.tm_min).ok()?,
        second: u8::try_from(tm.tm_sec).ok()?,
        utc_offset: i32::try_from(tm.tm_gmtoff).ok()?,
    })
}

/// Now, in the session's time zone.
pub fn now() -> Option<LocalTime> {
    local(SystemTime::now())
}

/// The moment the session's clock shows `hour:minute:second` on `day` of `month` (1 … 12) of `year`. A day past the end
/// of its month runs into the next ones, and a day of 0 or less back into the ones before, so `day + n` is `n` days
/// later; a time that daylight saving time skips or shows twice is resolved as the time zone's rules say. `None` for
/// a moment the C library cannot represent.
pub fn from_local(
    year: i32,
    month: u8,
    day: i32,
    hour: u8,
    minute: u8,
    second: u8,
) -> Option<SystemTime> {
    // SAFETY: an all-zero `tm` is a valid value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year.checked_sub(1900)?;
    tm.tm_mon = i32::from(month) - 1;
    tm.tm_mday = day;
    tm.tm_hour = i32::from(hour);
    tm.tm_min = i32::from(minute);
    tm.tm_sec = i32::from(second);
    // the time zone decides whether daylight saving time is in effect
    tm.tm_isdst = -1;
    // SAFETY: `errno` is the calling thread's; `mktime` reads and normalises the `tm` it is given, and sets `errno` when
    // it returns -1 for a moment it cannot represent (-1 is also one second before the epoch).
    let t = unsafe {
        *libc::__errno_location() = 0;
        libc::mktime(&mut tm)
    };
    if t == -1 && std::io::Error::last_os_error().raw_os_error() != Some(0) {
        return None;
    }
    Some(if t >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(t.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(t.unsigned_abs())
    })
}
