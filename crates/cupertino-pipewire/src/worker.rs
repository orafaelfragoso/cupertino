//! A target's volume read and written on a thread of its own, so a dragged slider, a held key or a panel's periodic
//! reading never waits for `wpctl`.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::{Error, Result, Sink, Target, Volume, Wpctl};

/// What the worker last read: the target's volume and, when asked for, the sinks. A failure is `wpctl`'s message.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub volume: std::result::Result<Volume, String>,
    /// `None` until a reading with the sinks was asked for.
    pub sinks: Option<std::result::Result<Vec<Sink>, String>>,
}

/// Reads and writes the volume of one target in the background. Writes go first, the latest value winning: values
/// asked while a write runs replace each other, and only the last is written next. The mute state is written only
/// when it differs from the one last written or read. A reading runs when asked for, after the writes; one that a
/// write overtook is dropped, and a value asked shows in [`VolumeWorker::latest`] at once. Dropping the worker writes
/// what is still asked and ends the thread.
pub struct VolumeWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    asked: Option<Volume>,
    /// A reading is asked for, with the sinks when `true`.
    read: Option<bool>,
    busy: bool,
    /// Counts the values asked, so a reading that started before one is known to be stale.
    asks: u64,
    latest: Option<Reading>,
    failed: Option<Error>,
    closed: bool,
}

enum Job {
    Write(Volume),
    Read { sinks: bool, asks: u64 },
}

impl VolumeWorker {
    pub fn new(wpctl: Wpctl, target: Target) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("cupertino-pipewire-volume".into())
                .spawn(move || work(&wpctl, target, &shared))
                .expect("a thread can be started")
        };
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Ask for `volume` and return at once. The error is that of a write that failed since the last call, if any.
    pub fn set(&self, volume: Volume) -> Result<()> {
        let mut state = self.shared.lock();
        state.asked = Some(volume);
        state.asks += 1;
        let sinks = state.latest.as_ref().and_then(|r| r.sinks.clone());
        state.latest = Some(Reading {
            volume: Ok(volume),
            sinks,
        });
        self.shared.changed.notify_all();
        state.failed.take().map_or(Ok(()), Err)
    }

    /// Ask for a reading, with the sinks when `sinks`, and return at once; [`VolumeWorker::latest`] has it once done.
    pub fn request_read(&self, sinks: bool) {
        let mut state = self.shared.lock();
        state.read = Some(state.read.unwrap_or(false) || sinks);
        self.shared.changed.notify_all();
    }

    /// The last reading, or the value asked since; `None` before the first.
    pub fn latest(&self) -> Option<Reading> {
        self.shared.lock().latest.clone()
    }

    /// Wait until every value asked is written and every reading asked for is done.
    pub fn wait_idle(&self) {
        let mut state = self.shared.lock();
        while state.asked.is_some() || state.read.is_some() || state.busy {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for VolumeWorker {
    fn drop(&mut self) {
        {
            let mut state = self.shared.lock();
            state.closed = true;
            state.read = None;
        }
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn next_job(shared: &Shared) -> Option<Job> {
    let mut state = shared.lock();
    loop {
        if let Some(volume) = state.asked.take() {
            state.busy = true;
            return Some(Job::Write(volume));
        }
        if let Some(sinks) = state.read.take() {
            state.busy = true;
            return Some(Job::Read {
                sinks,
                asks: state.asks,
            });
        }
        if state.closed {
            return None;
        }
        state = shared
            .changed
            .wait(state)
            .unwrap_or_else(|e| e.into_inner());
    }
}

fn work(wpctl: &Wpctl, target: Target, shared: &Shared) {
    let mut muted = None;
    while let Some(job) = next_job(shared) {
        match job {
            Job::Write(volume) => {
                let mut written = wpctl.set_level(target, volume.level);
                if written.is_ok() && muted != Some(volume.muted) {
                    written = wpctl.set_muted(target, volume.muted);
                    muted = written.is_ok().then_some(volume.muted);
                }
                let mut state = shared.lock();
                if let Err(error) = written {
                    state.failed = Some(error);
                }
                state.busy = false;
            }
            Job::Read { sinks, asks } => {
                let volume = wpctl.volume(target).map_err(|e| e.to_string());
                let listed = sinks.then(|| wpctl.sinks().map_err(|e| e.to_string()));
                if let Ok(read) = &volume {
                    muted = Some(read.muted);
                }
                let mut state = shared.lock();
                if state.asks == asks {
                    let kept = state.latest.as_ref().and_then(|r| r.sinks.clone());
                    state.latest = Some(Reading {
                        volume,
                        sinks: listed.or(kept),
                    });
                }
                state.busy = false;
            }
        }
        shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeWpctl;

    const QUIET: Volume = Volume {
        level: 0.25,
        muted: false,
    };

    fn worker(fake: &FakeWpctl) -> VolumeWorker {
        VolumeWorker::new(Wpctl::new(fake.program()), Target::DefaultSink)
    }

    #[test]
    fn the_last_value_asked_is_the_one_the_sink_ends_with() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        let worker = worker(&fake);
        for step in 1..=20 {
            worker
                .set(Volume {
                    level: f64::from(step) / 40.0,
                    muted: false,
                })
                .unwrap();
        }
        let last = Volume {
            level: 0.75,
            muted: true,
        };
        worker.set(last).unwrap();
        assert_eq!(worker.latest().unwrap().volume, Ok(last), "shown at once");
        worker.wait_idle();
        assert_eq!(fake.volume(Target::DefaultSink), last);
        assert_eq!(fake.volume(Target::DefaultSource), QUIET);
    }

    #[test]
    fn a_reading_asked_for_arrives_without_the_caller_waiting_and_follows_the_writes() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        fake.set_sinks(&[(52, "Speakers")], 52);
        let worker = worker(&fake);
        assert_eq!(worker.latest(), None);
        worker.request_read(true);
        worker.wait_idle();
        let read = worker.latest().unwrap();
        assert_eq!(read.volume, Ok(QUIET));
        assert_eq!(
            read.sinks
                .unwrap()
                .unwrap()
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            [52]
        );
        let loud = Volume {
            level: 0.5,
            muted: false,
        };
        worker.set(loud).unwrap();
        worker.request_read(false);
        worker.wait_idle();
        let read = worker.latest().unwrap();
        assert_eq!(read.volume, Ok(loud), "the reading came after the write");
        assert!(
            read.sinks.is_some(),
            "a reading without the sinks keeps them"
        );
    }

    #[test]
    fn a_failed_write_is_reported_by_the_next_call_and_a_failed_reading_in_the_latest() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        fake.set_running(false);
        let worker = worker(&fake);
        worker.set(QUIET).unwrap();
        worker.wait_idle();
        assert!(worker.set(QUIET).is_err());
        worker.wait_idle();
        worker.request_read(false);
        worker.wait_idle();
        assert!(worker.latest().unwrap().volume.is_err());
    }

    #[test]
    fn dropping_the_worker_writes_what_is_still_asked() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        let asked = Volume {
            level: 0.5,
            muted: false,
        };
        worker(&fake).set(asked).unwrap();
        assert_eq!(fake.volume(Target::DefaultSink), asked);
    }
}
