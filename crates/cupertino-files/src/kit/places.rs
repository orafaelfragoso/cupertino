//! The sidebar's places kept live: the folders they come from are followed by the family's listings
//! ([`DirectoryModel`], with their watch), and a change in any of them wakes the application, which reads the places
//! again. The trash's place comes with the first item trashed and leaves when the trash's `files` folder goes
//! ([`follow_trash`]); with a source of the session's places ([`super::Start::with_places_source`]) a change of the user
//! directories or their configuration shows too.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cupertino_kit::files::model::{
    DirectoryModel, ModelDelta, ModelOptions, ModelWatch, ModelWork,
};
use cupertino_kit::files::places::{follow_trash, Place};
use cupertino_kit::prelude::Command;

use super::{wake_channel, Message, Session, Wake};

/// What reads the session's places again: the standard ones, without the removable media.
pub type PlacesSource = Arc<dyn Fn() -> io::Result<Vec<Place>> + Send + Sync>;

/// The folders the places come from, each followed by a listing whose changes wake the application.
pub(super) struct PlacesWatch {
    /// The folders asked for and the trash's parent.
    folders: Vec<DirectoryModel>,
    /// The trash's folder, whose `files` folder makes its place, followed once it exists; opened again whenever
    /// another folder changed while it was not there.
    trash: PathBuf,
    trash_model: DirectoryModel,
    wake: Wake,
    pub woken: async_channel::Receiver<()>,
}

/// How the places' folders are listed: watched, with nothing but what tells that a folder changed.
fn followed(dir: &Path, wake: &Wake) -> DirectoryModel {
    let options = ModelOptions {
        watch: true,
        ..ModelOptions::default()
    };
    DirectoryModel::open(dir, options, Arc::clone(wake))
}

/// Whether `delta` says the folder changed: a folder that is not there changes nothing until it comes.
fn changes(delta: &ModelDelta) -> bool {
    !matches!(delta, ModelDelta::OpenFailed { .. })
}

impl PlacesWatch {
    /// Follow `folders` (each alone, not what it holds) and the trash at `trash`, with its parent until it exists.
    /// The places were read before the folders are listed: their first listing wakes the application, so what changed
    /// in between is read too. Nothing is read here: the listings are I/O work ([`PlacesWatch::work`]).
    pub fn start(folders: Vec<PathBuf>, trash: &Path) -> PlacesWatch {
        let (wake, woken) = wake_channel();
        let folders = folders
            .iter()
            .map(PathBuf::as_path)
            .chain(trash.parent())
            .map(|dir| followed(dir, &wake))
            .collect();
        PlacesWatch {
            folders,
            trash: trash.to_path_buf(),
            trash_model: followed(trash, &wake),
            wake,
            woken,
        }
    }

    /// Whether a folder of the places changed since the last call. A trash folder that was not there is followed
    /// again once another folder changed: that may be where it came.
    pub fn changed(&mut self) -> bool {
        // every listing is polled, so none keeps what it heard for the next call
        let polled: Vec<bool> = self
            .folders
            .iter_mut()
            .map(|model| model.poll().iter().any(changes))
            .collect();
        let folders = polled.contains(&true);
        let trash = self.trash_model.poll().iter().any(changes);
        if folders {
            self.follow();
        }
        folders || trash
    }

    /// Follow the trash's folder again when it was not there: an item trashed may have made it.
    pub fn follow(&mut self) {
        if self.trash_model.opened() == Some(false) && !self.trash_model.is_working() {
            self.trash_model = followed(&self.trash, &self.wake);
        }
    }

    /// The listings' work and watches waiting, which the program runs as I/O tasks and futures on its UI loop.
    pub fn work(&mut self) -> (Vec<ModelWork>, Vec<ModelWatch>) {
        let models = self
            .folders
            .iter_mut()
            .chain(std::iter::once(&mut self.trash_model));
        let (mut works, mut watches) = (Vec::new(), Vec::new());
        for model in models {
            // the watch first: the first work registers the folder with it
            watches.extend(model.take_watch());
            works.extend(model.take_work());
        }
        (works, watches)
    }
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
        self.places_watch.follow();
    }

    /// A folder of the places may have changed: the places are read again when one did.
    pub(super) fn places_woken(&mut self) {
        if self.places_watch.changed() {
            self.follow_places();
        }
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
