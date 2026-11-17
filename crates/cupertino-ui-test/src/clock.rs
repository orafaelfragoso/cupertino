use cupertino_ui_core::Clock;
use cupertino_ui_platform::Timestamp;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeWentBackwards {
    pub now: Timestamp,
    pub requested: Timestamp,
}

/// A clock that moves only when the test moves it, or, while it ticks ([`ManualClock::tick_on_read`]), by a step each
/// time it is read. Clones share the same time.
#[derive(Clone, Debug, Default)]
pub struct ManualClock {
    nanos: Arc<AtomicU64>,
    /// How far each read moves the time on; zero while it does not tick.
    step: Arc<AtomicU64>,
    /// How many more reads it may tick for.
    reads_left: Arc<AtomicU64>,
}

impl ManualClock {
    pub fn new(start: Timestamp) -> Self {
        Self {
            nanos: Arc::new(AtomicU64::new(start.0)),
            ..Self::default()
        }
    }

    /// From now on each read of the time moves it on by `step` nanoseconds once read, as work that takes time does,
    /// for at most `limit` reads: the read past them panics, so a loop that never ends fails rather than hangs. A `step`
    /// of zero stops the ticking.
    pub fn tick_on_read(&self, step: u64, limit: u64) {
        self.reads_left.store(limit, Ordering::SeqCst);
        self.step.store(step, Ordering::SeqCst);
    }

    pub fn advance_to(&self, to: Timestamp) -> Result<(), TimeWentBackwards> {
        let now = Timestamp(self.nanos.load(Ordering::SeqCst));
        if to < now {
            return Err(TimeWentBackwards { now, requested: to });
        }
        self.nanos.store(to.0, Ordering::SeqCst);
        Ok(())
    }

    pub fn advance_by(&self, nanos: u64) {
        self.nanos.fetch_add(nanos, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        let step = self.step.load(Ordering::SeqCst);
        if step == 0 {
            return Timestamp(self.nanos.load(Ordering::SeqCst));
        }
        let left = self.reads_left.load(Ordering::SeqCst);
        assert!(left > 0, "the ticking clock was read past its limit");
        self.reads_left.store(left - 1, Ordering::SeqCst);
        Timestamp(self.nanos.fetch_add(step, Ordering::SeqCst))
    }
}
