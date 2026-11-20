//! Tasks on a bus connection's executor that live no longer than what started them. A task that holds the connection
//! and outlived its owner would keep the connection, and the connection its executor and so the task, until the task
//! ends, which a task that follows signals never does. So no task is detached: each belongs to a [`Spawned`] set its
//! owner holds, and dropping the set cancels the tasks still running; the executor drops their futures when it runs
//! next (the component's end closes its connections and runs their executors empty, `crate::component`). A task that
//! ends leaves the set itself.

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
}

impl Set {
    fn lock(set: &Mutex<Set>) -> std::sync::MutexGuard<'_, Set> {
        set.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Spawned {
    /// What starts tasks of this set on `executor`.
    pub fn spawner(&self, executor: &zbus::Executor<'static>) -> Spawner {
        Spawner {
            set: Arc::downgrade(&self.set),
            executor: executor.clone(),
        }
    }
}

impl Drop for Spawned {
    fn drop(&mut self) {
        let running = std::mem::take(&mut Set::lock(&self.set).running);
        // each cancelled task is scheduled to drop its future when its executor runs next
        drop(running);
    }
}

/// Starts tasks of a [`Spawned`] set on an executor. It does not keep the set: a task started after the set's owner
/// dropped it is not started.
#[derive(Clone)]
pub struct Spawner {
    set: Weak<Mutex<Set>>,
    executor: zbus::Executor<'static>,
}

impl Spawner {
    /// The same set's tasks on `executor`, another connection's.
    pub fn on(&self, executor: &zbus::Executor<'static>) -> Spawner {
        Spawner {
            set: Weak::clone(&self.set),
            executor: executor.clone(),
        }
    }

    /// Run `future` as a task of the set, named `name`.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static, name: &str) {
        let Some(set) = self.set.upgrade() else {
            return;
        };
        let id = {
            let mut set = Set::lock(&set);
            set.next += 1;
            set.next
        };
        let leaves = Weak::clone(&self.set);
        let task = self.executor.spawn(
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
