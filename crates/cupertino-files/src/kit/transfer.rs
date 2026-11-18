//! The worker of a file operation: Paste Item, Move Item Here, Duplicate, New Folder with Selection and drops each run
//! their [`Work`] on a thread of its own, which hands its progress over and wakes the application at most every
//! [`PROGRESS_INTERVAL`], and once more when it ends. Nothing here copies or moves on the window's thread.
//! [`Transfer::stop`] sets the work's [`Cancel`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::format::human_size;
use crate::trash::Trash;
use cupertino_kit::files::operations::{self, Cancel, DropAction, Progress};

/// How often a running transfer wakes the application to show its progress.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);
/// How long a transfer runs before its time left is estimated from its pace so far.
const ESTIMATE_AFTER: Duration = Duration::from_secs(2);

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
    /// Delete for good everything in `trash`, the home trash and the mounted volumes'; `sources` are the items the
    /// home trash held when the work began, until the work says what it took ([`Outcome::took`]).
    EmptyTrash { sources: Vec<PathBuf>, trash: Trash },
}

/// What a transfer did.
#[derive(Debug)]
pub(super) struct Outcome {
    /// One result per source, in order: where it went.
    pub results: Vec<io::Result<PathBuf>>,
    /// The folder New Folder with Selection made.
    pub folder: Option<PathBuf>,
    /// The sources, for a work that found them itself: what an emptying took from the home trash and the volumes.
    pub took: Option<Vec<PathBuf>>,
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
    /// How long it had run at that progress.
    elapsed: Duration,
    /// Set once the transfer ended.
    outcome: Option<Outcome>,
}

/// One running transfer.
pub(super) struct Transfer {
    pub id: u64,
    pub work: Work,
    pub cancel: Cancel,
    pub progress: Progress,
    /// How long it had run at that progress.
    pub elapsed: Duration,
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
                let started = Instant::now();
                let mut woke = started - PROGRESS_INTERVAL;
                let mut report = |progress: &Progress| {
                    let mut shared = lock(&thread_shared);
                    shared.progress = progress.clone();
                    shared.elapsed = started.elapsed();
                    drop(shared);
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
            elapsed: Duration::ZERO,
            woken,
            shared,
            thread: Some(thread),
        }
    }

    /// Take what the thread reported: its progress, and what it did once it ended. The outcome is handed over once.
    pub fn poll(&mut self) -> Option<Outcome> {
        let mut shared = lock(&self.shared);
        self.progress = shared.progress.clone();
        self.elapsed = shared.elapsed;
        shared.outcome.take()
    }

    /// Stop the transfer: the item being copied is removed again, the others stay where they are.
    pub fn stop(&self) {
        self.cancel.cancel();
    }

    /// Take the sources the work found itself ([`Outcome::took`]), so its end is worded and its failures named by
    /// what it took.
    pub fn took(&mut self, took: Vec<PathBuf>) {
        if let Work::EmptyTrash { sources, .. } = &mut self.work {
            *sources = took;
        }
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

    /// How far it came, as "428 KB of 960 KB", or "2 of 5 items" for items that weigh nothing, then the time left once
    /// its pace is known: "428 KB of 960 KB, 5 seconds left".
    pub fn detail(&self) -> String {
        let p = &self.progress;
        let done = match p.total_bytes {
            0 => format!("{} of {} items", p.items, p.total_items),
            total => format!("{} of {}", human_size(p.bytes), human_size(total)),
        };
        match time_left(self.elapsed, self.fraction()) {
            Some(left) => format!("{done}, {}", left_words(left)),
            None => done,
        }
    }

    /// What the work is, as the progress window's title names it: "Copy" for a copy or a duplicate, "Move" for a move,
    /// "Empty Trash" for emptying the trash.
    pub fn kind(&self) -> &'static str {
        match &self.work {
            Work::Transfer {
                action: DropAction::Copy,
                ..
            }
            | Work::Duplicate { .. } => "Copy",
            Work::Transfer { .. } | Work::NewFolderWith { .. } | Work::Trash { .. } => "Move",
            Work::EmptyTrash { .. } => "Empty Trash",
        }
    }

    /// From 0 to 1.
    pub fn fraction(&self) -> f64 {
        self.progress.fraction()
    }
}

/// The time a transfer `fraction` done after `elapsed` still needs at its pace so far; none before [`ESTIMATE_AFTER`]
/// or with nothing done yet.
fn time_left(elapsed: Duration, fraction: f64) -> Option<Duration> {
    if elapsed < ESTIMATE_AFTER || !(fraction > 0.0 && fraction < 1.0) {
        return None;
    }
    Some(elapsed.mul_f64((1.0 - fraction) / fraction))
}

/// `left` as words: "5 seconds left", "3 minutes left", "2 hours left", rounded up.
fn left_words(left: Duration) -> String {
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    let (count, unit) = match seconds {
        0..=59 => (seconds.max(1), "second"),
        60..=3599 => (seconds.div_ceil(60), "minute"),
        _ => (seconds.div_ceil(3600), "hour"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} left")
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
            took: None,
        },
        Work::Duplicate { sources } => Outcome {
            results: operations::duplicate_with(&sources, cancel, report),
            folder: None,
            took: None,
        },
        Work::NewFolderWith { dir, sources } => match operations::new_folder(&dir) {
            Ok(folder) => Outcome {
                results: operations::transfer(&sources, &folder, DropAction::Move, cancel, report),
                folder: Some(folder),
                took: None,
            },
            Err(e) => Outcome {
                results: sources
                    .iter()
                    .map(|_| Err(io::Error::new(e.kind(), e.to_string())))
                    .collect(),
                folder: None,
                took: None,
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
                took: None,
            }
        }
        Work::EmptyTrash { sources, trash } => {
            // each item deleted counts as one; the trash is read again here, so what was put in since goes too. The
            // home trash goes first and whole: a mounted volume that is slow to answer holds up only the volumes'
            let mut emptying = Emptying {
                done: Progress {
                    total_items: sources.len(),
                    ..Progress::default()
                },
                results: Vec::new(),
                took: Vec::new(),
            };
            let home = emptying.take(
                trash.list(),
                || trash.home_entries(),
                &trash,
                cancel,
                report,
            );
            let emptied = match home {
                Ok(()) => emptying.take(
                    trash.list_volumes(),
                    || trash.volume_entries(),
                    &trash,
                    cancel,
                    report,
                ),
                failed => failed,
            };
            let Emptying {
                mut results,
                mut took,
                ..
            } = emptying;
            // a trash that cannot be listed fails as a whole, whatever it held: the home trash by the items it had
            // when the work began, the volumes' as one more item that stayed
            if let Err(e) = emptied {
                if results.is_empty() {
                    took = match sources.is_empty() {
                        true => vec![trash.home_trash().join("files")],
                        false => sources,
                    };
                    results = took
                        .iter()
                        .map(|_| Err(io::Error::other(e.to_string())))
                        .collect();
                } else {
                    took.push(trash.home_trash().to_path_buf());
                    results.push(Err(io::Error::other(e.to_string())));
                }
            }
            Outcome {
                results,
                folder: None,
                took: Some(took),
            }
        }
    }
}

/// An emptying of the trash under way: how far it came, and each item it took with what became of it.
struct Emptying {
    done: Progress,
    results: Vec<io::Result<PathBuf>>,
    took: Vec<PathBuf>,
}

impl Emptying {
    /// Delete the items `listed`, then what `entries` still finds without a record, which is in no list and would
    /// keep the trash looking full. Nothing is deleted from a part of the trash that cannot be listed.
    fn take(
        &mut self,
        listed: Result<Vec<cupertino_trash::TrashedItem>, cupertino_trash::TrashError>,
        entries: impl FnOnce() -> Vec<PathBuf>,
        trash: &Trash,
        cancel: &Cancel,
        report: &mut dyn FnMut(&Progress),
    ) -> Result<(), cupertino_trash::TrashError> {
        let items = listed?;
        self.done.total_items = self.done.total_items.max(self.done.items + items.len());
        for item in &items {
            self.took.push(item.file_path());
            if cancel.is_cancelled() {
                self.results.push(Err(operations::stopped()));
                continue;
            }
            self.done.current = item
                .file_path()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            report(&self.done);
            self.results.push(
                trash
                    .delete(item)
                    .map(|()| item.file_path())
                    .map_err(|e| io::Error::other(e.to_string())),
            );
            self.done.items += 1;
            self.done.current = None;
            report(&self.done);
        }
        if !cancel.is_cancelled() {
            for orphan in entries()
                .into_iter()
                .filter(|p| cupertino_trash::trashed_item(p).is_none())
            {
                let gone = match std::fs::symlink_metadata(&orphan) {
                    Ok(m) if m.is_dir() => std::fs::remove_dir_all(&orphan),
                    Ok(_) => std::fs::remove_file(&orphan),
                    Err(_) => Ok(()),
                };
                self.results.push(gone.map(|()| orphan.clone()));
                self.took.push(orphan);
            }
        }
        Ok(())
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
