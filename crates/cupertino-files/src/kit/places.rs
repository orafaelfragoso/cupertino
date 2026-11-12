//! The sidebar's places kept live: a watch on the folders they come from wakes the application, which reads them again.
//! The trash's place comes with the first item trashed and leaves when the trash's `files` folder goes
//! ([`follow_trash`]); with a source of the session's places ([`super::Start::with_places_source`]) a change of the user
//! directories or their configuration shows too.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::places::{follow_trash, Place};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places::{Medium, PlaceGroup, PlaceKind};

    fn place(name: &str, kind: PlaceKind) -> Place {
        Place {
            name: name.into(),
            path: PathBuf::from(format!("/{name}")),
            group: PlaceGroup::Favorites,
            kind,
        }
    }

    #[test]
    fn the_standard_places_are_read_again_and_the_devices_kept() {
        let d = tempfile::tempdir().unwrap();
        let trash = d.path().join("Trash/files");
        let device = Place::removable("Backup", None, false, Medium::Disk);
        let mut places = vec![place("Documents", PlaceKind::Documents), device.clone()];
        assert_eq!(reread(&mut places, 1, None, &trash), 1, "no trash yet");
        std::fs::create_dir_all(&trash).unwrap();
        assert_eq!(reread(&mut places, 1, None, &trash), 2);
        assert_eq!(places[1].kind, PlaceKind::Trash);
        assert_eq!(
            places[2], device,
            "a device stays after the standard places"
        );

        let source: PlacesSource = Box::new(|| Ok(vec![place("Music", PlaceKind::Music)]));
        assert_eq!(reread(&mut places, 2, Some(&source), &trash), 1);
        assert_eq!(
            places.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["Music", "Backup"]
        );
    }

    #[test]
    fn reading_the_places_files_does_not_wake() {
        use notify::event::{AccessKind, AccessMode, CreateKind, EventKind, ModifyKind};
        let event = |kind| {
            Ok(notify::Event::new(kind).add_path(PathBuf::from("/home/.config/user-dirs.dirs")))
        };
        for kind in [
            AccessKind::Open(AccessMode::Any),
            AccessKind::Read,
            AccessKind::Close(AccessMode::Read),
        ] {
            assert!(!wakes(&event(EventKind::Access(kind))), "{kind:?}");
        }
        assert!(wakes(&event(EventKind::Create(CreateKind::File))));
        assert!(wakes(&event(EventKind::Modify(ModifyKind::Any))));
        assert!(!wakes(&Err(notify::Error::generic("lost"))));
    }

    /// The places are read before the watch begins: a trash made in between (by another process at the session's start)
    /// is read again at once, not only at the next change.
    #[test]
    fn a_new_watch_starts_woken_so_what_changed_before_it_is_read() {
        let d = tempfile::tempdir().unwrap();
        let trash = d.path().join("Trash");
        std::fs::create_dir_all(trash.join("files")).unwrap();
        let watch = PlacesWatch::start(Vec::new(), &trash);
        assert_eq!(watch.woken.try_recv(), Ok(()));
    }

    #[test]
    fn a_change_in_a_watched_folder_wakes_and_the_trash_is_watched_once_it_exists() {
        let d = tempfile::tempdir().unwrap();
        let trash = d.path().join("Trash");
        let mut watch = PlacesWatch::start(Vec::new(), &trash);
        assert_eq!(watch.woken.try_recv(), Ok(()), "the wake it starts with");
        std::fs::create_dir_all(trash.join("files")).unwrap();
        watch
            .woken
            .recv_blocking()
            .expect("the trash's arrival is heard");
        watch.follow(&trash);
        assert_eq!(
            watch.woken.try_recv(),
            Ok(()),
            "a trash watched anew reads the places again, in case its files folder came first"
        );
        watch.follow(&trash);
        assert!(
            watch.woken.try_recv().is_err(),
            "a trash watched already does not"
        );
        while watch.woken.try_recv().is_ok() {}
        std::fs::remove_dir(trash.join("files")).unwrap();
        watch
            .woken
            .recv_blocking()
            .expect("the removal of its files folder is heard");
    }
}
