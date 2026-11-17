//! Text of the clock slot and when it changes.
//!
//! The string is a date part and a time part. The date part holds the weekday (`E`) and the day with the short month
//! (`d MMM`); the time part the hour, minutes, seconds and the day period. With the day and month shown the two parts are
//! joined by two spaces, else by one (the English join strings). The locale's skeleton-to-pattern step is
//! replaced by the English patterns (`EEE MMM d`, `h:mm a`, `HH:mm`) with the commas removed as the design removes them
//! (the prototype has no locale data). Ticks come every second while seconds or flashing separators show,
//! else every minute, 0.1 s after the boundary.

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// What a flashing separator becomes on its off phase: a punctuation space (U+2008). The design wraps it in a
/// left-to-right embedding (U+202A … U+202C); the bar lays out left-to-right runs only, so the marks are left out
/// (they change nothing in such a run).
const FLASH_OFF_SEPARATOR: &str = "\u{2008}";

/// Delay after the second or minute boundary at which the clock is redrawn (0.1 s).
pub const TICK_LATENESS: f64 = 0.1;

pub use cupertino_defaults::menubar::{ClockSettings, HourCycle, ShowDate};

/// Seconds between ticks: 1 while seconds or flashing separators show, else 60.
pub fn tick_interval(settings: &ClockSettings) -> u32 {
    if settings.show_seconds || settings.flash_separators {
        1
    } else {
        60
    }
}

/// The local calendar fields the clock shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    /// 0 = Sunday.
    pub weekday: u8,
    /// 1 … 31.
    pub day: u8,
    /// 0 = January.
    pub month: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// The local time of `unix_seconds` in the session's time zone.
#[cfg(feature = "app")]
pub fn local_time(unix_seconds: i64) -> LocalTime {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = unix_seconds as libc::time_t;
    // Safety: both pointers are valid for the duration of the call.
    let converted = unsafe { libc::localtime_r(&t, &mut tm) };
    assert!(
        !converted.is_null(),
        "localtime_r cannot represent {unix_seconds}"
    );
    LocalTime {
        weekday: tm.tm_wday as u8,
        day: tm.tm_mday as u8,
        month: tm.tm_mon as u8,
        hour: tm.tm_hour as u8,
        minute: tm.tm_min as u8,
        second: tm.tm_sec as u8,
    }
}

/// Whether the separators are in their off phase at `t` seconds: `remainder(t, 2) > 0`, the IEEE
/// remainder (quotient rounded to even).
pub fn separators_off(t: f64) -> bool {
    t - 2.0 * (t / 2.0).round_ties_even() > 0.0
}

/// The clock string for `time` under `settings`; `separators_off` hides the `:` (flashing separators).
pub fn clock_text(time: &LocalTime, settings: &ClockSettings, separators_off: bool) -> String {
    let show_date = settings.show_date != ShowDate::Never;
    let mut date = Vec::new();
    if settings.show_day_of_week {
        date.push(WEEKDAYS[usize::from(time.weekday)].to_owned());
    }
    if show_date {
        date.push(format!("{} {}", MONTHS[usize::from(time.month)], time.day));
    }

    let am_pm = settings.hour_cycle == HourCycle::H12 && settings.show_am_pm;
    let separator = if settings.flash_separators && separators_off {
        FLASH_OFF_SEPARATOR
    } else {
        ":"
    };
    let mut clock = match settings.hour_cycle {
        HourCycle::H12 => format!("{}{separator}{:02}", (time.hour + 11) % 12 + 1, time.minute),
        HourCycle::H24 => format!("{:02}{separator}{:02}", time.hour, time.minute),
    };
    if settings.show_seconds {
        clock.push_str(&format!("{separator}{:02}", time.second));
    }
    if am_pm {
        clock.push_str(if time.hour < 12 { " AM" } else { " PM" });
    }

    if date.is_empty() {
        return clock;
    }
    let join = if show_date { "  " } else { " " };
    format!("{}{join}{clock}", date.join(" "))
}

/// The next time, in Unix seconds, at which the clock is redrawn: `TICK_LATENESS` after the next multiple of the interval.
pub fn next_tick(t: f64, settings: &ClockSettings) -> f64 {
    let interval = f64::from(tick_interval(settings));
    ((t - TICK_LATENESS) / interval)
        .floor()
        .mul_add(interval, interval)
        + TICK_LATENESS
}
