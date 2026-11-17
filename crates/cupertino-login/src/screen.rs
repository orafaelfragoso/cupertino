//! What the lock screen and the greeter share: the clock text and the pacing of their timers.

use cupertino_defaults::menubar::{HourCycle, MenuBarSettings};
use cupertino_kit::core::Command;
use cupertino_login::user::{format_time, locale_hour_cycle};
use cupertino_login::view::Clock;
use std::error::Error;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// `strftime` patterns for the date (`EEEMMMd`: the short weekday, the short month and the day, "Mon Nov 16") and the
/// time (`Jmm`) in the 24-hour and the 12-hour cycle: hours and minutes, the 12-hour hour without a leading zero and
/// without the day period, as the large time shows it.
const DATE_FORMAT: &std::ffi::CStr = c"%a %b %-d";
const TIME_FORMAT_24: &std::ffi::CStr = c"%H:%M";
const TIME_FORMAT_12: &std::ffi::CStr = c"%-I:%M";

/// The `strftime` pattern of the time in `cycle`.
pub fn time_format(cycle: HourCycle) -> &'static std::ffi::CStr {
    match cycle {
        HourCycle::H24 => TIME_FORMAT_24,
        HourCycle::H12 => TIME_FORMAT_12,
    }
}

/// The hour cycle of the session's clock: the menu bar's setting, else the locale's.
pub fn session_hour_cycle() -> HourCycle {
    MenuBarSettings::path()
        .and_then(|path| MenuBarSettings::load(&path, None).ok())
        .map_or_else(locale_hour_cycle, |(settings, _)| settings.clock.hour_cycle)
}

/// The interval of the frames of a fade, a shake or the spinner.
pub const FRAME: f64 = 1.0 / 60.0;

/// The time the screens show: only the display clock, never what times authentication, back-off or the lock
/// ([`cupertino_shell_client::test_clock`]).
pub fn display_unix_seconds() -> Result<i64> {
    Ok(cupertino_shell_client::test_clock::display_unix_seconds()?)
}

/// The clock text now, its time in `cycle`.
pub fn clock(cycle: HourCycle) -> Result<Clock> {
    let now = display_unix_seconds()?;
    Ok(Clock {
        date: format_time(now, DATE_FORMAT)?,
        time: format_time(now, time_format(cycle))?,
    })
}

/// Seconds until the clock's minute turns.
pub fn to_next_minute() -> Result<f64> {
    Ok((60 - display_unix_seconds()?.rem_euclid(60)) as f64)
}

/// One timer of an application: only the latest one counts, and a new one is set only when it would fire before the
/// one that is pending, so a stream of input does not pile timers up.
#[derive(Debug, Default)]
pub struct Timer {
    id: u64,
    /// When the pending timer fires, on the application's clock.
    pending: Option<f64>,
}

impl Timer {
    /// The timer `id` fired; whether it is the one that counts.
    pub fn fired(&mut self, id: u64) -> bool {
        if id == self.id {
            self.pending = None;
            true
        } else {
            false
        }
    }

    /// Fire `wait` seconds after `now` (`None`: no timer is needed), unless the pending timer fires before that.
    pub fn schedule<M: Send + 'static>(
        &mut self,
        now: f64,
        wait: Option<f64>,
        message: fn(u64) -> M,
    ) -> Option<Command<M>> {
        let wait = wait?.max(0.0);
        let at = now + wait;
        if self
            .pending
            .is_some_and(|pending| pending > now && pending <= at)
        {
            return None;
        }
        self.id += 1;
        self.pending = Some(at);
        let id = self.id;
        Some(Command::Perform(Box::pin(async move {
            async_io::Timer::after(Duration::from_secs_f64(wait)).await;
            message(id)
        })))
    }
}
