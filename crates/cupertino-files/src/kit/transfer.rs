//! The worker of a file operation: Paste Item, Move Item Here, Duplicate, New Folder with Selection and drops each run
//! their [`Work`] on a thread of its own, which hands its progress over and wakes the application at most every
//! [`PROGRESS_INTERVAL`], and once more when it ends. Nothing here copies or moves on the window's thread.
//! [`Transfer::stop`] sets the work's [`Cancel`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::format::human_size;
use crate::operations::{self, Cancel, DropAction, Progress};
use crate::trash::Trash;

/// How often a running transfer wakes the application to show its progress.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// What a transfer calls with its progress after each step, on its own thread, before it looks for a stop: tests pace
/// a transfer with it.
pub type Observer = Arc<dyn Fn(&Progress) + Send + Sync>;

/// What a transfer does.
#[derive(Clone, Debug)]
pub(super) enum Work {
    /// Copy or move `sources` into the folder `dest`: Paste Item, Move Item Here, a drop on a folder.
    Transfer {
        sources: Vec<PathBuf>,
        dest: PathBuf,
        action: DropAction,
    },
    /// Copy each of `sources` next to itself.
    Duplicate { sources: Vec<PathBuf> },
    /// Make a new folder in `dir` and move `sources` into it.
    NewFolderWith { dir: PathBuf, sources: Vec<PathBuf> },
    /// Move `sources` to `trash`: a drop on the Trash.
    Trash { sources: Vec<PathBuf>, trash: Trash },
    /// Delete for good everything in `trash`; `sources` are the items it held when the work began.
    EmptyTrash { sources: Vec<PathBuf>, trash: Trash },
}

/// What a transfer did.
#[derive(Debug)]
pub(super) struct Outcome {
    /// One result per source, in order: where it went.
    pub results: Vec<io::Result<PathBuf>>,
    /// The folder New Folder with Selection made.
    pub folder: Option<PathBuf>,
}

impl Outcome {
    /// Whether the Stop button ended it.
    pub fn stopped(&self) -> bool {
        self.results
            .iter()
            .any(|r| r.as_ref().is_err_and(operations::is_cancelled))
    }

    /// The sources that failed, with their errors; a stop is no failure.
    pub fn failures<'a>(
        &'a self,
        sources: &'a [PathBuf],
    ) -> impl Iterator<Item = (&'a PathBuf, &'a io::Error)> {
        sources
            .iter()
            .zip(&self.results)
            .filter_map(|(source, result)| match result {
                Err(e) if !operations::is_cancelled(e) => Some((source, e)),
                _ => None,
            })
    }

    /// The last item that arrived.
    pub fn arrived(&self) -> Option<&Path> {
        self.results
            .iter()
            .rev()
            .find_map(|p| p.as_ref().ok())
            .map(PathBuf::as_path)
    }
}

/// What the transfer's thread hands to the application.
#[derive(Default)]
struct Shared {
    progress: Progress,
    /// Set once the transfer ended.
    outcome: Option<Outcome>,
}

/// One running transfer.
pub(super) struct Transfer {
    pub id: u64,
    pub work: Work,
    pub cancel: Cancel,
    pub progress: Progress,
    pub woken: async_channel::Receiver<()>,
    shared: Arc<Mutex<Shared>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Transfer {
    /// Do `work` on a thread of its own.
    pub fn start(id: u64, work: Work, observer: Option<Observer>) -> Transfer {
        let (wake, woken) = async_channel::bounded(1);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let cancel = Cancel::default();
        let (thread_shared, thread_cancel, thread_work) =
            (Arc::clone(&shared), cancel.clone(), work.clone());
        let thread = std::thread::Builder::new()
            .name("cupertino-files transfer".into())
            .spawn(move || {
                let mut woke = Instant::now() - PROGRESS_INTERVAL;
                let mut report = |progress: &Progress| {
                    lock(&thread_shared).progress = progress.clone();
                    if woke.elapsed() >= PROGRESS_INTERVAL {
                        woke = Instant::now();
                        // a wake already waiting shows this progress too
                        let _ = wake.try_send(());
                    }
                    if let Some(observer) = &observer {
                        observer(progress);
                    }
                };
                let outcome = run(thread_work, &thread_cancel, &mut report);
                lock(&thread_shared).outcome = Some(outcome);
                // the application may be gone: then nobody waits for the end
                let _ = wake.send_blocking(());
            })
            .expect("a thread for the transfer");
        Transfer {
            id,
            work,
            cancel,
            progress: Progress::default(),
            woken,
            shared,
            thread: Some(thread),
        }
    }

    /// Take what the thread reported: its progress, and what it did once it ended. The outcome is handed over once.
    pub fn poll(&mut self) -> Option<Outcome> {
        let mut shared = lock(&self.shared);
        self.progress = shared.progress.clone();
        shared.outcome.take()
    }

    /// Stop the transfer: the item being copied is removed again, the others stay where they are.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    pub fn sources(&self) -> &[PathBuf] {
        match &self.work {
            Work::Transfer { sources, .. }
            | Work::Duplicate { sources }
            | Work::NewFolderWith { sources, .. }
            | Work::Trash { sources, .. }
            | Work::EmptyTrash { sources, .. } => sources,
        }
    }

    /// The words of the work: doing, done, the verb, and where to.
    fn wording(&self) -> (&'static str, &'static str, &'static str, String) {
        match &self.work {
            Work::Transfer {
                dest,
                action: DropAction::Copy,
                ..
            } => ("Copying", "Copied", "copy", format!(" to “{}”", name(dest))),
            Work::Transfer { dest, .. } => {
                ("Moving", "Moved", "move", format!(" to “{}”", name(dest)))
            }
            Work::Duplicate { .. } => ("Duplicating", "Duplicated", "duplicate", String::new()),
            Work::NewFolderWith { .. } => ("Moving", "Moved", "move", " into a new folder".into()),
            Work::Trash { .. } => ("Moving", "Moved", "move", " to the Trash".into()),
            Work::EmptyTrash { .. } => ("Emptying", "Emptied", "empty", String::new()),
        }
    }

    /// The items, as "“a.txt”" or "3 items".
    fn what(&self) -> String {
        if matches!(self.work, Work::EmptyTrash { .. }) {
            return "the Trash".into();
        }
        match self.sources() {
            [one] => format!("“{}”", name(one)),
            many => format!("{} items", many.len()),
        }
    }

    /// What the work does, as "Copying 3 items to “Documents”".
    pub fn title(&self) -> String {
        let (doing, _, _, to) = self.wording();
        format!("{doing} {}{to}", self.what())
    }

    /// What the work did once it ended, `stopped` by its Stop button or with `failures` items that could not go:
    /// "Copied 3 items to “Documents”", "Stopped copying “a.txt” to “Documents”", "Moved 2 of 3 items to
    /// “Documents”" or "Could not copy “a.txt” to “Documents”".
    pub fn ended(&self, stopped: bool, failures: usize) -> String {
        let (doing, done, verb, to) = self.wording();
        let what = self.what();
        let total = self.sources().len();
        if stopped {
            format!("Stopped {} {what}{to}", doing.to_lowercase())
        } else if failures == 0 {
            format!("{done} {what}{to}")
        } else if failures >= total {
            format!("Could not {verb} {what}{to}")
        } else {
            format!("{done} {} of {total} items{to}", total - failures)
        }
    }

    /// How far it came, as "428 KB of 960 KB", or "2 of 5 items" for items that weigh nothing.
    pub fn detail(&self) -> String {
        let p = &self.progress;
        match p.total_bytes {
            0 => format!("{} of {} items", p.items, p.total_items),
            total => format!("{} of {}", human_size(p.bytes), human_size(total)),
        }
    }

    /// From 0 to 1.
    pub fn fraction(&self) -> f64 {
        self.progress.fraction()
    }
}

/// Do `work`, reporting to `report` and stopping at `cancel`.
fn run(work: Work, cancel: &Cancel, report: &mut dyn FnMut(&Progress)) -> Outcome {
    match work {
        Work::Transfer {
            sources,
            dest,
            action,
        } => Outcome {
            results: operations::transfer(&sources, &dest, action, cancel, report),
            folder: None,
        },
        Work::Duplicate { sources } => Outcome {
            results: operations::duplicate_with(&sources, cancel, report),
            folder: None,
        },
        Work::NewFolderWith { dir, sources } => match operations::new_folder(&dir) {
            Ok(folder) => Outcome {
                results: operations::transfer(&sources, &folder, DropAction::Move, cancel, report),
                folder: Some(folder),
            },
            Err(e) => Outcome {
                results: sources
                    .iter()
                    .map(|_| Err(io::Error::new(e.kind(), e.to_string())))
                    .collect(),
                folder: None,
            },
        },
        Work::Trash { sources, trash } => {
            // a move to the trash renames: each item counts as one
            let mut done = Progress {
                total_items: sources.len(),
                ..Progress::default()
            };
            let mut results = Vec::with_capacity(sources.len());
            for source in &sources {
                if cancel.is_cancelled() {
                    results.push(Err(operations::stopped()));
                    continue;
                }
                done.current = source.file_name().map(|n| n.to_string_lossy().into_owned());
                report(&done);
                results.push(
                    trash
                        .trash(source)
                        .map(|item| item.file_path())
                        .map_err(|e| io::Error::other(e.to_string())),
                );
                done.items += 1;
                done.current = None;
                report(&done);
            }
            Outcome {
                results,
                folder: None,
            }
        }
        Work::EmptyTrash { sources, trash } => {
            // each item deleted counts as one; the trash is read again here, so what was put in since goes too
            let listed = trash.list();
            let items = match &listed {
                Ok(items) => items.as_slice(),
                Err(_) => &[],
            };
            let mut done = Progress {
                total_items: items.len().max(sources.len()),
                ..Progress::default()
            };
            let mut results = Vec::with_capacity(items.len());
            for item in items {
                if cancel.is_cancelled() {
                    results.push(Err(operations::stopped()));
                    continue;
                }
                done.current = item
                    .file_path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned());
                report(&done);
                results.push(
                    trash
                        .delete(item)
                        .map(|()| item.file_path())
                        .map_err(|e| io::Error::other(e.to_string())),
                );
                done.items += 1;
                done.current = None;
                report(&done);
            }
            // a trash that cannot be listed fails as a whole, whatever it held
            if let Err(e) = listed {
                results = sources
                    .iter()
                    .map(|_| Err(io::Error::other(e.to_string())))
                    .collect();
            }
            Outcome {
                results,
                folder: None,
            }
        }
    }
}

/// A transfer dropped before it ended is stopped and waited for: Files exits only once its transfers ended, so this
/// is a session that went away, and a copy cut off with the process would leave a half-written item at its
/// destination under the item's own name.
impl Drop for Transfer {
    fn drop(&mut self) {
        self.cancel.cancel();
        // a thread that waits to hand over its last wake gives up at once
        self.woken.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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

    fn copy(sources: Vec<PathBuf>, dest: PathBuf) -> Work {
        Work::Transfer {
            sources,
            dest,
            action: DropAction::Copy,
        }
    }

    fn finish(transfer: &mut Transfer) -> Outcome {
        loop {
            transfer.woken.recv_blocking().unwrap();
            if let Some(outcome) = transfer.poll() {
                break outcome;
            }
        }
    }

    #[test]
    fn a_transfer_runs_on_its_own_thread_and_names_what_it_does() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("Documents");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(d.path().join("a.txt"), "a").unwrap();
        std::fs::write(d.path().join("b.txt"), "b").unwrap();
        let sources = vec![d.path().join("a.txt"), d.path().join("b.txt")];
        let mut transfer = Transfer::start(7, copy(sources, dest.clone()), None);
        assert_eq!(transfer.title(), "Copying 2 items to “Documents”");
        let outcome = finish(&mut transfer);
        assert!(transfer.poll().is_none(), "the outcome is handed over once");
        assert_eq!(
            outcome
                .results
                .into_iter()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            [dest.join("a.txt"), dest.join("b.txt")]
        );
        assert_eq!(transfer.fraction(), 1.0);
        assert_eq!(transfer.detail(), "2 bytes of 2 bytes");
        assert_eq!(transfer.ended(false, 0), "Copied 2 items to “Documents”");
        assert_eq!(
            transfer.ended(false, 1),
            "Copied 1 of 2 items to “Documents”"
        );
        assert_eq!(
            transfer.ended(false, 2),
            "Could not copy 2 items to “Documents”"
        );
        let move_one = Work::Transfer {
            sources: vec![d.path().join("a.txt")],
            dest,
            action: DropAction::Move,
        };
        let one = Transfer::start(8, move_one, None);
        assert_eq!(one.title(), "Moving “a.txt” to “Documents”");
        assert_eq!(one.ended(true, 0), "Stopped moving “a.txt” to “Documents”");
    }

    #[test]
    fn duplicate_new_folder_with_selection_and_trash_run_as_work_too() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (d.path().join("a.txt"), d.path().join("b.txt"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "bb").unwrap();

        let mut duplicate = Transfer::start(
            1,
            Work::Duplicate {
                sources: vec![a.clone()],
            },
            None,
        );
        assert_eq!(duplicate.title(), "Duplicating “a.txt”");
        let outcome = finish(&mut duplicate);
        assert_eq!(
            outcome.results[0].as_ref().unwrap(),
            &d.path().join("a copy.txt")
        );
        assert_eq!(duplicate.ended(false, 0), "Duplicated “a.txt”");

        let mut grouped = Transfer::start(
            2,
            Work::NewFolderWith {
                dir: d.path().to_path_buf(),
                sources: vec![a.clone(), b.clone()],
            },
            None,
        );
        assert_eq!(grouped.title(), "Moving 2 items into a new folder");
        let outcome = finish(&mut grouped);
        let folder = outcome.folder.clone().unwrap();
        assert_eq!(folder, d.path().join("untitled folder"));
        assert!(folder.join("a.txt").is_file() && folder.join("b.txt").is_file());
        assert_eq!(outcome.failures(grouped.sources()).count(), 0);

        let trash = Trash::with_home(d.path().join("Trash"));
        let mut trashed = Transfer::start(
            3,
            Work::Trash {
                sources: vec![folder.join("a.txt"), d.path().join("gone")],
                trash,
            },
            None,
        );
        let outcome = finish(&mut trashed);
        assert_eq!(trashed.detail(), "2 of 2 items");
        let failed: Vec<_> = outcome
            .failures(trashed.sources())
            .map(|(p, _)| p)
            .collect();
        assert_eq!(failed, [&d.path().join("gone")]);
        assert_eq!(trashed.ended(false, 1), "Moved 1 of 2 items to the Trash");
        assert!(d.path().join("Trash/files/a.txt").is_file());
    }

    #[test]
    fn a_transfer_dropped_before_it_ended_stops_and_leaves_nothing_half_written() {
        let d = tempfile::tempdir().unwrap();
        let dest = d.path().join("Documents");
        std::fs::create_dir(&dest).unwrap();
        let big = d.path().join("big");
        std::fs::write(&big, vec![3u8; 8 << 20]).unwrap();
        // the copy holds after its first chunk until it is stopped, so it is dropped mid-file
        let (started, start) = std::sync::mpsc::sync_channel(1);
        let stopped: Arc<std::sync::OnceLock<Cancel>> = Arc::default();
        let held = Arc::clone(&stopped);
        let observer: Observer = Arc::new(move |p: &Progress| {
            if p.bytes > 0 && p.items == 0 {
                let _ = started.try_send(());
                while !held.get().is_some_and(Cancel::is_cancelled) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });
        let transfer = Transfer::start(9, copy(vec![big.clone()], dest.clone()), Some(observer));
        start.recv().unwrap();
        stopped.set(transfer.cancel.clone()).unwrap();
        drop(transfer);
        assert_eq!(
            std::fs::read_dir(&dest).unwrap().count(),
            0,
            "the part copied is removed before the transfer is gone"
        );
        assert_eq!(std::fs::metadata(&big).unwrap().len(), 8 << 20);
    }
}
