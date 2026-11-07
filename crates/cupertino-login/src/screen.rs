//! What the lock screen and the greeter share: the clock text and the pacing of their timers.

use cupertino_kit::core::Command;
use cupertino_login::user::format_time;
use cupertino_login::view::Clock;
use std::error::Error;
use std::time::Duration;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// `strftime` patterns for the date (`EEEEMMMMd`) and the time (`Jmm`). The hour cycle of `J` follows
/// the locale; the 24-hour cycle is fixed here (approximate), as in the menu bar clock.
const DATE_FORMAT: &std::ffi::CStr = c"%A, %B %-d";
const TIME_FORMAT: &std::ffi::CStr = c"%H:%M";

/// The interval of the frames of a fade, a shake or the spinner.
pub const FRAME: f64 = 1.0 / 60.0;

/// The time the screens show: only the display clock, never what times authentication, back-off or the lock
/// ([`cupertino_shell_client::test_clock`]).
pub fn display_unix_seconds() -> Result<i64> {
    Ok(cupertino_shell_client::test_clock::display_unix_seconds()?)
}

/// The clock text now.
pub fn clock() -> Result<Clock> {
    let now = display_unix_seconds()?;
    Ok(Clock {
        date: format_time(now, DATE_FORMAT)?,
        time: format_time(now, TIME_FORMAT)?,
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
