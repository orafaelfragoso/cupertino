//! Reads that are running on threads: a read that is already running is not started again, so a folder on a mount that
//! never answers costs one blocked thread, not one for every visit; and work that takes turns, so that many tasks of
//! one kind hold only a few of the window's threads of the I/O pool.

use std::collections::{HashMap, HashSet, VecDeque};
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

/// How many thumbnails of the Wallpaper pane are made at once, and how many of its folders of pictures are read at
/// once: with the one task of the services and the window's other work, they stay within the window's share of Kit's
/// I/O pool (`cupertino_kit::task::IO_SHARE`), so a call or a reading of the services never waits behind them.
pub const THUMBNAIL_TURNS: usize = 1;
pub const PICTURE_FOLDER_TURNS: usize = 1;

/// Work that takes turns: at most a number of jobs run at once, and the others wait in the order they were asked for.
#[derive(Debug)]
pub struct Turns<J> {
    limit: usize,
    running: usize,
    waiting: VecDeque<J>,
}

impl<J> Turns<J> {
    /// No job running, and at most `limit` (at least one) at once.
    pub fn new(limit: usize) -> Self {
        Turns {
            limit: limit.max(1),
            running: 0,
            waiting: VecDeque::new(),
        }
    }

    /// `job` is wanted: `Some(job)` when it may start now, `None` when it waits for a running one to finish.
    pub fn ask(&mut self, job: J) -> Option<J> {
        if self.running < self.limit {
            self.running += 1;
            Some(job)
        } else {
            self.waiting.push_back(job);
            None
        }
    }

    /// A running job finished: the first waiting one, which starts now in its place.
    pub fn finished(&mut self) -> Option<J> {
        match self.waiting.pop_front() {
            Some(job) => Some(job),
            None => {
                self.running = self.running.saturating_sub(1);
                None
            }
        }
    }

    /// How many jobs are running.
    pub fn running(&self) -> usize {
        self.running
    }

    /// How many jobs wait for their turn.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }
}
