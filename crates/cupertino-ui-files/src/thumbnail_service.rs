//! Thumbnails of the items a window shows, made off the UI thread. A window says which of its items are in view
//! ([`ThumbnailService::want`]); the program takes the next to make as a job ([`ThumbnailService::take_job`]) and runs
//! it on an I/O thread ([`ThumbnailJob::run`], within [`JOB_DEADLINE`]), which takes it from the freedesktop thumbnail
//! cache ([`cupertino_icons::thumbnail`]) or creates it there, decodes it to pixels and calls the wake function. A
//! request no window wants any more is dropped before a job starts it, so scrolling past a folder costs nothing.
//!
//! The decoded thumbnails stay in memory up to a byte budget: past it those no window wants go, the least recently
//! drawn first; those in view stay, so a window showing more than the budget holds is not made again and again. A
//! thumbnail is made for one [`Stamp`] of its file: when the file changes, the window wants it with the new stamp and
//! it is made again, while the old one is still drawn. A file that cannot be thumbnailed, or whose making panics, is
//! not tried again until it changes. Nothing under the cache directory itself is thumbnailed. Dropping the service
//! does not wait for the files being decoded.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use crate::entry::Entry;
use cupertino_icons::thumbnail::{Pixels, ThumbError, ThumbSize, Thumbnailer};

/// The types whose thumbnails are made: the formats the thumbnail cache decodes.
pub const THUMBNAIL_TYPES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];
/// How many bytes of decoded thumbnails a process keeps beyond those in view: 512 thumbnails of 128 pixels a side.
pub const MEMORY_BUDGET: usize = 32 << 20;
/// How many thumbnails are made at once: decoding is the cost, and two keep the UI's own core free.
pub const JOBS: usize = 2;
/// How long making one thumbnail should take: reading a picture of a local disk and decoding it.
pub const JOB_DEADLINE: Duration = Duration::from_secs(5);

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
}

/// The making of one thumbnail, which the program runs off its UI thread ([`ThumbnailService::take_job`]).
pub struct ThumbnailJob {
    shared: Arc<Shared>,
    key: Key,
    stamp: Stamp,
    /// The job ran: dropping it hands nothing back.
    ran: bool,
}

impl ThumbnailJob {
    /// Blocking: reads and decodes. It catches a panic of the making, as of a decoder, and keeps it as the file's
    /// failure, so the panic is not the program's.
    pub fn run(mut self) {
        let shared = Arc::clone(&self.shared);
        let key = &self.key;
        let size = ThumbSize::for_icon(f64::from(key.edge), 1.0);
        let made = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let pixels = (shared.make)(&key.path, size, key.edge)?;
            Ok::<_, ThumbError>((pixels, (shared.ids)()))
        }))
        .map_err(|_| ())
        .and_then(|made| made.map_err(|_| ()));
        let mut state = shared.lock();
        state.working.remove(key);
        let done = match made {
            Ok((pixels, id)) => {
                state.clock += 1;
                Done::Ready {
                    stamp: self.stamp,
                    thumbnail: Arc::new(Thumbnail {
                        id,
                        width: pixels.width,
                        height: pixels.height,
                        rgba: Arc::from(pixels.rgba),
                    }),
                    used: state.clock,
                }
            }
            Err(()) => Done::Failed { stamp: self.stamp },
        };
        state.insert(key.clone(), done);
        state.evict(shared.budget, key);
        drop(state);
        self.ran = true;
        (shared.wake)();
    }
}

impl Drop for ThumbnailJob {
    /// A job dropped without running goes back to the queue's front while a window wants it.
    fn drop(&mut self) {
        if self.ran {
            return;
        }
        let mut state = self.shared.lock();
        state.working.remove(&self.key);
        if state.wants(&self.key, self.stamp) {
            state.queue.push_front((self.key.clone(), self.stamp));
        }
        drop(state);
        (self.shared.wake)();
    }
}

/// Keeps the thumbnails windows want, and hands out the making of each as a job.
pub struct ThumbnailService {
    shared: Arc<Shared>,
}

impl ThumbnailService {
    /// A service writing to `thumbnailer`'s cache, keeping at most `budget` bytes of pixels. It calls `wake` when a
    /// job waits to be taken and when a thumbnail is made or failed, and takes each thumbnail's id from `ids`.
    pub fn new(
        thumbnailer: Thumbnailer,
        budget: usize,
        wake: Arc<dyn Fn() + Send + Sync>,
        ids: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> ThumbnailService {
        let root = thumbnailer.root().to_path_buf();
        let make = Box::new(move |path: &Path, size, edge| thumbnailer.pixels(path, size, edge));
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            root,
            make,
            budget,
            wake,
            ids,
        });
        ThumbnailService { shared }
    }

    /// The next thumbnail to make, while fewer than [`JOBS`] are being made.
    pub fn take_job(&self) -> Option<ThumbnailJob> {
        let mut state = self.shared.lock();
        if state.working.len() >= JOBS {
            return None;
        }
        let (key, stamp) = state.queue.pop_front()?;
        // under the same lock, so a want meanwhile does not queue it again
        state.working.insert(key.clone(), stamp);
        Some(ThumbnailJob {
            shared: Arc::clone(&self.shared),
            key,
            stamp,
            ran: false,
        })
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
        let waits = !state.queue.is_empty() && state.working.len() < JOBS;
        drop(state);
        if waits {
            (self.shared.wake)();
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

    /// The files waiting for a job, the next first.
    pub fn queued(&self) -> Vec<PathBuf> {
        let state = self.shared.lock();
        state.queue.iter().map(|(k, _)| k.path.clone()).collect()
    }

    /// The bytes of pixels the ready thumbnails hold.
    pub fn memory(&self) -> usize {
        self.shared.lock().bytes
    }
}

impl fmt::Debug for ThumbnailService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ThumbnailService")
    }
}
