//! What the lock screen and the greeter share: the clock text, the pacing of their timers, and why the program ended.

use crate::user::{format_time, locale_hour_cycle};
use crate::view::Clock;
use cupertino_defaults::menubar::{HourCycle, MenuBarSettings};
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_kit::core::Command;
use std::error::Error;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};
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

/// How long reading a settings file should take on the I/O pool: a few lines on a local disk.
pub const SETTINGS_DEADLINE: Duration = Duration::from_secs(2);

/// The hour cycle of the session's clock: the menu bar's setting, else the locale's. Reads a file: run it on the I/O
/// pool.
pub fn session_hour_cycle() -> HourCycle {
    MenuBarSettings::path()
        .and_then(|path| MenuBarSettings::load(&path, None).ok())
        .map_or_else(locale_hour_cycle, |(settings, _)| settings.clock.hour_cycle)
}

/// Follow the settings files `paths` for the life of the program: `changed` runs after each burst of changes, on the
/// loop. A watch that cannot be set up is reported, and `what` then follows the file only at the next start.
pub fn watch<M: Send + 'static>(
    paths: Vec<PathBuf>,
    what: &'static str,
    changed: impl Fn() + Send + 'static,
) -> Option<Command<M>> {
    if paths.is_empty() {
        return None;
    }
    match ConfigWatcher::new(paths) {
        Ok(watch) => Some(Command::Perform(Box::pin(
            cupertino_kit::task::follow_settings(watch, move |event| match event {
                WatchEvent::Changed(_) => changed(),
                WatchEvent::Error(e) => eprintln!("cupertino-login: following {what}: {e}"),
            }),
        ))),
        Err(e) => {
            eprintln!("cupertino-login: {what} follows its settings only at the next start: {e}");
            None
        }
    }
}

/// The interval of the frames of a fade, a shake or the spinner.
pub const FRAME: f64 = 1.0 / 60.0;

/// The time the screens show: only the display clock, never what times authentication, back-off or the lock
/// ([`cupertino_kit_shell::test_clock`]).
pub fn display_unix_seconds() -> Result<i64> {
    Ok(cupertino_kit_shell::test_clock::display_unix_seconds()?)
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

/// Why the running program asked its loop to end, for its `run` to return: set just before `Command::Exit`.
static ENDED: Mutex<Option<std::result::Result<(), String>>> = Mutex::new(None);

/// The program ends: `Ok` for an end the server should not restart, else the failure it should.
pub fn end<M>(outcome: std::result::Result<(), String>) -> Vec<Command<M>> {
    *ENDED.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
    vec![Command::Exit]
}

/// How the program ended: what it said at [`end`], or a failure when the loop ended without it.
pub fn ended(what: &str) -> std::result::Result<(), String> {
    ENDED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_else(|| Err(format!("{what} ended")))
}

/// How long the program's end waits for its bus connections to close and their tasks to end: a bus that answers
/// closes them at once, and one that does not is not waited for longer.
pub const CLOSE_WAIT: Duration = Duration::from_millis(500);

/// Close `connections` on the program's thread, whose loop has ended, and run their executors until every task has
/// ended, at most [`CLOSE_WAIT`].
pub fn close(connections: impl IntoIterator<Item = Option<zbus::Connection>>) {
    let connections: Vec<zbus::Connection> = connections.into_iter().flatten().collect();
    if connections.is_empty() {
        return;
    }
    let closing = async {
        for connection in &connections {
            let _ = connection.clone().close().await;
        }
        for connection in &connections {
            let executor = connection.executor().clone();
            while !executor.is_empty() {
                executor.tick().await;
            }
        }
    };
    let waited = async {
        async_io::Timer::after(CLOSE_WAIT).await;
    };
    wait(futures_lite::future::or(closing, waited));
}

/// Wait for `closing` on the program's thread, whose loop has ended.
#[expect(
    clippy::disallowed_methods,
    reason = "the program's loop has ended: nothing on this thread waits to be drawn, and the wait is bounded"
)]
fn wait(closing: impl std::future::Future<Output = ()>) {
    async_io::block_on(closing);
}
