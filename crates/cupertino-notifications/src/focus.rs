//! Focus durations: the "For 1 hour", "Until this evening" and "Until tomorrow morning" rows the menu offers for do not
//! disturb, and when each one ends.
//!
//! The the design the list in its do-not-disturb service and the menus only show it; here the notification
//! process owns it the same way and serves it on `org.cupertino.NotificationCenter1`. The location and calendar-event
//! rows need a place and a calendar source the session does not have, so they are never offered.
//! The calendar is the session's local time, behind [`LocalCalendar`] so the rules are tested without a time zone.

/// The longest a duration may run: the design clamps every duration to one day (86400 s).
pub const MAX_DURATION: i64 = 86_400;

/// Length of "For 1 hour" (3600 s).
pub const ONE_HOUR: i64 = 3_600;

/// A duration row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lifetime {
    OneHour,
    ThisEvening,
    TomorrowMorning,
}

impl Lifetime {
    /// The row identifier: the suffix of the design's identifiers (`…lifetime.one-hour`, `evening`, `morning`).
    pub fn id(self) -> &'static str {
        match self {
            Self::OneHour => "one-hour",
            Self::ThisEvening => "evening",
            Self::TomorrowMorning => "morning",
        }
    }

    /// The English menu title (the design's localisation table).
    pub fn title(self) -> &'static str {
        match self {
            Self::OneHour => "For 1 hour",
            Self::ThisEvening => "Until this evening",
            Self::TomorrowMorning => "Until tomorrow morning",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        [Self::OneHour, Self::ThisEvening, Self::TomorrowMorning]
            .into_iter()
            .find(|l| l.id() == id)
    }
}

/// Local calendar arithmetic in the session's time zone.
pub trait LocalCalendar {
    /// The local hour (0 … 23) at `unix` seconds.
    fn hour(&self, unix: i64) -> u8;
    /// Unix seconds of `hour`:00:00 local time on the local day of `unix`, `days_ahead` days later.
    fn at(&self, unix: i64, days_ahead: i32, hour: u8) -> i64;
}

/// The rows offered at local hour `hour`: "For 1 hour" always, then "Until this evening" from 04:00 to 17:59 and
/// "Until tomorrow morning" otherwise.
pub fn offered(hour: u8) -> [Lifetime; 2] {
    let second = if (4..=17).contains(&hour) {
        Lifetime::ThisEvening
    } else {
        Lifetime::TomorrowMorning
    };
    [Lifetime::OneHour, second]
}

/// When `lifetime` started at `now` ends. The evening and morning rows share one rule: before 04:00
/// today 07:00, before 18:00 today 19:00, else tomorrow 07:00; so the end is never in the past.
pub fn end(lifetime: Lifetime, now: i64, calendar: &dyn LocalCalendar) -> i64 {
    let end = match lifetime {
        Lifetime::OneHour => now + ONE_HOUR,
        Lifetime::ThisEvening | Lifetime::TomorrowMorning => match calendar.hour(now) {
            0..=3 => calendar.at(now, 0, 7),
            4..=17 => calendar.at(now, 0, 19),
            _ => calendar.at(now, 1, 7),
        },
    };
    end.min(now + MAX_DURATION)
}

/// The session's calendar through the C library (`localtime_r` and `mktime`, daylight saving time resolved by the
/// time zone rules).
#[cfg(feature = "dbus")]
pub struct SystemCalendar;

#[cfg(feature = "dbus")]
impl SystemCalendar {
    fn local(unix: i64) -> libc::tm {
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let t = unix as libc::time_t;
        // Safety: both pointers are valid for the duration of the call.
        let converted = unsafe { libc::localtime_r(&t, &mut tm) };
        assert!(!converted.is_null(), "localtime_r cannot represent {unix}");
        tm
    }
}

#[cfg(feature = "dbus")]
impl LocalCalendar for SystemCalendar {
    fn hour(&self, unix: i64) -> u8 {
        Self::local(unix).tm_hour as u8
    }

    fn at(&self, unix: i64, days_ahead: i32, hour: u8) -> i64 {
        let mut tm = Self::local(unix);
        // mktime normalises a day past the month's end
        tm.tm_mday += days_ahead;
        tm.tm_hour = i32::from(hour);
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        // Safety: the pointer is valid for the duration of the call.
        let t = unsafe { libc::mktime(&mut tm) };
        assert!(
            t != -1,
            "mktime cannot represent {hour}:00 {days_ahead} days after {unix}"
        );
        t as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed-offset zone: local = UTC + `offset` seconds.
    struct Fixed {
        offset: i64,
    }

    impl LocalCalendar for Fixed {
        fn hour(&self, unix: i64) -> u8 {
            ((unix + self.offset).rem_euclid(86_400) / 3_600) as u8
        }

        fn at(&self, unix: i64, days_ahead: i32, hour: u8) -> i64 {
            let local_day = (unix + self.offset).div_euclid(86_400) + i64::from(days_ahead);
            local_day * 86_400 + i64::from(hour) * 3_600 - self.offset
        }
    }

    const ZONE: Fixed = Fixed { offset: -3 * 3_600 };
    /// 2026-10-25 00:00 local in [`ZONE`].
    const MIDNIGHT: i64 = 1_792_897_200;

    fn local(hour: i64, minute: i64) -> i64 {
        MIDNIGHT + hour * 3_600 + minute * 60
    }

    #[test]
    fn the_fixture_midnight_is_local_midnight() {
        assert_eq!(ZONE.hour(MIDNIGHT), 0);
        assert_eq!(ZONE.hour(MIDNIGHT - 1), 23);
    }

    #[test]
    fn the_evening_row_shows_from_four_to_six_and_the_morning_row_otherwise() {
        assert_eq!(offered(3), [Lifetime::OneHour, Lifetime::TomorrowMorning]);
        assert_eq!(offered(4), [Lifetime::OneHour, Lifetime::ThisEvening]);
        assert_eq!(offered(17), [Lifetime::OneHour, Lifetime::ThisEvening]);
        assert_eq!(offered(18), [Lifetime::OneHour, Lifetime::TomorrowMorning]);
    }

    #[test]
    fn one_hour_ends_an_hour_later() {
        assert_eq!(end(Lifetime::OneHour, local(23, 30), &ZONE), local(24, 30));
    }

    #[test]
    fn evening_and_morning_end_at_seven_in_the_evening_or_the_next_morning() {
        assert_eq!(
            end(Lifetime::ThisEvening, local(10, 15), &ZONE),
            local(19, 0)
        );
        assert_eq!(
            end(Lifetime::TomorrowMorning, local(2, 0), &ZONE),
            local(7, 0),
            "before four the morning is today's"
        );
        assert_eq!(
            end(Lifetime::TomorrowMorning, local(18, 0), &ZONE),
            local(24 + 7, 0)
        );
        // the same rule serves both titles, so a stale evening row picked after 18:00 still ends in the future
        assert_eq!(
            end(Lifetime::ThisEvening, local(20, 0), &ZONE),
            local(24 + 7, 0)
        );
    }

    #[test]
    fn rows_are_found_by_their_identifier() {
        assert_eq!(Lifetime::from_id("evening"), Some(Lifetime::ThisEvening));
        assert_eq!(Lifetime::from_id("location"), None);
    }
}
