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
    fn local(unix: i64) -> cupertino_kit::time::LocalTime {
        cupertino_kit::time::local(cupertino_kit::time::from_unix(unix))
            .unwrap_or_else(|| panic!("localtime_r cannot represent {unix}"))
    }
}

#[cfg(feature = "dbus")]
impl LocalCalendar for SystemCalendar {
    fn hour(&self, unix: i64) -> u8 {
        Self::local(unix).hour
    }

    fn at(&self, unix: i64, days_ahead: i32, hour: u8) -> i64 {
        let local = Self::local(unix);
        // a day past the month's end runs into the next month
        cupertino_kit::time::from_local(
            local.year,
            local.month,
            i32::from(local.day) + days_ahead,
            hour,
            0,
            0,
        )
        .and_then(cupertino_kit::time::to_unix)
        .unwrap_or_else(|| {
            panic!("mktime cannot represent {hour}:00 {days_ahead} days after {unix}")
        })
    }
}
