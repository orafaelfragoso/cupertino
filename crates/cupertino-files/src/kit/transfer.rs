//! The worker of a file operation: Paste Item, Move Item Here, Duplicate, New Folder with Selection and drops each run
//! their [`Work`] as Kit I/O tasks, one slice at a time ([`Transfer::slice`]): a slice works for about
//! [`operations::SLICE`] and gives its turn back, so listings and thumbnails go on beside it and a stop takes effect at
//! the next slice. Copies and moves are the family's [`operations::Transfer`]; moving to the trash and emptying it go an
//! item a step. Nothing here copies or moves on the window's thread. [`Transfer::stop`] sets the work's [`Cancel`].

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::format::human_size;
use crate::trash::Trash;
use cupertino_kit::files::operations::{self, Cancel, DropAction, Progress, SLICE, SLICE_DEADLINE};
use cupertino_kit::prelude::Command;
use cupertino_trash::{TrashError, TrashedItem};

use super::{JobEvent, Message};

/// How long a transfer runs before its time left is estimated from its pace so far.
const ESTIMATE_AFTER: Duration = Duration::from_secs(2);

/// What a transfer calls with its progress after each slice, on the I/O pool, before the next slice is asked for:
/// tests pace a transfer with it.
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

/// What a transfer and its slices share.
#[derive(Default)]
struct Shared {
    progress: Progress,
    /// How long it had run at that progress.
    elapsed: Duration,
    /// What is left of the work, while no slice holds it; `None` while one does and once the work ended.
    run: Option<Run>,
    /// Set once the transfer ended.
    outcome: Option<Outcome>,
    /// The transfer was dropped before it ended: the slice that holds the work stops it and runs it to its end.
    dropped: bool,
}

/// One running transfer.
pub(super) struct Transfer {
    pub id: u64,
    pub work: Work,
    pub cancel: Cancel,
    pub progress: Progress,
    /// How long it had run at that progress.
    pub elapsed: Duration,
    shared: Arc<Mutex<Shared>>,
    started: Instant,
    observer: Option<Observer>,
}

impl Transfer {
    /// `work`, to be done slice by slice ([`Transfer::slice`]); nothing runs yet.
    pub fn start(id: u64, work: Work, observer: Option<Observer>) -> Transfer {
        let shared = Shared {
            run: Some(Run::of(work.clone())),
            ..Shared::default()
        };
        Transfer {
            id,
            work,
            cancel: Cancel::default(),
            progress: Progress::default(),
            elapsed: Duration::ZERO,
            shared: Arc::new(Mutex::new(shared)),
            started: Instant::now(),
            observer,
        }
    }

    /// The I/O task that runs the next slice, whose end is the transfer's [`JobEvent::Reported`]; `None` while a slice
    /// is out and once the work ended.
    pub fn slice(&self) -> Option<Command<Message>> {
        let run = lock(&self.shared).run.take()?;
        let shared = Arc::clone(&self.shared);
        let (cancel, observer, started, id) = (
            self.cancel.clone(),
            self.observer.clone(),
            self.started,
            self.id,
        );
        Some(cupertino_kit::task::io(
            SLICE_DEADLINE,
            move || slice(run, &shared, &cancel, observer.as_ref(), started),
            move |()| Message::Job(JobEvent::Reported(id)),
        ))
    }

    /// Take what the slices reported: the progress, and what the work did once it ended. The outcome is handed over
    /// once.
    pub fn poll(&mut self) -> Option<Outcome> {
        let mut shared = lock(&self.shared);
        self.progress = shared.progress.clone();
        self.elapsed = shared.elapsed;
        shared.outcome.take()
    }

    /// Stop the transfer: the next slice removes the item being copied, the others stay where they are.
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

/// Run one slice of `run` and hand what is left, or the outcome, back to `shared`, then tell `observer`. A transfer
/// dropped meanwhile is stopped and run to its end here, so no half-copied item is left under its own name.
fn slice(
    run: Run,
    shared: &Mutex<Shared>,
    cancel: &Cancel,
    observer: Option<&Observer>,
    started: Instant,
) {
    let mut run = run;
    loop {
        let (sliced, progress) = run.slice(cancel);
        let mut held = lock(shared);
        held.progress = progress.clone();
        held.elapsed = started.elapsed();
        match sliced {
            Sliced::Left(left) if held.dropped => {
                drop(held);
                cancel.cancel();
                run = left;
                continue;
            }
            Sliced::Left(left) => held.run = Some(left),
            Sliced::Ended(outcome) => held.outcome = Some(outcome),
        }
        drop(held);
        if let Some(observer) = observer {
            observer(&progress);
        }
        return;
    }
}

/// What a slice leaves.
enum Sliced {
    /// The work goes on with the next slice.
    Left(Run),
    Ended(Outcome),
}

/// A work as it runs, between two slices.
enum Run {
    /// Copied or moved by the family's transfer; the folder New Folder with Selection made, which it fills.
    Moving {
        transfer: operations::Transfer,
        folder: Option<PathBuf>,
    },
    /// New Folder with Selection, before its folder is made.
    NewFolder { dir: PathBuf, sources: Vec<PathBuf> },
    /// Items moved to the trash, one a step.
    Trashing(Trashing),
    /// The trash emptied, an item a step.
    Emptying(Box<Emptying>),
}

impl Run {
    fn of(work: Work) -> Run {
        match work {
            Work::Transfer {
                sources,
                dest,
                action,
            } => Run::Moving {
                transfer: operations::Transfer::into_folder(sources, dest, action),
                folder: None,
            },
            Work::Duplicate { sources } => Run::Moving {
                transfer: operations::Transfer::duplicate(sources),
                folder: None,
            },
            Work::NewFolderWith { dir, sources } => Run::NewFolder { dir, sources },
            Work::Trash { sources, trash } => Run::Trashing(Trashing {
                done: Progress {
                    total_items: sources.len(),
                    ..Progress::default()
                },
                sources: sources.into(),
                trash,
                results: Vec::new(),
            }),
            Work::EmptyTrash { sources, trash } => Run::Emptying(Box::new(Emptying {
                done: Progress {
                    total_items: sources.len(),
                    ..Progress::default()
                },
                sources,
                trash,
                results: Vec::new(),
                took: Vec::new(),
                step: Step::List(Part::Home),
            })),
        }
    }

    /// Work for about [`SLICE`], stopping at `cancel`: what is left, and the progress so far.
    fn slice(self, cancel: &Cancel) -> (Sliced, Progress) {
        match self {
            Run::Moving {
                mut transfer,
                folder,
            } => {
                if cancel.is_cancelled() {
                    transfer.cancel();
                }
                if let Some(slice) = transfer.take_slice() {
                    slice.run();
                }
                let progress = transfer.progress();
                match transfer.results() {
                    Some(results) => (
                        Sliced::Ended(Outcome {
                            results,
                            folder,
                            took: None,
                        }),
                        progress,
                    ),
                    None => (Sliced::Left(Run::Moving { transfer, folder }), progress),
                }
            }
            Run::NewFolder { dir, sources } => {
                let progress = Progress {
                    total_items: sources.len(),
                    ..Progress::default()
                };
                match operations::new_folder(&dir) {
                    Ok(folder) => (
                        Sliced::Left(Run::Moving {
                            transfer: operations::Transfer::into_folder(
                                sources,
                                folder.clone(),
                                DropAction::Move,
                            ),
                            folder: Some(folder),
                        }),
                        progress,
                    ),
                    Err(e) => (
                        Sliced::Ended(Outcome {
                            results: sources
                                .iter()
                                .map(|_| Err(io::Error::new(e.kind(), e.to_string())))
                                .collect(),
                            folder: None,
                            took: None,
                        }),
                        progress,
                    ),
                }
            }
            Run::Trashing(mut trashing) => {
                let start = Instant::now();
                while !trashing.sources.is_empty() && start.elapsed() < SLICE {
                    trashing.step(cancel);
                }
                let progress = trashing.done.clone();
                match trashing.sources.is_empty() {
                    true => (
                        Sliced::Ended(Outcome {
                            results: trashing.results,
                            folder: None,
                            took: None,
                        }),
                        progress,
                    ),
                    false => (Sliced::Left(Run::Trashing(trashing)), progress),
                }
            }
            Run::Emptying(mut emptying) => {
                let start = Instant::now();
                while !matches!(emptying.step, Step::Ended(_)) && start.elapsed() < SLICE {
                    emptying.step(cancel);
                }
                let progress = emptying.done.clone();
                match emptying.step {
                    Step::Ended(_) => (Sliced::Ended(emptying.outcome()), progress),
                    _ => (Sliced::Left(Run::Emptying(emptying)), progress),
                }
            }
        }
    }
}

/// A move to the trash under way: each move renames, so an item counts as one.
struct Trashing {
    sources: VecDeque<PathBuf>,
    trash: Trash,
    done: Progress,
    results: Vec<io::Result<PathBuf>>,
}

impl Trashing {
    /// Move the next item to the trash, or say it was stopped.
    fn step(&mut self, cancel: &Cancel) {
        let Some(source) = self.sources.pop_front() else {
            return;
        };
        if cancel.is_cancelled() {
            self.results.push(Err(operations::stopped()));
            return;
        }
        self.done.current = source.file_name().map(|n| n.to_string_lossy().into_owned());
        self.results.push(
            self.trash
                .trash(&source)
                .map(|item| item.file_path())
                .map_err(|e| io::Error::other(e.to_string())),
        );
        self.done.items += 1;
        self.done.current = None;
    }
}

/// A part of the trash: the home trash, emptied first and whole, then the mounted volumes', which a volume slow to
/// answer holds up alone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Home,
    Volumes,
}

/// Where an emptying is.
enum Step {
    /// The part's items are listed; the trash is read again here, so what was put in since goes too.
    List(Part),
    /// The part's items listed are deleted, one a step.
    Delete(Part, VecDeque<TrashedItem>),
    /// The part's entries that have no record, which are in no list and would keep the trash looking full.
    Orphans(Part),
    /// Done, or a part could not be listed.
    Ended(Result<(), TrashError>),
}

/// An emptying of the trash under way: how far it came, and each item it took with what became of it.
struct Emptying {
    /// The items the home trash held when the work began.
    sources: Vec<PathBuf>,
    trash: Trash,
    done: Progress,
    results: Vec<io::Result<PathBuf>>,
    took: Vec<PathBuf>,
    step: Step,
}

impl Emptying {
    /// Take the next step. Nothing is deleted from a part of the trash that cannot be listed, and nothing after it.
    fn step(&mut self, cancel: &Cancel) {
        self.step = match std::mem::replace(&mut self.step, Step::Ended(Ok(()))) {
            Step::List(part) => {
                let listed = match part {
                    Part::Home => self.trash.list(),
                    Part::Volumes => self.trash.list_volumes(),
                };
                match listed {
                    Ok(items) => {
                        self.done.total_items =
                            self.done.total_items.max(self.done.items + items.len());
                        Step::Delete(part, items.into())
                    }
                    Err(e) => Step::Ended(Err(e)),
                }
            }
            Step::Delete(part, mut items) => match items.pop_front() {
                Some(item) => {
                    self.delete(&item, cancel);
                    Step::Delete(part, items)
                }
                // a stopped emptying lists the volumes' items still, as stopped, and deletes no entry without a record
                None if cancel.is_cancelled() => match part {
                    Part::Home => Step::List(Part::Volumes),
                    Part::Volumes => Step::Ended(Ok(())),
                },
                None => Step::Orphans(part),
            },
            Step::Orphans(part) => {
                let entries = match part {
                    Part::Home => self.trash.home_entries(),
                    Part::Volumes => self.trash.volume_entries(),
                };
                for orphan in entries
                    .into_iter()
                    .filter(|p| cupertino_trash::trashed_item(p).is_none())
                {
                    self.results
                        .push(remove_entry(&orphan).map(|()| orphan.clone()));
                    self.took.push(orphan);
                }
                match part {
                    Part::Home => Step::List(Part::Volumes),
                    Part::Volumes => Step::Ended(Ok(())),
                }
            }
            ended @ Step::Ended(_) => ended,
        };
    }

    /// Delete `item`, or say it was stopped.
    fn delete(&mut self, item: &TrashedItem, cancel: &Cancel) {
        self.took.push(item.file_path());
        if cancel.is_cancelled() {
            self.results.push(Err(operations::stopped()));
            return;
        }
        self.done.current = item
            .file_path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        self.results.push(
            self.trash
                .delete(item)
                .map(|()| item.file_path())
                .map_err(|e| io::Error::other(e.to_string())),
        );
        self.done.items += 1;
        self.done.current = None;
    }

    /// What the emptying did. A trash that cannot be listed fails as a whole, whatever it held: the home trash by the
    /// items it had when the work began, the volumes' as one more item that stayed.
    fn outcome(self) -> Outcome {
        let Emptying {
            sources,
            trash,
            mut results,
            mut took,
            step,
            ..
        } = self;
        if let Step::Ended(Err(e)) = step {
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

/// Delete the trash entry `orphan`, a folder with what it holds; one that is gone already is no failure.
#[expect(
    clippy::disallowed_methods,
    reason = "runs in a transfer's slice, on the I/O pool (kit::task::io)"
)]
fn remove_entry(orphan: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(orphan) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(orphan),
        Ok(_) => std::fs::remove_file(orphan),
        Err(_) => Ok(()),
    }
}

/// A transfer dropped before it ended is stopped and run to its end: Files exits only once its transfers ended, so
/// this is a session that went away, and a copy cut off with the process would leave a half-written item at its
/// destination under the item's own name. A slice that holds the work does that itself; otherwise it is done here,
/// where a stopped slice only removes the item it was copying.
impl Drop for Transfer {
    fn drop(&mut self) {
        self.cancel.cancel();
        let left = {
            let mut shared = lock(&self.shared);
            shared.dropped = true;
            shared.run.take()
        };
        let mut left = left;
        while let Some(run) = left.take() {
            if let (Sliced::Left(more), _) = run.slice(&self.cancel) {
                left = Some(more);
            }
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
