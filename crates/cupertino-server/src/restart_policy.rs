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

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS: i32 = 0;
    const EXIT_CODE_1: i32 = 1 << 8;
    const KILLED_BY_SIGSEGV: i32 = 11;

    fn at(ms: u64, base: Instant) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn successful_exit_is_never_restarted() {
        let t0 = Instant::now();
        assert_eq!(
            decide(
                ExitStatus::from_raw(SUCCESS),
                t0,
                at(5_000, t0),
                &mut Backoff::default()
            ),
            Decision::Stop
        );
        assert_eq!(
            decide(
                ExitStatus::from_raw(SUCCESS),
                t0,
                at(10, t0),
                &mut Backoff::default()
            ),
            Decision::Stop
        );
    }

    #[test]
    fn failure_after_the_throttle_restarts_now() {
        let t0 = Instant::now();
        assert_eq!(
            decide(
                ExitStatus::from_raw(EXIT_CODE_1),
                t0,
                at(1_000, t0),
                &mut Backoff::default()
            ),
            Decision::RestartNow
        );
        assert_eq!(
            decide(
                ExitStatus::from_raw(EXIT_CODE_1),
                t0,
                at(60_000, t0),
                &mut Backoff::default()
            ),
            Decision::RestartNow
        );
    }

    #[test]
    fn failure_inside_the_throttle_waits_until_a_second_after_the_start() {
        let t0 = Instant::now();
        assert_eq!(
            decide(
                ExitStatus::from_raw(EXIT_CODE_1),
                t0,
                at(0, t0),
                &mut Backoff::default()
            ),
            Decision::RestartAt(at(1_000, t0))
        );
        assert_eq!(
            decide(
                ExitStatus::from_raw(EXIT_CODE_1),
                t0,
                at(999, t0),
                &mut Backoff::default()
            ),
            Decision::RestartAt(at(1_000, t0))
        );
    }

    #[test]
    fn death_by_signal_is_a_failure() {
        let t0 = Instant::now();
        assert_eq!(
            decide(
                ExitStatus::from_raw(KILLED_BY_SIGSEGV),
                t0,
                at(2_000, t0),
                &mut Backoff::default()
            ),
            Decision::RestartNow
        );
        assert_eq!(
            decide(
                ExitStatus::from_raw(KILLED_BY_SIGSEGV),
                t0,
                at(100, t0),
                &mut Backoff::default()
            ),
            Decision::RestartAt(at(1_000, t0))
        );
    }

    #[test]
    fn a_decision_says_how_long_until_the_restart() {
        let t0 = Instant::now();
        assert_eq!(Decision::RestartNow.restart_in(t0), Some(Duration::ZERO));
        assert_eq!(
            Decision::RestartAt(at(1_500, t0)).restart_in(at(500, t0)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            Decision::RestartAt(t0).restart_in(at(500, t0)),
            Some(Duration::ZERO)
        );
        assert_eq!(Decision::Stop.restart_in(t0), None);
    }

    #[test]
    fn the_throttle_doubles_with_every_failure_up_to_the_cap() {
        let mut backoff = Backoff::default();
        let intervals: Vec<u64> = (0..8)
            .map(|_| backoff.next_interval(Duration::ZERO).as_secs())
            .collect();
        assert_eq!(intervals, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn a_run_of_a_minute_starts_the_throttle_over_but_a_shorter_one_does_not() {
        let mut backoff = Backoff::default();
        for _ in 0..4 {
            backoff.next_interval(Duration::ZERO);
        }
        assert_eq!(backoff.next_interval(Duration::from_secs(59)).as_secs(), 16);
        assert_eq!(backoff.next_interval(STABLE_AFTER).as_secs(), 1);
        assert_eq!(backoff.next_interval(Duration::ZERO).as_secs(), 2);
    }

    #[test]
    fn a_failure_waits_for_the_grown_throttle_and_a_success_does_not_count() {
        let t0 = Instant::now();
        let mut backoff = Backoff::default();
        let failed = ExitStatus::from_raw(EXIT_CODE_1);
        decide(failed, t0, at(0, t0), &mut backoff);
        decide(ExitStatus::from_raw(SUCCESS), t0, at(0, t0), &mut backoff);
        assert_eq!(
            decide(failed, t0, at(0, t0), &mut backoff),
            Decision::RestartAt(at(2_000, t0))
        );
        assert_eq!(
            decide(failed, t0, at(0, t0), &mut backoff),
            Decision::RestartAt(at(4_000, t0))
        );
    }
}
