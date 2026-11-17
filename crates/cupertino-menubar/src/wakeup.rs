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
