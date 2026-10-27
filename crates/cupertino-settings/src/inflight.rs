//! Reads that are running on threads: a read that is already running is not started again, so a folder on a mount that
//! never answers costs one blocked thread, not one for every visit.

use std::collections::{HashMap, HashSet};
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

    /// The read of `key` could not start or is given up: nothing is running or waiting for it any more.
    pub fn abandon(&mut self, key: &K) {
        self.running.remove(key);
        self.again.remove(key);
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

/// The reads of a set of keys: one runs at a time per key; one asked for meanwhile waits, only the latest of them, and
/// runs once the first is answered.
#[derive(Debug)]
pub struct Reads<K, J> {
    in_flight: InFlight<K>,
    waiting: HashMap<K, J>,
}

impl<K, J> Default for Reads<K, J> {
    fn default() -> Self {
        Reads {
            in_flight: InFlight::default(),
            waiting: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Clone, J> Reads<K, J> {
    /// A read of `key` is wanted, with `job` to run it. `Some(job)` when it may start now; `None` when one is running
    /// already: `job` waits for its answer (replacing an earlier waiting one).
    pub fn ask(&mut self, key: K, job: J) -> Option<J> {
        if self.in_flight.start(key.clone()) {
            Some(job)
        } else {
            self.waiting.insert(key, job);
            None
        }
    }

    /// The read of `key` could not start (no thread, no job): forget it and the job waiting for it, so that the next ask
    /// starts a read.
    pub fn abandon(&mut self, key: &K) {
        self.in_flight.abandon(key);
        self.waiting.remove(key);
    }

    /// The read of `key` was answered. `Some(job)` when another was asked for meanwhile: start it now (the key stays
    /// in flight for it).
    pub fn answered(&mut self, key: &K) -> Option<J> {
        if self.in_flight.finish(key) {
            self.waiting.remove(key)
        } else {
            None
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

    #[test]
    fn a_read_asked_for_while_one_runs_waits_and_only_the_latest_runs_after_the_answer() {
        let mut reads: Reads<&str, u32> = Reads::default();
        assert_eq!(
            reads.ask("/mnt/dead", 1),
            Some(1),
            "nothing runs: start at once"
        );
        assert_eq!(reads.ask("/mnt/dead", 2), None);
        assert_eq!(
            reads.ask("/mnt/dead", 3),
            None,
            "the latest request replaces the waiting one"
        );
        assert_eq!(
            reads.ask("/home/u", 10),
            Some(10),
            "another folder is its own read"
        );
        assert_eq!(reads.answered(&"/home/u"), None, "nobody waited");
        assert_eq!(
            reads.answered(&"/mnt/dead"),
            Some(3),
            "the waiting job starts"
        );
        assert_eq!(reads.ask("/mnt/dead", 4), None, "that read is running now");
        assert_eq!(reads.answered(&"/mnt/dead"), Some(4));
        assert_eq!(
            reads.answered(&"/mnt/dead"),
            None,
            "nothing is waiting: it is over"
        );
        assert_eq!(
            reads.ask("/mnt/dead", 5),
            Some(5),
            "after the last answer a new read starts"
        );
    }

    #[test]
    fn a_read_that_could_not_start_is_forgotten_so_the_next_ask_starts_one() {
        let mut reads: Reads<&str, u32> = Reads::default();
        assert_eq!(reads.ask("/home/u", 1), Some(1));
        assert_eq!(reads.ask("/home/u", 2), None, "a job is waiting");
        reads.abandon(&"/home/u");
        assert_eq!(
            reads.answered(&"/home/u"),
            None,
            "nothing runs or waits any more"
        );
        assert_eq!(reads.ask("/home/u", 3), Some(3), "the folder lists again");
    }
}
