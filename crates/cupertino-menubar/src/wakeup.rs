//! The one timer the process keeps scheduled: for the earliest time something is due (the clock, a menu's delay).
//! A timer scheduled before the one that counts still fires, and is ignored.

use std::time::Instant;

/// Numbers the timers and remembers which one counts.
#[derive(Debug, Default)]
pub struct Wakeup {
    /// The timer that counts.
    id: u64,
    /// When it fires; `None` while none is scheduled.
    due: Option<Instant>,
}

impl Wakeup {
    /// Something is due at `due`: `Some(id)` when a timer must be scheduled for it, `None` when the timer that counts
    /// fires no later.
    pub fn request(&mut self, due: Instant) -> Option<u64> {
        if self.due.is_some_and(|scheduled| scheduled <= due) {
            return None;
        }
        self.id += 1;
        self.due = Some(due);
        Some(self.id)
    }

    /// The timer `id` fired: whether it is the one that counts, after which none is scheduled.
    pub fn fired(&mut self, id: u64) -> bool {
        let counts = id == self.id && self.due.is_some();
        if counts {
            self.due = None;
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn the_first_request_schedules_a_timer() {
        let start = Instant::now();
        let mut wakeup = Wakeup::default();
        assert_eq!(wakeup.request(at(start, 500)), Some(1));
    }

    #[test]
    fn a_later_or_equal_request_keeps_the_scheduled_timer() {
        let start = Instant::now();
        let mut wakeup = Wakeup::default();
        wakeup.request(at(start, 500));
        assert_eq!(wakeup.request(at(start, 500)), None);
        assert_eq!(wakeup.request(at(start, 900)), None);
    }

    #[test]
    fn an_earlier_request_replaces_the_timer_and_the_old_one_is_ignored() {
        let start = Instant::now();
        let mut wakeup = Wakeup::default();
        let old = wakeup.request(at(start, 60_000)).unwrap();
        let new = wakeup.request(at(start, 200)).unwrap();
        assert_eq!((old, new), (1, 2));
        assert!(!wakeup.fired(old));
        assert!(wakeup.fired(new));
    }

    #[test]
    fn after_the_timer_fires_the_next_request_schedules_again() {
        let start = Instant::now();
        let mut wakeup = Wakeup::default();
        let first = wakeup.request(at(start, 500)).unwrap();
        assert!(wakeup.fired(first));
        assert_eq!(wakeup.request(at(start, 900)), Some(2));
    }

    #[test]
    fn a_timer_that_fired_counts_once() {
        let start = Instant::now();
        let mut wakeup = Wakeup::default();
        let id = wakeup.request(at(start, 500)).unwrap();
        assert!(wakeup.fired(id));
        assert!(!wakeup.fired(id));
    }
}
