//! The applications of the session as they are now: a [`Catalogue`] is the index of the application folders
//! ([`crate::entry::index_applications`]) that can be read again, saying whether anything changed, and (feature
//! `watch`) an [`ApplicationWatch`] reads it again whenever the folders change, off the caller's thread.
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
pub use watch::{ApplicationWatch, Change};

#[cfg(feature = "watch")]
mod watch {
    use super::{Burst, Catalogue, Timing};
    use crate::entry::DesktopEntry;
    use notify::{RecursiveMode, Watcher};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::Instant;

    /// What the callback of an [`ApplicationWatch`] is told.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Change {
        /// The applications changed: all of them, as they are now.
        Applications(Vec<DesktopEntry>),
        /// A folder could not be watched; its changes may be missed until the process restarts.
        Error(String),
    }

    /// What the watch thread receives.
    enum Msg {
        /// Something changed in an application folder; `outside` when it may be the folder itself, or a folder
        /// above it, that appeared or went.
        Touched {
            outside: bool,
        },
        Stop,
    }

    /// Follows the application folders of a [`Catalogue`] while it lives; dropping it ends the watch and its thread.
    pub struct ApplicationWatch {
        stop: mpsc::Sender<Msg>,
    }

    impl Drop for ApplicationWatch {
        fn drop(&mut self) {
            let _ = self.stop.send(Msg::Stop);
        }
    }

    /// What is watched for `dirs`: each folder that exists with everything under it (true), and the nearest folder
    /// that exists above each, alone (false), so a folder that is made later, or removed and made again, is seen.
    pub(super) fn plan(dirs: &[PathBuf]) -> BTreeMap<PathBuf, bool> {
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

    /// Whether a file event at `path` concerns `dirs`: `Some(false)` inside one of them, `Some(true)` when it is one
    /// of them or a folder above one, `None` for anything else (the other entries of a folder above).
    pub(super) fn concerns(dirs: &[PathBuf], path: &Path) -> Option<bool> {
        if dirs.iter().any(|d| d.starts_with(path)) {
            Some(true)
        } else if dirs.iter().any(|d| path.starts_with(d)) {
            Some(false)
        } else {
            None
        }
    }

    impl ApplicationWatch {
        /// Follow the folders of `catalogue`, subfolders included, and call `report` from the watcher's thread with
        /// the applications each time a burst of changes leaves them different from the catalogue's last reading
        /// (a change made between that reading and the start of the watch is reported at once). A folder that does
        /// not exist yet is followed from the moment it is made.
        pub fn start(
            catalogue: Catalogue,
            timing: Timing,
            report: impl FnMut(Change) + Send + 'static,
        ) -> Result<Self, String> {
            let (sender, inbox) = mpsc::channel();
            let dirs = catalogue.dirs().to_vec();
            let events = sender.clone();
            let watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    let outside = match event {
                        // reading a file changes nothing
                        Ok(e) if e.kind.is_access() => return,
                        // events were lost: everything is looked at again
                        Ok(e) if e.need_rescan() => Some(true),
                        Ok(e) => e
                            .paths
                            .iter()
                            .filter_map(|p| concerns(&dirs, p))
                            .reduce(|a, b| a || b),
                        Err(_) => Some(true),
                    };
                    if let Some(outside) = outside {
                        let _ = events.send(Msg::Touched { outside });
                    }
                })
                .map_err(|e| e.to_string())?;
            std::thread::Builder::new()
                .name("cupertino-apps-watch".into())
                .spawn(move || follow(catalogue, timing, watcher, &inbox, report))
                .map_err(|e| e.to_string())?;
            Ok(Self { stop: sender })
        }
    }

    /// Watch what `plan` says and stop watching what it no longer does; with `again`, every watch is set anew (a
    /// folder that was removed and made again lost its watch).
    fn arm(
        watcher: &mut impl Watcher,
        watched: &mut BTreeMap<PathBuf, bool>,
        plan: BTreeMap<PathBuf, bool>,
        again: bool,
        report: &mut dyn FnMut(Change),
    ) {
        for (path, deep) in watched.iter() {
            if again || plan.get(path) != Some(deep) {
                let _ = watcher.unwatch(path);
            }
        }
        let mut now = BTreeMap::new();
        for (path, deep) in plan {
            if again || watched.get(&path) != Some(&deep) {
                let mode = if deep {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };
                if let Err(e) = watcher.watch(&path, mode) {
                    // a folder that went since the plan was made is found again through the folder above it
                    if path.is_dir() {
                        report(Change::Error(format!("{}: {e}", path.display())));
                    }
                    continue;
                }
            }
            now.insert(path, deep);
        }
        *watched = now;
    }

    /// How often at most the watches are set again for folders that keep appearing while they are being set.
    const SETTLES: usize = 16;

    /// Set the watches for `dirs` ([`arm`]) until the plan made after them is the plan they were set for. A folder
    /// made while the watches were being set was seen by none of them, yet the reading that follows finds its
    /// entries: without a watch of its own its later changes would be missed. Once the two plans agree, whatever is
    /// made next is made under a watch.
    pub(super) fn settle(
        watcher: &mut impl Watcher,
        watched: &mut BTreeMap<PathBuf, bool>,
        dirs: &[PathBuf],
        mut again: bool,
        report: &mut dyn FnMut(Change),
    ) {
        let mut planned = plan(dirs);
        for _ in 0..SETTLES {
            arm(
                watcher,
                watched,
                planned.clone(),
                std::mem::take(&mut again),
                report,
            );
            let now = plan(dirs);
            if now == planned {
                return;
            }
            planned = now;
        }
    }

    /// The watch thread: sets the watches, then reads the folders after each burst of events.
    fn follow(
        mut catalogue: Catalogue,
        timing: Timing,
        mut watcher: impl Watcher,
        inbox: &mpsc::Receiver<Msg>,
        mut report: impl FnMut(Change),
    ) {
        let dirs = catalogue.dirs().to_vec();
        let mut watched = BTreeMap::new();
        let mut burst = Burst::new(timing);
        // the first reading was taken before the watch began: a change in between shows now
        let mut again = true;
        let mut over = true;
        loop {
            if over {
                burst.end();
                // the watches first: what changes after them comes as an event, what changed before is read
                settle(
                    &mut watcher,
                    &mut watched,
                    &dirs,
                    std::mem::take(&mut again),
                    &mut report,
                );
                if catalogue.reread() {
                    report(Change::Applications(catalogue.entries().to_vec()));
                }
            }
            let message = match burst.left(Instant::now()) {
                None => inbox.recv().ok(),
                Some(left) => match inbox.recv_timeout(left) {
                    Ok(message) => Some(message),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        over = true;
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => None,
                },
            };
            match message {
                Some(Msg::Touched { outside }) => {
                    burst.touch(Instant::now());
                    again |= outside;
                    over = false;
                }
                Some(Msg::Stop) | None => return,
            }
        }
    }
}
