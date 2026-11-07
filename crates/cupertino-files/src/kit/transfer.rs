//! Paste Item and Move Item Here off the window's thread: each runs [`operations::transfer`] on a thread of its own,
//! which hands its progress over and wakes the window at most every [`PROGRESS_INTERVAL`], and once more when it ends.
//! The window shows a row for each running transfer, with a Stop button that sets its [`Cancel`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::operations::{self, Cancel, DropAction, Progress};

/// How often a running transfer wakes the window to show its progress.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// What a transfer calls with its progress after each step, on its own thread, before it looks for a stop: tests pace
/// a transfer with it.
pub type Observer = Arc<dyn Fn(&Progress) + Send + Sync>;

/// What the transfer's thread hands to the window.
#[derive(Default)]
struct Shared {
    progress: Progress,
    /// Set once the transfer ended: one result per source.
    results: Option<Vec<io::Result<PathBuf>>>,
}

/// One running transfer of a window.
pub(super) struct Transfer {
    pub id: u64,
    pub moving: bool,
    pub sources: Vec<PathBuf>,
    pub dest: PathBuf,
    pub cancel: Cancel,
    pub progress: Progress,
    pub woken: async_channel::Receiver<()>,
    shared: Arc<Mutex<Shared>>,
}

impl Transfer {
    /// Copy `sources` into the folder `dest`, or move them when `moving`, on a thread of its own.
    pub fn start(
        id: u64,
        sources: Vec<PathBuf>,
        dest: PathBuf,
        moving: bool,
        observer: Option<Observer>,
    ) -> Transfer {
        let (wake, woken) = async_channel::bounded(1);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let cancel = Cancel::default();
        let action = if moving {
            DropAction::Move
        } else {
            DropAction::Copy
        };
        let (thread_shared, thread_cancel) = (Arc::clone(&shared), cancel.clone());
        let (thread_sources, thread_dest) = (sources.clone(), dest.clone());
        std::thread::Builder::new()
            .name("cupertino-files transfer".into())
            .spawn(move || {
                let mut woke = Instant::now() - PROGRESS_INTERVAL;
                let results = operations::transfer(
                    &thread_sources,
                    &thread_dest,
                    action,
                    &thread_cancel,
                    &mut |progress| {
                        lock(&thread_shared).progress = progress.clone();
                        if woke.elapsed() >= PROGRESS_INTERVAL {
                            woke = Instant::now();
                            // a wake already waiting shows this progress too
                            let _ = wake.try_send(());
                        }
                        if let Some(observer) = &observer {
                            observer(progress);
                        }
                    },
                );
                lock(&thread_shared).results = Some(results);
                // the window may have closed: then nobody waits for the end
                let _ = wake.send_blocking(());
            })
            .expect("a thread for the transfer");
        Transfer {
            id,
            moving,
            sources,
            dest,
            cancel,
            progress: Progress::default(),
            woken,
            shared,
        }
    }

    /// Take what the thread reported: its progress, and its results once it ended.
    pub fn poll(&mut self) -> Option<Vec<io::Result<PathBuf>>> {
        let mut shared = lock(&self.shared);
        self.progress = shared.progress.clone();
        shared.results.take()
    }

    /// Stop the transfer: the item being copied is removed again, the others stay where they are.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// What the window says the transfer does, as "Copying 3 items to “Documents”".
    pub fn title(&self) -> String {
        let verb = if self.moving { "Moving" } else { "Copying" };
        let what = match self.sources.as_slice() {
            [one] => format!("“{}”", name(one)),
            many => format!("{} items", many.len()),
        };
        format!("{verb} {what} to “{}”", name(&self.dest))
    }

    /// From 0 to 1.
    pub fn fraction(&self) -> f64 {
        self.progress.fraction()
    }
}

fn name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transfer_runs_on_its_own_thread_and_names_what_it_does() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("Documents");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(d.path().join("a.txt"), "a").unwrap();
        std::fs::write(d.path().join("b.txt"), "b").unwrap();
        let sources = vec![d.path().join("a.txt"), d.path().join("b.txt")];
        let mut transfer = Transfer::start(7, sources, dest.clone(), false, None);
        assert_eq!(transfer.title(), "Copying 2 items to “Documents”");
        let results = loop {
            transfer.woken.recv_blocking().unwrap();
            if let Some(results) = transfer.poll() {
                break results;
            }
        };
        assert_eq!(
            results.into_iter().map(Result::unwrap).collect::<Vec<_>>(),
            [dest.join("a.txt"), dest.join("b.txt")]
        );
        assert_eq!(transfer.fraction(), 1.0);
        let one = Transfer::start(8, vec![d.path().join("a.txt")], dest, true, None);
        assert_eq!(one.title(), "Moving “a.txt” to “Documents”");
    }
}
