//! Local time: the wall clock in the session's time zone, its daylight saving time included, through the C library
//! (`localtime_r` and `mktime`, which read the zone once, from `TZ` or `/etc/localtime`), and as text by the C
//! library's current `LC_TIME` (`strftime`, `nl_langinfo`).
//!
//! That locale is the C library's state of the whole process, set only by `setlocale`: until some code of the process
//! calls it (no Cupertino program does), every thread runs in the `C` locale, whatever `LANG` or `LC_TIME` says, so
//! the text is in English with the `C` formats (`%H:%M:%S`, a 24-hour clock). A `setlocale` anywhere in the process
//! changes what every thread gets here.

use std::ffi::CStr;
use std::io;
use std::time::{Duration, SystemTime};

/// The longest text `format` returns, in bytes: room for a long weekday and month name, a date and a time in any
/// locale.
const FORMATTED_CAPACITY: usize = 128;

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
    let tm = local_tm(at).ok()?;
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

/// The C library's broken-down local time of `at`; the error `localtime_r` sets when it cannot represent it.
fn local_tm(at: SystemTime) -> io::Result<libc::tm> {
    let unrepresentable =
        || io::Error::new(io::ErrorKind::InvalidInput, "the moment is out of range");
    let seconds = to_unix(at).ok_or_else(unrepresentable)?;
    let t = libc::time_t::try_from(seconds).map_err(|_| unrepresentable())?;
    // SAFETY: an all-zero `tm` is a valid value; `localtime_r` reads `t` and writes only `tm`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(tm)
}

/// The moment `seconds` after the Unix epoch, before it when negative.
pub fn from_unix(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

/// The whole seconds from the Unix epoch to `at`, negative before it; a moment between two whole seconds belongs to the
/// earlier one. `None` past the range of an `i64`.
pub fn to_unix(at: SystemTime) -> Option<i64> {
    match at.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok(),
        Err(before) => {
            let before = before.duration();
            let whole = i64::try_from(before.as_secs()).ok()?;
            Some(-whole - i64::from(before.subsec_nanos() > 0))
        }
    }
}

/// `at` in the session's time zone as text by the `strftime` pattern `pattern`, in the words of the C library's
/// current `LC_TIME`, which is the `C` locale's English in a program that has not called `setlocale` (see the module):
/// `c"%a %b %-d"` is `Thu Nov 19`. An error when the C library cannot represent the moment, or the text is empty or
/// longer than [`FORMATTED_CAPACITY`].
pub fn format(at: SystemTime, pattern: &CStr) -> io::Result<String> {
    let tm = local_tm(at)?;
    let mut buf = [0u8; FORMATTED_CAPACITY];
    // SAFETY: the length passed is the buffer's, `pattern` is nul terminated and `tm` a valid broken-down time.
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), pattern.as_ptr(), &tm) };
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "strftime produced no text",
        ));
    }
    Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// The time format of the C library's current `LC_TIME` (`nl_langinfo(T_FMT)`), a `strftime` pattern such as `%H:%M:%S`
/// or `%I:%M:%S %p`: what tells whether that locale counts hours to 12 or to 24. In a program that has not called
/// `setlocale` (see the module) it is the `C` locale's `%H:%M:%S`.
pub fn locale_time_format() -> String {
    // SAFETY: `nl_langinfo` returns a pointer to a nul-terminated string that stays valid until the locale changes; it
    // is copied at once.
    let format = unsafe { CStr::from_ptr(libc::nl_langinfo(libc::T_FMT)) };
    format.to_string_lossy().into_owned()
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
