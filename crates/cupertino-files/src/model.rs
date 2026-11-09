//! Directory model: batched background enumeration, inotify watching (through `notify`), a sorted
//! visible list and fine-grained change notifications for views.
//!
//! Enumeration runs on a worker thread and delivers entries in batches (`ModelOptions::batch_size`,
//! 256 by default: approximate, the design batch size is not known). The consumer calls
//! [`DirectoryModel::poll`] from its main loop and receives [`ModelDelta`]s.
//!
//! Ordering is the one of [`crate::sort`]; hidden files (leading dot) are omitted unless
//! `show_hidden` is set (default off, preference default of the design).

use crate::entry::Entry;
use crate::recents;
use crate::sort::{compare, insertion_index, sort_entries, SortSpec};
use cupertino_apps::MimeDatabase;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

pub const DEFAULT_BATCH: usize = 256;

/// Called from the enumeration and watcher threads after they queued a message, so a consumer that
/// blocks on something else (a display connection) knows to [`DirectoryModel::poll`].
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
    Error(String),
}

enum Msg {
    Batch(Vec<Entry>),
    /// The whole listing again (the recently-used list was rewritten).
    Replace(Vec<Entry>),
    Done,
    Error(String),
    Fs(notify::Event),
    FsError(String),
}

/// The sending end of the model's threads: queues a message, then tells the consumer.
#[derive(Clone)]
struct Sink {
    tx: Sender<Msg>,
    on_message: Option<Notify>,
}

impl Sink {
    /// Queue `m`; `false` once the model is gone.
    fn send(&self, m: Msg) -> bool {
        let sent = self.tx.send(m).is_ok();
        if let (true, Some(notify)) = (sent, &self.on_message) {
            notify();
        }
        sent
    }
}

pub struct DirectoryModel {
    dir: PathBuf,
    opts: ModelOptions,
    entries: Vec<Entry>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    worker: Option<JoinHandle<()>>,
    watcher: Option<RecommendedWatcher>,
    loading: bool,
}

fn stat_entry(path: PathBuf, read_tags: bool, types: Option<&MimeDatabase>) -> io::Result<Entry> {
    let mut e = typed(Entry::from_path(path)?, types);
    if read_tags {
        e.tags = crate::tags::read_tags(&e.path).unwrap_or_default();
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
    /// Open `dir` for a consumer that polls on its own schedule.
    pub fn open(dir: impl Into<PathBuf>, opts: ModelOptions) -> io::Result<Self> {
        Self::start(dir.into(), opts, None)
    }

    /// Open `dir`; `notify` runs on the model's threads whenever there is something to [`DirectoryModel::poll`].
    pub fn open_notifying(
        dir: impl Into<PathBuf>,
        opts: ModelOptions,
        notify: Notify,
    ) -> io::Result<Self> {
        Self::start(dir.into(), opts, Some(notify))
    }

    /// The Recents folder ([`crate::recents`]) of the bookmark file `xbel`: listed in one batch, and listed again
    /// whenever the file is rewritten when `opts.watch` is set. Its [`DirectoryModel::dir`] is [`recents::RECENTS`].
    pub fn open_recents(xbel: PathBuf, opts: ModelOptions, notify: Notify) -> io::Result<Self> {
        let (tx, rx) = channel();
        let mut model = DirectoryModel {
            dir: recents::location().to_path_buf(),
            opts: opts.clone(),
            entries: Vec::new(),
            rx,
            tx: tx.clone(),
            worker: None,
            watcher: None,
            loading: true,
        };
        let sink = Sink {
            tx,
            on_message: Some(notify),
        };
        if opts.watch {
            let folder = xbel
                .parent()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "the bookmark file has no folder",
                    )
                })?
                .to_path_buf();
            let (wsink, file, types) = (sink.clone(), xbel.clone(), opts.types.clone());
            // the file is replaced by a rename, so its folder is watched
            let mut w =
                notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
                    Ok(ev)
                        if ev.paths.contains(&file) && !matches!(ev.kind, EventKind::Access(_)) =>
                    {
                        wsink.send(match recents::recent_entries(&file) {
                            Ok(entries) => Msg::Replace(
                                entries
                                    .into_iter()
                                    .map(|e| typed(e, types.as_deref()))
                                    .collect(),
                            ),
                            Err(e) => Msg::Error(format!("{}: {e}", file.display())),
                        });
                    }
                    Ok(_) => {}
                    Err(e) => {
                        wsink.send(Msg::FsError(e.to_string()));
                    }
                })
                .map_err(io::Error::other)?;
            w.watch(&folder, RecursiveMode::NonRecursive)
                .map_err(io::Error::other)?;
            model.watcher = Some(w);
        }
        let (spec, types) = (opts.sort, opts.types.clone());
        model.worker = Some(std::thread::spawn(move || {
            match recents::recent_entries(&xbel) {
                Ok(entries) => {
                    let mut entries: Vec<Entry> = entries
                        .into_iter()
                        .map(|e| typed(e, types.as_deref()))
                        .collect();
                    sort_entries(&mut entries, &spec);
                    sink.send(Msg::Batch(entries));
                }
                Err(e) => {
                    sink.send(Msg::Error(format!("{}: {e}", xbel.display())));
                }
            }
            sink.send(Msg::Done);
        }));
        Ok(model)
    }

    fn start(dir: PathBuf, opts: ModelOptions, on_message: Option<Notify>) -> io::Result<Self> {
        let md = std::fs::metadata(&dir)?;
        if !md.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "not a directory",
            ));
        }
        let (tx, rx) = channel();
        let mut model = DirectoryModel {
            dir: dir.clone(),
            opts: opts.clone(),
            entries: Vec::new(),
            rx,
            tx: tx.clone(),
            worker: None,
            watcher: None,
            loading: true,
        };
        let sink = Sink { tx, on_message };
        // watch first so that nothing created during enumeration is missed
        if opts.watch {
            let wsink = sink.clone();
            let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                // a send fails only once the model is gone, and then nobody is left to tell
                wsink.send(match res {
                    Ok(ev) => Msg::Fs(ev),
                    Err(e) => Msg::FsError(e.to_string()),
                });
            })
            .map_err(io::Error::other)?;
            w.watch(&dir, RecursiveMode::NonRecursive)
                .map_err(io::Error::other)?;
            model.watcher = Some(w);
        }
        let batch = opts.batch_size.max(1);
        let spec = opts.sort;
        let show_hidden = opts.show_hidden;
        let read_tags = opts.read_tags;
        let types = opts.types.clone();
        let wdir = dir.clone();
        model.worker = Some(std::thread::spawn(move || {
            let rd = match std::fs::read_dir(&wdir) {
                Ok(r) => r,
                Err(e) => {
                    sink.send(Msg::Error(e.to_string()));
                    return;
                }
            };
            let mut cur = Vec::with_capacity(batch);
            for de in rd.flatten() {
                if !show_hidden && de.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                if let Ok(e) = stat_entry(de.path(), read_tags, types.as_deref()) {
                    cur.push(e);
                }
                if cur.len() >= batch {
                    sort_entries(&mut cur, &spec);
                    if !sink.send(Msg::Batch(std::mem::take(&mut cur))) {
                        return;
                    }
                }
            }
            if !cur.is_empty() {
                sort_entries(&mut cur, &spec);
                sink.send(Msg::Batch(cur));
            }
            sink.send(Msg::Done);
        }));
        Ok(model)
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

    /// Process pending worker and watcher messages without blocking.
    pub fn poll(&mut self) -> Vec<ModelDelta> {
        let mut out = Vec::new();
        while let Ok(m) = self.rx.try_recv() {
            self.apply(m, &mut out);
        }
        out
    }

    /// Block up to `timeout` for the next message, then drain the queue.
    pub fn poll_timeout(&mut self, timeout: std::time::Duration) -> Vec<ModelDelta> {
        let mut out = Vec::new();
        if let Ok(m) = self.rx.recv_timeout(timeout) {
            self.apply(m, &mut out);
        }
        out.extend(self.poll());
        out
    }

    /// Block until the enumeration is complete (tests, command line tools).
    pub fn wait_loaded(&mut self) -> Vec<ModelDelta> {
        let mut out = Vec::new();
        while self.loading {
            out.extend(self.poll_timeout(std::time::Duration::from_millis(50)));
        }
        out
    }

    fn apply(&mut self, m: Msg, out: &mut Vec<ModelDelta>) {
        match m {
            Msg::Batch(mut b) => {
                // the watcher runs during the enumeration: an item it listed already is not listed twice
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
            Msg::Error(e) | Msg::FsError(e) => out.push(ModelDelta::Error(e)),
            Msg::Fs(ev) => self.apply_fs(ev, out),
        }
    }

    fn visible(&self, name: &str) -> bool {
        self.opts.show_hidden || !name.starts_with('.')
    }

    fn upsert(&mut self, path: &Path, out: &mut Vec<ModelDelta>) {
        if path.parent() != Some(self.dir.as_path()) {
            return;
        }
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return;
        };
        if !self.visible(&name) {
            return;
        }
        let Ok(new) = stat_entry(
            path.to_path_buf(),
            self.opts.read_tags,
            self.opts.types.as_deref(),
        ) else {
            self.remove_name(&name, out);
            return;
        };
        match self.entries.iter().position(|e| e.name == name) {
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

    fn apply_fs(&mut self, ev: notify::Event, out: &mut Vec<ModelDelta>) {
        use notify::event::{ModifyKind, RenameMode};
        if self.loading {
            // the worker will see the state of the directory; events during loading are re-applied
            // idempotently below (upsert / remove are idempotent)
        }
        match ev.kind {
            EventKind::Create(_) => {
                for p in &ev.paths {
                    self.upsert(p, out);
                }
            }
            EventKind::Remove(_) => {
                for p in &ev.paths {
                    if p == &self.dir {
                        out.push(ModelDelta::DirectoryGone);
                    } else if p.parent() == Some(self.dir.as_path()) {
                        if let Some(n) = p.file_name() {
                            self.remove_name(&n.to_string_lossy(), out);
                        }
                    }
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                for p in &ev.paths {
                    if let Some(n) = p.file_name() {
                        self.remove_name(&n.to_string_lossy(), out);
                    }
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                for p in &ev.paths {
                    self.upsert(p, out);
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if ev.paths.len() == 2 => {
                if let Some(n) = ev.paths[0].file_name() {
                    self.remove_name(&n.to_string_lossy(), out);
                }
                self.upsert(&ev.paths[1], out);
            }
            EventKind::Modify(_) | EventKind::Any | EventKind::Other => {
                for p in &ev.paths {
                    if p == &self.dir && !self.dir.exists() {
                        out.push(ModelDelta::DirectoryGone);
                    } else {
                        self.upsert(p, out);
                    }
                }
            }
            EventKind::Access(_) => {}
        }
    }
}

impl Drop for DirectoryModel {
    fn drop(&mut self) {
        self.watcher = None;
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        let _ = &self.tx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn names(m: &DirectoryModel) -> Vec<String> {
        m.entries().iter().map(|e| e.name.clone()).collect()
    }

    fn pump_until(
        m: &mut DirectoryModel,
        mut cond: impl FnMut(&DirectoryModel) -> bool,
    ) -> Vec<ModelDelta> {
        let t = Instant::now();
        let mut all = Vec::new();
        while !cond(m) && t.elapsed() < Duration::from_secs(5) {
            all.extend(m.poll_timeout(Duration::from_millis(50)));
        }
        all
    }

    #[test]
    fn blocking_enumeration_sorts_and_hides_dotfiles() {
        let d = tempfile::tempdir().unwrap();
        for n in ["b10", "b2", ".hidden", "A"] {
            std::fs::write(d.path().join(n), b"x").unwrap();
        }
        let v = enumerate_blocking(d.path(), &ModelOptions::default()).unwrap();
        assert_eq!(
            v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["A", "b2", "b10"]
        );
        let all = enumerate_blocking(
            d.path(),
            &ModelOptions {
                show_hidden: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn batches_arrive_and_merge_in_order() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..1000 {
            std::fs::write(d.path().join(format!("f{i}")), b"").unwrap();
        }
        let mut m = DirectoryModel::open(
            d.path(),
            ModelOptions {
                batch_size: 100,
                watch: false,
                ..Default::default()
            },
        )
        .unwrap();
        let deltas = m.wait_loaded();
        assert!(
            deltas
                .iter()
                .filter(|d| matches!(d, ModelDelta::Reloaded { .. }))
                .count()
                >= 2,
            "{deltas:?}"
        );
        assert_eq!(m.len(), 1000);
        assert_eq!(&names(&m)[..3], ["f0", "f1", "f2"]);
        assert!(m
            .entries()
            .windows(2)
            .all(|w| w[0].name_key <= w[1].name_key));
        assert!(!m.is_loading());
    }

    #[test]
    fn the_shared_mime_database_types_the_entries_as_the_icons_are_chosen() {
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(data.path().join("mime")).unwrap();
        std::fs::write(
            data.path().join("mime/globs2"),
            "50:application/x-desktop:*.desktop\n50:image/png:*.png\n",
        )
        .unwrap();
        let types = Arc::new(MimeDatabase::load(&[data.path().to_path_buf()]).unwrap());
        let d = tempfile::tempdir().unwrap();
        for f in ["editor.desktop", "photo.png", "notes.unknown"] {
            std::fs::write(d.path().join(f), b"x").unwrap();
        }
        std::fs::create_dir(d.path().join("folder")).unwrap();
        let kinds = |types: Option<Arc<MimeDatabase>>| -> Vec<(String, String)> {
            let mut m = DirectoryModel::open(
                d.path(),
                ModelOptions {
                    watch: false,
                    types,
                    ..ModelOptions::default()
                },
            )
            .unwrap();
            m.wait_loaded();
            m.entries()
                .iter()
                .map(|e| {
                    (
                        e.name.clone(),
                        crate::mime::kind_description(&crate::mime::entry_mime(e)),
                    )
                })
                .collect()
        };
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            kinds(Some(types)),
            [
                s("editor.desktop", "Application"),
                s("folder", "Folder"),
                s("notes.unknown", "Document"),
                s("photo.png", "PNG image")
            ]
        );
        // without the database the extension guess knows no desktop entry
        assert_eq!(kinds(None)[0], s("editor.desktop", "Document"));
    }

    #[test]
    fn an_item_the_watcher_listed_during_the_enumeration_is_not_listed_twice() {
        let d = tempfile::tempdir().unwrap();
        let mut m = DirectoryModel::open(
            d.path(),
            ModelOptions {
                watch: false,
                ..ModelOptions::default()
            },
        )
        .unwrap();
        m.wait_loaded();
        std::fs::write(d.path().join("made"), b"x").unwrap();
        let mut out = Vec::new();
        m.apply(
            Msg::Fs(
                notify::Event::new(EventKind::Create(notify::event::CreateKind::File))
                    .add_path(d.path().join("made")),
            ),
            &mut out,
        );
        m.apply(
            Msg::Batch(vec![Entry::from_path(d.path().join("made")).unwrap()]),
            &mut out,
        );
        assert_eq!(names(&m), ["made"]);
    }

    #[test]
    fn watcher_reports_create_modify_rename_remove() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("b"), b"1").unwrap();
        let mut m = DirectoryModel::open(d.path(), ModelOptions::default()).unwrap();
        m.wait_loaded();
        assert_eq!(names(&m), ["b"]);

        std::fs::write(d.path().join("a"), b"1").unwrap();
        pump_until(&mut m, |m| m.len() == 2);
        assert_eq!(names(&m), ["a", "b"]);

        std::fs::rename(d.path().join("a"), d.path().join("c")).unwrap();
        pump_until(&mut m, |m| names(m) == ["b", "c"]);
        assert_eq!(names(&m), ["b", "c"]);

        std::fs::remove_file(d.path().join("b")).unwrap();
        pump_until(&mut m, |m| m.len() == 1);
        assert_eq!(names(&m), ["c"]);

        // hidden files do not appear: a visible one made after it is listed, so the hidden one's event came before
        std::fs::write(d.path().join(".x"), b"1").unwrap();
        std::fs::write(d.path().join("d"), b"1").unwrap();
        pump_until(&mut m, |m| m.len() == 2);
        assert_eq!(names(&m), ["c", "d"]);
    }

    #[test]
    fn modification_updates_size_in_place() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), b"1").unwrap();
        let mut m = DirectoryModel::open(d.path(), ModelOptions::default()).unwrap();
        m.wait_loaded();
        std::fs::write(d.path().join("a"), b"12345").unwrap();
        pump_until(&mut m, |m| m.entries()[0].size == 5);
        assert_eq!(m.entries()[0].size, 5);
    }

    #[test]
    fn changing_the_sort_reorders() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a"), vec![0u8; 10]).unwrap();
        std::fs::write(d.path().join("b"), vec![0u8; 1]).unwrap();
        let mut m = DirectoryModel::open(
            d.path(),
            ModelOptions {
                watch: false,
                ..Default::default()
            },
        )
        .unwrap();
        m.wait_loaded();
        m.set_sort(SortSpec {
            key: crate::sort::SortKey::Size,
            ascending: true,
            folders_first: false,
        });
        assert_eq!(names(&m), ["b", "a"]);
    }

    #[test]
    fn not_a_directory_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("f");
        std::fs::write(&f, b"").unwrap();
        assert!(DirectoryModel::open(&f, ModelOptions::default()).is_err());
        assert!(DirectoryModel::open(d.path().join("missing"), ModelOptions::default()).is_err());
    }
}
