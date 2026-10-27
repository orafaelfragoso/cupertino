//! Reads that are running on threads: a read that is already running is not started again, so a folder on a mount that
//! never answers costs one blocked thread, not one for every visit.

use std::collections::HashSet;
use std::hash::Hash;

/// The keys of the reads in flight.
#[derive(Debug)]
pub struct InFlight<K>(HashSet<K>);

impl<K> Default for InFlight<K> {
    fn default() -> Self {
        InFlight(HashSet::new())
    }
}

impl<K: Eq + Hash> InFlight<K> {
    /// Note that the read of `key` starts; `false` when one is running already and no second one should start.
    pub fn start(&mut self, key: K) -> bool {
        self.0.insert(key)
    }

    /// The read of `key` is over.
    pub fn finish(&mut self, key: &K) {
        self.0.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_that_is_running_is_not_started_again_until_it_finishes() {
        let mut reads = InFlight::default();
        assert!(reads.start("/mnt/dead"));
        assert!(!reads.start("/mnt/dead"), "the second visit starts nothing");
        assert!(
            reads.start("/home/u/Pictures"),
            "another folder is another read"
        );
        reads.finish(&"/mnt/dead");
        assert!(
            reads.start("/mnt/dead"),
            "after the answer a new read may start"
        );
    }
}
