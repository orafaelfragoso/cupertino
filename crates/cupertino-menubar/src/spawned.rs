//! Tasks on a bus connection's executor that live no longer than what started them. A task that holds the connection
//! and outlived its owner would keep the connection, and the connection its executor and so the task, until the task
//! ends, which a task that follows signals never does. So no task is detached: each belongs to a [`Spawned`] set its
//! owner holds, and dropping the set cancels the tasks still running; the executor drops their futures when it runs
//! next (once the component's runtime has ended, Kit closes its connections and runs their executors to the end,
//! `cupertino_kit::bus`). A task that ends leaves the set itself.
//!
//! The executors are the set's: a [`Spawner`], which the tasks hold to start more, keeps neither the set nor an
//! executor. A task that held its own executor would keep it, with the task's cancelled future still queued on it,
//! once Kit's thread had stopped running it (the connection gone before the set was dropped, or before the executor
//! ran next): then nothing would drop the future, nor what it holds (another connection, which Kit's thread for that
//! one then serves for as long as the process runs). With the set gone, the executor's last owner drops it, and with
//! it every future still on it.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError, Weak};

/// The tasks of an owner: dropped, it cancels those still running.
#[derive(Default)]
pub struct Spawned {
    set: Arc<Mutex<Set>>,
}

#[derive(Default)]
struct Set {
    next: u64,
    running: HashMap<u64, zbus::Task<()>>,
    /// Tasks that ended before their handle was put in `running`.
    ended: HashSet<u64>,
    /// The executors the set's tasks run on, as the spawners name them by index.
    executors: Vec<zbus::Executor<'static>>,
}

impl Set {
    fn lock(set: &Mutex<Set>) -> std::sync::MutexGuard<'_, Set> {
        set.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The index of `executor` among the set's executors, which it joins.
    fn executor(&mut self, executor: &zbus::Executor<'static>) -> usize {
        self.executors.push(executor.clone());
        self.executors.len() - 1
    }
}

impl Spawned {
    /// What starts tasks of this set on `executor`.
    pub fn spawner(&self, executor: &zbus::Executor<'static>) -> Spawner {
        Spawner {
            set: Arc::downgrade(&self.set),
            executor: Set::lock(&self.set).executor(executor),
        }
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let (running, executors) = {
            let mut set = Set::lock(&self.set);
            (
                std::mem::take(&mut set.running),
                std::mem::take(&mut set.executors),
            )
        };
        // each cancelled task is scheduled to drop its future when its executor runs next, or when the executor's
        // last owner drops it
        drop(running);
        drop(executors);
    }
}

/// Starts tasks of a [`Spawned`] set on one of its executors. It keeps neither the set nor the executor: a task
/// started after the set's owner dropped it is not started.
#[derive(Clone)]
pub struct Spawner {
    set: Weak<Mutex<Set>>,
    /// Which of the set's executors.
    executor: usize,
}

impl Spawner {
    /// The same set's tasks on `executor`, another connection's.
    pub fn on(&self, executor: &zbus::Executor<'static>) -> Spawner {
        let at = self
            .set
            .upgrade()
            .map(|set| Set::lock(&set).executor(executor));
        Spawner {
            set: Weak::clone(&self.set),
            // a set already dropped starts nothing, on whichever executor
            executor: at.unwrap_or(self.executor),
        }
    }

    /// Run `future` as a task of the set, named `name`.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static, name: &str) {
        let Some(set) = self.set.upgrade() else {
            return;
        };
        let (id, executor) = {
            let mut set = Set::lock(&set);
            let Some(executor) = set.executors.get(self.executor).cloned() else {
                // the owner is dropping the set
                return;
            };
            set.next += 1;
            (set.next, executor)
        };
        let leaves = Weak::clone(&self.set);
        let task = executor.spawn(
            async move {
                future.await;
                if let Some(set) = leaves.upgrade() {
                    let mut set = Set::lock(&set);
                    match set.running.remove(&id) {
                        // the task is running: its handle lets it finish
                        Some(task) => task.detach(),
                        None => {
                            set.ended.insert(id);
                        }
                    }
                }
            },
            name,
        );
        let mut set = Set::lock(&set);
        if set.ended.remove(&id) {
            task.detach();
        } else {
            set.running.insert(id, task);
        }
    }
}
