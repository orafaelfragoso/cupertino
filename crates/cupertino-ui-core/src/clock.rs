use crate::Clock;
use cupertino_ui_platform::Timestamp;
use rustix::time::{clock_gettime, ClockId};

/// The system's monotonic clock, the one the window system stamps its events with.
#[derive(Clone, Copy, Debug, Default)]
pub struct MonotonicClock;

impl Clock for MonotonicClock {
    fn now(&self) -> Timestamp {
        let t = clock_gettime(ClockId::Monotonic);
        Timestamp(t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64)
    }
}
