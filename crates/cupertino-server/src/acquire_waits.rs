//! Commits held back until a buffer is ready (an explicit-sync acquire point, or the implicit fences of a dmabuf). Each wait
//! is an event source in the main loop with a file descriptor of its own, so a client may have only [`LIMIT`] at once, and
//! the sources of a client that went away are removed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::{LoopHandle, RegistrationToken};

/// Most commits of one client that may wait for their buffers at the same time. A client has at most a few buffers in
/// flight, so more than this is a client that never signals what it set.
pub const LIMIT: usize = 64;

#[derive(Debug, Default)]
struct Waits {
    next: u64,
    /// The waits of the client: reserved, and registered once the source is in the loop.
    live: HashMap<u64, Option<RegistrationToken>>,
}

/// One client's waits, kept in its [`ClientState`](crate::state::ClientState).
#[derive(Debug, Default)]
pub struct AcquireWaits {
    waits: Mutex<Waits>,
    closed: AtomicBool,
    listed: AtomicBool,
}

/// What admitting one more wait decided.
#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    /// The wait may go on; give its id back to [`AcquireWaits::registered`] or [`AcquireWaits::cleared`].
    Granted(u64),
    /// The client already has [`LIMIT`] waits.
    OverLimit,
    /// The client is gone.
    Closed,
}

impl AcquireWaits {
    pub fn admit(&self) -> Admission {
        if self.closed.load(Ordering::SeqCst) {
            return Admission::Closed;
        }
        let mut waits = self.waits.lock().unwrap_or_else(|e| e.into_inner());
        if waits.live.len() >= LIMIT {
            return Admission::OverLimit;
        }
        waits.next += 1;
        let id = waits.next;
        waits.live.insert(id, None);
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
            *slot = Some(token);
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
            .filter_map(|(_, token)| token)
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

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::calloop::ping::{make_ping, Ping};
    use smithay::reexports::calloop::EventLoop;
    use std::time::Duration;

    /// A wait whose source never fires on its own: the client set a point it never signals.
    fn stuck_wait(waits: &AcquireWaits, handle: &LoopHandle<'_, usize>) -> Option<Ping> {
        let Admission::Granted(id) = waits.admit() else {
            return None;
        };
        let (ping, source) = make_ping().unwrap();
        let token = handle
            .insert_source(source, move |_, _, fired| *fired += 1)
            .unwrap();
        waits.registered(id, token);
        Some(ping)
    }

    fn dispatch(event_loop: &mut EventLoop<'_, usize>) -> usize {
        let mut fired = 0;
        event_loop
            .dispatch(Some(Duration::from_millis(10)), &mut fired)
            .unwrap();
        fired
    }

    #[test]
    fn a_client_is_refused_its_waits_past_the_limit_until_one_ends() {
        let waits = AcquireWaits::default();
        let ids: Vec<u64> = (0..LIMIT)
            .map(|_| match waits.admit() {
                Admission::Granted(id) => id,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(waits.admit(), Admission::OverLimit);
        waits.cleared(ids[0]);
        assert!(matches!(waits.admit(), Admission::Granted(_)));
        assert_eq!(waits.admit(), Admission::OverLimit);
        assert_eq!(waits.live(), LIMIT);
    }

    #[test]
    fn a_disconnected_client_leaves_no_source_in_the_loop() {
        let mut event_loop: EventLoop<usize> = EventLoop::try_new().unwrap();
        let handle = event_loop.handle();
        let gone = Arc::new(AcquireWaits::default());
        let staying = Arc::new(AcquireWaits::default());
        let gone_pings: Vec<Ping> = (0..3).filter_map(|_| stuck_wait(&gone, &handle)).collect();
        let staying_ping = stuck_wait(&staying, &handle).unwrap();
        let mut registry = vec![gone.clone(), staying.clone()];

        gone.close();
        assert_eq!(gone.admit(), Admission::Closed);
        sweep(&mut registry, &handle);

        assert_eq!(registry.len(), 1);
        assert_eq!(gone.live(), 0);
        for ping in &gone_pings {
            ping.ping();
        }
        assert_eq!(
            dispatch(&mut event_loop),
            0,
            "a removed source must not fire"
        );
        staying_ping.ping();
        assert_eq!(
            dispatch(&mut event_loop),
            1,
            "the connected client's wait stays"
        );
    }

    #[test]
    fn a_client_is_listed_once() {
        let waits = AcquireWaits::default();
        assert!(waits.first_listing());
        assert!(!waits.first_listing());
    }
}
