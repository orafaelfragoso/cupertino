//! Restart policy of a supervised shell process: it is restarted only after an exit with failure (a successful exit stops
//! it for good), and never sooner than the throttle after its last start. The throttle starts at 1 s and doubles with every failure up to [`BACKOFF_CAP`], so a process that
//! cannot run is tried less and less often; it starts over once a run lasted [`STABLE_AFTER`].

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::{Event, Value};

/// Minimum time between the start of a process and its next start after the first failure.
pub const RESTART_THROTTLE: Duration = Duration::from_secs(1);
/// The throttle never grows beyond this.
pub const BACKOFF_CAP: Duration = Duration::from_secs(30);
/// A run this long counts as working: the throttle starts over at [`RESTART_THROTTLE`].
pub const STABLE_AFTER: Duration = Duration::from_secs(60);

/// The throttle of one process: what the next failure waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    interval: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            interval: RESTART_THROTTLE,
        }
    }
}

impl Backoff {
    /// The minimum time between a start and the next, for a failure after a run of `ran`; the one after doubles.
    pub fn next_interval(&mut self, ran: Duration) -> Duration {
        if ran >= STABLE_AFTER {
            *self = Self::default();
        }
        let interval = self.interval;
        self.interval = (interval * 2).min(BACKOFF_CAP);
        scaled(interval)
    }
}

/// The environment variable a test harness sets to a factor for the restart delays (`0.05` waits a twentieth). Only a
/// build with the `test` feature reads it, once, at the first delay, so a production server cannot be slowed or sped
/// up by its environment. It scales the waits ([`RESTART_THROTTLE`] up to [`BACKOFF_CAP`], the shell components'
/// backoff and the shared host's quiet moment), never the windows that count failures or runs ([`STABLE_AFTER`] and
/// the give-up window), so the events, their order and their counts stay the same and only come sooner.
pub const TEST_TIME_SCALE_ENV: &str = "CUPERTINO_TEST_TIME_SCALE";

/// `delay` as this process waits it: as given, or times [`TEST_TIME_SCALE_ENV`] in a test build that sets it.
pub fn scaled(delay: Duration) -> Duration {
    test_time_scale().map_or(delay, |scale| delay.mul_f64(scale))
}

/// The factor [`TEST_TIME_SCALE_ENV`] gives when it is a finite number above 0, read once.
#[cfg(feature = "test")]
fn test_time_scale() -> Option<f64> {
    static SCALE: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    *SCALE.get_or_init(|| {
        std::env::var(TEST_TIME_SCALE_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|s| s.is_finite() && *s > 0.0)
    })
}

/// A build without the `test` feature waits every delay as given.
#[cfg(not(feature = "test"))]
fn test_time_scale() -> Option<f64> {
    None
}

/// What to do with a process that exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Start it again immediately.
    RestartNow,
    /// Start it again at this time.
    RestartAt(Instant),
    /// Leave it stopped.
    Stop,
}

impl Decision {
    /// How long after `now` the process starts again; `None` when it stays stopped.
    pub fn restart_in(self, now: Instant) -> Option<Duration> {
        match self {
            Decision::RestartNow => Some(Duration::ZERO),
            Decision::RestartAt(at) => Some(at.saturating_duration_since(now)),
            Decision::Stop => None,
        }
    }
}

/// `event` with how a process ended: `status`, its exit code, or `signal`, the signal that ended it.
pub fn with_exit_status(event: Event<'_>, status: ExitStatus) -> Event<'_> {
    match (status.code(), status.signal()) {
        (Some(code), _) => event.field("status", Value::Int(code.into())),
        (None, Some(signal)) => event.field("signal", Value::Int(signal.into())),
        (None, None) => event,
    }
}

/// Decision for a shell host that exited, after it was last started at `last_start`: whatever its status it starts
/// again, at once when the server `expected` the exit (not counted), else never sooner than the throttle after its last
/// start, which grows with each such exit and starts over after a run of [`STABLE_AFTER`]. A host is never given up.
pub fn decide_host(
    expected: bool,
    last_start: Instant,
    now: Instant,
    backoff: &mut Backoff,
) -> Decision {
    if expected {
        return Decision::RestartNow;
    }
    let earliest = last_start + backoff.next_interval(now.saturating_duration_since(last_start));
    if earliest <= now {
        Decision::RestartNow
    } else {
        Decision::RestartAt(earliest)
    }
}

/// Decision for a process that exited with `status`, after it was last started at `last_start`.
pub fn decide(
    status: ExitStatus,
    last_start: Instant,
    now: Instant,
    backoff: &mut Backoff,
) -> Decision {
    // a signal death has no exit code and is a failure
    if status.success() {
        return Decision::Stop;
    }
    let earliest = last_start + backoff.next_interval(now.saturating_duration_since(last_start));
    if earliest <= now {
        Decision::RestartNow
    } else {
        Decision::RestartAt(earliest)
    }
}
