//! The process's render thread: it owns the process's one GPU device and the device's only context, and everything
//! made on it (the windows' GPU renderers, the textures their frames stay in, the buffers a presenter draws into)
//! lives there as a [`Resident`]. Every instance of the process draws through it: a UI thread hands it work with
//! [`run`], which runs on the render thread and returns its result, or with [`hand_over`], whose result comes back
//! another way (a window's frame, drawn and committed there); the render thread does one piece of work at a time, in
//! the order it was handed over, and its [`heartbeat`] says how long the current one has taken.
//!
//! The device is made on a warm-up thread ([`request`]: the context, then the programs, compiled or loaded from saved
//! binaries, and drawn once) and then handed to the render thread, so a driver slow to make a context never holds up
//! work already on the device. [`status`] reads where that stands from any thread without waiting for the render
//! thread. A [`Claim`] holds the device: while one is held the render thread keeps it, and the last one going lets it
//! go (the process ending with its windows). A loss ([`lost`]) lets it go too, once for every instance that saw it,
//! and the next [`request`] makes a new one.
//!
//! A device someone else made and lends (the server's) is not the render thread's: a [`crate::gpu::GpuRenderer`] made
//! on it draws on the lender's thread, as before.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use cupertino_gpu::{
    program_cache, DetachedDevice, Device, DeviceDesc, DeviceInfo, Error as GpuError, Loss,
    Platform, ProgramCacheStats, FOURCC_ARGB8888,
};

use crate::gpu::GpuPrograms;

/// The render thread's name.
pub const RENDER_THREAD: &str = "kit-render";

/// The warm-up thread's name.
pub const WARM_UP_THREAD: &str = "gpu-warm-up";

/// The longest `Device::new` may take on the warm-up thread before a caller gives the GPU up and stays on the CPU: a
/// context is made in well under a second, so one still being made after this is a driver that hangs.
pub const DEVICE_HANG: Duration = Duration::from_secs(10);

/// How long a warm-up waits for another process to finish making its programs before making its own. Making them all
/// takes some 80 ms with the compiler and 5 ms from saved binaries; the frames meanwhile go out on the CPU.
const PROGRAM_TURN_PATIENCE: Duration = Duration::from_secs(2);

/// The longest the last [`Claim`] going waits for a warm-up thread still inside `Device::new`, and for the render
/// thread to let the device go, before leaving them.
const RELEASE_WAIT: Duration = Duration::from_secs(5);

/// How long a warm-up took.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WarmTiming {
    /// Making the context on the device.
    pub context: Duration,
    /// Making the programs (compiled, or loaded from saved binaries).
    pub programs: Duration,
    pub cache: ProgramCacheStats,
}

/// Where the process's device stands.
#[derive(Clone, Debug)]
pub enum Status {
    /// No device: none was asked for, the last claim let it go, or it was lost and none was asked for since.
    Idle,
    /// The warm-up thread has been making it since `since`; `device_made` once `Device::new` returned.
    Warming { since: Instant, device_made: bool },
    /// The render thread has it.
    Ready(Ready),
    /// The warm-up could not make it, and why.
    Failed(String),
}

/// The device the render thread has.
#[derive(Clone, Debug)]
pub struct Ready {
    /// Which device of the process it is: each one made has the next number.
    pub generation: u64,
    pub info: DeviceInfo,
    pub timing: WarmTiming,
    /// The modifiers the driver renders ARGB8888 to.
    pub renderable: Vec<u64>,
}

/// The render thread's own state, handed to the work [`run`] gives it. Only the render thread has one, so a
/// [`Resident`] opened with it is opened there.
pub struct RenderThread {
    on: Option<OnDevice>,
    /// The last device let go for a loss, and the loss.
    lost: Option<(u64, Loss)>,
    /// Bound to the render thread.
    _thread: PhantomData<*const ()>,
}

struct OnDevice {
    device: Rc<Device>,
    programs: Rc<GpuPrograms>,
    generation: u64,
}

impl RenderThread {
    /// The process's device, while it has one.
    pub fn device(&self) -> Option<&Rc<Device>> {
        self.on.as_ref().map(|on| &on.device)
    }

    /// The windows' renderers' programs on the device, while it has one.
    pub fn programs(&self) -> Option<&Rc<GpuPrograms>> {
        self.on.as_ref().map(|on| &on.programs)
    }

    /// The device's [`Ready::generation`], while it has one.
    pub fn generation(&self) -> Option<u64> {
        self.on.as_ref().map(|on| on.generation)
    }

    /// The device of `generation` while it is the process's and works. One the render thread let go reads as lost, by
    /// the loss that let it go ([`lost`]).
    pub fn device_of(&self, generation: u64) -> Result<&Rc<Device>, GpuError> {
        match &self.on {
            Some(on) if on.generation == generation => on.device.status().map(|()| &on.device),
            _ => Err(GpuError::Lost(match self.lost {
                Some((g, loss)) if g == generation => loss,
                _ => Loss::Reset { guilty: None },
            })),
        }
    }
}

/// A value that lives on the render thread: made there, opened only there (with the [`RenderThread`] that [`run`]
/// hands over), and dropped there, wherever its owner drops it. Its owner may be on any thread.
pub struct Resident<T: 'static> {
    value: Option<Box<T>>,
}

// Safety: the value is reached only through `get` and `get_mut`, which take the render thread's own state and so run
// only on the render thread, and it is dropped there (see `Drop`); other threads move the box, never what is in it.
unsafe impl<T: 'static> Send for Resident<T> {}
// Safety: as for `Send`: a shared reference on another thread cannot open the value.
unsafe impl<T: 'static> Sync for Resident<T> {}

impl<T: 'static> Resident<T> {
    /// `value`, kept on the render thread.
    pub fn new(_on: &RenderThread, value: T) -> Resident<T> {
        Resident {
            value: Some(Box::new(value)),
        }
    }

    pub fn get<'a>(&'a self, _on: &'a RenderThread) -> &'a T {
        self.value.as_deref().expect("held until dropped")
    }

    pub fn get_mut<'a>(&'a mut self, _on: &'a RenderThread) -> &'a mut T {
        self.value.as_deref_mut().expect("held until dropped")
    }

    /// The value, taken out on the render thread.
    pub fn into_inner(mut self, _on: &RenderThread) -> T {
        *self.value.take().expect("held until dropped")
    }
}

impl<T: 'static> Drop for Resident<T> {
    fn drop(&mut self) {
        let Some(value) = self.value.take() else {
            return;
        };
        let thread = handle();
        if thread.is_current() {
            drop(value);
            return;
        }
        let value = Moved(value);
        thread.send(Box::new(move || value.drop_here()));
    }
}

/// A resident value on its way to the render thread to be dropped.
struct Moved<T>(Box<T>);

// Safety: the box only moves to the render thread, where the value was made; nothing in it is touched on the way.
unsafe impl<T> Send for Moved<T> {}

impl<T> Moved<T> {
    /// Drops the value, on the render thread.
    fn drop_here(self) {
        drop(self.0);
    }
}

/// Runs `work` on the render thread and returns its result, once the work handed over before it is done. A panic in
/// `work` goes on in the caller. Called on the render thread itself, from work [`hand_over`] gave it, `work` runs
/// there at once; work that `run` gave it cannot call `run` again.
pub fn run<T: Send>(work: impl FnOnce(&mut RenderThread) -> T + Send) -> T {
    let thread = handle();
    if thread.is_current() {
        return with_state(work);
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let job: Box<dyn FnOnce() + Send + '_> = Box::new(move || {
        let _ = tx.send(catch_unwind(AssertUnwindSafe(|| with_state(work))));
    });
    // Safety: the job borrows from the caller's frame for no longer than this call: it returns only once the job has
    // sent its result, or once the job was dropped unrun (its sender with it, which ends the wait below), so nothing
    // the job holds is used after the borrow ends.
    let job: Job = unsafe { std::mem::transmute::<Box<dyn FnOnce() + Send + '_>, Job>(job) };
    thread.send(job);
    match rx.recv() {
        Ok(Ok(value)) => value,
        Ok(Err(panic)) => resume_unwind(panic),
        Err(_) => panic!("the render thread is gone"),
    }
}

/// Hands `work` to the render thread without waiting for it: it runs after the work handed over before it, and
/// reaches the render thread's state through [`run`], which runs there at once. For work whose result comes back
/// another way (a channel), such as drawing and committing a window's frame, or that nobody waits for, such as freeing
/// what a closing surface drew into. A panic in `work` is the render thread's and ends only `work`.
pub fn hand_over(work: impl FnOnce() + Send + 'static) {
    handle().send(Box::new(work));
}

/// What the render thread is doing, read from any thread without waiting for it: what a watchdog looks at (D17).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heartbeat {
    /// How long the work it is doing now has taken; `None` while it waits for work.
    pub busy_for: Option<Duration>,
    /// The pieces of work it finished since the process started.
    pub done: u64,
}

/// The render thread's heartbeat.
pub fn heartbeat() -> Heartbeat {
    let busy = BUSY_SINCE.load(Ordering::Acquire);
    Heartbeat {
        busy_for: (busy != 0).then(|| (epoch() + Duration::from_nanos(busy - 1)).elapsed()),
        done: DONE.load(Ordering::Acquire),
    }
}

/// When the work the render thread is doing began, in nanoseconds after [`epoch`] plus one; 0 while it waits.
static BUSY_SINCE: AtomicU64 = AtomicU64::new(0);

/// The pieces of work the render thread finished.
static DONE: AtomicU64 = AtomicU64::new(0);

/// The instant the heartbeat counts from.
fn epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

std::thread_local! {
    /// The render thread's state, on the render thread.
    static STATE: RefCell<Option<RenderThread>> = const { RefCell::new(None) };
}

/// `work` with the render thread's state, on the render thread.
fn with_state<T>(work: impl FnOnce(&mut RenderThread) -> T) -> T {
    STATE.with(|state| {
        let mut state = state.try_borrow_mut().expect(
            "render_thread::run inside work that render_thread::run gave the render thread",
        );
        work(
            state
                .as_mut()
                .expect("the render thread's state is made as it starts"),
        )
    })
}

/// A job for the render thread that takes its state.
fn job(work: impl FnOnce(&mut RenderThread) + Send + 'static) -> Job {
    Box::new(move || with_state(work))
}
/// The device as it stands, without waiting.
pub fn status() -> Status {
    let shared = &handle().shared;
    lock(&shared.state).status()
}

/// The device as it stands once it is not being made any more, waiting up to `timeout` for that.
pub fn wait(timeout: Duration) -> Status {
    let shared = &handle().shared;
    let until = Instant::now() + timeout;
    let mut state = lock(&shared.state);
    loop {
        let status = state.status();
        let now = Instant::now();
        if !matches!(status, Status::Warming { .. }) || now >= until {
            return status;
        }
        state = shared
            .changed
            .wait_timeout(state, until - now)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// Asks for the process's device, made on `node` (the driver's default device without one) with saved programs under
/// `cache`, unless it is ready or being made; `wake` runs once it is ready or could not be made, at once when it is
/// ready already. The device stays while the returned claim, or another, is held.
pub fn request(
    node: Option<&Path>,
    cache: Option<&Path>,
    wake: impl FnOnce() + Send + 'static,
) -> Claim {
    let thread = handle();
    let mut state = lock(&thread.shared.state);
    state.claims += 1;
    let claim = Claim { _private: () };
    match state.phase {
        Phase::Ready(_) => {
            drop(state);
            wake();
            return claim;
        }
        Phase::Failed(_) if thread.id.is_none() => {
            drop(state);
            wake();
            return claim;
        }
        Phase::Warming => {
            state.wakes.push(Box::new(wake));
            return claim;
        }
        Phase::Idle | Phase::Failed(_) => {}
    }
    state.generation += 1;
    let generation = state.generation;
    state.wakes.push(Box::new(wake));
    let signals = Arc::new(WarmSignals::default());
    let theirs = Arc::clone(&signals);
    let (node, cache) = (node.map(Path::to_path_buf), cache.map(Path::to_path_buf));
    let jobs = thread.jobs.clone();
    let (done_tx, done) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(WARM_UP_THREAD.into())
        .spawn(move || {
            let made = warm(node.as_deref(), cache.as_deref(), &theirs);
            let _ = jobs.send(job(move |on| attach(on, generation, made)));
            let _ = done_tx.send(());
        });
    match spawned {
        Ok(thread) => {
            state.phase = Phase::Warming;
            state.warm_up = Some(WarmUp {
                signals,
                since: Instant::now(),
                done,
                thread: Some(thread),
            });
        }
        Err(e) => {
            state.phase = Phase::Failed(format!("no warm-up thread: {e}"));
            let wakes = std::mem::take(&mut state.wakes);
            drop(state);
            wakes.into_iter().for_each(|wake| wake());
        }
    }
    claim
}

/// The device of `generation` was lost by `loss` (reset or purged): the render thread lets it go (once, however many
/// instances saw the loss), and the next [`request`] makes another. What was made on it goes as its owners drop it.
pub fn lost(generation: u64, loss: Loss) {
    let thread = handle();
    {
        let mut state = lock(&thread.shared.state);
        match &state.phase {
            Phase::Ready(ready) if ready.generation == generation => state.phase = Phase::Idle,
            _ => return,
        }
    }
    thread.send(job(move |on| {
        if on.generation() == Some(generation) {
            on.on = None;
            on.lost = Some((generation, loss));
        }
    }));
}

/// A hold on the process's device ([`request`]); the last one going lets the device go.
#[must_use = "the device goes when the last claim does"]
pub struct Claim {
    _private: (),
}

impl Drop for Claim {
    fn drop(&mut self) {
        let thread = handle();
        let warm_up = {
            let mut state = lock(&thread.shared.state);
            state.claims -= 1;
            if state.claims > 0 || thread.id.is_none() {
                return;
            }
            state.phase = Phase::Idle;
            state.wakes.clear();
            state.warm_up.take()
        };
        thread.shared.changed.notify_all();
        if let Some(mut warm_up) = warm_up {
            warm_up.signals.stop();
            warm_up.end();
        }
        let (tx, rx) = mpsc::sync_channel(1);
        thread.send(job(move |on| {
            on.on = None;
            let _ = tx.send(());
        }));
        if rx.recv_timeout(RELEASE_WAIT).is_err() {
            eprintln!(
                "renderer: the render thread did not let the device go within {} s; the process leaves it",
                RELEASE_WAIT.as_secs()
            );
        }
    }
}

/// The render thread's work.
type Job = Box<dyn FnOnce() + Send + 'static>;

struct Handle {
    jobs: Sender<Job>,
    /// The render thread's, unless it could not be started.
    id: Option<ThreadId>,
    shared: Shared,
}

impl Handle {
    fn is_current(&self) -> bool {
        Some(std::thread::current().id()) == self.id
    }

    fn send(&self, job: Job) {
        // the render thread runs as long as the process does
        let _ = self.jobs.send(job);
    }
}

/// The process's render thread, started on first use.
fn handle() -> &'static Handle {
    static THREAD: OnceLock<Handle> = OnceLock::new();
    THREAD.get_or_init(|| {
        let (jobs, queue) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name(RENDER_THREAD.into())
            .spawn(move || serve(queue));
        // without a render thread the process never has a device: every request fails
        let (id, phase) = match spawned {
            Ok(thread) => (Some(thread.thread().id()), Phase::Idle),
            Err(e) => (None, Phase::Failed(format!("no render thread: {e}"))),
        };
        Handle {
            jobs,
            id,
            shared: Shared {
                state: Mutex::new(SharedState {
                    phase,
                    generation: 0,
                    wakes: Vec::new(),
                    claims: 0,
                    warm_up: None,
                }),
                changed: Condvar::new(),
            },
        }
    })
}

/// The render thread: runs each job handed over, in order, its heartbeat beating around each.
fn serve(queue: Receiver<Job>) {
    STATE.with(|state| {
        *state.borrow_mut() = Some(RenderThread {
            on: None,
            lost: None,
            _thread: PhantomData,
        })
    });
    let epoch = epoch();
    while let Ok(job) = queue.recv() {
        let since = epoch.elapsed().as_nanos() as u64 + 1;
        BUSY_SINCE.store(since, Ordering::Release);
        // `run` catches its work's panic; a panic in handed-over work or in dropping a resident value is the render
        // thread's own and is caught here so it keeps serving
        let _ = catch_unwind(AssertUnwindSafe(job));
        BUSY_SINCE.store(0, Ordering::Release);
        DONE.fetch_add(1, Ordering::AcqRel);
    }
}

/// What every thread reads of the render thread without waiting for it.
struct Shared {
    state: Mutex<SharedState>,
    /// The device is ready, failed or let go.
    changed: Condvar,
}

struct SharedState {
    phase: Phase,
    /// The last device asked for.
    generation: u64,
    /// Run once the device asked for is ready or failed.
    wakes: Vec<Box<dyn FnOnce() + Send>>,
    claims: usize,
    warm_up: Option<WarmUp>,
}

enum Phase {
    Idle,
    Warming,
    Ready(Ready),
    Failed(String),
}

impl SharedState {
    fn status(&self) -> Status {
        match &self.phase {
            Phase::Idle => Status::Idle,
            Phase::Warming => match &self.warm_up {
                Some(warm_up) => Status::Warming {
                    since: warm_up.since,
                    device_made: warm_up.signals.device_made(),
                },
                None => Status::Idle,
            },
            Phase::Ready(ready) => Status::Ready(ready.clone()),
            Phase::Failed(why) => Status::Failed(why.clone()),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The warm-up thread of the device being made.
struct WarmUp {
    signals: Arc<WarmSignals>,
    since: Instant,
    /// Closed or sent to as the thread ends.
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl WarmUp {
    /// Waits for the thread to end and joins it; one still inside `Device::new` (which no stop cuts short) is waited
    /// for up to [`RELEASE_WAIT`] and then left behind.
    fn end(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let ended = match self.done.recv_timeout(RELEASE_WAIT) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
            Err(mpsc::RecvTimeoutError::Timeout) if self.signals.device_made() => {
                let _ = self.done.recv();
                true
            }
            Err(mpsc::RecvTimeoutError::Timeout) => false,
        };
        if ended {
            let _ = thread.join();
        } else {
            eprintln!(
                "renderer: the warm-up thread is still making its device after {} s; the process leaves it",
                self.since.elapsed().as_secs()
            );
        }
    }
}

/// What the warm-up thread and the render thread share: the stop the last claim sets as it goes, and whether the
/// thread is past making the device (the one step a stop cannot cut short, and the one a hung driver keeps it in).
#[derive(Debug, Default)]
struct WarmSignals {
    stop: AtomicBool,
    device_made: AtomicBool,
}

impl WarmSignals {
    fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn device_made(&self) -> bool {
        self.device_made.load(Ordering::Acquire)
    }

    fn note_device_made(&self) {
        self.device_made.store(true, Ordering::Release);
    }
}

/// What the warm-up thread made, and how long it took.
struct Warm {
    device: DetachedDevice,
    programs: GpuPrograms,
    timing: WarmTiming,
}

// Safety: the programs are GL names of the detached device's context, used only once the device is attached on the
// render thread; they carry no thread-bound state of their own.
unsafe impl Send for Warm {}

/// What [`warm`] gives once the last claim stopped it.
const STOPPED: &str = "the warm-up was stopped";

/// Make the device on `node` (or the driver's default device), with saved programs under `cache`, and the windows'
/// renderers' programs, drawn once. Once `signals` asks it to stop it returns after the step it is in (making the
/// device, waiting for the programs' turn, making them).
fn warm(node: Option<&Path>, cache: Option<&Path>, signals: &WarmSignals) -> Result<Warm, String> {
    let _warm = tracing::trace_span!("gpu.warm").entered();
    let started = Instant::now();
    let platform = match node {
        Some(node) => Platform::RenderNode(node.to_path_buf()),
        None => Platform::Surfaceless,
    };
    let made =
        tracing::trace_span!("gpu.device").in_scope(|| Device::new(&DeviceDesc { platform }));
    signals.note_device_made();
    let mut device = made.map_err(|e| e.to_string())?;
    let context = started.elapsed();
    if signals.stopped() {
        return Err(STOPPED.into());
    }
    device.set_program_cache(cache);
    let compiling = Instant::now();
    // the processes of a session starting together make their programs in turn, so on a cold cache only the first
    // loads the driver's compiler; a turn that cannot be held is not waited for, and a stop ends the wait
    let turn = tracing::trace_span!("gpu.program_turn").in_scope(|| {
        device
            .program_cache_dir()
            .and_then(|dir| program_cache::wait_turn(dir, PROGRAM_TURN_PATIENCE, &signals.stop))
    });
    if signals.stopped() {
        return Err(STOPPED.into());
    }
    let programs = GpuPrograms::new(&device).map_err(|e| e.to_string())?;
    if signals.stopped() {
        return Err(STOPPED.into());
    }
    // the turn covers making the programs only: drawing with them is each process's own work, and holding the turn
    // through it would make processes starting together wait on one another's draws
    drop(turn);
    // the driver finishes each program at its first draw: drawn once here, so the process's first frames do not
    // wait on its compiler
    programs.prime(&device).map_err(|e| e.to_string())?;
    let timing = WarmTiming {
        context,
        programs: compiling.elapsed(),
        cache: device.program_cache_stats(),
    };
    let device = device.detach().map_err(|e| e.to_string())?;
    Ok(Warm {
        device,
        programs,
        timing,
    })
}

/// On the render thread: takes the device the warm-up of `generation` made, unless another was asked for since or
/// the last claim went meanwhile, and wakes those who asked.
fn attach(on: &mut RenderThread, generation: u64, made: Result<Warm, String>) {
    let shared = &handle().shared;
    let current = {
        let state = lock(&shared.state);
        state.generation == generation && matches!(state.phase, Phase::Warming)
    };
    let attached = made.and_then(|warm| {
        let device = warm.device.attach().map_err(|e| e.to_string())?;
        Ok((device, warm.programs, warm.timing))
    });
    let phase = match attached {
        Ok((device, programs, timing)) if current => {
            let ready = Ready {
                generation,
                info: device.info().clone(),
                timing,
                renderable: device.renderable_modifiers(FOURCC_ARGB8888),
            };
            on.on = Some(OnDevice {
                device: Rc::new(device),
                programs: Rc::new(programs),
                generation,
            });
            Phase::Ready(ready)
        }
        Ok((device, programs, _)) => {
            programs.destroy(&device);
            return;
        }
        Err(_) if !current => return,
        Err(why) => Phase::Failed(why),
    };
    let wakes = {
        let mut state = lock(&shared.state);
        if state.generation != generation || !matches!(state.phase, Phase::Warming) {
            // the last claim went while the device was attached
            on.on = None;
            return;
        }
        state.phase = phase;
        // the thread has sent its result and ends by itself now
        state.warm_up = None;
        std::mem::take(&mut state.wakes)
    };
    shared.changed.notify_all();
    wakes.into_iter().for_each(|wake| wake());
}
