//! Reads that are running on threads: a read that is already running is not started again, so a folder on a mount that
//! never answers costs one blocked thread, not one for every visit.

use std::collections::HashSet;
use std::hash::Hash;

/// The keys of the reads in flight, and those asked for again while one was running.
#[derive(Debug)]
pub struct InFlight<K> {
    running: HashSet<K>,
    again: HashSet<K>,
}

impl<K> Default for InFlight<K> {
    fn default() -> Self {
        InFlight {
            running: HashSet::new(),
            again: HashSet::new(),
        }
    }
}

impl<K: Eq + Hash + Clone> InFlight<K> {
    /// Note that the read of `key` starts; `false` when one is running already: no second one starts, but the key is
    /// marked so that [`InFlight::finish`] asks for one more read (the folder may have changed meanwhile).
    pub fn start(&mut self, key: K) -> bool {
        if self.running.insert(key.clone()) {
            true
        } else {
            self.again.insert(key);
            false
        }
    }

    /// The read of `key` is over. `true` when it was asked for again meanwhile: the caller starts one more read, and
    /// the key stays in flight for it (however often it was asked for, one read follows).
    pub fn finish(&mut self, key: &K) -> bool {
        if self.again.remove(key) {
            true
        } else {
            self.running.remove(key);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_read_that_is_running_is_not_started_again_and_one_more_read_follows_a_second_ask() {
        let mut reads = InFlight::default();
        assert!(reads.start("/mnt/dead"));
        assert!(!reads.start("/mnt/dead"), "the second visit starts nothing");
        assert!(!reads.start("/mnt/dead"), "nor does the third");
        assert!(
            reads.start("/home/u/Pictures"),
            "another folder is another read"
        );
        assert!(!reads.finish(&"/home/u/Pictures"), "nobody asked again");
        assert!(
            reads.finish(&"/mnt/dead"),
            "asked again while it ran: one more read"
        );
        assert!(!reads.start("/mnt/dead"), "that read is running now");
        assert!(
            reads.finish(&"/mnt/dead"),
            "asked once more during the re-read"
        );
        assert!(!reads.finish(&"/mnt/dead"), "no further ask: it is over");
        assert!(
            reads.start("/mnt/dead"),
            "after the last answer a new read may start"
        );
    }
}
