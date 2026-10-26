//! What a compromised login screen can cost: a bound on the attempts it may start. The design has no back-off or lock-out
//! on this class of machine (back-off 0, lock-out 0), so this is not parity; the account lock-out
//! stays `pam_faillock`'s. The numbers are approximate, ours.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Attempts accepted within [`ATTEMPT_WINDOW`].
pub const ATTEMPTS_PER_WINDOW: usize = 10;
/// The rolling window of the attempt limit.
pub const ATTEMPT_WINDOW: Duration = Duration::from_secs(60);
/// How long an attempt may wait for the screen to answer a prompt, or for its worker to make progress, before it is cancelled.
pub const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(60);

/// A rolling-window counter: at most `max` events within `window`.
#[derive(Debug)]
pub struct RateLimit {
    max: usize,
    window: Duration,
    events: VecDeque<Instant>,
}

impl RateLimit {
    pub fn new(max: usize, window: Duration) -> Self {
        RateLimit {
            max,
            window,
            events: VecDeque::new(),
        }
    }

    /// Count an event at `now`; false (and not counted) when `max` events already fall within the window before `now`.
    pub fn allow(&mut self, now: Instant) -> bool {
        while self
            .events
            .front()
            .is_some_and(|&at| now.duration_since(at) >= self.window)
        {
            self.events.pop_front();
        }
        if self.events.len() >= self.max {
            return false;
        }
        self.events.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_eleventh_attempt_within_a_minute_is_refused_and_the_window_rolls() {
        let start = Instant::now();
        let mut limit = RateLimit::new(ATTEMPTS_PER_WINDOW, ATTEMPT_WINDOW);
        for second in 0..10 {
            assert!(
                limit.allow(start + Duration::from_secs(second)),
                "attempt {second}"
            );
        }
        assert!(!limit.allow(start + Duration::from_secs(10)), "the 11th");
        // a refused attempt is not counted: when the first falls out of the window one more fits, and only one
        assert!(limit.allow(start + Duration::from_secs(60)));
        assert!(!limit.allow(start + Duration::from_secs(60)));
        // the second attempt falls out one second later
        assert!(limit.allow(start + Duration::from_secs(61)));
    }
}
