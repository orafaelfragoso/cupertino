//! Change notification for the settings files: a [`ConfigWatcher`] watches the directory of each file (inotify through
//! `notify`), so an atomic replacement (temporary file and rename, [`crate::atomic`]), an editor's save and the file's
//! creation or removal are all seen, also for a file an environment variable moved out of the settings directory.
//!
//! The events of one change come in bursts (create, write and rename of the temporary file; an editor's backup and
//! write). The watcher waits until events touching watched files or their directories are quiet for [`COALESCE`],
//! then reports each watched file whose contents changed since the last report, once. A burst that leaves a file
//! as it was (a `touch`, a write of the same text) reports nothing. Reports go to a callback on the watcher's thread;
//! a process forwards them to its event loop
//! (the server's calloop channel, a shell process's waker).

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

/// How long a burst of file events must be quiet before it is reported. Ours (approximate): long enough for a rename
/// and an editor's save sequence, short enough to feel immediate.
pub const COALESCE: Duration = Duration::from_millis(50);

/// What the callback of a [`ConfigWatcher`] is told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    /// These watched files changed (contents, creation or removal), in the order they were given.
    Changed(Vec<PathBuf>),
    /// The file system notification failed; changes may be missed until the process restarts.
    Error(String),
}

/// Watches settings files while it lives; dropping it stops the watch and its thread.
pub struct ConfigWatcher {
    stop: mpsc::Sender<Msg>,
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        // the thread is gone already when its callback panicked; then there is nothing to stop
        let _ = self.stop.send(Msg::Stop);
    }
}

/// What the watch thread receives.
enum Msg {
    Event(notify::Result<notify::Event>),
    Stop,
}

/// The contents of `path` as the watcher compares them: `None` when the file does not exist.
fn snapshot(path: &Path) -> io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        // without O_NONBLOCK a FIFO put at a watched path would block the open until a writer came
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// The directory a file is watched through.
fn dir_of(file: &Path) -> PathBuf {
    file.parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

impl ConfigWatcher {
    /// Watch `files` and call `report` from the watcher's thread for each quiet burst that changed some of them. The
    /// directories of the files are created when missing (they are the settings directories). Their parents are
    /// watched too, so a settings directory that is removed and created again is watched again. A panic in `report`
    /// ends the watch; it is written to stderr.
    pub fn new(
        files: Vec<PathBuf>,
        report: impl FnMut(WatchEvent) + Send + 'static,
    ) -> io::Result<Self> {
        let mut last: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
        for f in &files {
            last.insert(f.clone(), snapshot(f)?);
        }
        let dirs: BTreeSet<PathBuf> = files.iter().map(|f| dir_of(f)).collect();
        let (tx, rx) = mpsc::channel::<Msg>();
        let events = tx.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            // the receiver is gone only once the thread ended, when no event matters any more
            let _ = events.send(Msg::Event(event));
        })
        .map_err(io::Error::other)?;
        for dir in &dirs {
            std::fs::create_dir_all(dir)?;
            watcher
                .watch(dir, RecursiveMode::NonRecursive)
                .map_err(|e| io::Error::other(format!("{}: {e}", dir.display())))?;
            if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
                watcher
                    .watch(parent, RecursiveMode::NonRecursive)
                    .map_err(|e| io::Error::other(format!("{}: {e}", parent.display())))?;
            }
        }
        let mut report = report;
        std::thread::Builder::new().name("cupertino-defaults-watch".into()).spawn(move || {
            let run = std::panic::AssertUnwindSafe(|| watch_loop(watcher, &rx, &files, &dirs, &mut last, &mut report));
            if let Err(panic) = std::panic::catch_unwind(run) {
                let what = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
                eprintln!("cupertino-defaults: the settings watch callback panicked ({what}); the settings files are no longer watched");
            }
        })?;
        Ok(Self { stop: tx })
    }
}

/// Whether a file event of `kind` may have changed a file. Opening, reading and closing a file change nothing; the
/// watcher's own reads, and every other process's watcher's reads of the same directory, would otherwise wake it
/// again without end.
fn may_change(kind: &notify::EventKind) -> bool {
    !kind.is_access()
}

/// Where the watch thread's messages come from, and the clock their quiet is timed on: the channel and the monotonic
/// clock, or in tests a script of messages and quiet spans on a clock of its own.
trait Inbox {
    /// The next message, waiting as long as it takes; `None` once no more can come.
    fn recv(&self) -> Option<Msg>;
    /// The next message if one comes within `timeout`.
    fn recv_timeout(&self, timeout: Duration) -> Result<Msg, mpsc::RecvTimeoutError>;
    fn now(&self) -> Instant;
}

impl Inbox for mpsc::Receiver<Msg> {
    fn recv(&self) -> Option<Msg> {
        mpsc::Receiver::recv(self).ok()
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<Msg, mpsc::RecvTimeoutError> {
        mpsc::Receiver::recv_timeout(self, timeout)
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// The watch thread: waits for a burst, lets it go quiet, then reports the files whose contents changed.
fn watch_loop(
    mut watcher: notify::RecommendedWatcher,
    rx: &impl Inbox,
    files: &[PathBuf],
    dirs: &BTreeSet<PathBuf>,
    last: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
    report: &mut impl FnMut(WatchEvent),
) {
    let watched: BTreeSet<&PathBuf> = files.iter().collect();
    // whether `msg` may have changed a watched file; a settings directory that appears again is watched again
    let mut handle = |msg: Msg, report: &mut dyn FnMut(WatchEvent)| -> Option<bool> {
        match msg {
            Msg::Stop => None,
            Msg::Event(Ok(event)) if !may_change(&event.kind) => Some(false),
            Msg::Event(Ok(event)) => {
                for dir in event.paths.iter().filter(|p| dirs.contains(*p)) {
                    if event.kind.is_create() {
                        if let Err(e) = watcher.watch(dir, RecursiveMode::NonRecursive) {
                            report(WatchEvent::Error(format!("{}: {e}", dir.display())));
                        }
                    }
                }
                Some(
                    event.need_rescan()
                        || event
                            .paths
                            .iter()
                            .any(|p| watched.contains(p) || dirs.contains(p)),
                )
            }
            // A notification error may have lost events: compare every file.
            Msg::Event(Err(e)) => {
                report(WatchEvent::Error(e.to_string()));
                Some(true)
            }
        }
    };
    // the files were read before the watch began: a change in between shows now
    compare(files, last, report);
    loop {
        // wait for the first event of a burst, then until the burst is quiet
        let Some(first) = rx.recv() else { return };
        let Some(touched) = handle(first, report) else {
            return;
        };
        if !touched {
            continue;
        }
        let mut deadline = rx.now() + COALESCE;
        while let Some(left) = deadline.checked_duration_since(rx.now()) {
            match rx.recv_timeout(left) {
                Ok(msg) => match handle(msg, report) {
                    Some(true) => deadline = rx.now() + COALESCE,
                    Some(false) => (),
                    None => return,
                },
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        compare(files, last, report);
    }
}

/// Report each of `files` whose contents differ from `last`, and remember them.
fn compare(
    files: &[PathBuf],
    last: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
    report: &mut dyn FnMut(WatchEvent),
) {
    let mut changed = Vec::new();
    for f in files {
        match snapshot(f) {
            Ok(now) => {
                if last.get(f) != Some(&now) {
                    last.insert(f.clone(), now);
                    changed.push(f.clone());
                }
            }
            Err(e) => report(WatchEvent::Error(format!("{}: {e}", f.display()))),
        }
    }
    if !changed.is_empty() {
        report(WatchEvent::Changed(changed));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    fn watch(files: Vec<PathBuf>) -> (ConfigWatcher, Receiver<WatchEvent>) {
        let (tx, rx) = mpsc::channel();
        let w = ConfigWatcher::new(files, move |e| tx.send(e).unwrap()).unwrap();
        (w, rx)
    }

    /// Every report within `window`.
    fn reports(rx: &Receiver<WatchEvent>, window: Duration) -> Vec<WatchEvent> {
        let end = std::time::Instant::now() + window;
        let mut out = Vec::new();
        while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(e) => out.push(e),
                Err(_) => break,
            }
        }
        out
    }

    const WINDOW: Duration = Duration::from_millis(600);

    #[test]
    fn unrelated_parent_activity_does_not_delay_a_change_report() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cupertino/dock.conf");
        let noise = root.path().join("other.conf");
        let (_w, rx) = watch(vec![path.clone()]);
        let (ready_tx, ready_rx) = mpsc::channel();
        let received = std::thread::scope(|scope| {
            let (stop_tx, stop_rx) = mpsc::channel::<()>();
            let writer = scope.spawn(move || {
                std::fs::write(&noise, "unrelated").unwrap();
                ready_tx.send(()).unwrap();
                while matches!(
                    stop_rx.recv_timeout(Duration::from_millis(2)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    std::fs::remove_file(&noise).unwrap();
                    std::fs::write(&noise, "unrelated").unwrap();
                }
            });
            ready_rx.recv_timeout(WINDOW).unwrap();
            crate::atomic::write_atomic(&path, b"tile_size = 64\n").unwrap();
            let received = rx.recv_timeout(WINDOW);
            drop(stop_tx);
            writer.join().unwrap();
            received
        });
        assert_eq!(received, Ok(WatchEvent::Changed(vec![path])));
        assert_eq!(reports(&rx, WINDOW), vec![]);
    }

    #[test]
    fn a_fifo_at_a_watched_path_does_not_block_the_watch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("desktop-control.conf");
        let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // Safety: `name` is a NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (done_tx, done_rx) = mpsc::channel();
        let watched = path.clone();
        std::thread::spawn(move || {
            let started = ConfigWatcher::new(vec![watched], |_| ()).map(drop);
            let _ = done_tx.send(started.map_err(|e| e.to_string()));
        });
        assert_eq!(
            done_rx.recv_timeout(WINDOW),
            Ok(Ok(())),
            "a FIFO without a writer reads as empty"
        );
        assert_eq!(snapshot(&path).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn a_rescan_reports_a_change_without_a_path_event() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dock.conf");
        std::fs::write(&path, "tile_size = 48\n").unwrap();
        let mut last = BTreeMap::from([(path.clone(), snapshot(&path).unwrap())]);
        std::fs::write(&path, "tile_size = 64\n").unwrap();
        let (tx, rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        tx.send(Msg::Event(Ok(
            notify::Event::new(notify::EventKind::Other).set_flag(notify::event::Flag::Rescan)
        )))
        .unwrap();
        let received = std::thread::scope(|scope| {
            let files = [path.clone()];
            let worker = scope.spawn(move || {
                let watcher = notify::recommended_watcher(|_| {}).unwrap();
                watch_loop(
                    watcher,
                    &rx,
                    &files,
                    &BTreeSet::new(),
                    &mut last,
                    &mut |event| report_tx.send(event).unwrap(),
                );
            });
            let received = report_rx.recv_timeout(WINDOW);
            tx.send(Msg::Stop).unwrap();
            worker.join().unwrap();
            received
        });
        assert_eq!(received, Ok(WatchEvent::Changed(vec![path])));
    }

    /// The files are read before the watch begins; a change made in between is reported once the watch runs, not
    /// only with the next change.
    #[test]
    fn a_change_before_the_watch_began_is_reported() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("dock.conf");
        std::fs::write(&path, "tile_size = 48\n").unwrap();
        let mut last = BTreeMap::from([(path.clone(), Some(b"tile_size = 64\n".to_vec()))]);
        let (tx, rx) = mpsc::channel();
        tx.send(Msg::Stop).unwrap();
        let watcher = notify::recommended_watcher(|_| {}).unwrap();
        let mut reports = Vec::new();
        watch_loop(
            watcher,
            &rx,
            std::slice::from_ref(&path),
            &BTreeSet::new(),
            &mut last,
            &mut |event| reports.push(event),
        );
        assert_eq!(reports, vec![WatchEvent::Changed(vec![path])]);
    }

    #[test]
    fn opening_and_reading_a_watched_file_is_not_a_change() {
        use notify::event::{
            AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RemoveKind,
        };
        use notify::EventKind;
        for kind in [
            AccessKind::Open(AccessMode::Any),
            AccessKind::Read,
            AccessKind::Close(AccessMode::Read),
        ] {
            assert!(!may_change(&EventKind::Access(kind)), "{kind:?}");
        }
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Remove(RemoveKind::File),
            EventKind::Other,
        ] {
            assert!(may_change(&kind), "{kind:?}");
        }
    }

    #[test]
    fn an_atomic_replacement_is_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let (dock, menubar) = (
            dir.path().join("dock.conf"),
            dir.path().join("menubar.conf"),
        );
        std::fs::write(&dock, "tile_size = 48\n").unwrap();
        let (_w, rx) = watch(vec![dock.clone(), menubar.clone()]);
        crate::atomic::write_atomic(&dock, b"tile_size = 64\n").unwrap();
        assert_eq!(
            reports(&rx, WINDOW),
            vec![WatchEvent::Changed(vec![dock.clone()])]
        );
        // a second change is a second report
        crate::atomic::write_atomic(&dock, b"tile_size = 32\n").unwrap();
        assert_eq!(reports(&rx, WINDOW), vec![WatchEvent::Changed(vec![dock])]);
    }

    /// What a scripted [`Inbox`] plays: a message handed out at once, a quiet span that passes on its clock, or
    /// something done to the files at that point, taking no time.
    enum Step {
        Msg(Msg),
        Quiet(Duration),
        Do(Box<dyn FnOnce()>),
    }

    /// An [`Inbox`] that plays `steps` in order on a clock that moves only through its quiet spans, so a burst is a
    /// burst however slowly the test runs. It ends when the script does.
    struct Script {
        steps: std::cell::RefCell<std::collections::VecDeque<Step>>,
        now: std::cell::Cell<Instant>,
    }

    impl Script {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps: std::cell::RefCell::new(steps.into()),
                now: std::cell::Cell::new(Instant::now()),
            }
        }
    }

    impl Inbox for Script {
        fn recv(&self) -> Option<Msg> {
            loop {
                let step = self.steps.borrow_mut().pop_front()?;
                match step {
                    Step::Msg(msg) => return Some(msg),
                    Step::Quiet(span) => self.now.set(self.now.get() + span),
                    Step::Do(act) => act(),
                }
            }
        }

        fn recv_timeout(&self, timeout: Duration) -> Result<Msg, mpsc::RecvTimeoutError> {
            let mut steps = self.steps.borrow_mut();
            match steps.pop_front() {
                Some(Step::Msg(msg)) => Ok(msg),
                Some(Step::Quiet(span)) if span > timeout => {
                    self.now.set(self.now.get() + timeout);
                    steps.push_front(Step::Quiet(span - timeout));
                    Err(mpsc::RecvTimeoutError::Timeout)
                }
                Some(Step::Quiet(span)) => {
                    self.now.set(self.now.get() + span);
                    drop(steps);
                    self.recv_timeout(timeout - span)
                }
                Some(Step::Do(act)) => {
                    drop(steps);
                    act();
                    self.recv_timeout(timeout)
                }
                None => Err(mpsc::RecvTimeoutError::Disconnected),
            }
        }

        fn now(&self) -> Instant {
            self.now.get()
        }
    }

    fn written(path: &Path) -> Step {
        Step::Msg(Msg::Event(Ok(notify::Event::new(
            notify::EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Any,
            )),
        )
        .add_path(path.to_path_buf()))))
    }

    /// The reports of a watch of `path`, begun as the file is now, playing `steps`.
    fn played(path: &Path, steps: Vec<Step>) -> Vec<WatchEvent> {
        let mut last = BTreeMap::from([(path.to_path_buf(), snapshot(path).unwrap())]);
        let watcher = notify::recommended_watcher(|_| {}).unwrap();
        let mut reports = Vec::new();
        watch_loop(
            watcher,
            &Script::new(steps),
            &[path.to_path_buf()],
            &BTreeSet::new(),
            &mut last,
            &mut |event| reports.push(event),
        );
        reports
    }

    /// A write of `text` to `path` and its event.
    fn write(path: &Path, text: &'static str) -> [Step; 2] {
        let file = path.to_path_buf();
        [
            Step::Do(Box::new(move || std::fs::write(file, text).unwrap())),
            written(path),
        ]
    }

    /// Writes closer together than the quiet time are one burst, reported once when it goes quiet, however long the
    /// burst lasts in all and however slowly the test runs; writes a quiet span apart are a report each.
    #[test]
    fn a_burst_of_writes_to_one_file_is_one_report() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keyboard.conf");
        let gap = || Step::Quiet(COALESCE / 2);
        let quiet = || Step::Quiet(COALESCE * 2);
        let mut burst = Vec::new();
        for (i, text) in [
            "repeat_delay = 1\n",
            "repeat_delay = 2\n",
            "repeat_delay = 3\n",
        ]
        .into_iter()
        .enumerate()
        {
            if i > 0 {
                burst.push(gap());
            }
            burst.extend(write(&path, text));
        }
        burst.push(quiet());
        assert_eq!(
            played(&path, burst),
            vec![WatchEvent::Changed(vec![path.clone()])],
            "written three times, 25 ms apart"
        );
        let mut apart = Vec::new();
        for text in ["repeat_delay = 4\n", "repeat_delay = 5\n"] {
            apart.extend(write(&path, text));
            apart.push(quiet());
        }
        assert_eq!(
            played(&path, apart),
            vec![
                WatchEvent::Changed(vec![path.clone()]),
                WatchEvent::Changed(vec![path])
            ],
            "written twice, 100 ms apart"
        );
    }

    #[test]
    fn unchanged_contents_other_files_and_temporary_files_are_not_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("power.conf");
        std::fs::write(&path, "dim_before_display_sleep = on\n").unwrap();
        let (_w, rx) = watch(vec![path.clone()]);
        crate::atomic::write_atomic(&path, b"dim_before_display_sleep = on\n").unwrap();
        std::fs::write(dir.path().join("other.conf"), "x = 1\n").unwrap();
        std::fs::write(dir.path().join(".power.conf.1.0.tmp"), "half").unwrap();
        assert_eq!(reports(&rx, WINDOW), vec![]);
    }

    #[test]
    fn removal_and_a_file_outside_the_settings_directory_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("nested/redirected/dock.conf");
        let (_w, rx) = watch(vec![elsewhere.clone()]);
        std::fs::write(&elsewhere, "autohide = on\n").unwrap();
        assert_eq!(
            reports(&rx, WINDOW),
            vec![WatchEvent::Changed(vec![elsewhere.clone()])],
            "the directory was created and the new file seen"
        );
        std::fs::remove_file(&elsewhere).unwrap();
        assert_eq!(
            reports(&rx, WINDOW),
            vec![WatchEvent::Changed(vec![elsewhere])]
        );
    }

    #[test]
    fn a_removed_and_recreated_settings_directory_is_watched_again() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("cupertino");
        let path = dir.join("dock.conf");
        let (_w, rx) = watch(vec![path.clone()]);
        std::fs::write(&path, "tile_size = 48\n").unwrap();
        assert_eq!(
            reports(&rx, WINDOW),
            vec![WatchEvent::Changed(vec![path.clone()])]
        );
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            reports(&rx, WINDOW),
            vec![WatchEvent::Changed(vec![path.clone()])],
            "the file went with its directory"
        );
        std::fs::create_dir(&dir).unwrap();
        std::thread::sleep(WINDOW);
        std::fs::write(&path, "tile_size = 64\n").unwrap();
        assert_eq!(
            reports(&rx, WINDOW),
            vec![WatchEvent::Changed(vec![path])],
            "the new directory is watched"
        );
    }

    #[test]
    fn a_panicking_callback_ends_the_watch_without_taking_the_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("menubar.conf");
        let w = ConfigWatcher::new(vec![path.clone()], |_| panic!("the forwarder broke")).unwrap();
        std::fs::write(&path, "clock.show_seconds = on\n").unwrap();
        std::thread::sleep(WINDOW);
        drop(w);
    }
}
