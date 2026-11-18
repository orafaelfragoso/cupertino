//! The sidebar's places kept live: a watch on the folders they come from wakes the application, which reads them again.
//! The trash's place comes with the first item trashed and leaves when the trash's `files` folder goes
//! ([`follow_trash`]); with a source of the session's places ([`super::Start::with_places_source`]) a change of the user
//! directories or their configuration shows too.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use cupertino_kit::files::places::{follow_trash, Place};

/// What reads the session's places again: the standard ones, without the removable media.
pub type PlacesSource = Box<dyn Fn() -> io::Result<Vec<Place>>>;

/// The watch on the folders the places come from.
pub(super) struct PlacesWatch {
    watcher: Option<RecommendedWatcher>,
    /// The folders watched; the trash's own folder is added once it exists.
    watched: Vec<PathBuf>,
    /// Wakes the application to read the places again.
    wake: Arc<async_channel::Sender<()>>,
    pub woken: async_channel::Receiver<()>,
}

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
            watcher,
            watched: Vec::new(),
            wake,
            woken,
        };
        for folder in folders {
            watch.add(&folder);
        }
        if let Some(parent) = trash.parent() {
            watch.add(parent);
        }
        watch.follow(trash);
        let _ = watch.wake.try_send(());
        watch
    }

    /// Watch the trash's folder too once it exists, so the removal of its `files` folder is heard. Its `files` folder
    /// may have come between the read that found the trash and this watch, so a new watch wakes the application once
    /// to read the places again.
    pub fn follow(&mut self, trash: &Path) {
        if trash.is_dir() && self.add(trash) {
            let _ = self.wake.try_send(());
        }
    }

    /// Watch `folder`; whether it was not watched before and is now.
    fn add(&mut self, folder: &Path) -> bool {
        if self.watched.iter().any(|w| w == folder) {
            return false;
        }
        let Some(watcher) = self.watcher.as_mut() else {
            return false;
        };
        let added = watcher.watch(folder, RecursiveMode::NonRecursive).is_ok();
        if added {
            self.watched.push(folder.to_path_buf());
        }
        added
    }
}

/// Whether a file event may have changed a place. An open or a read changes none, and reading the places again opens
/// their files: waking on it would read them again without end.
fn wakes(event: &notify::Result<notify::Event>) -> bool {
    event.as_ref().is_ok_and(|e| !e.kind.is_access())
}

/// The standard places, `standard` first of `places`, read again: from `source` when there is one, else the same with
/// the trash's place following `trash_files`. The removable media's places after them are kept. The number of standard
/// places after.
pub(super) fn reread(
    places: &mut Vec<Place>,
    standard: usize,
    source: Option<&PlacesSource>,
    trash_files: &Path,
) -> usize {
    let read = source.map(|source| source());
    match read {
        Some(Ok(fresh)) => {
            let count = fresh.len();
            places.splice(..standard, fresh);
            count
        }
        Some(Err(e)) => {
            eprintln!("cupertino-files: cannot read the sidebar's places again: {e}");
            follow_trash(places, standard, trash_files)
        }
        None => follow_trash(places, standard, trash_files),
    }
}
