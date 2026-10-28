use cupertino_ui_core::Clock;
use cupertino_ui_platform::Timestamp;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeWentBackwards {
    pub now: Timestamp,
    pub requested: Timestamp,
}

/// A clock that moves only when the test moves it. Clones share the same time.
#[derive(Clone, Debug, Default)]
pub struct ManualClock {
    nanos: Arc<AtomicU64>,
}

impl ManualClock {
    pub fn new(start: Timestamp) -> Self {
        Self {
            nanos: Arc::new(AtomicU64::new(start.0)),
        }
    }

    pub fn advance_to(&self, to: Timestamp) -> Result<(), TimeWentBackwards> {
        let now = self.now();
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
        Timestamp(self.nanos.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_moves_only_forward_and_clones_share_it() {
        let clock = ManualClock::new(Timestamp(100));
        let other = clock.clone();
        clock.advance_to(Timestamp(250)).unwrap();
        assert_eq!(other.now(), Timestamp(250));
        other.advance_by(50);
        assert_eq!(clock.now(), Timestamp(300));
        assert_eq!(
            clock.advance_to(Timestamp(299)),
            Err(TimeWentBackwards {
                now: Timestamp(300),
                requested: Timestamp(299)
            })
        );
        assert_eq!(clock.now(), Timestamp(300));
        clock.advance_to(Timestamp(300)).unwrap();
    }
}
