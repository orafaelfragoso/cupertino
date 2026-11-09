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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    /// How long a test waits for a worker.
    const WAIT: Duration = Duration::from_secs(10);

    /// A PNG `w` × `h` of one colour at `path`.
    fn picture(path: &Path, w: u32, h: u32, red: u8) {
        let mut pm = tiny_skia::Pixmap::new(w, h).unwrap();
        pm.fill(tiny_skia::Color::from_rgba8(red, 20, 40, 255));
        pm.save_png(path).unwrap();
    }

    fn stamp(path: &Path) -> Stamp {
        let meta = std::fs::metadata(path).unwrap();
        Stamp {
            modified: meta.modified().ok(),
            size: meta.len(),
        }
    }

    /// A service over a cache in `dir` with `workers` threads and `budget` bytes, and what its wakes arrive on.
    fn service(
        dir: &Path,
        workers: usize,
        budget: usize,
    ) -> (ThumbnailService, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let next = Arc::new(AtomicU64::new(1));
        let service = ThumbnailService::new(
            Thumbnailer::new(dir.join("cache"), "cupertino-files"),
            workers,
            budget,
            Arc::new(move || {
                let _ = tx.lock().unwrap().send(());
            }),
            Arc::new(move || next.fetch_add(1, Ordering::Relaxed)),
        );
        (service, rx)
    }

    /// Wait until `path` at `edge` is made or failed.
    fn settle(service: &ThumbnailService, rx: &mpsc::Receiver<()>, path: &Path, edge: u32) {
        let key = Key {
            path: path.to_path_buf(),
            edge,
        };
        while !service.shared.lock().done.contains_key(&key) {
            rx.recv_timeout(WAIT).expect("a worker finishes");
        }
    }

    #[test]
    fn only_pictures_of_the_decoded_formats_get_a_thumbnail() {
        assert!(has_thumbnail("image/png"));
        assert!(has_thumbnail("image/jpeg"));
        assert!(has_thumbnail("image/webp"));
        assert!(!has_thumbnail("image/svg+xml"));
        assert!(!has_thumbnail("text/plain"));
    }

    #[test]
    fn a_thumbnail_is_made_off_the_ui_thread_into_the_cache_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let (service, rx) = service(dir.path(), 1, MEMORY_BUDGET);
        let path = dir.path().join("wide.png");
        picture(&path, 400, 200, 200);
        assert_eq!(service.get(&path, 64), None);
        service.want(0, 64, [(path.clone(), stamp(&path))]);
        settle(&service, &rx, &path, 64);
        let thumb = service.get(&path, 64).expect("made");
        assert_eq!((thumb.width, thumb.height), (64, 32));
        assert_eq!(&thumb.rgba[..4], &[200, 20, 40, 255]);
        assert_eq!(service.memory(), 64 * 32 * 4);
        let cache = Thumbnailer::new(dir.path().join("cache"), "cupertino-files");
        assert!(
            cache.lookup(&path, ThumbSize::Normal).is_some(),
            "the freedesktop cache of the service has it"
        );
        // wanted again, it is not made again
        service.want(0, 64, [(path.clone(), stamp(&path))]);
        assert!(service.queued().is_empty());
        assert_eq!(service.get(&path, 64).unwrap().id, thumb.id);
    }

    #[test]
    fn what_no_window_wants_any_more_is_dropped_before_it_is_made() {
        let dir = tempfile::tempdir().unwrap();
        // no workers: the queue stays as the windows leave it
        let (service, _) = service(dir.path(), 0, MEMORY_BUDGET);
        let file = |name: &str| {
            let path = dir.path().join(name);
            picture(&path, 8, 8, 1);
            (path.clone(), stamp(&path))
        };
        let (a, b, c, d) = (file("a.png"), file("b.png"), file("c.png"), file("d.png"));
        service.want(1, 64, [a.clone(), b.clone(), c.clone()]);
        assert_eq!(service.queued(), [a.0.clone(), b.0.clone(), c.0.clone()]);
        // scrolled: a and b went out of view
        service.want(1, 64, [c.clone(), d.clone()]);
        assert_eq!(service.queued(), [c.0.clone(), d.0.clone()]);
        // another window wants a: its items come first, the first window's still wait
        service.want(2, 64, [a.clone()]);
        assert_eq!(service.queued(), [a.0.clone(), c.0.clone(), d.0.clone()]);
        service.forget(1);
        assert_eq!(service.queued(), vec![a.0.clone()]);
        service.forget(2);
        assert!(service.queued().is_empty());
    }

    /// A service over a cache in `dir` with one worker that makes its first thumbnail at once and holds every later one:
    /// it says on the second channel when it starts one, and makes it once the first channel lets it go (or is
    /// dropped). What the wakes arrive on comes last.
    fn held_after_first(
        dir: &Path,
    ) -> (
        ThumbnailService,
        mpsc::Sender<()>,
        mpsc::Receiver<()>,
        mpsc::Receiver<()>,
    ) {
        let thumbnailer = Thumbnailer::new(dir.join("cache"), "cupertino-files");
        let root = thumbnailer.root().to_path_buf();
        let (release, held) = mpsc::channel::<()>();
        let held = Mutex::new(held);
        let (started, starts) = mpsc::channel();
        let started = Mutex::new(started);
        let makes = AtomicU64::new(0);
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let next = Arc::new(AtomicU64::new(1));
        let service = ThumbnailService::with_make(
            root,
            Box::new(move |path: &Path, size, edge| {
                if makes.fetch_add(1, Ordering::SeqCst) > 0 {
                    let _ = started.lock().unwrap().send(());
                    let _ = held.lock().unwrap().recv();
                }
                thumbnailer.pixels(path, size, edge)
            }),
            1,
            MEMORY_BUDGET,
            Arc::new(move || {
                let _ = tx.lock().unwrap().send(());
            }),
            Arc::new(move || next.fetch_add(1, Ordering::Relaxed)),
        );
        (service, release, starts, rx)
    }

    #[test]
    fn a_changed_file_is_made_again_and_the_old_thumbnail_drawn_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let (service, release, starts, rx) = held_after_first(dir.path());
        let path = dir.path().join("p.png");
        picture(&path, 20, 20, 10);
        let first = stamp(&path);
        service.want(0, 64, [(path.clone(), first)]);
        settle(&service, &rx, &path, 64);
        let old = service.get(&path, 64).unwrap();
        // a new version: another size and time
        picture(&path, 40, 10, 250);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        let second = stamp(&path);
        assert_ne!(first, second);
        service.want(0, 64, [(path.clone(), second)]);
        // the worker is making the new one, and holds it until let go
        starts
            .recv_timeout(WAIT)
            .expect("the worker starts the new one");
        assert_eq!(
            service.get(&path, 64).map(|t| t.id),
            Some(old.id),
            "the old one is drawn until the new one is made"
        );
        release.send(()).unwrap();
        while service.get(&path, 64).unwrap().id == old.id {
            rx.recv_timeout(WAIT).expect("a worker finishes");
        }
        let new = service.get(&path, 64).unwrap();
        assert_eq!((new.width, new.height), (40, 10));
        assert_eq!(&new.rgba[..4], &[250, 20, 40, 255]);
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_not_tried_again_until_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (service, rx) = service(dir.path(), 1, MEMORY_BUDGET);
        let path = dir.path().join("broken.png");
        std::fs::write(&path, b"not a picture").unwrap();
        service.want(0, 64, [(path.clone(), stamp(&path))]);
        settle(&service, &rx, &path, 64);
        assert_eq!(service.get(&path, 64), None);
        service.want(0, 64, [(path.clone(), stamp(&path))]);
        assert!(service.queued().is_empty(), "a known failure");
        picture(&path, 8, 8, 99);
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        service.want(0, 64, [(path.clone(), stamp(&path))]);
        while service.get(&path, 64).is_none() {
            rx.recv_timeout(WAIT).expect("a worker finishes");
        }
    }

    #[test]
    fn the_thumbnails_kept_fit_the_budget_and_those_out_of_view_go_first() {
        let dir = tempfile::tempdir().unwrap();
        // room for two 16 × 16 thumbnails
        let budget = 2 * 16 * 16 * 4;
        let (service, rx) = service(dir.path(), 1, budget);
        let file = |name: &str| {
            let path = dir.path().join(name);
            picture(&path, 16, 16, 7);
            (path.clone(), stamp(&path))
        };
        let (a, b, c) = (file("a.png"), file("b.png"), file("c.png"));
        service.want(0, 16, [a.clone()]);
        settle(&service, &rx, &a.0, 16);
        service.want(0, 16, [b.clone(), a.clone()]);
        settle(&service, &rx, &b.0, 16);
        assert_eq!(service.memory(), budget);
        // a and c in view, b not: b makes room for c although a was drawn earlier
        assert!(service.get(&a.0, 16).is_some());
        assert!(service.get(&b.0, 16).is_some());
        service.want(0, 16, [a.clone(), c.clone()]);
        settle(&service, &rx, &c.0, 16);
        assert!(service.memory() <= budget);
        assert!(service.get(&a.0, 16).is_some());
        assert!(service.get(&c.0, 16).is_some());
        assert_eq!(service.get(&b.0, 16), None, "out of view, dropped");
    }

    /// A service with one worker whose thumbnails `make` makes, and what its wakes arrive on.
    fn made_by(
        make: impl Fn(&Path) -> Result<Pixels, ThumbError> + Send + Sync + 'static,
        budget: usize,
    ) -> (ThumbnailService, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let next = Arc::new(AtomicU64::new(1));
        let service = ThumbnailService::with_make(
            PathBuf::from("/no-cache"),
            Box::new(move |path: &Path, _, _| make(path)),
            1,
            budget,
            Arc::new(move || {
                let _ = tx.lock().unwrap().send(());
            }),
            Arc::new(move || next.fetch_add(1, Ordering::Relaxed)),
        );
        (service, rx)
    }

    fn square(edge: u32) -> Pixels {
        Pixels {
            width: edge,
            height: edge,
            rgba: vec![9; (edge * edge * 4) as usize],
        }
    }

    fn unchanged() -> Stamp {
        Stamp {
            modified: None,
            size: 1,
        }
    }

    #[test]
    fn a_decoder_that_panics_fails_its_file_and_the_worker_goes_on() {
        let (service, rx) = made_by(
            |path| {
                if path.ends_with("bad.png") {
                    panic!("a decoder bug");
                }
                Ok(square(4))
            },
            MEMORY_BUDGET,
        );
        let (bad, good) = (PathBuf::from("/d/bad.png"), PathBuf::from("/d/good.png"));
        service.want(0, 4, [(bad.clone(), unchanged())]);
        settle(&service, &rx, &bad, 4);
        assert_eq!(service.get(&bad, 4), None);
        service.want(0, 4, [(bad.clone(), unchanged())]);
        assert!(service.queued().is_empty(), "not tried again unchanged");
        // the only worker is still there
        service.want(0, 4, [(bad, unchanged()), (good.clone(), unchanged())]);
        settle(&service, &rx, &good, 4);
        assert!(service.get(&good, 4).is_some());
    }

    #[test]
    fn dropping_the_service_does_not_wait_for_a_decode_that_hangs() {
        let (hold, held) = mpsc::channel::<()>();
        let held = Mutex::new(held);
        let (started, starts) = mpsc::channel();
        let started = Mutex::new(started);
        let (service, _) = made_by(
            move |_| {
                let _ = started.lock().unwrap().send(());
                // a read stalled on a network mount
                let _ = held.lock().unwrap().recv();
                Ok(square(4))
            },
            MEMORY_BUDGET,
        );
        service.want(0, 4, [(PathBuf::from("/mnt/slow.png"), unchanged())]);
        starts.recv_timeout(WAIT).expect("the worker starts");
        let (dropped, done) = mpsc::channel();
        std::thread::spawn(move || {
            drop(service);
            let _ = dropped.send(());
        });
        done.recv_timeout(WAIT)
            .expect("the service is dropped while the decode hangs");
        drop(hold);
    }

    #[test]
    fn thumbnails_in_view_stay_past_the_budget_and_are_not_made_again() {
        let made = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&made);
        // room for one 4 × 4 thumbnail
        let (service, rx) = made_by(
            move |_| {
                count.fetch_add(1, Ordering::Relaxed);
                Ok(square(4))
            },
            4 * 4 * 4,
        );
        let items: Vec<(PathBuf, Stamp)> = (0..3)
            .map(|i| (PathBuf::from(format!("/d/{i}.png")), unchanged()))
            .collect();
        service.want(0, 4, items.clone());
        for (path, _) in &items {
            settle(&service, &rx, path, 4);
        }
        for (path, _) in &items {
            assert!(service.get(path, 4).is_some(), "{path:?} in view stays");
        }
        service.want(0, 4, items.clone());
        assert!(service.queued().is_empty(), "nothing is made again");
        assert_eq!(made.load(Ordering::Relaxed), 3);
        // out of view, they go down to the budget
        let other = (PathBuf::from("/d/other.png"), unchanged());
        service.want(0, 4, [other.clone()]);
        settle(&service, &rx, &other.0, 4);
        assert_eq!(service.memory(), 4 * 4 * 4);
        assert!(service.get(&other.0, 4).is_some());
    }

    #[test]
    fn the_cache_s_own_files_get_no_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let (service, _) = service(dir.path(), 0, MEMORY_BUDGET);
        let normal = dir.path().join("cache/thumbnails/normal");
        std::fs::create_dir_all(&normal).unwrap();
        let inside = normal.join("0123.png");
        picture(&inside, 8, 8, 1);
        service.want(0, 64, [(inside.clone(), stamp(&inside))]);
        assert!(service.queued().is_empty());
    }
}
