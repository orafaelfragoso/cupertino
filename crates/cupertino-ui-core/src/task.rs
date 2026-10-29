//! The executor for [`Command::Perform`](crate::Command::Perform) futures. Tasks run only inside a runtime step, on
//! the UI thread; a future woken from another thread (an I/O reactor, a timer) queues its task and wakes the window
//! system's dispatch. Dropping the executor cancels every task.

use crate::Task;
use async_task::{Runnable, Task as Handle};
use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Waker;

pub(crate) struct Executor<M> {
    ready: Arc<Mutex<VecDeque<Runnable>>>,
    waker: Waker,
    tasks: Vec<Handle<()>>,
    done: Receiver<M>,
    send: Sender<M>,
}

impl<M: Send + 'static> Executor<M> {
    pub(crate) fn new(waker: Waker) -> Self {
        let (send, done) = channel();
        Self {
            ready: Arc::default(),
            waker,
            tasks: Vec::new(),
            done,
            send,
        }
    }

    pub(crate) fn spawn(&mut self, future: Task<M>) {
        let send = self.send.clone();
        let ready = Arc::clone(&self.ready);
        let waker = self.waker.clone();
        let schedule = move |runnable: Runnable| {
            ready
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(runnable);
            waker.wake_by_ref();
        };
        let (runnable, task) = async_task::spawn(
            async move {
                // The receiver is gone only when the executor is, and then the message has nowhere to go.
                let _ = send.send(future.await);
            },
            schedule,
        );
        runnable.schedule();
        self.tasks.push(task);
    }

    /// Polls the tasks that are ready now, at most once each, and returns the messages of those that finished.
    pub(crate) fn run_ready(&mut self) -> Vec<M> {
        let ready: Vec<Runnable> = self
            .ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for runnable in ready {
            runnable.run();
        }
        self.tasks.retain(|t| !t.is_finished());
        self.done.try_iter().collect()
    }

    /// Whether a task is ready to run.
    pub(crate) fn has_ready(&self) -> bool {
        !self
            .ready
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    /// Tasks not finished yet.
    pub(crate) fn pending(&self) -> usize {
        self.tasks.len()
    }
}

impl<M> Drop for Executor<M> {
    fn drop(&mut self) {
        // A cancelled idle task queues itself once more so its future is dropped here, on the UI thread; draining
        // the queue drops those futures and breaks the queue's cycle through the schedule function.
        self.tasks.clear();
        let queued =
            std::mem::take(&mut *self.ready.lock().unwrap_or_else(PoisonError::into_inner));
        drop(queued);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::poll_fn;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Poll, Wake};

    #[derive(Default)]
    struct CountingWaker(AtomicUsize);

    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_task_runs_only_when_stepped_and_its_wake_reaches_the_window_system() {
        let wakes = Arc::new(CountingWaker::default());
        let mut executor = Executor::new(Waker::from(Arc::clone(&wakes)));
        let slot: Arc<Mutex<Option<Waker>>> = Arc::default();
        let ready = Arc::new(Mutex::new(false));
        let (s, r) = (Arc::clone(&slot), Arc::clone(&ready));
        executor.spawn(Box::pin(poll_fn(move |cx| {
            if *r.lock().unwrap() {
                Poll::Ready(7)
            } else {
                *s.lock().unwrap() = Some(cx.waker().clone());
                Poll::Pending
            }
        })));
        assert_eq!(executor.pending(), 1);
        assert!(executor.run_ready().is_empty());
        assert!(!executor.has_ready());

        *ready.lock().unwrap() = true;
        let before = wakes.0.load(Ordering::SeqCst);
        std::thread::spawn(move || slot.lock().unwrap().take().unwrap().wake())
            .join()
            .unwrap();
        assert_eq!(wakes.0.load(Ordering::SeqCst), before + 1);
        assert!(executor.has_ready());
        assert_eq!(executor.run_ready(), vec![7]);
        assert_eq!(executor.pending(), 0);
    }

    #[test]
    fn dropping_the_executor_cancels_its_tasks() {
        struct Flag(Arc<AtomicUsize>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let flag = Flag(Arc::clone(&dropped));
        let mut executor = Executor::<u32>::new(Waker::noop().clone());
        executor.spawn(Box::pin(async move {
            let _flag = flag;
            std::future::pending::<u32>().await
        }));
        executor.run_ready();
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        drop(executor);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }
}
