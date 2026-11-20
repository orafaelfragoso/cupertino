//! The reaper of a host's child processes: one thread that sleeps in `poll(2)` on the pidfd of every child it holds,
//! and reaps each child the moment it ends, so no component's UI thread ever waits for a child and no child stays a
//! zombie until the next spawn.

use std::os::fd::OwnedFd;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use rustix::event::{eventfd, poll, EventfdFlags, PollFd, PollFlags};
use rustix::process::{pidfd_open, Pid, PidfdFlags};

/// Name of the reaper's thread.
const REAPER_THREAD: &str = "shell-reaper";

/// The children a host started for its components and the thread that reaps them.
pub(crate) struct Reaper {
    /// Each child with its pidfd, which turns readable when the child ends.
    children: Mutex<Vec<(Child, OwnedFd)>>,
    /// Wakes the reaper's thread when a child joins or the reaper goes.
    wake: OwnedFd,
    started: AtomicBool,
}

impl Reaper {
    /// A reaper with no children; its thread starts with the first child. `None` when the kernel gives no eventfd.
    pub(crate) fn new() -> Option<Arc<Reaper>> {
        match eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK) {
            Ok(wake) => Some(Arc::new(Reaper {
                children: Mutex::new(Vec::new()),
                wake,
                started: AtomicBool::new(false),
            })),
            Err(e) => {
                tracing::warn!(error = %e, "no eventfd for the child reaper");
                None
            }
        }
    }

    /// The reaper of the children started on threads that run for no host and no Kit instance.
    pub(crate) fn process() -> Option<Arc<Reaper>> {
        static PROCESS: OnceLock<Option<Arc<Reaper>>> = OnceLock::new();
        PROCESS.get_or_init(Reaper::new).clone()
    }

    /// Takes `child` over and reaps it when it ends.
    pub(crate) fn adopt(self: &Arc<Self>, child: Child) {
        let pidfd = pidfd_open(Pid::from_child(&child), PidfdFlags::empty()).ok();
        let Some(pidfd) = pidfd else {
            // no pidfd: a thread of its own waits for this child
            wait_on_thread(child);
            return;
        };
        self.children
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((child, pidfd));
        if !self.started.swap(true, Ordering::SeqCst) {
            let weak = Arc::downgrade(self);
            if let Err(e) = std::thread::Builder::new()
                .name(REAPER_THREAD.into())
                .spawn(move || run(weak))
            {
                tracing::warn!(error = %e, "child reaper thread did not start");
                self.started.store(false, Ordering::SeqCst);
            }
        }
        self.wake();
    }

    fn wake(&self) {
        let _ = rustix::io::write(&self.wake, &1u64.to_ne_bytes());
    }

    /// Reaps the children that ended; returns what to wait on next: the wake fd and each remaining child's pidfd.
    fn reap(&self) -> Option<Vec<OwnedFd>> {
        let mut drained = [0u8; 8];
        let _ = rustix::io::read(&self.wake, &mut drained);
        let mut children = self.children.lock().unwrap_or_else(PoisonError::into_inner);
        children.retain_mut(|(child, _)| matches!(child.try_wait(), Ok(None)));
        let mut fds = vec![self.wake.try_clone().ok()?];
        fds.extend(
            children
                .iter()
                .filter_map(|(_, pidfd)| pidfd.try_clone().ok()),
        );
        Some(fds)
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        // the thread holds no strong reference: wake it so that it sees the reaper gone and ends
        self.wake();
    }
}

/// The reaper's thread: reaps, then sleeps until a pidfd or the wake fd turns readable.
fn run(reaper: Weak<Reaper>) {
    loop {
        let Some(fds) = reaper.upgrade().and_then(|reaper| reaper.reap()) else {
            return;
        };
        let mut polled: Vec<PollFd<'_>> = fds
            .iter()
            .map(|fd| PollFd::new(fd, PollFlags::IN))
            .collect();
        match poll(&mut polled, None) {
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(e) => {
                tracing::warn!(error = %e, "child reaper cannot wait");
                return;
            }
        }
    }
}

/// Waits for `child` on a thread of its own, when no reaper can take it.
pub(crate) fn wait_on_thread(mut child: Child) {
    let spawned = std::thread::Builder::new()
        .name(REAPER_THREAD.into())
        .spawn(move || {
            let _ = child.wait();
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "a child is not reaped: no thread to wait for it");
    }
}
