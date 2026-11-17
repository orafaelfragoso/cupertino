//! The process's CPU pool: a thread per core for work that computes, such as drawing a window's frame on the CPU, one
//! task per surface, and the toolkit's CPU tasks. Tasks start in the order they were handed over. The pool is the
//! process's, started on first use and shared by every instance; a process that cannot start a thread runs each task
//! on the thread that hands it over.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

/// The prefix of the pool's thread names.
pub const CPU_THREAD: &str = "kit-cpu";

/// Runs `work` on a thread of the CPU pool. A panic in `work` ends only `work`.
pub fn cpu(work: impl FnOnce() + Send + 'static) {
    let pool = pool();
    if !pool.started {
        work();
        return;
    }
    lock(&pool.queue.waiting).push_back(Box::new(work));
    pool.queue.changed.notify_one();
}

type Task = Box<dyn FnOnce() + Send>;

struct Pool {
    queue: &'static Queue,
    /// Whether a thread could be started.
    started: bool,
}

struct Queue {
    waiting: Mutex<VecDeque<Task>>,
    /// A task was queued.
    changed: Condvar,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        static QUEUE: OnceLock<Queue> = OnceLock::new();
        let queue = QUEUE.get_or_init(|| Queue {
            waiting: Mutex::default(),
            changed: Condvar::new(),
        });
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        let mut started = false;
        for i in 0..cores {
            let spawned = std::thread::Builder::new()
                .name(format!("{CPU_THREAD}-{i}"))
                .spawn(move || queue.work());
            match spawned {
                Ok(_) => started = true,
                Err(e) => eprintln!("cupertino-ui-core: cannot start a {CPU_THREAD} thread: {e}"),
            }
        }
        Pool { queue, started }
    })
}

impl Queue {
    /// A pool thread: runs the first task waiting, and goes back for the next.
    fn work(&self) {
        loop {
            let task = {
                let mut waiting = lock(&self.waiting);
                loop {
                    if let Some(task) = waiting.pop_front() {
                        break task;
                    }
                    waiting = self
                        .changed
                        .wait(waiting)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            };
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
