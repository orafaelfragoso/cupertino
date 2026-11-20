//! The sidebar's places kept live: a watch on the folders they come from wakes the application, which reads them again.
//! The trash's place comes with the first item trashed and leaves when the trash's `files` folder goes
//! ([`follow_trash`]); with a source of the session's places ([`super::Start::with_places_source`]) a change of the user
//! directories or their configuration shows too.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use cupertino_kit::files::places::{follow_trash, Place};
use cupertino_kit::prelude::Command;

use super::{Message, Session};

/// What reads the session's places again: the standard ones, without the removable media.
pub type PlacesSource = Arc<dyn Fn() -> io::Result<Vec<Place>> + Send + Sync>;

/// The watch on the folders the places come from.
pub(super) struct PlacesWatch {
    /// The watcher and what it watches, shared with the I/O task that adds watches.
    shared: Arc<Mutex<Watched>>,
    /// Watches asked for and not yet handed to a task, and whether a task is adding watches.
    due: Vec<Due>,
    adding: bool,
    /// Wakes the application to read the places again.
    wake: Arc<async_channel::Sender<()>>,
    pub woken: async_channel::Receiver<()>,
}

/// The watcher and the folders watched; the trash's own folder is added once it exists.
struct Watched {
    watcher: Option<RecommendedWatcher>,
    folders: Vec<PathBuf>,
}

/// A watch to add: a folder watched as it is, or the trash's folder once it exists.
enum Due {
    Folder(PathBuf),
    Trash(PathBuf),
}

/// How long adding the sidebar's watches should take: an `is_dir` and an inotify add per folder.
pub(super) const WATCH_DEADLINE: Duration = Duration::from_secs(5);

impl PlacesWatch {
    /// Watch `folders` (each alone, not what it holds) and the trash at `trash`, or its parent until it exists. The
    /// places were read before the watch began, so it starts woken: what changed in between is read too.
    pub fn start(folders: Vec<PathBuf>, trash: &Path) -> PlacesWatch {
        let (wake, woken) = async_channel::bounded(1);
        let wake = Arc::new(wake);
        let events = Arc::clone(&wake);
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if wakes(&event) {
                // a wake already waiting covers this change too
                let _ = events.try_send(());
            }
        })
        .map_err(|e| eprintln!("cupertino-files: the sidebar's places are not watched: {e}"))
        .ok();
        let mut watch = PlacesWatch {
            shared: Arc::new(Mutex::new(Watched {
                watcher,
                folders: Vec::new(),
            })),
            due: Vec::new(),
            adding: false,
            wake,
            woken,
        };
        // the folders are watched by the first I/O task, not here
        watch.due.extend(folders.into_iter().map(Due::Folder));
        if let Some(parent) = trash.parent() {
            watch.due.push(Due::Folder(parent.to_path_buf()));
        }
        watch.follow(trash);
        let _ = watch.wake.try_send(());
        watch
    }

    /// Watch the trash's folder too once it exists, so the removal of its `files` folder is heard. Its `files` folder
    /// may have come between the read that found the trash and this watch, so a new watch wakes the application once
    /// to read the places again.
    /// Asked for here, done by [`PlacesWatch::task`]: the file system is not touched on the caller's thread.
    pub fn follow(&mut self, trash: &Path) {
        if !self
            .due
            .iter()
            .any(|due| matches!(due, Due::Trash(t) if t == trash))
        {
            self.due.push(Due::Trash(trash.to_path_buf()));
        }
    }

    /// The task that adds the watches asked for, when there are some and none is running; its result is whether the
    /// trash's folder is newly watched, which [`PlacesWatch::added`] takes.
    pub fn task(&mut self) -> Option<Command<Message>> {
        if self.due.is_empty() || self.adding {
            return None;
        }
        self.adding = true;
        let due = std::mem::take(&mut self.due);
        let shared = Arc::clone(&self.shared);
        Some(cupertino_kit::task::io(
            WATCH_DEADLINE,
            move || {
                let mut watched = shared.lock().unwrap_or_else(|e| e.into_inner());
                due.iter().fold(false, |trash_new, due| match due {
                    Due::Folder(folder) => {
                        watched.add(folder);
                        trash_new
                    }
                    Due::Trash(trash) => (trash.is_dir() && watched.add(trash)) || trash_new,
                })
            },
            Message::PlacesWatched,
        ))
    }

    /// The task ended: a trash folder newly watched may have gained its `files` folder before the watch, so the
    /// application is woken once to read the places again.
    pub fn added(&mut self, trash_new: bool) {
        self.adding = false;
        if trash_new {
            let _ = self.wake.try_send(());
        }
    }
}

impl Watched {
    /// Watch `folder`; whether it was not watched before and is now.
    fn add(&mut self, folder: &Path) -> bool {
        if self.folders.iter().any(|w| w == folder) {
            return false;
        }
        let Some(watcher) = self.watcher.as_mut() else {
            return false;
        };
        let added = watcher.watch(folder, RecursiveMode::NonRecursive).is_ok();
        if added {
            self.folders.push(folder.to_path_buf());
        }
        added
    }
}

/// Whether a file event may have changed a place. An open or a read changes none, and reading the places again opens
/// their files: waking on it would read them again without end.
fn wakes(event: &notify::Result<notify::Event>) -> bool {
    event.as_ref().is_ok_and(|e| !e.kind.is_access())
}

/// How long reading the standard places should take: the user directories' file, the machine's name and a few stat
/// calls, which only a home or a configuration folder on a mount that does not answer holds up.
pub(super) const READ_DEADLINE: Duration = Duration::from_secs(5);

/// The standard places read again, `standard` being the ones shown: from `source` when there is one, else the same with
/// the trash's place following `trash_files`. Blocks on the file system, so a program runs it on an I/O task.
pub(super) fn reread(
    mut standard: Vec<Place>,
    source: Option<&PlacesSource>,
    trash_files: &Path,
) -> Vec<Place> {
    match source.map(|source| source()) {
        Some(Ok(fresh)) => fresh,
        read => {
            if let Some(Err(e)) = read {
                eprintln!("cupertino-files: cannot read the sidebar's places again: {e}");
            }
            let count = standard.len();
            follow_trash(&mut standard, count, trash_files);
            standard
        }
    }
}

impl Session {
    /// The places are read on an I/O task ([`Session::places_task`]), which the next [`Model::io_work`] starts.
    pub(super) fn follow_places(&mut self) {
        self.places_due = true;
        let trash = self.trash.home_trash().to_path_buf();
        self.places_watch.follow(&trash);
    }

    /// The task that adds the sidebar's watches asked for, if any.
    pub(super) fn places_watch_task(&mut self) -> Option<Command<Message>> {
        self.places_watch.task()
    }

    /// The watches were added: the places are read again when the trash's folder is newly watched.
    pub(super) fn places_watched(&mut self, trash_new: bool) -> Vec<Command<Message>> {
        self.places_watch.added(trash_new);
        Vec::new()
    }

    /// The task that reads the write state of the folders drags hovered, if any are wanted.
    pub(super) fn mounts_task(&mut self) -> Option<Command<Message>> {
        let wanted = self.mounts.take_wanted();
        (!wanted.is_empty()).then(|| {
            cupertino_kit::task::io(
                crate::space::MOUNT_READ_DEADLINE,
                move || crate::space::read_each(wanted),
                Message::MountsRead,
            )
        })
    }

    /// The write states read are kept; the next hover over those folders uses them.
    pub(super) fn mounts_read(&mut self, read: Vec<(PathBuf, bool)>) -> Vec<Command<Message>> {
        let now = std::time::Instant::now();
        for (dir, read_only) in read {
            self.mounts.record(dir, read_only, now);
        }
        Vec::new()
    }

    /// The task that reads the standard places again, when they are due and none is being read: the places leave the
    /// UI thread whole, as the places source and the trash's folder are files.
    pub(super) fn places_task(&mut self) -> Option<Command<Message>> {
        if !self.places_due || self.places_reading {
            return None;
        }
        self.places_due = false;
        self.places_reading = true;
        let standard = self.places[..self.standard_places].to_vec();
        let source = self.places_source.clone();
        let trash_files = self.trash.home_trash().join("files");
        Some(cupertino_kit::task::io(
            READ_DEADLINE,
            move || reread(standard, source.as_ref(), &trash_files),
            Message::PlacesRead,
        ))
    }

    /// The standard places [`Session::places_task`] read replace the ones shown; the removable media's after them stay.
    pub(super) fn places_read(&mut self, fresh: Vec<Place>) -> Vec<Command<Message>> {
        self.places_reading = false;
        let count = fresh.len();
        self.places.splice(..self.standard_places, fresh);
        self.standard_places = count;
        Vec::new()
    }
}
