//! Helper threads and detached child processes managed by the host (`docs/architecture/shell-host.md#threads-and-child-processes`).

mod reaper;

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_kit::core::Command;

use crate::context::{self, Inbox};
use crate::oneshot::Oneshot;

pub(crate) use reaper::Reaper;

/// Maximum time the host waits to join a component's helper threads when stopping it.
pub const HELPER_JOIN_TIMEOUT: Duration = Duration::from_secs(1);

/// The name a program's problems with the children it starts are reported under, when it has no name of its own.
const PROGRAM: &str = "cupertino-kit-shell";

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
    /// Set as the helper's thread ends.
    pub finished: Arc<Finished>,
}

/// Whether a helper thread ended, which its host waits for without polling.
#[derive(Default)]
pub(crate) struct Finished {
    done: Mutex<bool>,
    cv: Condvar,
}

impl Finished {
    fn set(&self) {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.cv.notify_all();
    }

    /// Waits for the helper's end until `deadline`; whether it ended.
    pub(crate) fn wait_until(&self, deadline: Instant) -> bool {
        let done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        let timeout = deadline.saturating_duration_since(Instant::now());
        let (done, _) = self
            .cv
            .wait_timeout_while(done, timeout, |done| !*done)
            .unwrap_or_else(PoisonError::into_inner);
        *done
    }
}

/// Marks its helper finished when the helper's thread drops it, however the thread ends.
struct FinishOnDrop(Arc<Finished>);

impl Drop for FinishOnDrop {
    fn drop(&mut self) {
        self.0.set();
    }
}

/// What a component's helper threads tell its host: a helper's panic is the component's failure
/// (`shell-host.md#threads-and-child-processes`), also when the component never waits for that helper's result.
pub(crate) struct Fault {
    panicked: AtomicBool,
    /// The host cut the component off (`Running::shut`): its helpers' results are dropped unread.
    cut: AtomicBool,
    /// The component loop's stop flag: a failed component ends its loop.
    stop: Arc<AtomicBool>,
    /// The component's inbox, woken so its loop sees the stop.
    inbox: Arc<Inbox>,
}

impl Fault {
    pub(crate) fn new(stop: Arc<AtomicBool>, inbox: Arc<Inbox>) -> Self {
        Self {
            panicked: AtomicBool::new(false),
            cut: AtomicBool::new(false),
            stop,
            inbox,
        }
    }

    fn helper_panicked(&self) {
        self.panicked.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.inbox.wake();
    }

    /// The host cut the component off: what its helpers still give goes unread.
    pub(crate) fn cut(&self) {
        self.cut.store(true, Ordering::SeqCst);
    }

    pub(crate) fn is_cut(&self) -> bool {
        self.cut.load(Ordering::SeqCst)
    }

    /// Whether one of the component's helper threads panicked.
    pub(crate) fn panicked(&self) -> bool {
        self.panicked.load(Ordering::SeqCst)
    }
}

/// Start a helper thread for the calling thread's component and deliver `done(result)` as a message.
///
/// The returned task waits for the helper without polling: the helper's end wakes it. A panic of the helper is the
/// component's failure: the task unwinds the loop that runs it with the helper's panic, and the component's host
/// records the failure and ends the component's loop even when the task is never run.
pub fn thread<T: Send + 'static, M: Send + 'static>(
    name: &'static str,
    work: impl FnOnce(&Stop) -> T + Send + 'static,
    done: impl FnOnce(T) -> M + Send + 'static,
) -> Command<M> {
    let stop = Stop::new();
    let worker_stop = stop.clone();
    let (helpers, fault) = context::with_ctx(|ctx| (Arc::clone(&ctx.helpers), ctx.fault.clone()))
        .map_or((None, None), |(helpers, fault)| (Some(helpers), fault));
    let result = Arc::new(Oneshot::<std::thread::Result<T>>::default());
    let filled = Arc::clone(&result);
    let waiting_fault = fault.clone();
    let finished = Arc::new(Finished::default());
    let finish = FinishOnDrop(Arc::clone(&finished));

    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _finish = finish;
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&worker_stop)));
            if outcome.is_err() {
                tracing::error!(helper = name, "component helper thread panicked");
                if let Some(fault) = &fault {
                    fault.helper_panicked();
                }
            }
            filled.fill(outcome);
        });

    match spawned {
        Ok(handle) => {
            if let Some(helpers) = helpers {
                if let Ok(mut list) = helpers.lock() {
                    list.push(HelperHandle {
                        stop,
                        handle: Some(handle),
                        finished,
                    });
                }
            }
        }
        Err(e) => {
            tracing::error!(helper = name, error = %e, "component helper thread did not start");
            let panic: Box<dyn Any + Send> =
                Box::new(format!("helper thread {name} did not start: {e}"));
            result.fill(Err(panic));
        }
    }

    Command::Perform(Box::pin(async move {
        let outcome = result.wait().await;
        if waiting_fault.as_ref().is_some_and(|fault| fault.is_cut()) {
            // a component that was cut off hears nothing more
            return std::future::pending().await;
        }
        match outcome {
            Ok(value) => done(value),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }))
}

/// Start a program detached from the component; the host reaps it even after the component stops.
/// Never passes a component socket (`WAYLAND_SOCKET`) to the child.
///
/// A component's child goes to its host's reaper, which waits on the child's pidfd on a thread of its own. In a
/// program of its own the child goes to the program's Kit instance, which reaps it; on a thread with neither, to a
/// reaper of the process.
pub fn spawn(mut command: std::process::Command) -> bool {
    command.env_remove("WAYLAND_SOCKET");
    let reaper = context::with_ctx(|ctx| ctx.reaper.clone()).flatten();
    if reaper.is_none() && cupertino_kit::Instance::current().is_some() {
        return cupertino_kit::spawn(&program_name(), command);
    }
    match command.spawn() {
        Ok(child) => {
            match reaper.or_else(Reaper::process) {
                Some(reaper) => reaper.adopt(child),
                None => reaper::wait_on_thread(child),
            }
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "host::spawn failed");
            false
        }
    }
}

/// The calling program's name, which Kit reports a child's problems under.
fn program_name() -> String {
    std::env::args_os()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| PROGRAM.to_owned())
}
