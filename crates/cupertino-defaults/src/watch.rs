//! Change notification for the settings files: a [`ConfigWatcher`] watches the directory of each file with inotify,
//! so an atomic replacement (temporary file and rename, [`crate::atomic`]), an editor's save and the file's
//! creation or removal are all seen, also for a file an environment variable moved out of the settings directory.
//! Opening and reading a file wakes no watch: the directories are not watched for that.
//!
//! The events of one change come in bursts (create, write and rename of the temporary file; an editor's backup and
//! write). The watcher waits until events touching watched files or their directories are quiet for [`COALESCE`],
//! then reports each watched file whose contents changed since the last report, once. A burst that leaves a file
//! as it was (a `touch`, a write of the same text) reports nothing. Reports go to a callback on the watcher's thread;
//! a process forwards them to its event loop
//! (the server's calloop channel, a shell process's waker).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use inotify::{EventMask, Inotify, WatchMask};

/// How long a burst of file events must be quiet before it is reported. Ours (approximate): long enough for a rename
/// and an editor's save sequence, short enough to feel immediate.
pub const COALESCE: Duration = Duration::from_millis(50);

/// What a settings directory is watched for: its entries being created, written, changed in their attributes, removed
/// or renamed, and the directory itself going. Not opens, reads or closes after reading: they change nothing, and
/// every process that read a file there would wake the watch.
const DIR_EVENTS: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MODIFY)
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::ATTRIB)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/// What the parent of a settings directory is watched for: its entries appearing and going, so a settings directory
/// that is removed and made again is seen. A parent is a busy shared directory (the configuration directory, `/etc`):
/// what its other files go through is none of the watch's business.
const PARENT_EVENTS: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

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
    /// Closing it wakes the watch thread, which then ends.
    _stop: UnixStream,
}

/// What the watch thread receives.
#[derive(Debug)]
enum Msg {
    /// `path`, a watched directory or an entry of one, changed; `appeared` when it was created or moved there.
    Changed {
        path: PathBuf,
        appeared: bool,
    },
    /// Events were lost (the kernel's queue overflowed): every file is compared.
    Lost,
    /// The notification failed.
    Failed(String),
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

/// What each watched directory is watched for: the settings directories for [`DIR_EVENTS`], their parents for
/// [`PARENT_EVENTS`], and a directory that is both for both.
fn masks(dirs: &BTreeSet<PathBuf>) -> BTreeMap<PathBuf, WatchMask> {
    let mut masks: BTreeMap<PathBuf, WatchMask> = BTreeMap::new();
    for dir in dirs {
        *masks.entry(dir.clone()).or_insert(WatchMask::empty()) |= DIR_EVENTS;
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            *masks
                .entry(parent.to_path_buf())
                .or_insert(WatchMask::empty()) |= PARENT_EVENTS;
        }
    }
    masks
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
        for dir in &dirs {
            std::fs::create_dir_all(dir)?;
        }
        let (stop, stopped) = UnixStream::pair()?;
        let mut inbox = Inotified::new(masks(&dirs), stopped)?;
        let mut report = report;
        std::thread::Builder::new().name("cupertino-defaults-watch".into()).spawn(move || {
            let run = std::panic::AssertUnwindSafe(|| watch_loop(&mut inbox, &files, &dirs, &mut last, &mut report));
            if let Err(panic) = std::panic::catch_unwind(run) {
                let what = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
                eprintln!("cupertino-defaults: the settings watch callback panicked ({what}); the settings files are no longer watched");
            }
        })?;
        Ok(Self { _stop: stop })
    }
}

/// Where the watch thread's messages come from, and the clock their quiet is timed on: the kernel's file
/// notification and the monotonic clock, or in tests a script of messages and quiet spans on a clock of its own.
trait Inbox {
    /// The next message, waiting as long as it takes; `None` once no more can come.
    fn recv(&mut self) -> Option<Msg>;
    /// The next message if one comes within `timeout`.
    fn recv_timeout(&mut self, timeout: Duration) -> Result<Msg, mpsc::RecvTimeoutError>;
    fn now(&self) -> Instant;
    /// Watch `dir` again: it was made anew.
    fn watch_again(&mut self, dir: &Path) -> io::Result<()>;
}

/// The kernel's file notification for the watched directories, and the end of a socket whose other end the
/// [`ConfigWatcher`] holds: it closing is [`Msg::Stop`].
struct Inotified {
    inotify: Inotify,
    stop: UnixStream,
    masks: BTreeMap<PathBuf, WatchMask>,
    /// The directory each watch descriptor (by its number) watches.
    watching: HashMap<i32, PathBuf>,
    pending: VecDeque<Msg>,
    /// The notification failed; the watch ends once that is told.
    broken: bool,
}

impl Inotified {
    fn new(masks: BTreeMap<PathBuf, WatchMask>, stop: UnixStream) -> io::Result<Self> {
        let mut inbox = Self {
            inotify: Inotify::init()?,
            stop,
            masks,
            watching: HashMap::new(),
            pending: VecDeque::new(),
            broken: false,
        };
        let dirs: Vec<PathBuf> = inbox.masks.keys().cloned().collect();
        for dir in dirs {
            inbox
                .add(&dir)
                .map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", dir.display())))?;
        }
        Ok(inbox)
    }

    fn add(&mut self, dir: &Path) -> io::Result<()> {
        let mask = self.masks.get(dir).copied().unwrap_or(DIR_EVENTS);
        let wd = self.inotify.watches().add(dir, mask)?;
        self.watching
            .insert(wd.get_watch_descriptor_id(), dir.to_path_buf());
        Ok(())
    }

    /// Wait up to `timeout` (forever when `None`) for events or the stop, and queue what came.
    fn wait(&mut self, timeout: Option<Duration>) {
        let ms = timeout.map_or(-1, |t| {
            t.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32
        });
        let mut fds = [
            libc::pollfd {
                fd: self.inotify.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.stop.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // Safety: `fds` is an array of two initialised pollfd that outlives the call.
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                self.pending
                    .push_back(Msg::Failed(format!("waiting for file events: {e}")));
                self.broken = true;
            }
            return;
        }
        if fds[1].revents != 0 {
            self.pending.push_back(Msg::Stop);
            return;
        }
        if fds[0].revents != 0 {
            self.read();
        }
    }

    /// Queue every event the kernel has.
    fn read(&mut self) {
        let mut buffer = [0u8; 4096];
        loop {
            let events = match self.inotify.read_events(&mut buffer) {
                Ok(events) => events,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.pending
                        .push_back(Msg::Failed(format!("reading file events: {e}")));
                    self.broken = true;
                    return;
                }
            };
            for event in events {
                let id = event.wd.get_watch_descriptor_id();
                if event.mask.contains(EventMask::Q_OVERFLOW) {
                    self.pending.push_back(Msg::Lost);
                } else if event.mask.contains(EventMask::IGNORED) {
                    // the directory went, and its watch with it
                    self.watching.remove(&id);
                } else if let Some(dir) = self.watching.get(&id) {
                    let path = event
                        .name
                        .map_or_else(|| dir.clone(), |name| dir.join(name));
                    let appeared = event
                        .mask
                        .intersects(EventMask::CREATE | EventMask::MOVED_TO);
                    self.pending.push_back(Msg::Changed { path, appeared });
                }
            }
        }
    }
}

impl Inbox for Inotified {
    fn recv(&mut self) -> Option<Msg> {
        loop {
            if let Some(msg) = self.pending.pop_front() {
                return Some(msg);
            }
            if self.broken {
                return None;
            }
            self.wait(None);
        }
    }

    fn recv_timeout(&mut self, timeout: Duration) -> Result<Msg, mpsc::RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(msg) = self.pending.pop_front() {
                return Ok(msg);
            }
            if self.broken {
                return Err(mpsc::RecvTimeoutError::Disconnected);
            }
            let Some(left) = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
            else {
                return Err(mpsc::RecvTimeoutError::Timeout);
            };
            self.wait(Some(left));
        }
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn watch_again(&mut self, dir: &Path) -> io::Result<()> {
        self.add(dir)
    }
}

/// Whether `msg` may have changed a watched file; a settings directory that appears again is watched again. `None`
/// for the stop.
fn handle(
    msg: Msg,
    inbox: &mut impl Inbox,
    watched: &BTreeSet<&PathBuf>,
    dirs: &BTreeSet<PathBuf>,
    report: &mut dyn FnMut(WatchEvent),
) -> Option<bool> {
    match msg {
        Msg::Stop => None,
        Msg::Changed { path, appeared } => {
            if appeared && dirs.contains(&path) {
                if let Err(e) = inbox.watch_again(&path) {
                    report(WatchEvent::Error(format!("{}: {e}", path.display())));
                }
            }
            Some(watched.contains(&path) || dirs.contains(&path))
        }
        Msg::Lost => Some(true),
        // A notification error may have lost events: compare every file.
        Msg::Failed(e) => {
            report(WatchEvent::Error(e));
            Some(true)
        }
    }
}

/// The watch thread: waits for a burst, lets it go quiet, then reports the files whose contents changed.
fn watch_loop(
    rx: &mut impl Inbox,
    files: &[PathBuf],
    dirs: &BTreeSet<PathBuf>,
    last: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
    report: &mut impl FnMut(WatchEvent),
) {
    let watched: BTreeSet<&PathBuf> = files.iter().collect();
    // the files were read before the watch began: a change in between shows now
    compare(files, last, report);
    loop {
        // wait for the first event of a burst, then until the burst is quiet
        let Some(first) = rx.recv() else { return };
        let Some(touched) = handle(first, rx, &watched, dirs, report) else {
            return;
        };
        if !touched {
            continue;
        }
        let mut deadline = rx.now() + COALESCE;
        while let Some(left) = deadline.checked_duration_since(rx.now()) {
            match rx.recv_timeout(left) {
                Ok(msg) => match handle(msg, rx, &watched, dirs, report) {
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
