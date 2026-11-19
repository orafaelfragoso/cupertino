//! Commits held back until a buffer is ready. Each wait is an event source in the main loop with file descriptors of its
//! own, so a client may have only so many at once, and the sources of a client that went away are removed. The two kinds
//! are counted apart ([`Kind`]):
//! - an explicit-sync acquire point is whatever the client says, and may never be signalled: past [`ACQUIRE_LIMIT`] the
//!   client is disconnected;
//! - the implicit fences of a dmabuf always signal (the kernel times a GPU job out), so a client can flood the server with
//!   them but not make it leak: past [`IMPLICIT_LIMIT`] a commit goes through without waiting, and the client stays.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::{LoopHandle, RegistrationToken};

/// Most explicit-sync waits of one client at the same time. A client has at most a few buffers in flight, so more than
/// this is a client that never signals what it set.
pub const ACQUIRE_LIMIT: usize = 64;
/// Most implicit-fence waits of one client at the same time. Each holds up to four descriptors (one sync file per plane),
/// so they stay bounded, but far above what a client under GPU contention queues.
pub const IMPLICIT_LIMIT: usize = 1024;

/// What a commit waits for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    AcquirePoint,
    ImplicitFence,
}

#[derive(Debug)]
struct Wait {
    kind: Kind,
    token: Option<RegistrationToken>,
}

#[derive(Debug, Default)]
struct Waits {
    next: u64,
    /// The waits of the client: reserved, and registered once the source is in the loop.
    live: HashMap<u64, Wait>,
}

impl Waits {
    fn count(&self, kind: Kind) -> usize {
        self.live.values().filter(|w| w.kind == kind).count()
    }
}

/// One client's waits, kept in its per-client state.
#[derive(Debug, Default)]
pub struct AcquireWaits {
    waits: Mutex<Waits>,
    closed: AtomicBool,
    listed: AtomicBool,
    warned: AtomicBool,
}

/// What admitting one more wait decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    /// The wait may go on; give its id back to [`AcquireWaits::registered`] or [`AcquireWaits::cleared`].
    Granted(u64),
    /// The client already has [`ACQUIRE_LIMIT`] explicit-sync waits: it is disconnected.
    Disconnect,
    /// The client already has [`IMPLICIT_LIMIT`] implicit-fence waits: the commit goes through without waiting.
    /// `first` is true for the first such commit of the client, which is the one to log.
    PassUnblocked { first: bool },
    /// The client is gone.
    Closed,
}

impl AcquireWaits {
    pub fn admit(&self, kind: Kind) -> Admission {
        if self.closed.load(Ordering::SeqCst) {
            return Admission::Closed;
        }
        let mut waits = self.waits.lock().unwrap_or_else(|e| e.into_inner());
        match kind {
            Kind::AcquirePoint if waits.count(kind) >= ACQUIRE_LIMIT => {
                return Admission::Disconnect
            }
            Kind::ImplicitFence if waits.count(kind) >= IMPLICIT_LIMIT => {
                return Admission::PassUnblocked {
                    first: !self.warned.swap(true, Ordering::SeqCst),
                }
            }
            _ => {}
        }
        waits.next += 1;
        let id = waits.next;
        waits.live.insert(id, Wait { kind, token: None });
        Admission::Granted(id)
    }

    pub fn registered(&self, id: u64, token: RegistrationToken) {
        if let Some(slot) = self
            .waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .get_mut(&id)
        {
            slot.token = Some(token);
        }
    }

    /// The wait ended: its source fired (and removed itself), or it never got into the loop.
    pub fn cleared(&self, id: u64) {
        self.waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .remove(&id);
    }

    pub fn live(&self) -> usize {
        self.waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .len()
    }

    /// The client disconnected: no further wait is admitted, and [`sweep`] removes the sources that are left.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Whether the registry has this client's waits yet; true on the first call only.
    pub fn first_listing(&self) -> bool {
        !self.listed.swap(true, Ordering::SeqCst)
    }

    fn take_tokens(&self) -> Vec<RegistrationToken> {
        self.waits
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .live
            .drain()
            .filter_map(|(_, wait)| wait.token)
            .collect()
    }
}

/// Removes the sources of the clients that disconnected and forgets those clients. The sources of a client that is
/// still connected stay.
pub fn sweep<D>(registry: &mut Vec<Arc<AcquireWaits>>, handle: &LoopHandle<'_, D>) {
    registry.retain(|waits| {
        if !waits.closed.load(Ordering::SeqCst) {
            return true;
        }
        for token in waits.take_tokens() {
            handle.remove(token);
        }
        false
    });
}
