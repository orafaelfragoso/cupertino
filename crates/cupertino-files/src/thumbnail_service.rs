//! Thumbnails of the items a window shows, made off the UI thread. A window says which of its items are in view
//! ([`ThumbnailService::want`]); worker threads take them from the freedesktop thumbnail cache ([`crate::thumbnail`])
//! or create them there, decode them to pixels and call the wake function. A request no window wants any more is
//! dropped before a worker starts it, so scrolling past a folder costs nothing.
//!
//! The decoded thumbnails stay in memory up to a byte budget: past it those no window wants go, the least recently
//! drawn first; those in view stay, so a window showing more than the budget holds is not made again and again. A
//! thumbnail is made for one [`Stamp`] of its file: when the file changes, the window wants it with the new stamp and
//! it is made again, while the old one is still drawn. A file that cannot be thumbnailed, or whose decoder panics, is
//! not tried again until it changes. Nothing under the cache directory itself is thumbnailed. Dropping the service
//! does not wait for the files being decoded.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::SystemTime;

use crate::thumbnail::{Pixels, ThumbError, ThumbSize, Thumbnailer};
use crate::Entry;

/// The types whose thumbnails are made: the formats the thumbnail cache decodes.
pub const THUMBNAIL_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];
/// How many bytes of decoded thumbnails a process keeps beyond those in view: 512 thumbnails of 128 pixels a side.
pub const MEMORY_BUDGET: usize = 32 << 20;
/// How many threads make thumbnails: decoding is the cost, and two keep the UI's own core free.
pub const WORKERS: usize = 2;

/// Whether items of type `mime` get a thumbnail.
pub fn has_thumbnail(mime: &str) -> bool {
    THUMBNAIL_TYPES.contains(&mime)
}

/// The version of a file a thumbnail shows: its modification time and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Stamp {
    pub modified: Option<SystemTime>,
    pub size: u64,
}

impl Stamp {
    pub fn of(entry: &Entry) -> Stamp {
        Stamp {
            modified: entry.modified,
            size: entry.size,
        }
    }
}

/// A decoded thumbnail: straight RGBA pixels, at most the edge it was asked for a side, and an id no other thumbnail
/// of the process has.
#[derive(Clone, PartialEq, Eq)]
pub struct Thumbnail {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
}

impl fmt::Debug for Thumbnail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Thumbnail({} {}x{})", self.id, self.width, self.height)
    }
}

/// A thumbnail of a file at an edge in device pixels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    edge: u32,
}

enum Done {
    Ready {
        stamp: Stamp,
        thumbnail: Arc<Thumbnail>,
        /// When it was last drawn, on the service's own clock.
        used: u64,
    },
    Failed {
        stamp: Stamp,
    },
}

impl Done {
    fn stamp(&self) -> Stamp {
        match self {
            Done::Ready { stamp, .. } | Done::Failed { stamp } => *stamp,
        }
    }

    fn bytes(&self) -> usize {
        match self {
            Done::Ready { thumbnail, .. } => thumbnail.rgba.len(),
            Done::Failed { .. } => 0,
        }
    }
}

#[derive(Default)]
struct State {
    /// What each window wants, by its id.
    wanted: HashMap<u64, HashMap<Key, Stamp>>,
    queue: VecDeque<(Key, Stamp)>,
    working: HashMap<Key, Stamp>,
    done: HashMap<Key, Done>,
    /// The bytes of the ready thumbnails.
    bytes: usize,
    clock: u64,
    stop: bool,
}

impl State {
    fn wants(&self, key: &Key, stamp: Stamp) -> bool {
        self.wanted.values().any(|w| w.get(key) == Some(&stamp))
    }

    fn wanted(&self, key: &Key) -> bool {
        self.wanted.values().any(|w| w.contains_key(key))
    }

    /// Whether a thumbnail of `key` for `stamp` is made, being made or waiting.
    fn has(&self, key: &Key, stamp: Stamp) -> bool {
        self.done.get(key).is_some_and(|d| d.stamp() == stamp)
            || self.working.get(key) == Some(&stamp)
            || self.queue.iter().any(|(k, s)| k == key && *s == stamp)
    }

    fn insert(&mut self, key: Key, done: Done) {
        self.bytes += done.bytes();
        if let Some(old) = self.done.insert(key, done) {
            self.bytes -= old.bytes();
        }
    }

    /// Drop ready thumbnails no window wants until they fit `budget`, the least recently drawn first; `keep` stays.
    /// Those in view stay too, past the budget: dropping one would only have it made again at the next want.
    fn evict(&mut self, budget: usize, keep: &Key) {
        while self.bytes > budget {
            let oldest = self
                .done
                .iter()
                .filter(|(k, _)| *k != keep && !self.wanted(k))
                .filter_map(|(k, d)| match d {
                    Done::Ready { used, .. } => Some((*used, k)),
                    Done::Failed { .. } => None,
                })
                .min_by_key(|(used, _)| *used)
                .map(|(_, k)| k.clone());
            let Some(key) = oldest else { break };
            if let Some(done) = self.done.remove(&key) {
                self.bytes -= done.bytes();
            }
        }
    }
}

/// What makes the thumbnail of a file for a cache size, fitted to an edge.
type Make = dyn Fn(&Path, ThumbSize, u32) -> Result<Pixels, ThumbError> + Send + Sync;

struct Shared {
    state: Mutex<State>,
    work: Condvar,
    /// The cache directory, whose own files get no thumbnail.
    root: PathBuf,
    make: Box<Make>,
    budget: usize,
    wake: Arc<dyn Fn() + Send + Sync>,
    ids: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(&self) {
        loop {
            let (key, stamp) = {
                let mut state = self.lock();
                loop {
                    if state.stop {
                        return;
                    }
                    if let Some((key, stamp)) = state.queue.pop_front() {
                        // under the same lock, so a want meanwhile does not queue it again
                        state.working.insert(key.clone(), stamp);
                        break (key, stamp);
                    }
                    state = self.work.wait(state).unwrap_or_else(|e| e.into_inner());
                }
            };
            let size = ThumbSize::for_icon(f64::from(key.edge), 1.0);
            // a decoder that panics on a file fails that file and keeps the worker
            let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.make)(&key.path, size, key.edge)
            }))
            .map_err(|_| ())
            .and_then(|made| made.map_err(|_| ()));
            let mut state = self.lock();
            state.working.remove(&key);
            let done = match made {
                Ok(pixels) => {
                    state.clock += 1;
                    Done::Ready {
                        stamp,
                        thumbnail: Arc::new(Thumbnail {
                            id: (self.ids)(),
                            width: pixels.width,
                            height: pixels.height,
                            rgba: Arc::from(pixels.rgba),
                        }),
                        used: state.clock,
                    }
                }
                Err(_) => Done::Failed { stamp },
            };
            state.insert(key.clone(), done);
            state.evict(self.budget, &key);
            drop(state);
            (self.wake)();
        }
    }
}

/// Makes the thumbnails windows want on worker threads; dropping it stops them.
pub struct ThumbnailService {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl ThumbnailService {
    /// A service writing to `thumbnailer`'s cache with `workers` threads, keeping at most `budget` bytes of pixels.
    /// It calls `wake` when a thumbnail is made or failed, and takes each thumbnail's id from `ids`.
    pub fn new(
        thumbnailer: Thumbnailer,
        workers: usize,
        budget: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
        ids: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> ThumbnailService {
        let root = thumbnailer.root().to_path_buf();
        let make = Box::new(move |path: &Path, size, edge| thumbnailer.pixels(path, size, edge));
        Self::with_make(root, make, workers, budget, wake, ids)
    }

    /// A service whose workers make each thumbnail with `make`, nothing under `root` thumbnailed.
    fn with_make(
        root: PathBuf,
        make: Box<Make>,
        workers: usize,
        budget: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
        ids: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> ThumbnailService {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            work: Condvar::new(),
            root,
            make,
            budget,
            wake,
            ids,
        });
        let workers = (0..workers)
            .map(|n| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("thumbnails-{n}"))
                    .spawn(move || shared.run())
                    .expect("a thumbnail thread starts")
            })
            .collect();
        ThumbnailService { shared, workers }
    }

    /// Window `window` shows `items` at `edge` device pixels, most wanted first: they are made in that order, before
    /// what other windows wait for, and what no window wants any more is not made.
    pub fn want(&self, window: u64, edge: u32, items: impl IntoIterator<Item = (PathBuf, Stamp)>) {
        let root = &self.shared.root;
        let items: Vec<(Key, Stamp)> = items
            .into_iter()
            .filter(|(path, _)| !path.starts_with(root))
            .map(|(path, stamp)| (Key { path, edge }, stamp))
            .collect();
        let mut state = self.shared.lock();
        if items.is_empty() {
            state.wanted.remove(&window);
        } else {
            state.wanted.insert(window, items.iter().cloned().collect());
        }
        let waiting = std::mem::take(&mut state.queue);
        for (key, stamp) in items {
            if !state.has(&key, stamp) {
                state.queue.push_back((key, stamp));
            }
        }
        for (key, stamp) in waiting {
            if state.wants(&key, stamp) && !state.has(&key, stamp) {
                state.queue.push_back((key, stamp));
            }
        }
        // a failure is kept while some window wants it, so it is not tried again
        let failed: Vec<Key> = state
            .done
            .iter()
            .filter(|(k, d)| matches!(d, Done::Failed { .. }) && !state.wanted(k))
            .map(|(k, _)| k.clone())
            .collect();
        for key in failed {
            state.done.remove(&key);
        }
        if !state.queue.is_empty() {
            self.shared.work.notify_all();
        }
    }

    /// Window `window` closed: it wants nothing.
    pub fn forget(&self, window: u64) {
        self.want(window, 0, []);
    }

    /// The thumbnail of `path` at `edge`, once made: the one of the file's last version that had one, which may be
    /// older than the file.
    pub fn get(&self, path: &Path, edge: u32) -> Option<Arc<Thumbnail>> {
        let key = Key {
            path: path.to_path_buf(),
            edge,
        };
        let mut state = self.shared.lock();
        state.clock += 1;
        let now = state.clock;
        match state.done.get_mut(&key)? {
            Done::Ready {
                thumbnail, used, ..
            } => {
                *used = now;
                Some(Arc::clone(thumbnail))
            }
            Done::Failed { .. } => None,
        }
    }

    /// Whether making the thumbnail of `path` at `edge` failed: none will come while a window wants it.
    pub fn failed(&self, path: &Path, edge: u32) -> bool {
        let key = Key {
            path: path.to_path_buf(),
            edge,
        };
        matches!(self.shared.lock().done.get(&key), Some(Done::Failed { .. }))
    }

    /// The files waiting for a worker, the next first.
    pub fn queued(&self) -> Vec<PathBuf> {
        let state = self.shared.lock();
        state.queue.iter().map(|(k, _)| k.path.clone()).collect()
    }

    /// The bytes of pixels the ready thumbnails hold.
    pub fn memory(&self) -> usize {
        self.shared.lock().bytes
    }
}

impl Drop for ThumbnailService {
    /// The workers stop once their current file is done, and are not waited for: a read that hangs on a stalled
    /// network mount must not keep the application from exiting.
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.work.notify_all();
        self.workers.clear();
    }
}

impl fmt::Debug for ThumbnailService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ThumbnailService")
    }
}
