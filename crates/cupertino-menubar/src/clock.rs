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

#[cfg(test)]
mod tests {
    use super::*;

    /// Sunday 25 October, 15:04:09.
    const TIME: LocalTime = LocalTime {
        weekday: 0,
        day: 25,
        month: 9,
        hour: 15,
        minute: 4,
        second: 9,
    };

    fn with(edit: impl FnOnce(&mut ClockSettings)) -> ClockSettings {
        let mut settings = ClockSettings::UNSET;
        edit(&mut settings);
        settings
    }

    #[test]
    fn an_empty_domain_shows_the_time_with_the_day_period() {
        assert_eq!(clock_text(&TIME, &ClockSettings::UNSET, false), "3:04 PM");
        assert_eq!(
            clock_text(&LocalTime { hour: 0, ..TIME }, &ClockSettings::UNSET, false),
            "12:04 AM"
        );
    }

    #[test]
    fn the_date_joins_with_two_spaces_and_the_weekday_alone_with_one() {
        let full = with(|s| {
            s.show_day_of_week = true;
            s.show_date = ShowDate::Always;
        });
        assert_eq!(clock_text(&TIME, &full, false), "Sun Oct 25  3:04 PM");
        assert_eq!(
            clock_text(&TIME, &with(|s| s.show_day_of_week = true), false),
            "Sun 3:04 PM"
        );
        assert_eq!(
            clock_text(
                &TIME,
                &with(|s| s.show_date = ShowDate::WhenSpaceAllows),
                false
            ),
            "Oct 25  3:04 PM"
        );
    }

    #[test]
    fn the_day_period_shows_only_in_the_12_hour_cycle() {
        assert_eq!(
            clock_text(&TIME, &with(|s| s.show_am_pm = false), false),
            "3:04"
        );
        assert_eq!(
            clock_text(&TIME, &with(|s| s.hour_cycle = HourCycle::H24), false),
            "15:04"
        );
        let morning = LocalTime { hour: 7, ..TIME };
        assert_eq!(
            clock_text(&morning, &with(|s| s.hour_cycle = HourCycle::H24), false),
            "07:04"
        );
    }

    #[test]
    fn seconds_and_flashing_separators() {
        let seconds = with(|s| s.show_seconds = true);
        assert_eq!(clock_text(&TIME, &seconds, false), "3:04:09 PM");
        assert_eq!(
            clock_text(&TIME, &seconds, true),
            "3:04:09 PM",
            "no flashing unless asked for"
        );
        let flashing = with(|s| {
            s.show_seconds = true;
            s.flash_separators = true;
        });
        assert_eq!(
            clock_text(&TIME, &flashing, true),
            "3\u{2008}04\u{2008}09 PM"
        );
    }

    #[test]
    fn the_separators_are_off_on_the_even_seconds_as_the_ticks_land() {
        // ticks land 0.1 s after the boundary
        assert!(separators_off(10.1));
        assert!(!separators_off(11.1));
        assert!(separators_off(12.1));
    }

    #[test]
    fn ticks_come_every_minute_or_every_second_just_after_the_boundary() {
        assert_eq!(tick_interval(&ClockSettings::UNSET), 60);
        assert_eq!(tick_interval(&with(|s| s.flash_separators = true)), 1);
        assert!((next_tick(1_700_000_000.0, &ClockSettings::UNSET) - 1_700_000_040.1).abs() < 1e-6);
        // a tick that fired on time schedules the next boundary, not itself again
        assert!((next_tick(1_700_000_040.1, &ClockSettings::UNSET) - 1_700_000_100.1).abs() < 1e-6);
        let seconds = with(|s| s.show_seconds = true);
        assert!((next_tick(1_700_000_000.5, &seconds) - 1_700_000_001.1).abs() < 1e-6);
        assert!((next_tick(1_700_000_000.05, &seconds) - 1_700_000_000.1).abs() < 1e-6);
    }
}
