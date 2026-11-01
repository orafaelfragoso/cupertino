//! Where the state lives. The runtime is not `Send`, so it stays in a thread-local on the thread that created it. The
//! process-wide registry holds only what other threads need: the runtime's generation and owner, its posted messages
//! and its wake fd. The wake fd belongs to the registry entry, and is closed under the registry lock that every post
//! holds, so a post never writes to a closed or reused fd.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread::{self, ThreadId};

use cupertino_kit::core::{Application, Command, Runtime, Window};
use cupertino_ui_test::HeadlessWindowSystem;
use rustix::event::{eventfd, EventfdFlags};

use crate::ffi::{self, Error};
use crate::{CkRuntime, CkWaker};

/// The most posted messages that wait for the owner to poll.
pub(crate) const MAX_POSTED: usize = 1024;
/// The most wakers that are not released.
const MAX_WAKERS: usize = 4096;

const RUNTIME: u64 = 1;
const WAKER: u64 = 2;

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    issued: 0,
    runtime: None,
    wakers: Vec::new(),
});

thread_local! {
    static OWNED: RefCell<Option<Owned>> = const { RefCell::new(None) };
}

struct Registry {
    /// The last generation given out. Every handle of the process takes a new one, so no handle value comes back.
    issued: u32,
    /// The one runtime of the process.
    runtime: Option<Live>,
    /// Each waker slot's generation and its runtime's; generation 0 is a free slot.
    wakers: Vec<WakerSlot>,
}

struct Live {
    generation: u32,
    owner: ThreadId,
    poisoned: bool,
    posted: VecDeque<u64>,
    wake: OwnedFd,
}

#[derive(Clone, Copy, Default)]
struct WakerSlot {
    generation: u32,
    runtime: u32,
}

/// What a C application runs: its views come through the ABI, so the Rust side has no model of its own.
pub(crate) enum CApp {}

impl Application for CApp {
    type Model = ();
    type Message = u64;
    const RESIDENT: bool = true;

    fn init() -> ((), Vec<Command<u64>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: u64) -> Vec<Command<u64>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<u64>> {
        Vec::new()
    }
}

pub(crate) struct CRuntime {
    _core: Runtime<CApp, HeadlessWindowSystem>,
}

impl CRuntime {
    pub(crate) fn headless() -> Result<CRuntime, Error> {
        let options = cupertino_kit::app::options()
            .map_err(|e| Error::Unavailable(format!("the backend failed to start: {e}")))?;
        Ok(CRuntime {
            _core: Runtime::new(HeadlessWindowSystem::default(), options),
        })
    }
}

#[cfg(test)]
impl Drop for CRuntime {
    fn drop(&mut self) {
        ffi::hook("drop");
    }
}

/// The runtime in its owner's thread-local. When the thread exits without `ck_runtime_release`, this drops the runtime
/// and frees the slot. A panic there would abort the host, so the runtime is dropped inside `catch_unwind` and leaked
/// if it panics, no other thread-local is touched, and the registry lock is taken whether it is poisoned or not.
struct Owned {
    generation: u32,
    runtime: Option<Box<CRuntime>>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            ffi::drop_or_leak(runtime);
        }
        let mut registry = lock();
        if registry
            .runtime
            .as_ref()
            .is_some_and(|live| live.generation == self.generation)
        {
            registry.runtime = None;
        }
    }
}

fn lock() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

fn encode(kind: u64, index: usize, generation: u32) -> u64 {
    kind << 56 | (index as u64) << 32 | u64::from(generation)
}

fn decode(bits: u64, kind: u64) -> Result<(usize, u32), Error> {
    let generation = bits as u32;
    if bits >> 56 != kind || generation == 0 {
        return Err(Error::StaleHandle);
    }
    Ok((((bits >> 32) & 0xff_ffff) as usize, generation))
}

impl Registry {
    fn issue(&mut self) -> Result<u32, Error> {
        self.issued = self
            .issued
            .checked_add(1)
            .ok_or(Error::State("the process has used up its handles"))?;
        Ok(self.issued)
    }

    /// The live runtime of `handle`; a poisoned one only if `poisoned` allows it.
    fn live(&mut self, handle: CkRuntime, poisoned: bool) -> Result<&mut Live, Error> {
        let (index, generation) = decode(handle.bits, RUNTIME)?;
        let live = self
            .runtime
            .as_mut()
            .filter(|live| index == 0 && live.generation == generation)
            .ok_or(Error::StaleHandle)?;
        if live.poisoned && !poisoned {
            return Err(Error::Internal);
        }
        Ok(live)
    }

    /// The live runtime of `handle`, called on its owner thread.
    fn owned(&mut self, handle: CkRuntime, poisoned: bool) -> Result<&mut Live, Error> {
        let live = self.live(handle, poisoned)?;
        if live.owner != thread::current().id() {
            return Err(Error::WrongThread);
        }
        Ok(live)
    }

    fn waker(&self, handle: CkWaker) -> Result<usize, Error> {
        let (index, generation) = decode(handle.bits, WAKER)?;
        self.wakers
            .get(index)
            .filter(|slot| slot.generation == generation)
            .map(|_| index)
            .ok_or(Error::StaleHandle)
    }
}

/// Starts the process's one runtime on this thread with `make`.
pub(crate) fn create(make: impl FnOnce() -> Result<CRuntime, Error>) -> Result<CkRuntime, Error> {
    const ONE: Error = Error::State("a runtime already exists in this process");
    if lock().runtime.is_some() {
        return Err(ONE);
    }
    let runtime = make()?;
    let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
        .map_err(|e| Error::Unavailable(format!("cannot make the wake fd: {e}")))?;
    let mut registry = lock();
    if registry.runtime.is_some() {
        return Err(ONE);
    }
    let generation = registry.issue()?;
    registry.runtime = Some(Live {
        generation,
        owner: thread::current().id(),
        poisoned: false,
        posted: VecDeque::new(),
        wake,
    });
    drop(registry);
    let owned = Owned {
        generation,
        runtime: Some(Box::new(runtime)),
    };
    OWNED.with(|slot| ffi::drop_or_leak(slot.borrow_mut().replace(owned)));
    Ok(CkRuntime {
        bits: encode(RUNTIME, 0, generation),
    })
}

/// Releases the runtime, poisoned or not: its wakers go stale and its wake fd closes under the lock, then the runtime
/// is dropped, or leaked if its destructor panics.
pub(crate) fn release(handle: CkRuntime) -> Result<(), Error> {
    let mut registry = lock();
    registry.owned(handle, true)?;
    registry.runtime = None;
    drop(registry);
    let owned = OWNED.with(|slot| slot.borrow_mut().take());
    ffi::drop_or_leak(owned);
    Ok(())
}

pub(crate) fn poison(bits: u64) {
    let mut registry = lock();
    let generation = match bits >> 56 {
        RUNTIME => bits as u32,
        WAKER => match registry.waker(CkWaker { bits }) {
            Ok(index) => registry.wakers[index].runtime,
            Err(_) => return,
        },
        _ => return,
    };
    if let Some(live) = registry
        .runtime
        .as_mut()
        .filter(|live| live.generation == generation)
    {
        live.poisoned = true;
    }
}

pub(crate) fn new_waker(handle: CkRuntime) -> Result<CkWaker, Error> {
    let mut registry = lock();
    let runtime = registry.owned(handle, false)?.generation;
    let index = match registry.wakers.iter().position(|slot| slot.generation == 0) {
        Some(index) => index,
        None if registry.wakers.len() < MAX_WAKERS => {
            registry.wakers.push(WakerSlot::default());
            registry.wakers.len() - 1
        }
        None => return Err(Error::State("4096 wakers are not released")),
    };
    let generation = registry.issue()?;
    registry.wakers[index] = WakerSlot {
        generation,
        runtime,
    };
    Ok(CkWaker {
        bits: encode(WAKER, index, generation),
    })
}

pub(crate) fn release_waker(handle: CkWaker) -> Result<(), Error> {
    let mut registry = lock();
    let index = registry.waker(handle)?;
    registry.wakers[index] = WakerSlot::default();
    Ok(())
}

/// Queues `message` for the waker's runtime and wakes its loop, all under the lock that closing the wake fd takes.
pub(crate) fn post(handle: CkWaker, message: u64) -> Result<(), Error> {
    let mut registry = lock();
    ffi::hook("post locked");
    let runtime = registry.wakers[registry.waker(handle)?].runtime;
    let live = registry
        .runtime
        .as_mut()
        .filter(|live| live.generation == runtime)
        .ok_or(Error::StaleHandle)?;
    if live.poisoned {
        return Err(Error::Internal);
    }
    if live.posted.len() >= MAX_POSTED {
        return Err(Error::QueueFull);
    }
    live.posted.push_back(message);
    // only a full counter fails, and then the loop is already woken
    let _ = rustix::io::write(&live.wake, &1u64.to_ne_bytes());
    Ok(())
}

/// Hands up to `cap` posted messages to `out`, oldest first, and returns how many. With none handed while some wait,
/// the buffer is too small.
pub(crate) fn take_posted(
    handle: CkRuntime,
    cap: usize,
    mut out: impl FnMut(usize, u64),
) -> Result<usize, Error> {
    let mut registry = lock();
    let live = registry.owned(handle, false)?;
    if cap == 0 && !live.posted.is_empty() {
        return Err(Error::BufferTooSmall("the event buffer holds no event"));
    }
    let n = cap.min(live.posted.len());
    for (i, message) in live.posted.drain(..n).enumerate() {
        out(i, message);
    }
    if live.posted.is_empty() {
        // nothing is left to wake for; an empty counter fails with EAGAIN
        let _ = rustix::io::read(&live.wake, &mut [0; 8]);
    }
    Ok(n)
}

#[cfg(test)]
pub(crate) fn wake_fd(handle: CkRuntime) -> std::os::fd::RawFd {
    use std::os::fd::AsRawFd;
    lock().live(handle, true).unwrap().wake.as_raw_fd()
}
