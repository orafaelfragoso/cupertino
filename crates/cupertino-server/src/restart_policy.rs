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
        interval
    }
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
