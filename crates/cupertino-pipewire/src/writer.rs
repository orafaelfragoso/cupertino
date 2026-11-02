//! A target's volume written on a thread of its own, so a dragged slider or a held key never waits for `wpctl`.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use crate::{Error, Result, Target, Volume, Wpctl};

/// Writes the volume of one target in the background, the latest value winning: values asked while a write runs
/// replace each other, and only the last is written next. The mute state is written only when it differs from the one
/// written before. Dropping the writer writes what is still asked and ends the thread.
pub struct LevelWriter {
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
    writing: bool,
    failed: Option<Error>,
    closed: bool,
}

impl LevelWriter {
    pub fn new(wpctl: Wpctl, target: Target) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        });
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("cupertino-pipewire-writer".into())
                .spawn(move || write_asked(&wpctl, target, &shared))
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
        self.shared.changed.notify_all();
        state.failed.take().map_or(Ok(()), Err)
    }

    /// Wait until every value asked is written, so a reading that follows sees the last one.
    pub fn wait_written(&self) {
        let mut state = self.shared.lock();
        while state.asked.is_some() || state.writing {
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

impl Drop for LevelWriter {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
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

fn write_asked(wpctl: &Wpctl, target: Target, shared: &Shared) {
    let mut muted = None;
    loop {
        let volume = {
            let mut state = shared.lock();
            loop {
                if let Some(volume) = state.asked.take() {
                    state.writing = true;
                    break volume;
                }
                if state.closed {
                    return;
                }
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|e| e.into_inner());
            }
        };
        let mut written = wpctl.set_level(target, volume.level);
        if written.is_ok() && muted != Some(volume.muted) {
            written = wpctl.set_muted(target, volume.muted);
            muted = written.is_ok().then_some(volume.muted);
        }
        let mut state = shared.lock();
        state.writing = false;
        if let Err(error) = written {
            state.failed = Some(error);
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

    #[test]
    fn the_last_value_asked_is_the_one_the_sink_ends_with() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        let writer = LevelWriter::new(Wpctl::new(fake.program()), Target::DefaultSink);
        for step in 1..=20 {
            writer
                .set(Volume {
                    level: f64::from(step) / 40.0,
                    muted: false,
                })
                .unwrap();
        }
        writer
            .set(Volume {
                level: 0.75,
                muted: true,
            })
            .unwrap();
        writer.wait_written();
        assert_eq!(
            fake.volume(Target::DefaultSink),
            Volume {
                level: 0.75,
                muted: true
            }
        );
        assert_eq!(fake.volume(Target::DefaultSource), QUIET);
    }

    #[test]
    fn a_failed_write_is_reported_by_the_next_call() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        fake.set_running(false);
        let writer = LevelWriter::new(Wpctl::new(fake.program()), Target::DefaultSink);
        writer.set(QUIET).unwrap();
        writer.wait_written();
        assert!(writer.set(QUIET).is_err());
    }

    #[test]
    fn dropping_the_writer_writes_what_is_still_asked() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        let asked = Volume {
            level: 0.5,
            muted: false,
        };
        LevelWriter::new(Wpctl::new(fake.program()), Target::DefaultSink)
            .set(asked)
            .unwrap();
        assert_eq!(fake.volume(Target::DefaultSink), asked);
    }
}
