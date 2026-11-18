//! The process's CPU pool: up to a thread per core for work that computes, such as drawing a window's frame on the
//! CPU and the toolkit's CPU tasks. A thread starts only when work finds every started one busy, so a process that
//! draws one window keeps one or two. A window's frames go to one lane ([`cpu_for`]): they run one after another on
//! the same thread, whose caches and allocator arena keep what the window's renderer uses, and the lanes share at most
//! [`FRAME_LANES`] threads. Other work ([`cpu`]) takes an idle thread. The pool is the process's, shared by every
//! instance; a process that cannot start a thread runs each task on the thread that hands it over.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

/// The prefix of the pool's thread names.
pub const CPU_THREAD: &str = "kit-cpu";

/// The most pool threads the lanes of frames use ([`cpu_for`]). Ours: a process seldom draws more windows at the same
/// moment, and every thread more holds memory of its own (an allocator arena, warm caches).
pub const FRAME_LANES: usize = 4;

/// Runs `work` on a thread of the CPU pool: an idle one, else one started for it while the pool has fewer than a
/// thread per core, else the one with the least work waiting. A panic in `work` ends only `work`.
pub fn cpu(work: impl FnOnce() + Send + 'static) {
    let pool = pool();
    let mut least: Option<(usize, &Arc<Worker>)> = None;
    for slot in &pool.workers {
        let Some(worker) = slot.get() else {
            continue;
        };
        let Some(worker) = worker else {
            continue;
        };
        let load = worker.load();
        if load == 0 {
            worker.push(Box::new(work));
            return;
        }
        if least.is_none_or(|(l, _)| load < l) {
            least = Some((load, worker));
        }
    }
    if let Some(worker) = pool.workers.iter().enumerate().find_map(|(i, slot)| {
        let fresh = slot.get().is_none();
        fresh.then(|| pool.worker(i)).flatten()
    }) {
        worker.push(Box::new(work));
        return;
    }
    match least {
        Some((_, worker)) => worker.push(Box::new(work)),
        None => work(),
    }
}

/// Runs `work` on the thread of `lane` (a window's frames: one lane each), after the work given to that lane before.
/// A panic in `work` ends only `work`.
pub fn cpu_for(lane: u64, work: impl FnOnce() + Send + 'static) {
    let pool = pool();
    let lanes = FRAME_LANES.min(pool.workers.len()) as u64;
    match pool.worker((lane % lanes) as usize) {
        Some(worker) => worker.push(Box::new(work)),
        None => work(),
    }
}

type Task = Box<dyn FnOnce() + Send>;

struct Pool {
    /// A thread per core, each started on first use; `None` for one that could not be started.
    workers: Vec<OnceLock<Option<Arc<Worker>>>>,
}

impl Pool {
    /// Worker `index`, started now when it was not.
    fn worker(&self, index: usize) -> Option<&Arc<Worker>> {
        self.workers[index]
            .get_or_init(|| {
                let worker = Arc::new(Worker {
                    state: Mutex::default(),
                    changed: Condvar::new(),
                });
                let theirs = Arc::clone(&worker);
                match std::thread::Builder::new()
                    .name(format!("{CPU_THREAD}-{index}"))
                    .spawn(move || theirs.work())
                {
                    Ok(_) => Some(worker),
                    Err(e) => {
                        eprintln!("cupertino-ui-core: cannot start a {CPU_THREAD} thread: {e}");
                        None
                    }
                }
            })
            .as_ref()
    }
}

/// One thread of the pool and the work waiting for it.
struct Worker {
    state: Mutex<WorkerState>,
    /// Work was handed over.
    changed: Condvar,
}

#[derive(Default)]
struct WorkerState {
    waiting: VecDeque<Task>,
    /// Whether the thread runs a task now.
    busy: bool,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        Pool {
            workers: (0..cores).map(|_| OnceLock::new()).collect(),
        }
    })
}

impl Worker {
    /// The work it runs and has waiting.
    fn load(&self) -> usize {
        let state = lock(&self.state);
        state.waiting.len() + usize::from(state.busy)
    }

    fn push(&self, task: Task) {
        lock(&self.state).waiting.push_back(task);
        self.changed.notify_one();
    }

    /// The thread: runs its tasks in the order they came, and waits for more.
    fn work(&self) {
        loop {
            let task = {
                let mut state = lock(&self.state);
                state.busy = false;
                let task = loop {
                    if let Some(task) = state.waiting.pop_front() {
                        break task;
                    }
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                };
                state.busy = true;
                task
            };
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
