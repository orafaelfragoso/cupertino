//! A folder's items as a sorted list that follows the folder's changes, read by work the program runs off its UI
//! thread.
//!
//! A [`DirectoryModel`] touches no file. It hands out its blocking work one at a time ([`DirectoryModel::take_work`]),
//! which the program runs on an I/O thread ([`ModelWork::run`], within [`WORK_DEADLINE`]) and which leaves what it read
//! in the model's queue, and its watch ([`DirectoryModel::take_watch`]), a future the program runs on its UI loop
//! ([`ModelWatch::follow`]) that hands the names of changed items to the model. The model calls its [`Notify`]
//! whenever there is something to [`DirectoryModel::poll`], which applies what was read and gives [`ModelDelta`]s.
//!
//! The first work checks that the folder is one, registers the watch, opens the folder and lists it in batches
//! (`ModelOptions::batch_size`, 256 by default: approximate, the design batch size is not known); a later work reads
//! the items a change named, or lists the folder again: a virtual folder that changed, or any folder whose watch lost
//! changes. Ordering is the one of [`crate::sort`]; hidden files (leading dot) are omitted unless `show_hidden` is set
//! (default off, preference default of the design).

use crate::entry::Entry;
use crate::sort::{compare, insertion_index, sort_entries, SortSpec};
use crate::{applications, recents, tags};
use cupertino_apps::MimeDatabase;
use inotify::{EventMask, Inotify, WatchMask, Watches};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

pub const DEFAULT_BATCH: usize = 256;

/// How long one work of a model should take: the listing of a large folder on a local disk, or the items a change
/// named, comes well within it.
pub const WORK_DEADLINE: Duration = Duration::from_secs(10);

/// What the watch hears of in a watched folder: items made, removed, written, given other attributes (tags,
/// permissions) and renamed in or out, and the folder itself removed or moved.
const WATCHED: WatchMask = WatchMask::CREATE
    .union(WatchMask::DELETE)
    .union(WatchMask::MODIFY)
    .union(WatchMask::ATTRIB)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/// The bytes the watch reads the kernel's events into at a time: a few dozen events with long names.
const EVENT_BUFFER: usize = 4096;

/// Called by the model's work and watch after they queued something, so a consumer that waits on something else (a
/// display connection) knows to [`DirectoryModel::poll`].
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug)]
pub struct ModelOptions {
    pub batch_size: usize,
    pub show_hidden: bool,
    pub sort: SortSpec,
    pub watch: bool,
    /// Read the `user.xdg.tags` attribute of every entry (one extra syscall per file).
    pub read_tags: bool,
    /// The shared MIME database that types every entry by its name, as the icons are chosen (`MimeDatabase::file_type`), so the
    /// Kind column, kind sorting and grouping agree with the icon. `None` leaves the type to the extension guess of
    /// [`crate::mime::entry_mime`] (tools and tests without the session's data directories).
    pub types: Option<Arc<MimeDatabase>>,
}

impl Default for ModelOptions {
    fn default() -> Self {
        ModelOptions {
            batch_size: DEFAULT_BATCH,
            show_hidden: false,
            sort: SortSpec::default(),
            watch: true,
            read_tags: false,
            types: None,
        }
    }
}

/// What changed in the visible list; indexes refer to the list after the change was applied unless noted.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelDelta {
    /// The folder is a folder, is watched and can be read: the first update of a model that opens.
    Opened,
    /// The folder could not be opened (it is missing, is not a folder, gives no read permission, cannot be
    /// reached): the model's first and only update, with the error's kind and its text.
    OpenFailed {
        kind: io::ErrorKind,
        error: String,
    },
    /// Enumeration delivered a batch; the whole list was re-merged (indexes of older items may shift).
    Reloaded {
        count: usize,
    },
    /// Enumeration finished.
    LoadComplete {
        count: usize,
    },
    Inserted {
        index: usize,
    },
    /// `index` refers to the list before removal.
    Removed {
        index: usize,
    },
    /// The entry at `index` changed in place.
    Changed {
        index: usize,
    },
    /// The entry moved (its sort key changed or it was renamed).
    Moved {
        from: usize,
        to: usize,
    },
    /// The directory itself disappeared.
    DirectoryGone,
    /// A read that failed once the folder was open.
    Error(String),
}

/// What a model's work and watch leave for [`DirectoryModel::poll`].
enum Msg {
    Opened,
    OpenFailed(io::ErrorKind, String),
    Batch(Vec<Entry>),
    /// The whole listing again (a folder listed again).
    Replace(Vec<Entry>),
    Done,
    Error(String),
    /// The items a change named, read again: each name with its item, or none when it is gone.
    Read(Vec<(String, Option<Entry>)>),
    /// The folder itself is gone.
    Gone,
    /// The watch heard of changes to `paths`, and to the watched folders `itself`; `overflowed` when the kernel's
    /// queue overflowed and changes were lost.
    Changed {
        paths: Vec<PathBuf>,
        itself: Vec<PathBuf>,
        overflowed: bool,
    },
}

/// What a model shares with its work and its watch.
struct Shared {
    state: Mutex<State>,
}

struct State {
    queue: VecDeque<Msg>,
    notify: Notify,
    /// A work of the model is out.
    working: bool,
    /// The model is gone: a listing stops at its next batch.
    dropped: bool,
    /// The watch's descriptor, once the watch is taken: what the first work registers the folders with.
    watches: Option<Watches>,
    /// The folder each watch descriptor (by its number) watches.
    watched: HashMap<i32, PathBuf>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue `msg`, then tell the consumer.
    fn send(&self, msg: Msg) {
        let notify = {
            let mut state = self.lock();
            state.queue.push_back(msg);
            Arc::clone(&state.notify)
        };
        notify();
    }

    fn dropped(&self) -> bool {
        self.lock().dropped
    }
}

/// What a model lists.
#[derive(Clone)]
enum Source {
    /// A folder of the file system.
    Folder,
    /// The Recents folder of this bookmark file.
    Recents(PathBuf),
    /// The Applications folder of these application folders, most important first.
    Applications(Vec<PathBuf>),
}

/// A work asked for and not handed out yet.
enum Job {
    /// The first: check, watch, open and list.
    Open,
    /// Read the items of these names again; check the folder itself too when `itself`.
    Read {
        names: BTreeSet<String>,
        itself: bool,
    },
    /// List the folder again.
    Relist,
}

pub struct DirectoryModel {
    dir: PathBuf,
    source: Source,
    opts: ModelOptions,
    entries: Vec<Entry>,
    loading: bool,
    /// Whether the folder opened, once a poll told.
    opened: Option<bool>,
    shared: Arc<Shared>,
    /// The works asked for and not handed out yet, in order.
    asked: VecDeque<Job>,
    watch_taken: bool,
    /// Closed as the model drops, which ends its watch's [`ModelWatch::follow`].
    _alive: async_channel::Sender<()>,
    closed: async_channel::Receiver<()>,
}

fn stat_entry(path: PathBuf, read_tags: bool, types: Option<&MimeDatabase>) -> io::Result<Entry> {
    let mut e = typed(Entry::from_path(path)?, types);
    if read_tags {
        e.tags = tags::read_tags(&e.path).unwrap_or_default();
    }
    Ok(e)
}

/// `e` with its MIME type from the shared database `types`, when there is one.
fn typed(mut e: Entry, types: Option<&MimeDatabase>) -> Entry {
    if let Some(db) = types {
        e.mime = Some(db.file_type(&e.path, e.is_dir).to_string());
    }
    e
}

/// Enumerate `dir` synchronously (all entries, sorted, hidden entries included when requested).
pub fn enumerate_blocking(dir: &Path, opts: &ModelOptions) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for de in std::fs::read_dir(dir)? {
        let de = de?;
        if !opts.show_hidden && de.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if let Ok(e) = stat_entry(de.path(), opts.read_tags, opts.types.as_deref()) {
            out.push(e);
        }
    }
    sort_entries(&mut out, &opts.sort);
    Ok(out)
}

fn merge_sorted(a: Vec<Entry>, b: Vec<Entry>, spec: &SortSpec) -> Vec<Entry> {
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut ia, mut ib) = (a.into_iter().peekable(), b.into_iter().peekable());
    loop {
        match (ia.peek(), ib.peek()) {
            (Some(x), Some(y)) => {
                if compare(x, y, spec) != std::cmp::Ordering::Greater {
                    out.push(ia.next().unwrap());
                } else {
                    out.push(ib.next().unwrap());
                }
            }
            (Some(_), None) => out.push(ia.next().unwrap()),
            (None, Some(_)) => out.push(ib.next().unwrap()),
            (None, None) => break,
        }
    }
    out
}

impl DirectoryModel {
    /// A model of `dir`. It touches no file: `notify` wakes the program whenever there is something to `poll`.
    pub fn open(dir: impl Into<PathBuf>, opts: ModelOptions, notify: Notify) -> DirectoryModel {
        Self::start(dir.into(), Source::Folder, opts, notify)
    }

    /// The Recents folder ([`crate::recents`]) of the bookmark file `xbel`: listed in one batch, and listed again
    /// whenever the file is rewritten when `opts.watch` is set (the watch is on its folder, which must exist). Its
    /// [`DirectoryModel::dir`] is [`recents::location`].
    pub fn open_recents(xbel: PathBuf, opts: ModelOptions, notify: Notify) -> DirectoryModel {
        let dir = recents::location().to_path_buf();
        Self::start(dir, Source::Recents(xbel), opts, notify)
    }

    /// The Applications folder ([`crate::applications`]) of the application folders `dirs`, most important first:
    /// listed in one batch, and listed again whenever an entry in one of them changes when `opts.watch` is set. Its
    /// [`DirectoryModel::dir`] is [`applications::location`].
    pub fn open_applications(
        dirs: Vec<PathBuf>,
        opts: ModelOptions,
        notify: Notify,
    ) -> DirectoryModel {
        let dir = applications::location().to_path_buf();
        Self::start(dir, Source::Applications(dirs), opts, notify)
    }

    fn start(dir: PathBuf, source: Source, opts: ModelOptions, notify: Notify) -> DirectoryModel {
        let (alive, closed) = async_channel::bounded(1);
        DirectoryModel {
            dir,
            source,
            opts,
            entries: Vec::new(),
            loading: true,
            opened: None,
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queue: VecDeque::new(),
                    notify,
                    working: false,
                    dropped: false,
                    watches: None,
                    watched: HashMap::new(),
                }),
            }),
            asked: VecDeque::from([Job::Open]),
            watch_taken: false,
            _alive: alive,
            closed,
        }
    }

    /// The model's next blocking work; `None` while one of its works is out, and when none waits. A model runs one
    /// work at a time, in the order asked: a change is applied on top of the one before it. The first work checks
    /// that the folder is one, registers its watch, opens it and then lists it in batches (for Applications it walks
    /// the application folders and registers each); a later one reads the items a change named, or lists the folder
    /// again (a virtual folder that changed, or any folder whose watch lost changes).
    pub fn take_work(&mut self) -> Option<ModelWork> {
        let mut state = self.shared.lock();
        if state.working {
            return None;
        }
        let job = self.asked.pop_front()?;
        state.working = true;
        drop(state);
        Some(ModelWork {
            shared: Arc::clone(&self.shared),
            dir: self.dir.clone(),
            source: self.source.clone(),
            opts: self.opts.clone(),
            job,
        })
    }

    /// The folder's watch, once; `None` without `opts.watch`. Its descriptor is made here and holds no folder until
    /// the first work registers one, so it is taken before the first work runs.
    pub fn take_watch(&mut self) -> Option<ModelWatch> {
        if !self.opts.watch || self.watch_taken {
            return None;
        }
        self.watch_taken = true;
        let inotify = match Inotify::init() {
            Ok(inotify) => inotify,
            Err(e) => {
                let error = format!("{}: cannot watch for changes: {e}", self.dir.display());
                self.shared.send(Msg::Error(error));
                return None;
            }
        };
        self.shared.lock().watches = Some(inotify.watches());
        Some(ModelWatch {
            inotify,
            shared: Arc::clone(&self.shared),
            closed: self.closed.clone(),
        })
    }

    /// Whether a work of the model is out: taken and not ended.
    pub fn is_working(&self) -> bool {
        self.shared.lock().working
    }

    /// Whether the folder opened: `Some(true)` once a poll gave [`ModelDelta::Opened`], `Some(false)` once it gave
    /// [`ModelDelta::OpenFailed`], `None` before.
    pub fn opened(&self) -> Option<bool> {
        self.opened
    }

    /// Calls `notify` from now on whenever there is something to poll, in place of the one the model was opened with:
    /// another consumer takes the model.
    pub fn set_notify(&mut self, notify: Notify) {
        self.shared.lock().notify = notify;
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn is_loading(&self) -> bool {
        self.loading
    }
    pub fn options(&self) -> &ModelOptions {
        &self.opts
    }

    pub fn index_of_name(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name == name)
    }

    /// Change the order; the list is re-sorted in place.
    pub fn set_sort(&mut self, spec: SortSpec) {
        self.opts.sort = spec;
        sort_entries(&mut self.entries, &spec);
    }

    /// Apply what the work and the watch queued, without blocking.
    pub fn poll(&mut self) -> Vec<ModelDelta> {
        let queue = std::mem::take(&mut self.shared.lock().queue);
        let mut out = Vec::new();
        for m in queue {
            self.apply(m, &mut out);
        }
        out
    }

    fn apply(&mut self, m: Msg, out: &mut Vec<ModelDelta>) {
        match m {
            Msg::Opened => {
                self.opened = Some(true);
                out.push(ModelDelta::Opened);
            }
            Msg::OpenFailed(kind, error) => {
                self.opened = Some(false);
                self.loading = false;
                self.asked.clear();
                out.push(ModelDelta::OpenFailed { kind, error });
            }
            Msg::Batch(mut b) => {
                // a change read during the enumeration may have listed an item already
                if !self.entries.is_empty() {
                    let listed: std::collections::HashSet<&str> =
                        self.entries.iter().map(|e| e.name.as_str()).collect();
                    b.retain(|e| !listed.contains(e.name.as_str()));
                }
                let old = std::mem::take(&mut self.entries);
                self.entries = if old.is_empty() {
                    b
                } else {
                    merge_sorted(old, b, &self.opts.sort)
                };
                out.push(ModelDelta::Reloaded {
                    count: self.entries.len(),
                });
            }
            Msg::Replace(mut entries) => {
                sort_entries(&mut entries, &self.opts.sort);
                self.entries = entries;
                out.push(ModelDelta::Reloaded {
                    count: self.entries.len(),
                });
            }
            Msg::Done => {
                self.loading = false;
                out.push(ModelDelta::LoadComplete {
                    count: self.entries.len(),
                });
            }
            Msg::Error(e) => out.push(ModelDelta::Error(e)),
            Msg::Read(items) => {
                for (name, entry) in items {
                    match entry {
                        Some(entry) => self.upsert(entry, out),
                        None => self.remove_name(&name, out),
                    }
                }
            }
            Msg::Gone => out.push(ModelDelta::DirectoryGone),
            Msg::Changed {
                paths,
                itself,
                overflowed,
            } => self.changed(&paths, !itself.is_empty(), overflowed),
        }
    }

    /// The watch heard of changes to `paths`, and to a watched folder itself when `itself`: the work that reads them
    /// is asked for. When `overflowed`, changes were lost, and the folder is listed again.
    fn changed(&mut self, paths: &[PathBuf], itself: bool, overflowed: bool) {
        if self.opened == Some(false) {
            return;
        }
        if overflowed {
            return self.relist();
        }
        match &self.source {
            Source::Folder => {
                let names: BTreeSet<String> = paths
                    .iter()
                    .filter(|p| p.parent() == Some(self.dir.as_path()))
                    .filter_map(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| self.visible(n))
                    .collect();
                if names.is_empty() && !itself {
                    return;
                }
                match self.asked.back_mut() {
                    Some(Job::Read {
                        names: asked,
                        itself: asked_itself,
                    }) => {
                        asked.extend(names);
                        *asked_itself |= itself;
                    }
                    _ => self.asked.push_back(Job::Read { names, itself }),
                }
            }
            Source::Recents(xbel) => {
                if paths.contains(xbel) {
                    self.relist();
                }
            }
            Source::Applications(_) => self.relist(),
        }
    }

    /// Ask for the folder to be listed again, once for every change heard before that work is taken.
    fn relist(&mut self) {
        if !matches!(self.asked.back(), Some(Job::Relist)) {
            self.asked.push_back(Job::Relist);
        }
    }

    fn visible(&self, name: &str) -> bool {
        self.opts.show_hidden || !name.starts_with('.')
    }

    fn upsert(&mut self, new: Entry, out: &mut Vec<ModelDelta>) {
        match self.entries.iter().position(|e| e.name == new.name) {
            Some(i) => {
                let old = self.entries.remove(i);
                let j = insertion_index(&self.entries, &new, &self.opts.sort);
                let changed = old.size != new.size
                    || old.modified != new.modified
                    || old.kind != new.kind
                    || old.tags != new.tags;
                self.entries.insert(j, new);
                if j != i {
                    out.push(ModelDelta::Moved { from: i, to: j });
                } else if changed {
                    out.push(ModelDelta::Changed { index: i });
                }
            }
            None => {
                let j = insertion_index(&self.entries, &new, &self.opts.sort);
                self.entries.insert(j, new);
                out.push(ModelDelta::Inserted { index: j });
            }
        }
    }

    fn remove_name(&mut self, name: &str, out: &mut Vec<ModelDelta>) {
        if let Some(i) = self.entries.iter().position(|e| e.name == name) {
            self.entries.remove(i);
            out.push(ModelDelta::Removed { index: i });
        }
    }
}

impl Drop for DirectoryModel {
    fn drop(&mut self) {
        self.shared.lock().dropped = true;
    }
}

/// A model's blocking work, which the program runs off its UI thread ([`DirectoryModel::take_work`]).
pub struct ModelWork {
    shared: Arc<Shared>,
    dir: PathBuf,
    source: Source,
    opts: ModelOptions,
    job: Job,
}

impl ModelWork {
    /// Blocking. What it read is in the model's queue when it returns. It catches a panic of the work, so the panic is
    /// not the program's: the first work then leaves [`ModelDelta::OpenFailed`], a later one [`ModelDelta::Error`].
    pub fn run(self) {
        let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match (&self.job, &self.source) {
                (Job::Open, Source::Folder) => self.open_folder(),
                (Job::Open, Source::Recents(xbel)) => self.open_recents(xbel),
                (Job::Open, Source::Applications(dirs)) => self.open_applications(dirs),
                (Job::Read { names, itself }, _) => self.read(names, *itself),
                (Job::Relist, _) => self.relist(),
            }
        }));
        if ran.is_err() {
            let error = format!("{}: the folder could not be read", self.dir.display());
            self.shared.send(match self.job {
                Job::Open => Msg::OpenFailed(io::ErrorKind::Other, error),
                Job::Read { .. } | Job::Relist => Msg::Error(error),
            });
        }
    }

    fn failed(&self, e: io::Error) {
        self.shared.send(Msg::OpenFailed(e.kind(), e.to_string()));
    }

    /// Register `dir` with the watch, when the watch was taken.
    fn watch(&self, dir: &Path) -> io::Result<()> {
        let Some(mut watches) = self.shared.lock().watches.clone() else {
            return Ok(());
        };
        let wd = watches.add(dir, WATCHED)?;
        self.shared
            .lock()
            .watched
            .insert(wd.get_watch_descriptor_id(), dir.to_path_buf());
        Ok(())
    }

    fn open_folder(&self) {
        match std::fs::metadata(&self.dir) {
            Ok(md) if md.is_dir() => {}
            Ok(_) => {
                return self.failed(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    "not a directory",
                ))
            }
            Err(e) => return self.failed(e),
        }
        // watch first so that nothing created during enumeration is missed
        if let Err(e) = self.watch(&self.dir) {
            return self.failed(e);
        }
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) => return self.failed(e),
        };
        self.shared.send(Msg::Opened);
        let batch = self.opts.batch_size.max(1);
        let types = self.opts.types.as_deref();
        let mut cur = Vec::with_capacity(batch);
        for de in rd.flatten() {
            if !self.opts.show_hidden && de.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if let Ok(e) = stat_entry(de.path(), self.opts.read_tags, types) {
                cur.push(e);
            }
            if cur.len() >= batch {
                if self.shared.dropped() {
                    return;
                }
                sort_entries(&mut cur, &self.opts.sort);
                self.shared.send(Msg::Batch(std::mem::take(&mut cur)));
            }
        }
        if !cur.is_empty() {
            sort_entries(&mut cur, &self.opts.sort);
            self.shared.send(Msg::Batch(cur));
        }
        self.shared.send(Msg::Done);
    }

    fn open_recents(&self, xbel: &Path) {
        // the file is replaced by a rename, so its folder is watched
        let watched = match xbel.parent() {
            Some(folder) => self.watch(folder),
            None if self.shared.lock().watches.is_some() => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the bookmark file has no folder",
            )),
            None => Ok(()),
        };
        if let Err(e) = watched {
            return self.failed(e);
        }
        self.shared.send(Msg::Opened);
        match recents::recent_entries(xbel) {
            Ok(entries) => {
                let mut entries = self.typed(entries);
                sort_entries(&mut entries, &self.opts.sort);
                self.shared.send(Msg::Batch(entries));
            }
            Err(e) => self
                .shared
                .send(Msg::Error(format!("{}: {e}", xbel.display()))),
        }
        self.shared.send(Msg::Done);
    }

    fn open_applications(&self, dirs: &[PathBuf]) {
        // a folder made later (the user's own, at a first install) is not watched until the place is shown again
        for dir in dirs.iter().filter(|d| d.is_dir()) {
            if let Err(e) = self.watch_tree(dir) {
                return self.failed(e);
            }
        }
        self.shared.send(Msg::Opened);
        let mut entries = applications::session_entries(dirs);
        sort_entries(&mut entries, &self.opts.sort);
        self.shared.send(Msg::Batch(entries));
        self.shared.send(Msg::Done);
    }

    /// Register `dir` and the folders under it with the watch; a folder under it that cannot be watched is passed
    /// over.
    fn watch_tree(&self, dir: &Path) -> io::Result<()> {
        self.watch(dir)?;
        if self.shared.lock().watches.is_none() {
            return Ok(());
        }
        let mut folders: Vec<PathBuf> = Vec::new();
        let mut next = vec![dir.to_path_buf()];
        while let Some(folder) = next.pop() {
            let Ok(rd) = std::fs::read_dir(&folder) else {
                continue;
            };
            for de in rd.flatten() {
                if de.file_type().is_ok_and(|t| t.is_dir()) {
                    folders.push(de.path());
                    next.push(de.path());
                }
            }
        }
        for folder in folders {
            let _ = self.watch(&folder);
        }
        Ok(())
    }

    fn typed(&self, entries: Vec<Entry>) -> Vec<Entry> {
        let types = self.opts.types.as_deref();
        entries.into_iter().map(|e| typed(e, types)).collect()
    }

    fn read(&self, names: &BTreeSet<String>, itself: bool) {
        let types = self.opts.types.as_deref();
        let items: Vec<(String, Option<Entry>)> = names
            .iter()
            .map(|name| {
                let entry = stat_entry(self.dir.join(name), self.opts.read_tags, types).ok();
                (name.clone(), entry)
            })
            .collect();
        if !items.is_empty() {
            self.shared.send(Msg::Read(items));
        }
        if itself && !self.dir.is_dir() {
            self.shared.send(Msg::Gone);
        }
    }

    fn relist(&self) {
        match &self.source {
            Source::Folder => match enumerate_blocking(&self.dir, &self.opts) {
                Ok(entries) => self.shared.send(Msg::Replace(entries)),
                Err(_) if !self.dir.is_dir() => self.shared.send(Msg::Gone),
                Err(e) => self
                    .shared
                    .send(Msg::Error(format!("{}: {e}", self.dir.display()))),
            },
            Source::Recents(xbel) => match recents::recent_entries(xbel) {
                Ok(entries) => self.shared.send(Msg::Replace(self.typed(entries))),
                Err(e) => self
                    .shared
                    .send(Msg::Error(format!("{}: {e}", xbel.display()))),
            },
            Source::Applications(dirs) => {
                self.shared
                    .send(Msg::Replace(applications::session_entries(dirs)));
            }
        }
    }
}

impl Drop for ModelWork {
    /// The work ended, run or not: the model may hand out its next.
    fn drop(&mut self) {
        let notify = {
            let mut state = self.shared.lock();
            state.working = false;
            Arc::clone(&state.notify)
        };
        notify();
    }
}

/// A model's watch, which the program follows on its UI loop ([`DirectoryModel::take_watch`]).
pub struct ModelWatch {
    inotify: Inotify,
    shared: Arc<Shared>,
    closed: async_channel::Receiver<()>,
}

/// What ends a wait of the watch.
enum Woke {
    /// The kernel has events.
    Events,
    /// The model is gone.
    Closed,
    /// Waiting on the descriptor failed.
    Failed(io::Error),
}

impl ModelWatch {
    /// Waits on the descriptor, hands each burst of changed names to the model and wakes it. It touches no file. It
    /// ends as soon as its model is dropped, with no event needed, and closes the descriptor.
    pub fn follow(self) -> impl Future<Output = ()> + Send + 'static {
        let ModelWatch {
            mut inotify,
            shared,
            closed,
        } = self;
        async move {
            match inotify
                .as_fd()
                .try_clone_to_owned()
                .and_then(async_io::Async::new)
            {
                Ok(ready) => {
                    let mut buffer = [0u8; EVENT_BUFFER];
                    loop {
                        let gone = async {
                            let _ = closed.recv().await;
                            Woke::Closed
                        };
                        let readable = async {
                            match ready.readable().await {
                                Ok(()) => Woke::Events,
                                Err(e) => Woke::Failed(e),
                            }
                        };
                        let failed = match futures_lite::future::or(gone, readable).await {
                            Woke::Events => match take_events(&mut inotify, &mut buffer, &shared) {
                                Ok(()) => continue,
                                Err(e) => e,
                            },
                            Woke::Closed => break,
                            Woke::Failed(e) => e,
                        };
                        if !shared.dropped() {
                            shared.send(Msg::Error(format!(
                                "the folder's changes are no longer followed: {failed}"
                            )));
                        }
                        break;
                    }
                }
                Err(e) => shared.send(Msg::Error(format!(
                    "the folder's changes cannot be followed: {e}"
                ))),
            }
            shared.lock().watches = None;
        }
    }
}

/// Hand every event the kernel has for the watch to the model as one change, and wake it.
fn take_events(inotify: &mut Inotify, buffer: &mut [u8], shared: &Shared) -> io::Result<()> {
    let (mut paths, mut itself, mut overflowed) = (Vec::new(), Vec::new(), false);
    loop {
        let events = match inotify.read_events(buffer) {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let mut state = shared.lock();
        for event in events {
            if event.mask.contains(EventMask::Q_OVERFLOW) {
                // the kernel's queue was full: the events after it are lost, whichever folder they were of
                overflowed = true;
                continue;
            }
            let id = event.wd.get_watch_descriptor_id();
            if event.mask.contains(EventMask::IGNORED) {
                // the folder went, and its watch with it
                state.watched.remove(&id);
                continue;
            }
            let Some(dir) = state.watched.get(&id) else {
                continue;
            };
            match event.name {
                Some(name) => paths.push(dir.join(name)),
                None => itself.push(dir.clone()),
            }
        }
    }
    if overflowed || !paths.is_empty() || !itself.is_empty() {
        shared.send(Msg::Changed {
            paths,
            itself,
            overflowed,
        });
    }
    Ok(())
}
