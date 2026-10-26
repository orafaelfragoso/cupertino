//! Restart policy of a supervised shell process: the design session's KeepAlive with
//! `SuccessfulExit = false` and a 1 s throttle (rule 1).
//! A process is restarted only after an exit with failure, and never sooner than the throttle
//! after its last start.

use std::process::ExitStatus;
use std::time::{Duration, Instant};

/// Minimum time between the start of a process and its next start (ThrottleInterval 1).
pub const RESTART_THROTTLE: Duration = Duration::from_secs(1);

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

/// Decision for a process that exited with `status`, after it was last started at `last_start`.
pub fn decide(status: ExitStatus, last_start: Instant, now: Instant) -> Decision {
    // a signal death has no exit code and is a failure
    if status.success() {
        return Decision::Stop;
    }
    let earliest = last_start + RESTART_THROTTLE;
    if earliest <= now {
        Decision::RestartNow
    } else {
        Decision::RestartAt(earliest)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;

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
        assert_eq!(decide(ExitStatus::from_raw(SUCCESS), t0, at(5_000, t0)), Decision::Stop);
        assert_eq!(decide(ExitStatus::from_raw(SUCCESS), t0, at(10, t0)), Decision::Stop);
    }

    #[test]
    fn failure_after_the_throttle_restarts_now() {
        let t0 = Instant::now();
        assert_eq!(decide(ExitStatus::from_raw(EXIT_CODE_1), t0, at(1_000, t0)), Decision::RestartNow);
        assert_eq!(decide(ExitStatus::from_raw(EXIT_CODE_1), t0, at(60_000, t0)), Decision::RestartNow);
    }

    #[test]
    fn failure_inside_the_throttle_waits_until_a_second_after_the_start() {
        let t0 = Instant::now();
        assert_eq!(decide(ExitStatus::from_raw(EXIT_CODE_1), t0, at(0, t0)), Decision::RestartAt(at(1_000, t0)));
        assert_eq!(decide(ExitStatus::from_raw(EXIT_CODE_1), t0, at(999, t0)), Decision::RestartAt(at(1_000, t0)));
    }

    #[test]
    fn death_by_signal_is_a_failure() {
        let t0 = Instant::now();
        assert_eq!(decide(ExitStatus::from_raw(KILLED_BY_SIGSEGV), t0, at(2_000, t0)), Decision::RestartNow);
        assert_eq!(decide(ExitStatus::from_raw(KILLED_BY_SIGSEGV), t0, at(100, t0)), Decision::RestartAt(at(1_000, t0)));
    }
}
