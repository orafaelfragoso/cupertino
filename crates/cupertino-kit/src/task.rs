//! Slow work off the UI thread. A task runs on one of the process's two pools and its result comes back to the UI thread
//! that asked, as a message: [`cpu`] for work that computes (decoding, layout of a large document), on a pool of up to a
//! thread per core; [`io`] for work that waits (files, a device, a slow call), on a pool of up to [`IO_THREADS`], where
//! every task names the time it should take and is reported when it takes longer. One instance runs at most [`IO_SHARE`]
//! tasks on the I/O pool at once; its others wait their turn, so a blocked mount stalls only the instance that met it.
//!
//! A task runs for the [`Instance`] whose thread asked for it: its diagnostic events are that instance's. A task that
//! panics is recorded by the process's panic hook as the instance's panic, and the panic goes on in the UI thread that
//! waits for its result, as if that thread had panicked itself.
//!
//! The pools are the process's, shared by every instance; each starts a thread only when work finds every one it
//! started busy, up to its size.

use crate::Instance;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_diagnostic_events::{self as diag, Severity, Value};
use cupertino_ui_core::Command;
use std::collections::VecDeque;
use std::future::Future;
use std::panic::{AssertUnwindSafe, Location};
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// Threads of the I/O pool: enough that the instances of a shell host each keep their share while some of them wait
/// on a stalled device.
pub const IO_THREADS: usize = 16;

/// The most I/O tasks one instance runs at once: a quarter of the pool.
pub const IO_SHARE: usize = IO_THREADS / 4;

/// The id of the event that reports an I/O task still running past its deadline.
pub const OVERDUE: &str = "toolkit.task.overdue";

/// Runs `work` on the CPU pool and `done` with its result on the UI thread, whose message the application then hears.
pub fn cpu<T, M>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) -> M + Send + 'static,
) -> Command<M>
where
    T: Send + 'static,
    M: Send + 'static,
{
    perform(cpu_submit, None, work, done)
}

/// Runs `work` on the I/O pool and `done` with its result on the UI thread, whose message the application then hears.
/// `deadline` is how long the work should take: a task still running past it is reported once, as the diagnostic
/// event [`OVERDUE`] with its instance, its deadline and where it was asked for, and keeps running.
#[track_caller]
pub fn io<T, M>(
    deadline: Duration,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) -> M + Send + 'static,
) -> Command<M>
where
    T: Send + 'static,
    M: Send + 'static,
{
    perform(io_submit, Some((deadline, Location::caller())), work, done)
}

fn perform<T, M>(
    submit: fn(Job),
    deadline: Option<(Duration, &'static Location<'static>)>,
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) -> M + Send + 'static,
) -> Command<M>
where
    T: Send + 'static,
    M: Send + 'static,
{
    let outcome = outcome(submit, deadline, work);
    Command::Perform(Box::pin(async move { done(outcome.await) }))
}

/// The result of `work`, run on a pool by `submit` once the future is first polled, for the instance of the calling
/// thread; its panic goes on where the future is polled.
fn outcome<T: Send + 'static>(
    submit: fn(Job),
    deadline: Option<(Duration, &'static Location<'static>)>,
    work: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = T> + Send {
    let instance = Instance::current();
    async move {
        let slot = Arc::new(Slot::default());
        let filled = Arc::clone(&slot);
        submit(Job {
            instance,
            deadline,
            run: Box::new(move || {
                let result = std::panic::catch_unwind(AssertUnwindSafe(work));
                filled.fill(result);
            }),
        });
        match slot.wait().await {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

/// How long reading the followed settings files should take ([`follow_settings`]): a settings file is a few lines on
/// a local disk.
pub const SETTINGS_READ_DEADLINE: Duration = Duration::from_secs(2);

/// Follows the settings files `watch` watches while the application runs: the UI thread only waits for each burst of
/// changes to be quiet ([`ConfigWatcher::quiet`]), the files are read on the I/O pool ([`ConfigWatcher::reread`]), and
/// `report` hears each report where the future is polled: the UI thread, which runs it as a task
/// (`Command::Perform`). It never ends, so it stands for a task of any output; once the notification failed for good,
/// after its failure was reported, it only waits.
#[track_caller]
pub fn follow_settings<T: 'static>(
    mut watch: ConfigWatcher,
    mut report: impl FnMut(WatchEvent) + Send + 'static,
) -> impl Future<Output = T> + Send + 'static {
    let site = Location::caller();
    async move {
        while watch.quiet().await {
            let events;
            (watch, events) = outcome(io_submit, Some((SETTINGS_READ_DEADLINE, site)), move || {
                let events = watch.reread();
                (watch, events)
            })
            .await;
            events.into_iter().for_each(&mut report);
        }
        std::future::pending().await
    }
}

/// A value another thread gives, waited for by a future on the UI thread: a task's outcome, a bus connection's
/// executor.
pub(crate) struct Slot<T> {
    state: Mutex<(Option<T>, Option<Waker>)>,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot {
            state: Mutex::new((None, None)),
        }
    }
}

impl<T> Slot<T> {
    /// Gives `value`, waking the future that waits for it.
    pub(crate) fn fill(&self, value: T) {
        let waker = {
            let mut state = lock(&self.state);
            state.0 = Some(value);
            state.1.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// The value, once given.
    pub(crate) fn wait(&self) -> Wait<'_, T> {
        Wait { slot: self }
    }
}

pub(crate) struct Wait<'a, T> {
    slot: &'a Slot<T>,
}

impl<T> Future for Wait<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut state = lock(&self.slot.state);
        match state.0.take() {
            Some(value) => Poll::Ready(value),
            None => {
                state.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A task handed to a pool.
struct Job {
    instance: Option<Instance>,
    /// How long it should take, and where it was asked for; I/O tasks only.
    deadline: Option<(Duration, &'static Location<'static>)>,
    run: Box<dyn FnOnce() + Send>,
}

/// Runs `job` for its instance on the process's CPU pool ([`cupertino_ui_core::pool`]), which windows drawn on the
/// CPU share.
fn cpu_submit(job: Job) {
    cupertino_ui_core::pool::cpu(move || {
        let _entered = job.instance.as_ref().map(Instance::enter);
        (job.run)();
    });
}

fn io_submit(job: Job) {
    io_pool().submit(job);
}

/// The process's I/O pool: up to [`IO_THREADS`] threads, started as work needs them, [`IO_SHARE`] for one instance at
/// most.
fn io_pool() -> &'static Pool {
    static IO: OnceLock<Pool> = OnceLock::new();
    IO.get_or_init(|| Pool::start("kit-io", IO_THREADS, Some(IO_SHARE)))
}

/// Threads that take jobs from one queue, each started when a job finds every started one busy, up to `threads`.
struct Pool {
    name: &'static str,
    threads: usize,
    queue: Arc<Queue>,
}

struct Queue {
    state: Mutex<QueueState>,
    /// A job was queued or a running one ended.
    changed: Condvar,
    /// The most jobs of one instance running at once, when the pool limits it.
    share: Option<usize>,
}

#[derive(Default)]
struct QueueState {
    waiting: VecDeque<Job>,
    /// The jobs running with a deadline: their number, instance, when they are due, where they were asked for, and
    /// whether they were reported overdue.
    running: Vec<Running>,
    next: u64,
    /// The threads started, and how many of them wait for a job.
    started: usize,
    idle: usize,
}

struct Running {
    id: u64,
    instance: Option<Instance>,
    due: Instant,
    deadline: Duration,
    site: &'static Location<'static>,
    reported: bool,
}

impl Pool {
    fn start(name: &'static str, threads: usize, share: Option<usize>) -> Pool {
        let queue = Arc::new(Queue {
            state: Mutex::default(),
            changed: Condvar::new(),
            share,
        });
        Pool {
            name,
            threads,
            queue,
        }
    }

    /// Queues `job`, starting a thread for it when none waits for one and the pool has room; with no thread at all,
    /// the job runs on the thread that hands it over.
    fn submit(&self, job: Job) {
        let mut state = lock(&self.queue.state);
        if state.idle == 0 && state.started < self.threads {
            let index = state.started;
            let queue = Arc::clone(&self.queue);
            match std::thread::Builder::new()
                .name(format!("{}-{index}", self.name))
                .spawn(move || queue.work())
            {
                Ok(_) => {
                    state.started += 1;
                    // the thread that reports overdue jobs, with the first that runs them
                    if index == 0 && self.queue.share.is_some() {
                        let queue = Arc::clone(&self.queue);
                        if let Err(e) = std::thread::Builder::new()
                            .name(format!("{}-deadlines", self.name))
                            .spawn(move || queue.watch())
                        {
                            eprintln!(
                                "cupertino-kit: cannot start the {} deadline thread: {e}",
                                self.name
                            );
                        }
                    }
                }
                Err(e) => eprintln!("cupertino-kit: cannot start a {} thread: {e}", self.name),
            }
        }
        if state.started == 0 {
            drop(state);
            let _entered = job.instance.as_ref().map(Instance::enter);
            (job.run)();
            return;
        }
        state.waiting.push_back(job);
        drop(state);
        self.queue.changed.notify_all();
    }
}

impl Queue {
    /// Whether `job` may start now: its instance runs fewer than its share.
    fn may_start(&self, job: &Job) -> bool {
        match (self.share, &job.instance) {
            (Some(share), Some(instance)) => instance.io_running().load(Ordering::Relaxed) < share,
            _ => true,
        }
    }

    /// A pool thread: takes the first job that may start, runs it for its instance, and goes back for the next.
    fn work(&self) {
        loop {
            let (job, id) = {
                let mut state = lock(&self.state);
                let at = loop {
                    if let Some(at) = state.waiting.iter().position(|job| self.may_start(job)) {
                        break at;
                    }
                    state.idle += 1;
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                    state.idle -= 1;
                };
                let job = state.waiting.remove(at).expect("the position found");
                let id = state.next;
                state.next += 1;
                if self.share.is_some() {
                    if let Some(instance) = &job.instance {
                        instance.io_running().fetch_add(1, Ordering::Relaxed);
                    }
                }
                // a deadline past what the clock reaches never comes: the task is never overdue
                let due = job.deadline.and_then(|(deadline, site)| {
                    Some((Instant::now().checked_add(deadline)?, deadline, site))
                });
                if let Some((due, deadline, site)) = due {
                    state.running.push(Running {
                        id,
                        instance: job.instance.clone(),
                        due,
                        deadline,
                        site,
                        reported: false,
                    });
                    // the deadline thread looks at the new deadline
                    self.changed.notify_all();
                }
                (job, id)
            };
            let instance = job.instance.clone();
            {
                let _entered = instance.as_ref().map(Instance::enter);
                // the job catches its own panic; this keeps the thread whatever happens
                let _ = std::panic::catch_unwind(AssertUnwindSafe(job.run));
            }
            let mut state = lock(&self.state);
            state.running.retain(|r| r.id != id);
            if self.share.is_some() {
                if let Some(instance) = &instance {
                    instance.io_running().fetch_sub(1, Ordering::Relaxed);
                }
            }
            drop(state);
            self.changed.notify_all();
        }
    }

    /// The deadline thread: reports each running job once it is past its deadline, waking at the next one due.
    fn watch(&self) {
        let mut state = lock(&self.state);
        loop {
            let now = Instant::now();
            let mut overdue = Vec::new();
            for running in state.running.iter_mut().filter(|r| !r.reported) {
                if running.due <= now {
                    running.reported = true;
                    overdue.push((running.instance.clone(), running.deadline, running.site));
                }
            }
            let next = state
                .running
                .iter()
                .filter(|r| !r.reported)
                .map(|r| r.due)
                .min();
            if !overdue.is_empty() {
                drop(state);
                for (instance, deadline, site) in overdue {
                    report(instance.as_ref(), deadline, site);
                }
                state = lock(&self.state);
                continue;
            }
            state = match next {
                Some(due) => {
                    self.changed
                        .wait_timeout(state, due.saturating_duration_since(now))
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            };
        }
    }
}

/// An I/O task of `instance`, asked for at `site`, still runs past its `deadline`: said on standard error and recorded
/// as [`OVERDUE`] in the instance's scope.
fn report(instance: Option<&Instance>, deadline: Duration, site: &Location<'_>) {
    let _entered = instance.map(Instance::enter);
    let program = diag::profile::program();
    let name = instance.map_or(program.as_str(), Instance::name);
    let site = format!("{}:{}", basename(site.file()), site.line());
    eprintln!(
        "cupertino-kit: a task of {name} asked for at {site} still runs past its {} ms",
        deadline.as_millis()
    );
    diag::event(
        Severity::Warn,
        OVERDUE,
        "an I/O task still runs past the deadline it was given",
    )
    .field("instance", Value::Public(name))
    .field("deadline", Value::Duration(deadline))
    .field("site", Value::Public(&site))
    .emit();
}

fn basename(file: &str) -> &str {
    file.rsplit('/').next().unwrap_or(file)
}
