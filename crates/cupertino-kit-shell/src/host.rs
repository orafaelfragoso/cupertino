//! Helper threads and detached child processes managed by the host (`docs/architecture/shell-host.md#threads-and-child-processes`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cupertino_kit::core::Command;

use crate::context;

/// Maximum time the host waits to join a component's helper threads when stopping it.
pub const HELPER_JOIN_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Default)]
struct StopInner {
    set: AtomicBool,
    lock: Mutex<bool>,
    cv: Condvar,
}

/// Cancellation signal handed to a helper thread started with [`thread`].
#[derive(Clone, Default)]
pub struct Stop {
    inner: Arc<StopInner>,
}

impl Stop {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn trigger(&self) {
        self.inner.set.store(true, Ordering::SeqCst);
        let mut guard = self
            .inner
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard = true;
        self.inner.cv.notify_all();
    }

    /// Whether the host asked this helper to stop.
    pub fn is_set(&self) -> bool {
        self.inner.set.load(Ordering::SeqCst)
    }

    /// Wait up to `timeout`; returns `true` when the stop was set before or during the wait.
    pub fn wait(&self, timeout: Duration) -> bool {
        if self.is_set() {
            return true;
        }
        let guard = self
            .inner
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *guard {
            return true;
        }
        let (guard, _) = self
            .inner
            .cv
            .wait_timeout_while(guard, timeout, |stopped| !*stopped)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard
    }
}

/// Internal tracking record for one helper thread spawned by a component.
pub(crate) struct HelperHandle {
    pub stop: Stop,
    pub handle: Option<JoinHandle<()>>,
}

/// Start a helper thread for the calling thread's component and deliver `done(result)` as a message.
pub fn thread<T: Send + 'static, M: Send + 'static>(
    name: &'static str,
    work: impl FnOnce(&Stop) -> T + Send + 'static,
    done: impl FnOnce(T) -> M + Send + 'static,
) -> Command<M> {
    let stop = Stop::new();
    let worker_stop = stop.clone();
    let helpers = context::with_ctx(|ctx| Arc::clone(&ctx.helpers));
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<T, ()>>(1);

    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&worker_stop)));
            match res {
                Ok(value) => {
                    let _ = tx.send(Ok(value));
                }
                Err(_) => {
                    tracing::error!(helper = name, "component helper thread panicked");
                    let _ = tx.send(Err(()));
                }
            }
        });

    if let (Ok(handle), Some(helpers)) = (spawned, helpers) {
        if let Ok(mut list) = helpers.lock() {
            list.push(HelperHandle {
                stop,
                handle: Some(handle),
            });
        }
    }

    Command::Perform(Box::pin(async move {
        // Yield until the helper posts its result; if the helper panicked, remain pending so the
        // host's panic/failure handling terminates the component cleanly.
        loop {
            match rx.try_recv() {
                Ok(Ok(value)) => return done(value),
                Ok(Err(())) => std::future::pending::<()>().await,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    std::future::pending::<()>().await
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }))
}

/// Start a program detached from the component; the host reaps it even after the component stops.
/// Never passes a component socket (`WAYLAND_SOCKET`) to the child.
pub fn spawn(mut command: std::process::Command) -> bool {
    command.env_remove("WAYLAND_SOCKET");
    match command.spawn() {
        Ok(child) => {
            if let Some(children) = context::with_ctx(|ctx| Arc::clone(&ctx.children)) {
                if let Ok(mut list) = children.lock() {
                    // Reap any already-exited children opportunistically.
                    list.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));
                    list.push(child);
                }
            } else {
                // Outside a host context, reap in a detached thread.
                let mut child = child;
                let _ = std::thread::Builder::new()
                    .name("shell-child-reaper".into())
                    .spawn(move || {
                        let _ = child.wait();
                    });
            }
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "host::spawn failed");
            false
        }
    }
}
