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
                arm(
                    &mut watcher,
                    &mut watched,
                    plan(&dirs),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn visibility() -> Visibility {
        Visibility {
            current_desktops: vec!["Cupertino".into()],
            path_dirs: vec![],
            skip_try_exec: true,
        }
    }

    fn entry(name: &str, extra: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec=program\n{extra}")
    }

    /// The person's folder and a system folder under `root`, the person's first; neither is made.
    fn folders(root: &Path) -> (PathBuf, PathBuf) {
        (
            root.join("home/share/applications"),
            root.join("system/share/applications"),
        )
    }

    fn catalogue(root: &Path) -> Catalogue {
        let (user, system) = folders(root);
        Catalogue::new(vec![user, system], Locale::default(), visibility())
    }

    fn put(dir: &Path, file: &str, text: &str) {
        std::fs::create_dir_all(dir.join(file).parent().unwrap()).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
    }

    /// Write to a temporary name in the same folder, then rename over the entry.
    fn replace(dir: &Path, file: &str, text: &str) {
        let temporary = dir.join(format!(".{file}.tmp"));
        std::fs::write(&temporary, text).unwrap();
        std::fs::rename(temporary, dir.join(file)).unwrap();
    }

    fn names(catalogue: &Catalogue) -> Vec<(&str, &str)> {
        let mut names: Vec<_> = catalogue
            .entries()
            .iter()
            .map(|e| (e.id.as_str(), e.name.as_str()))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_reading_follows_an_entry_added_changed_replaced_and_removed() {
        let root = tempfile::tempdir().unwrap();
        let (_, system) = folders(root.path());
        put(&system, "paint.desktop", &entry("Paint", ""));
        let mut catalogue = catalogue(root.path());
        assert_eq!(names(&catalogue), [("paint.desktop", "Paint")]);
        assert!(!catalogue.reread(), "nothing changed");

        put(&system, "notes.desktop", &entry("Notes", ""));
        assert!(catalogue.reread(), "an entry added");
        assert_eq!(
            names(&catalogue),
            [("notes.desktop", "Notes"), ("paint.desktop", "Paint")]
        );

        put(&system, "notes.desktop", &entry("Jotter", ""));
        assert!(catalogue.reread(), "an entry written in place");
        assert_eq!(
            names(&catalogue),
            [("notes.desktop", "Jotter"), ("paint.desktop", "Paint")]
        );

        replace(&system, "notes.desktop", &entry("Notebook", ""));
        assert!(catalogue.reread(), "an entry replaced by a rename");
        assert_eq!(
            names(&catalogue),
            [("notes.desktop", "Notebook"), ("paint.desktop", "Paint")],
            "the temporary file is no entry"
        );

        // written again with the same text: the applications are as they were
        replace(&system, "notes.desktop", &entry("Notebook", ""));
        assert!(!catalogue.reread());

        std::fs::remove_file(system.join("notes.desktop")).unwrap();
        assert!(catalogue.reread(), "an entry removed");
        assert_eq!(names(&catalogue), [("paint.desktop", "Paint")]);
    }

    #[test]
    fn a_reading_keeps_the_visibility_rules() {
        let root = tempfile::tempdir().unwrap();
        let (_, system) = folders(root.path());
        put(&system, "paint.desktop", &entry("Paint", ""));
        let mut catalogue = catalogue(root.path());
        // none of these is an application a person opens
        put(
            &system,
            "hidden.desktop",
            &entry("Hidden", "NoDisplay=true\n"),
        );
        put(&system, "shell.desktop", &entry("Shell", "Terminal=true\n"));
        put(
            &system,
            "other.desktop",
            &entry("Other", "OnlyShowIn=Elsewhere;\n"),
        );
        put(&system, "broken.desktop", "not a desktop entry");
        put(&system, "mimeinfo.cache", "[MIME Cache]\n");
        assert!(!catalogue.reread());
        // an entry that turns hidden goes, and comes back when it is shown again
        put(
            &system,
            "paint.desktop",
            &entry("Paint", "NoDisplay=true\n"),
        );
        assert!(catalogue.reread());
        assert_eq!(names(&catalogue), []);
        put(&system, "paint.desktop", &entry("Paint", ""));
        assert!(catalogue.reread());
        assert_eq!(names(&catalogue), [("paint.desktop", "Paint")]);
    }

    #[test]
    fn a_reading_keeps_the_person_s_entry_over_the_system_s() {
        let root = tempfile::tempdir().unwrap();
        let (user, system) = folders(root.path());
        put(&system, "paint.desktop", &entry("Paint", ""));
        let mut catalogue = catalogue(root.path());
        assert_eq!(
            catalogue.entries()[0].source,
            Some(system.join("paint.desktop"))
        );

        // the person's folder did not exist; made later, its entry covers the system's
        put(&user, "paint.desktop", &entry("My Paint", ""));
        assert!(catalogue.reread());
        assert_eq!(names(&catalogue), [("paint.desktop", "My Paint")]);
        assert_eq!(
            catalogue.entries()[0].source,
            Some(user.join("paint.desktop"))
        );

        // a change to the covered entry changes nothing
        put(&system, "paint.desktop", &entry("Paint 2", ""));
        assert!(!catalogue.reread());

        // the person hides it: the system's entry stays covered
        replace(&user, "paint.desktop", &entry("My Paint", "Hidden=true\n"));
        assert!(catalogue.reread());
        assert_eq!(names(&catalogue), []);

        // the person's entry goes: the system's is back
        std::fs::remove_file(user.join("paint.desktop")).unwrap();
        assert!(catalogue.reread());
        assert_eq!(names(&catalogue), [("paint.desktop", "Paint 2")]);
    }

    #[test]
    fn a_reading_follows_subfolders() {
        let root = tempfile::tempdir().unwrap();
        let (_, system) = folders(root.path());
        std::fs::create_dir_all(&system).unwrap();
        let mut catalogue = catalogue(root.path());
        put(&system, "vendor/draw.desktop", &entry("Draw", ""));
        assert!(catalogue.reread());
        assert_eq!(names(&catalogue), [("vendor-draw.desktop", "Draw")]);
        std::fs::remove_dir_all(system.join("vendor")).unwrap();
        assert!(catalogue.reread());
        assert_eq!(names(&catalogue), []);
    }

    #[test]
    fn a_burst_is_over_once_it_is_quiet_and_never_lasts_past_its_longest() {
        let timing = Timing {
            quiet: Duration::from_millis(250),
            longest: Duration::from_secs(2),
        };
        let ms = Duration::from_millis;
        let start = Instant::now();
        let mut burst = Burst::new(timing);
        assert_eq!(burst.left(start), None, "no event, nothing to wait for");
        burst.touch(start);
        assert_eq!(burst.left(start), Some(ms(250)));
        assert_eq!(burst.left(start + ms(100)), Some(ms(150)));
        // each event puts the end off
        burst.touch(start + ms(200));
        assert_eq!(burst.left(start + ms(200)), Some(ms(250)));
        assert_eq!(burst.left(start + ms(450)), Some(ms(0)));
        assert_eq!(burst.left(start + ms(900)), Some(ms(0)));
        burst.end();
        assert_eq!(burst.left(start + ms(900)), None);
        // events that never stop: read at the longest wait from the first
        let first = start + ms(1000);
        let mut at = first;
        burst.touch(at);
        while at < first + ms(1900) {
            at += ms(100);
            assert!(burst.left(at).is_some_and(|left| !left.is_zero()), "{at:?}");
            burst.touch(at);
        }
        assert_eq!(burst.left(at), Some(ms(100)));
        burst.touch(first + ms(1950));
        assert_eq!(burst.left(first + ms(1950)), Some(ms(50)));
        assert_eq!(burst.left(first + ms(2000)), Some(ms(0)));
    }

    #[cfg(feature = "watch")]
    mod watching {
        use super::super::watch::{concerns, plan};
        use super::*;
        use std::collections::BTreeMap;
        use std::sync::mpsc::{self, Receiver};

        /// Longer than any step takes on a busy machine; a step that passes never waits this long.
        const PATIENCE: Duration = Duration::from_secs(20);

        const FAST: Timing = Timing {
            quiet: Duration::from_millis(15),
            longest: Duration::from_millis(150),
        };

        fn watch(root: &Path) -> (ApplicationWatch, Receiver<Change>) {
            let (sender, changes) = mpsc::channel();
            let watch = ApplicationWatch::start(catalogue(root), FAST, move |change| {
                let _ = sender.send(change);
            })
            .unwrap();
            (watch, changes)
        }

        /// The next report whose applications are `want` (by ID and name); a burst cut in two reports twice.
        fn until(changes: &Receiver<Change>, want: &[(&str, &str)]) {
            let end = Instant::now() + PATIENCE;
            let mut seen = Vec::new();
            loop {
                let left = end.saturating_duration_since(Instant::now());
                match changes.recv_timeout(left) {
                    Ok(Change::Applications(entries)) => {
                        let mut names: Vec<_> = entries
                            .iter()
                            .map(|e| (e.id.clone(), e.name.clone()))
                            .collect();
                        names.sort();
                        if names
                            .iter()
                            .map(|(id, name)| (id.as_str(), name.as_str()))
                            .eq(want.iter().copied())
                        {
                            return;
                        }
                        seen.push(names);
                    }
                    Ok(Change::Error(e)) => panic!("the watch failed: {e}"),
                    Err(_) => panic!("no report of {want:?}; reported: {seen:?}"),
                }
            }
        }

        #[test]
        fn the_plan_watches_each_folder_whole_and_the_nearest_folder_above_it() {
            let root = tempfile::tempdir().unwrap();
            let (user, system) = folders(root.path());
            std::fs::create_dir_all(&system).unwrap();
            let dirs = [user.clone(), system.clone()];
            assert_eq!(
                plan(&dirs),
                BTreeMap::from([
                    (system.clone(), true),
                    (system.parent().unwrap().to_path_buf(), false),
                    // nothing of the person's folder exists yet
                    (root.path().to_path_buf(), false),
                ])
            );
            std::fs::create_dir_all(user.parent().unwrap()).unwrap();
            assert_eq!(plan(&dirs).get(user.parent().unwrap()), Some(&false));
            std::fs::create_dir_all(&user).unwrap();
            assert_eq!(plan(&dirs).get(&user), Some(&true));

            assert_eq!(concerns(&dirs, &system.join("a.desktop")), Some(false));
            assert_eq!(
                concerns(&dirs, &system.join("vendor/a.desktop")),
                Some(false)
            );
            assert_eq!(concerns(&dirs, &system), Some(true));
            assert_eq!(concerns(&dirs, user.parent().unwrap()), Some(true));
            assert_eq!(concerns(&dirs, &root.path().join("home/share/fonts")), None);
        }

        #[test]
        fn the_watch_reports_entries_added_changed_replaced_and_removed() {
            let root = tempfile::tempdir().unwrap();
            let (_, system) = folders(root.path());
            put(&system, "paint.desktop", &entry("Paint", ""));
            let (_watch, changes) = watch(root.path());
            put(&system, "notes.desktop", &entry("Notes", ""));
            until(
                &changes,
                &[("notes.desktop", "Notes"), ("paint.desktop", "Paint")],
            );
            put(&system, "notes.desktop", &entry("Jotter", ""));
            until(
                &changes,
                &[("notes.desktop", "Jotter"), ("paint.desktop", "Paint")],
            );
            replace(&system, "notes.desktop", &entry("Notebook", ""));
            until(
                &changes,
                &[("notes.desktop", "Notebook"), ("paint.desktop", "Paint")],
            );
            std::fs::remove_file(system.join("notes.desktop")).unwrap();
            until(&changes, &[("paint.desktop", "Paint")]);
            // a subfolder made while watching is followed too
            put(&system, "vendor/draw.desktop", &entry("Draw", ""));
            until(
                &changes,
                &[("paint.desktop", "Paint"), ("vendor-draw.desktop", "Draw")],
            );
            put(&system, "vendor/draw.desktop", &entry("Sketch", ""));
            until(
                &changes,
                &[
                    ("paint.desktop", "Paint"),
                    ("vendor-draw.desktop", "Sketch"),
                ],
            );
        }

        #[test]
        fn the_watch_follows_a_folder_made_later_and_one_made_again() {
            let root = tempfile::tempdir().unwrap();
            let (user, system) = folders(root.path());
            put(&system, "paint.desktop", &entry("Paint", ""));
            let (_watch, changes) = watch(root.path());
            // nothing of the person's folder existed when the watch began
            put(&user, "paint.desktop", &entry("My Paint", ""));
            until(&changes, &[("paint.desktop", "My Paint")]);
            replace(&user, "paint.desktop", &entry("Our Paint", ""));
            until(&changes, &[("paint.desktop", "Our Paint")]);
            // the folder goes with its entry: the system's entry is back
            std::fs::remove_dir_all(&user).unwrap();
            until(&changes, &[("paint.desktop", "Paint")]);
            put(&user, "notes.desktop", &entry("Notes", ""));
            until(
                &changes,
                &[("notes.desktop", "Notes"), ("paint.desktop", "Paint")],
            );
            std::fs::remove_file(user.join("notes.desktop")).unwrap();
            until(&changes, &[("paint.desktop", "Paint")]);
        }

        #[test]
        fn a_change_made_before_the_watch_began_is_reported_and_a_dropped_watch_ends() {
            let root = tempfile::tempdir().unwrap();
            let (_, system) = folders(root.path());
            let catalogue = catalogue(root.path());
            put(&system, "paint.desktop", &entry("Paint", ""));
            let (sender, changes) = mpsc::channel();
            let watch = ApplicationWatch::start(catalogue, FAST, move |change| {
                let _ = sender.send(change);
            })
            .unwrap();
            until(&changes, &[("paint.desktop", "Paint")]);
            drop(watch);
            // the thread ends and drops its callback: the channel closes
            let end = Instant::now() + PATIENCE;
            loop {
                match changes.recv_timeout(end.saturating_duration_since(Instant::now())) {
                    Ok(_) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => panic!("the watch thread goes on"),
                }
            }
        }
    }
}
