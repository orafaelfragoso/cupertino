//! The applications of the session as they are now: a [`Catalogue`] is the index of the application folders
//! ([`crate::entry::index_applications`]) that can be read again, saying whether anything changed, and (feature
//! `watch`) an [`ApplicationWatch`] waits out each burst of changes to the folders and reads it again, in a task of the
//! program's loop.
//!
//! A reading is always the whole index, so the visibility rules and the precedence of the folders hold after every
//! change: an entry removed from the person's folder brings back the system's entry of the same ID, and one put
//! there covers it.
//!
//! The file events of one change come in bursts (a package puts in many files; an editor writes a temporary file and
//! renames it over the entry). A [`Burst`] times them: the folders are read once the events are quiet for
//! [`Timing::quiet`], and at the latest [`Timing::longest`] after the burst's first event.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::entry::{index_applications, xdg_application_dirs, DesktopEntry, Locale, Visibility};

/// The applications of a list of folders (in precedence order), as last read.
#[derive(Clone, Debug)]
pub struct Catalogue {
    dirs: Vec<PathBuf>,
    locale: Locale,
    visibility: Visibility,
    entries: Vec<DesktopEntry>,
}

impl Catalogue {
    /// Read the applications of `dirs` (the first folder wins an ID), named for `locale` and shown as `visibility`
    /// says. A folder that does not exist has none; it is looked for again at every reading.
    pub fn new(dirs: Vec<PathBuf>, locale: Locale, visibility: Visibility) -> Self {
        let entries = index_applications(&dirs, &locale, &visibility);
        Self {
            dirs,
            locale,
            visibility,
            entries,
        }
    }

    /// The session's applications: the XDG application folders, the environment's language and desktop.
    pub fn from_env() -> Self {
        Self::new(
            xdg_application_dirs(),
            Locale::from_env(),
            Visibility::from_env(),
        )
    }

    /// The folders read, in precedence order.
    pub fn dirs(&self) -> &[PathBuf] {
        &self.dirs
    }

    /// The applications as last read.
    pub fn entries(&self) -> &[DesktopEntry] {
        &self.entries
    }

    /// Read the folders again. True when the applications differ from the last reading (an entry added, changed,
    /// replaced or removed, or now coming from another folder).
    pub fn reread(&mut self) -> bool {
        let entries = index_applications(&self.dirs, &self.locale, &self.visibility);
        if entries == self.entries {
            return false;
        }
        self.entries = entries;
        true
    }
}

/// How long the file events of a change are waited out before the folders are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// The folders are read once no event came for this long.
    pub quiet: Duration,
    /// The folders are read this long after a burst's first event even while events still come, so a long
    /// installation shows its applications as it goes.
    pub longest: Duration,
}

impl Default for Timing {
    /// Ours (approximate): the quiet is longer than the gaps between the files of one package and short enough to
    /// feel immediate; the longest wait is about how long a person takes to type an application's name.
    fn default() -> Self {
        Self {
            quiet: Duration::from_millis(250),
            longest: Duration::from_secs(2),
        }
    }
}

/// The burst of events under way, if any, and when it is over.
#[derive(Clone, Copy, Debug)]
pub struct Burst {
    timing: Timing,
    /// The first and the latest event of the burst.
    span: Option<(Instant, Instant)>,
}

impl Burst {
    pub fn new(timing: Timing) -> Self {
        Self { timing, span: None }
    }

    /// An event came at `now`: it starts a burst or goes on with the one under way.
    pub fn touch(&mut self, now: Instant) {
        let first = self.span.map_or(now, |(first, _)| first);
        self.span = Some((first, now));
    }

    /// How long from `now` until the burst is over (zero once it is); `None` when no event came since the last
    /// [`Burst::end`].
    pub fn left(&self, now: Instant) -> Option<Duration> {
        let (first, last) = self.span?;
        let over = (last + self.timing.quiet).min(first + self.timing.longest);
        Some(over.saturating_duration_since(now))
    }

    /// The burst was handled: the next event starts another.
    pub fn end(&mut self) {
        self.span = None;
    }
}

#[cfg(feature = "watch")]
pub use watch::{ApplicationWatch, Change, REREAD_DEADLINE};

#[cfg(feature = "watch")]
mod watch {
    use super::{Burst, Catalogue, Timing};
    use crate::entry::DesktopEntry;
    use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
    use std::collections::{BTreeMap, HashMap};
    use std::io;
    use std::os::fd::{AsFd, OwnedFd};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// How long [`ApplicationWatch::reread`] should take: setting the watches and reading every application folder of
    /// the session on a local disk takes well under it. The deadline a program gives the task that runs it.
    pub const REREAD_DEADLINE: Duration = Duration::from_secs(5);

    /// What an application folder, and every folder under it, is watched for: entries being created, written, changed
    /// in their attributes, removed or renamed, and the folder itself going. Not opens or reads, which change nothing.
    const FOLDER_EVENTS: WatchMask = WatchMask::CREATE
        .union(WatchMask::DELETE)
        .union(WatchMask::MODIFY)
        .union(WatchMask::CLOSE_WRITE)
        .union(WatchMask::ATTRIB)
        .union(WatchMask::MOVED_FROM)
        .union(WatchMask::MOVED_TO)
        .union(WatchMask::DELETE_SELF)
        .union(WatchMask::MOVE_SELF);

    /// What the nearest folder above an application folder is watched for: its entries appearing and going, so the
    /// application folder being made, removed or made again is seen.
    const ABOVE_EVENTS: WatchMask = WatchMask::CREATE
        .union(WatchMask::DELETE)
        .union(WatchMask::MOVED_FROM)
        .union(WatchMask::MOVED_TO)
        .union(WatchMask::DELETE_SELF)
        .union(WatchMask::MOVE_SELF);

    /// What [`ApplicationWatch::reread`] reports.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Change {
        /// The applications changed: all of them, as they are now.
        Applications(Vec<DesktopEntry>),
        /// A folder could not be watched; its changes may be missed until the process restarts.
        Error(String),
    }

    /// Follows the application folders of a [`Catalogue`] while it lives, starting no thread: the program's loop waits
    /// for each burst of changes to be over ([`ApplicationWatch::quiet`]) and then runs [`ApplicationWatch::reread`]
    /// as a task, since it reads the folders.
    pub struct ApplicationWatch {
        catalogue: Catalogue,
        timing: Timing,
        inotify: Inotify,
        /// The folder each watch watches.
        watching: HashMap<WatchDescriptor, PathBuf>,
        /// The descriptor [`ApplicationWatch::quiet`] waits on, once it has.
        ready: Option<async_io::Async<OwnedFd>>,
    }

    /// What is watched for `dirs`: each folder that exists with everything under it (true), and the nearest folder
    /// that exists above each, alone (false), so a folder that is made later, or removed and made again, is seen.
    fn plan(dirs: &[PathBuf]) -> BTreeMap<PathBuf, bool> {
        let mut plan = BTreeMap::new();
        for dir in dirs {
            if dir.is_dir() {
                plan.insert(dir.clone(), true);
            }
            if let Some(above) = dir.ancestors().skip(1).find(|a| a.is_dir()) {
                plan.entry(above.to_path_buf()).or_insert(false);
            }
        }
        plan
    }

    /// The folders `plan` watches, each with what it is watched for: a folder watched with everything under it, and
    /// every folder under it, for [`FOLDER_EVENTS`]; a folder above for [`ABOVE_EVENTS`] (both for one that is both).
    fn folders(plan: &BTreeMap<PathBuf, bool>) -> BTreeMap<PathBuf, WatchMask> {
        let mut folders: BTreeMap<PathBuf, WatchMask> = BTreeMap::new();
        for (path, &deep) in plan {
            if !deep {
                *folders.entry(path.clone()).or_insert(WatchMask::empty()) |= ABOVE_EVENTS;
                continue;
            }
            let mut stack = vec![path.clone()];
            while let Some(dir) = stack.pop() {
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for entry in entries.flatten() {
                        // a link to a folder is not followed: the reading does not look inside it either
                        if entry.file_type().is_ok_and(|t| t.is_dir()) {
                            stack.push(entry.path());
                        }
                    }
                }
                *folders.entry(dir).or_insert(WatchMask::empty()) |= FOLDER_EVENTS;
            }
        }
        folders
    }

    /// Whether a file event at `path` concerns `dirs`: `Some(false)` inside one of them, `Some(true)` when it is one
    /// of them or a folder above one, `None` for anything else (the other entries of a folder above).
    fn concerns(dirs: &[PathBuf], path: &Path) -> Option<bool> {
        if dirs.iter().any(|d| d.starts_with(path)) {
            Some(true)
        } else if dirs.iter().any(|d| path.starts_with(d)) {
            Some(false)
        } else {
            None
        }
    }

    /// How often at most the watches are set again for folders that keep appearing while they are being set.
    const SETTLES: usize = 16;

    impl ApplicationWatch {
        /// Follow the folders of `catalogue`, subfolders included, a folder that does not exist yet from the moment
        /// it is made. The first [`ApplicationWatch::reread`] sets the watches and reports a change made between the
        /// catalogue's last reading and then.
        pub fn new(catalogue: Catalogue, timing: Timing) -> Result<Self, String> {
            Ok(Self {
                catalogue,
                timing,
                inotify: Inotify::init().map_err(|e| e.to_string())?,
                watching: HashMap::new(),
                ready: None,
            })
        }

        /// Set the watches the folders need now, then read the folders again: all the applications when they differ
        /// from the last reading, and each folder that could not be watched. It blocks on the folders: a program runs
        /// it as a task, first and after each [`ApplicationWatch::quiet`].
        pub fn reread(&mut self) -> Vec<Change> {
            let mut changes = Vec::new();
            // the watches first: what changes after them comes as an event, what changed before is read
            self.settle(&mut changes);
            if self.catalogue.reread() {
                changes.push(Change::Applications(self.catalogue.entries().to_vec()));
            }
            changes
        }

        /// Set the watches for the folders until the plan made after them is the plan they were set for. A folder
        /// made while the watches were being set was seen by none of them, yet the reading that follows finds its
        /// entries: without a watch of its own its later changes would be missed. Once the two plans agree, whatever
        /// is made next is made under a watch.
        fn settle(&mut self, changes: &mut Vec<Change>) {
            let dirs = self.catalogue.dirs().to_vec();
            let mut planned = plan(&dirs);
            for _ in 0..SETTLES {
                self.arm(folders(&planned), changes);
                let now = plan(&dirs);
                if now == planned {
                    return;
                }
                planned = now;
            }
        }

        /// Watch the folders `wanted` names, each for what it names, and stop watching every other.
        fn arm(&mut self, wanted: BTreeMap<PathBuf, WatchMask>, changes: &mut Vec<Change>) {
            let mut watched = HashMap::new();
            for (path, mask) in &wanted {
                // a folder watched already keeps its watch, now for `mask`
                match self.inotify.watches().add(path, *mask) {
                    Ok(wd) => {
                        watched.insert(wd, path.clone());
                    }
                    // a folder that went since the plan was made is found again through the folder above it
                    Err(e) if path.is_dir() => {
                        changes.push(Change::Error(format!("{}: {e}", path.display())))
                    }
                    Err(_) => {}
                }
            }
            for (wd, _) in self.watching.drain() {
                if !watched.contains_key(&wd) {
                    let _ = self.inotify.watches().remove(wd);
                }
            }
            self.watching = watched;
        }

        /// Take the events the kernel holds, without waiting: each that concerns the folders goes on with the burst.
        fn take(&mut self, burst: &mut Burst) -> io::Result<()> {
            let dirs = self.catalogue.dirs();
            let mut buffer = [0u8; 4096];
            loop {
                let events = match self.inotify.read_events(&mut buffer) {
                    Ok(events) => events,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                };
                let now = Instant::now();
                for event in events {
                    let touched = if event.mask.contains(EventMask::Q_OVERFLOW) {
                        // events were lost: everything is read again
                        true
                    } else if event.mask.contains(EventMask::IGNORED) {
                        // the folder went, and its watch with it
                        self.watching.remove(&event.wd);
                        false
                    } else {
                        self.watching.get(&event.wd).is_some_and(|dir| {
                            let path = event
                                .name
                                .map_or_else(|| dir.clone(), |name| dir.join(name));
                            concerns(dirs, &path).is_some()
                        })
                    };
                    if touched {
                        burst.touch(now);
                    }
                }
            }
        }

        /// Waits, without blocking, until a burst of events in the folders is over: the next
        /// [`ApplicationWatch::reread`] then reads them. The future is woken when the notification's descriptor is
        /// readable or the burst is over. An error once the notification failed for good.
        pub async fn quiet(&mut self) -> Result<(), String> {
            let failed = |e: io::Error| format!("waiting for file events: {e}");
            if self.ready.is_none() {
                let ready = self
                    .inotify
                    .as_fd()
                    .try_clone_to_owned()
                    .and_then(async_io::Async::new)
                    .map_err(failed)?;
                self.ready = Some(ready);
            }
            let mut burst = Burst::new(self.timing);
            loop {
                self.take(&mut burst).map_err(failed)?;
                let left = burst.left(Instant::now());
                if left.is_some_and(|left| left.is_zero()) {
                    return Ok(());
                }
                let Some(ready) = &self.ready else {
                    return Ok(());
                };
                match left {
                    Some(left) => {
                        futures_lite::future::or(
                            async {
                                ready.readable().await.map_err(failed)?;
                                Ok::<_, String>(())
                            },
                            async {
                                async_io::Timer::after(left).await;
                                Ok(())
                            },
                        )
                        .await?
                    }
                    None => ready.readable().await.map_err(failed)?,
                }
            }
        }
    }
}
