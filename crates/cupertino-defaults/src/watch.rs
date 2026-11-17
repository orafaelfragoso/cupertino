//! Change notification for the settings files: a [`ConfigWatcher`] watches the directory of each file with inotify,
//! so an atomic replacement (temporary file and rename, [`crate::atomic`]), an editor's save and the file's
//! creation or removal are all seen, also for a file an environment variable moved out of the settings directory.
//! Opening and reading a file wakes no watch: the directories are not watched for that.
//!
//! The events of one change come in bursts (create, write and rename of the temporary file; an editor's backup and
//! write). The watcher waits until events touching watched files or their directories are quiet for [`COALESCE`],
//! then reports each watched file whose contents changed since the last report, once. A burst that leaves a file
//! as it was (a `touch`, a write of the same text) reports nothing.
//!
//! The watcher starts no thread: the loop of the process that holds it drives it. A loop that polls file descriptors
//! (the server's calloop) waits for the watcher's descriptor ([`AsFd`]) and for [`ConfigWatcher::quiet_at`], and
//! calls [`ConfigWatcher::read`], which never waits; a loop that polls futures (a Kit program's) runs
//! [`ConfigWatcher::follow`]; a thread of the caller's own waits in [`ConfigWatcher::wait`].

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
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

/// What a [`ConfigWatcher`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchEvent {
    /// These watched files changed (contents, creation or removal), in the order they were given.
    Changed(Vec<PathBuf>),
    /// The file system notification failed; changes may be missed until the process restarts.
    Error(String),
}

/// Watches settings files while it lives.
pub struct ConfigWatcher {
    inbox: Inotified,
    files: Vec<PathBuf>,
    dirs: BTreeSet<PathBuf>,
    /// The contents of each file at the last report.
    last: BTreeMap<PathBuf, Option<Vec<u8>>>,
    /// When the burst under way is quiet, and the files are compared.
    quiet_at: Option<Instant>,
    /// The descriptor [`ConfigWatcher::changes`] waits on, once it has.
    ready: Option<async_io::Async<OwnedFd>>,
}

/// What the kernel's notification says.
#[derive(Debug)]
enum Msg {
    /// `path`, a watched directory or an entry of one, changed; `appeared` when it was created or moved there.
    Changed { path: PathBuf, appeared: bool },
    /// Events were lost (the kernel's queue overflowed): every file is compared.
    Lost,
    /// The notification failed.
    Failed(String),
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
    /// Watch `files`. The directories of the files are created when missing (they are the settings directories).
    /// Their parents are watched too, so a settings directory that is removed and created again is watched again. The
    /// files are read now; a change made before the watches were set is reported by the first read.
    pub fn new(files: Vec<PathBuf>) -> io::Result<Self> {
        let mut last: BTreeMap<PathBuf, Option<Vec<u8>>> = BTreeMap::new();
        for f in &files {
            last.insert(f.clone(), snapshot(f)?);
        }
        let dirs: BTreeSet<PathBuf> = files.iter().map(|f| dir_of(f)).collect();
        for dir in &dirs {
            std::fs::create_dir_all(dir)?;
        }
        let inbox = Inotified::new(masks(&dirs))?;
        Ok(Self {
            inbox,
            files,
            dirs,
            last,
            // the files were read before the watch began: a change in between shows at the first read
            quiet_at: Some(Instant::now()),
            ready: None,
        })
    }

    /// When the burst under way is quiet: [`ConfigWatcher::read`] then reports what it changed. `None` when no burst
    /// is under way, and the next report comes after the descriptor is readable.
    pub fn quiet_at(&self) -> Option<Instant> {
        self.quiet_at
    }

    /// Takes the events the kernel holds, without waiting, and reports each watched file a burst that is quiet by
    /// `now` changed, once, with any failure of the notification. Empty while a burst is under way and when nothing
    /// changed.
    pub fn read(&mut self, now: Instant) -> Vec<WatchEvent> {
        let mut out = Vec::new();
        for msg in self.inbox.take() {
            let touched = match msg {
                Msg::Changed { path, appeared } => {
                    if appeared && self.dirs.contains(&path) {
                        if let Err(e) = self.inbox.add(&path) {
                            out.push(WatchEvent::Error(format!("{}: {e}", path.display())));
                        }
                    }
                    self.files.contains(&path) || self.dirs.contains(&path)
                }
                Msg::Lost => true,
                // a notification error may have lost events: every file is compared
                Msg::Failed(e) => {
                    out.push(WatchEvent::Error(e));
                    true
                }
            };
            if touched {
                self.quiet_at = Some(now + COALESCE);
            }
        }
        if self.quiet_at.is_some_and(|at| at <= now) {
            self.quiet_at = None;
            compare(&self.files, &mut self.last, &mut out);
        }
        out
    }

    /// Waits for the next report and returns it; `None` once the notification failed for good, after its failure was
    /// reported. It blocks: for a thread of the caller's own.
    pub fn wait(&mut self) -> Option<Vec<WatchEvent>> {
        loop {
            let events = self.read(Instant::now());
            if !events.is_empty() {
                return Some(events);
            }
            if self.inbox.broken {
                return None;
            }
            let left = self
                .quiet_at
                .map(|at| at.saturating_duration_since(Instant::now()));
            self.inbox.wait(left);
        }
    }

    /// The next report, waited for without blocking: the future is woken when the descriptor is readable or the burst
    /// is quiet. `None` once the notification failed for good, after its failure was reported.
    pub async fn changes(&mut self) -> Option<Vec<WatchEvent>> {
        loop {
            let events = self.read(Instant::now());
            if !events.is_empty() {
                return Some(events);
            }
            if self.inbox.broken {
                return None;
            }
            if self.ready.is_none() {
                let ready = self
                    .inbox
                    .inotify
                    .as_fd()
                    .try_clone_to_owned()
                    .and_then(async_io::Async::new);
                match ready {
                    Ok(ready) => self.ready = Some(ready),
                    Err(e) => {
                        self.inbox.broken = true;
                        return Some(vec![WatchEvent::Error(format!(
                            "waiting for file events: {e}"
                        ))]);
                    }
                }
            }
            let Some(ready) = &self.ready else {
                return None;
            };
            let readable = async {
                if let Err(e) = ready.readable().await {
                    self.inbox
                        .pending
                        .push_back(Msg::Failed(format!("waiting for file events: {e}")));
                    self.inbox.broken = true;
                }
            };
            match self.quiet_at {
                Some(at) => {
                    futures_lite::future::or(readable, async {
                        async_io::Timer::at(at).await;
                    })
                    .await
                }
                None => readable.await,
            }
        }
    }

    /// Calls `report` with each report, where the future is polled: the future a loop that polls futures runs for as
    /// long as the files are followed. It never ends, so it stands for a task of any output; once the notification
    /// failed for good, after its failure was reported, it only waits.
    pub async fn follow<T>(mut self, mut report: impl FnMut(WatchEvent)) -> T {
        while let Some(events) = self.changes().await {
            for event in events {
                report(event);
            }
        }
        std::future::pending().await
    }
}

/// Readable when the kernel holds file events for [`ConfigWatcher::read`].
impl AsFd for ConfigWatcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inbox.inotify.as_fd()
    }
}

/// The kernel's file notification for the watched directories.
struct Inotified {
    inotify: Inotify,
    masks: BTreeMap<PathBuf, WatchMask>,
    /// The directory each watch descriptor (by its number) watches.
    watching: HashMap<i32, PathBuf>,
    pending: VecDeque<Msg>,
    /// The notification failed; the watch ends once that is told.
    broken: bool,
}

impl Inotified {
    fn new(masks: BTreeMap<PathBuf, WatchMask>) -> io::Result<Self> {
        let mut inbox = Self {
            inotify: Inotify::init()?,
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

    /// Wait up to `timeout` (forever when `None`) for events.
    fn wait(&mut self, timeout: Option<Duration>) {
        let ms = timeout.map_or(-1, |t| {
            t.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32
        });
        let mut fd = libc::pollfd {
            fd: self.inotify.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // Safety: `fd` is an initialised pollfd that outlives the call, and the count says it is one.
        if unsafe { libc::poll(&mut fd, 1, ms) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                self.pending
                    .push_back(Msg::Failed(format!("waiting for file events: {e}")));
                self.broken = true;
            }
        }
    }

    /// What was queued and every event the kernel has, without waiting; nothing more once the notification failed.
    fn take(&mut self) -> VecDeque<Msg> {
        if !self.broken {
            self.read();
        }
        std::mem::take(&mut self.pending)
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

/// Report each of `files` whose contents differ from `last`, and remember them.
fn compare(
    files: &[PathBuf],
    last: &mut BTreeMap<PathBuf, Option<Vec<u8>>>,
    out: &mut Vec<WatchEvent>,
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
            Err(e) => out.push(WatchEvent::Error(format!("{}: {e}", f.display()))),
        }
    }
    if !changed.is_empty() {
        out.push(WatchEvent::Changed(changed));
    }
}
