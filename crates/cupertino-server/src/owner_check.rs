//! Whether a Wayland client owns a bus name: the one check behind every bus name a client hands the server for another
//! process to use (the menus a window announces). A request names the client and the name; the D-Bus worker asks the
//! bus daemon ([`crate::dbus::OwnerQuery`]) and the answer comes back to the loop as an [`OwnerChecked`] for the caller
//! of its key. The match itself (pidfds on both sides, the sandbox instance) stays behind this module.
//!
//! Requests are keyed by what they are for ([`OwnerKey`]), and the latest request of a key wins. At most one check of a
//! key runs on the worker: a request made while one runs waits in its place, replacing any request already waiting, and
//! is queued when the running check returns, whose answer is dropped. A key thus has one check running and one waiting
//! at most, however often a client asks. Cancelling a key drops its answer and takes it out of the queue.
//!
//! The queue is first in, first out. At most [`RUNNING_LIMIT`] checks run at once, and at most [`CLIENT_RUNNING_LIMIT`]
//! of one client, so a client with many surfaces waits behind its own checks while other clients' run. A check that has
//! not returned after [`DEADLINE`] is answered [`OwnerRefusal::TimedOut`] and its key goes on; its late answer is dropped.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::time::Duration;

use cupertino_peer::ProcessHandle;
use smithay::reexports::calloop::channel::{self, Event, Sender};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};
use smithay::reexports::wayland_server::backend::{ClientId, ObjectId};
use smithay::reexports::wayland_server::Client;

use crate::dbus::OwnerQuery;
pub use crate::dbus::OwnerRefusal;
use crate::state::{ClientState, Cupertino};

/// How many checks run on the D-Bus worker at once.
pub const RUNNING_LIMIT: usize = 8;
/// How many checks of one client run at once.
pub const CLIENT_RUNNING_LIMIT: usize = 2;
/// How long a check may run before it is answered as timed out.
pub const DEADLINE: Duration = Duration::from_secs(3);

/// What a check is for; the latest check of a key wins.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum OwnerKey {
    /// The menus a surface announced (`crate::protocols`), by the surface.
    MenuAddress(ObjectId),
}

/// A request: is `name` (unique or well-known) owned by `client`, or by a process of its sandbox instance?
pub struct OwnerCheck {
    pub key: OwnerKey,
    pub name: String,
    pub client: Client,
}

/// The answer to the latest request of `key`: the owner's unique name, or why the owner is not the client's.
pub struct OwnerChecked {
    pub key: OwnerKey,
    pub verdict: Result<String, OwnerRefusal>,
}

/// The checks of one key.
struct Slot<C, R> {
    client: C,
    /// The generation of the check running on the worker.
    running: Option<u64>,
    /// The latest request not sent yet: queued, or waiting for the running check.
    waiting: Option<(u64, R)>,
    cancelled: bool,
}

/// A check to send: its key, its generation and the request.
pub(crate) type Start<K, R> = (K, u64, R);

/// The bookkeeping of the checks, by key `K` of a client `C`, with requests `R`.
pub(crate) struct Scheduler<K, C, R> {
    slots: HashMap<K, Slot<C, R>>,
    /// Keys with a request to send, oldest first.
    queue: VecDeque<K>,
    /// The generation of the latest request of any key.
    generations: u64,
}

impl<K: Clone + Eq + Hash, C: Eq, R> Scheduler<K, C, R> {
    pub(crate) fn new() -> Self {
        Self {
            slots: HashMap::new(),
            queue: VecDeque::new(),
            generations: 0,
        }
    }

    fn running_of(&self, client: Option<&C>) -> usize {
        self.slots
            .values()
            .filter(|s| s.running.is_some() && client.is_none_or(|c| &s.client == c))
            .count()
    }

    /// The latest request of `key` from `client`; the checks to send now.
    pub(crate) fn request(&mut self, key: K, client: C, request: R) -> Vec<Start<K, R>> {
        self.generations += 1;
        let waiting = Some((self.generations, request));
        match self.slots.get_mut(&key) {
            Some(slot) => {
                slot.waiting = waiting;
                slot.cancelled = false;
            }
            None => {
                self.slots.insert(
                    key.clone(),
                    Slot {
                        client,
                        running: None,
                        waiting,
                        cancelled: false,
                    },
                );
                self.queue.push_back(key);
            }
        }
        self.pump()
    }

    /// Drop the answer to the requests of `key` made so far.
    pub(crate) fn cancel(&mut self, key: &K) {
        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };
        if slot.running.is_some() {
            slot.waiting = None;
            slot.cancelled = true;
        } else {
            self.slots.remove(key);
            self.queue.retain(|k| k != key);
        }
    }

    /// The check of generation `generation` of `key` returned or timed out: whether its answer goes to the caller,
    /// and the checks to send now.
    pub(crate) fn finished(&mut self, key: &K, generation: u64) -> (bool, Vec<Start<K, R>>) {
        let Some(slot) = self.slots.get_mut(key) else {
            return (false, Vec::new());
        };
        // kept although the deadline token goes with the answer: a late answer must never stand for a newer request
        if slot.running != Some(generation) {
            return (false, Vec::new());
        }
        slot.running = None;
        let deliver = if slot.waiting.is_some() {
            self.queue.push_back(key.clone());
            false
        } else {
            self.slots.remove(key).is_some_and(|slot| !slot.cancelled)
        };
        (deliver, self.pump())
    }

    /// Send what the limits allow, oldest first, skipping the keys of a client at its own limit.
    fn pump(&mut self) -> Vec<Start<K, R>> {
        let mut starts = Vec::new();
        let mut i = 0;
        while i < self.queue.len() && self.running_of(None) < RUNNING_LIMIT {
            let client = &self.slots[&self.queue[i]].client;
            if self.running_of(Some(client)) >= CLIENT_RUNNING_LIMIT {
                i += 1;
                continue;
            }
            let key = self.queue.remove(i).expect("the index is in the queue");
            let slot = self.slots.get_mut(&key).expect("a queued key has a slot");
            let (generation, request) = slot.waiting.take().expect("a queued key has a request");
            slot.running = Some(generation);
            starts.push((key, generation, request));
        }
        starts
    }

    /// How many checks run on the worker.
    pub(crate) fn running(&self) -> usize {
        self.running_of(None)
    }

    /// How many keys have a check running or to send.
    pub(crate) fn pending(&self) -> usize {
        self.slots.len()
    }
}

/// A request to send: the name, and the client's process from its connection.
struct Request {
    name: String,
    client: Option<ProcessHandle>,
}

/// The answer the worker sends back to the loop.
struct Reply {
    key: OwnerKey,
    generation: u64,
    verdict: Result<String, OwnerRefusal>,
}

/// The checks of the server's clients.
pub struct OwnerChecks {
    scheduler: Scheduler<OwnerKey, ClientId, Request>,
    /// The deadline of each running check.
    deadlines: HashMap<(OwnerKey, u64), RegistrationToken>,
    replies: Sender<Reply>,
}

impl OwnerChecks {
    pub fn new(loop_handle: &LoopHandle<'static, Cupertino>) -> anyhow::Result<Self> {
        let (replies, rx) = channel::channel::<Reply>();
        loop_handle
            .insert_source(rx, |event, _, state| {
                if let Event::Msg(reply) = event {
                    state.owner_replied(reply.key, reply.generation, reply.verdict);
                }
            })
            .map_err(|e| anyhow::anyhow!("inserting the owner check replies: {e}"))?;
        Ok(Self {
            scheduler: Scheduler::new(),
            deadlines: HashMap::new(),
            replies,
        })
    }

    /// How many checks run on the D-Bus worker.
    pub fn in_flight(&self) -> usize {
        self.scheduler.running()
    }

    /// How many keys have a check running or queued.
    pub fn pending(&self) -> usize {
        self.scheduler.pending()
    }
}

impl Cupertino {
    /// Check who owns `check.name`; the answer comes as an [`OwnerChecked`] for `check.key`, unless a newer request of
    /// the key or its cancellation comes first.
    pub fn check_owner(&mut self, check: OwnerCheck) {
        let OwnerCheck { key, name, client } = check;
        let request = Request {
            name,
            client: client
                .get_data::<ClientState>()
                .and_then(|data| data.peer.as_ref()?.try_clone().ok()),
        };
        let starts = self
            .owner_checks
            .scheduler
            .request(key, client.id(), request);
        self.send_owner_checks(starts);
    }

    /// Drop the answer to the requests of `key` made so far.
    pub fn cancel_owner_check(&mut self, key: &OwnerKey) {
        self.owner_checks.scheduler.cancel(key);
    }

    fn send_owner_checks(&mut self, starts: Vec<Start<OwnerKey, Request>>) {
        let mut refused = Vec::new();
        for (key, generation, request) in starts {
            match self.send_owner_check(&key, generation, request) {
                Ok(()) => self.arm_owner_deadline(key, generation),
                Err(refusal) => refused.push((key, generation, refusal)),
            }
        }
        for (key, generation, refusal) in refused {
            self.owner_replied(key, generation, Err(refusal));
        }
    }

    fn send_owner_check(
        &mut self,
        key: &OwnerKey,
        generation: u64,
        request: Request,
    ) -> Result<(), OwnerRefusal> {
        let client = request.client.ok_or(OwnerRefusal::NoClientProcess)?;
        let dbus = self.dbus.as_ref().ok_or(OwnerRefusal::NoBus)?;
        let replies = self.owner_checks.replies.clone();
        let reply_key = key.clone();
        let reply = Box::new(move |verdict| {
            // the loop is gone only while the server shuts down
            let _ = replies.send(Reply {
                key: reply_key,
                generation,
                verdict,
            });
        });
        let queued = dbus.check_owner(OwnerQuery {
            name: request.name,
            client,
            reply,
        });
        queued.then_some(()).ok_or(OwnerRefusal::NoBus)
    }

    fn arm_owner_deadline(&mut self, key: OwnerKey, generation: u64) {
        let timer_key = key.clone();
        let armed =
            self.loop_handle
                .insert_source(Timer::from_duration(DEADLINE), move |_, _, state| {
                    state.owner_replied(timer_key.clone(), generation, Err(OwnerRefusal::TimedOut));
                    TimeoutAction::Drop
                });
        match armed {
            Ok(token) => {
                self.owner_checks.deadlines.insert((key, generation), token);
            }
            Err(e) => tracing::warn!(error = %e, "cannot time an owner check"),
        }
    }

    /// The check of generation `generation` of `key` returned or timed out: the answer goes to the caller unless the
    /// key was cancelled or asked again, and the checks the limits now allow are sent.
    fn owner_replied(
        &mut self,
        key: OwnerKey,
        generation: u64,
        verdict: Result<String, OwnerRefusal>,
    ) {
        if let Some(token) = self
            .owner_checks
            .deadlines
            .remove(&(key.clone(), generation))
        {
            self.loop_handle.remove(token);
        }
        let (deliver, starts) = self.owner_checks.scheduler.finished(&key, generation);
        if deliver {
            self.owner_checked(OwnerChecked { key, verdict });
        }
        self.send_owner_checks(starts);
    }

    /// Hand an answer to the caller of its key.
    fn owner_checked(&mut self, checked: OwnerChecked) {
        match checked.key {
            OwnerKey::MenuAddress(surface) => self.menu_owner_checked(surface, checked.verdict),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Checks = Scheduler<u32, u32, ()>;

    fn keys(starts: &[Start<u32, ()>]) -> Vec<u32> {
        starts.iter().map(|(k, _, _)| *k).collect()
    }

    #[test]
    fn one_client_runs_at_most_its_limit_and_the_rest_waits_in_order() {
        let mut checks = Checks::new();
        let mut started = Vec::new();
        for key in 0..100 {
            started.extend(checks.request(key, 1, ()));
        }
        assert_eq!(keys(&started), [0, 1]);
        assert_eq!(checks.running(), CLIENT_RUNNING_LIMIT);
        assert_eq!(checks.pending(), 100);
        let (deliver, next) = checks.finished(&0, started[0].1);
        assert!(deliver);
        assert_eq!(keys(&next), [2]);
        assert_eq!(checks.running(), CLIENT_RUNNING_LIMIT);
    }

    #[test]
    fn many_clients_run_at_most_the_global_limit() {
        let mut checks = Checks::new();
        let mut started = 0;
        for client in 0..10 {
            for key in 0..10 {
                started += checks.request(client * 100 + key, client, ()).len();
            }
        }
        assert_eq!(started, RUNNING_LIMIT);
        assert_eq!(checks.running(), RUNNING_LIMIT);
    }

    #[test]
    fn a_client_behind_one_with_many_checks_runs_at_once() {
        let mut checks = Checks::new();
        for key in 0..100 {
            checks.request(key, 1, ());
        }
        assert_eq!(keys(&checks.request(1000, 2, ())), [1000]);
    }

    #[test]
    fn a_queued_request_is_replaced_in_place_and_a_cancelled_one_leaves_the_queue() {
        let mut checks = Checks::new();
        for key in [0, 1, 2, 3, 2] {
            checks.request(key, 1, ());
        }
        assert_eq!(checks.pending(), 4);
        checks.cancel(&2);
        assert_eq!(checks.pending(), 3);
        let (_, next) = checks.finished(&0, 1);
        assert_eq!(keys(&next), [3]);
    }

    #[test]
    fn a_request_made_while_its_key_runs_drops_the_running_answer_and_is_sent_after() {
        let mut checks = Checks::new();
        let first = checks.request(0, 1, ());
        assert!(checks.request(0, 1, ()).is_empty());
        let (deliver, next) = checks.finished(&0, first[0].1);
        assert!(!deliver);
        assert_eq!(keys(&next), [0]);
        assert!(checks.finished(&0, next[0].1).0);
        assert_eq!(checks.pending(), 0);
    }

    #[test]
    fn a_check_that_timed_out_frees_its_key_and_its_late_answer_is_dropped() {
        let mut checks = Checks::new();
        let first = checks.request(0, 1, ());
        checks.request(1, 1, ());
        checks.request(2, 1, ());
        // the deadline answers first
        let (deliver, next) = checks.finished(&0, first[0].1);
        assert!(deliver);
        assert_eq!(keys(&next), [2]);
        // then the worker
        assert_eq!(checks.finished(&0, first[0].1), (false, Vec::new()));
        // a new request of the key is not mistaken for the late answer
        checks.request(0, 1, ());
        assert_eq!(checks.finished(&0, first[0].1), (false, Vec::new()));
        assert_eq!(checks.pending(), 3);
    }

    #[test]
    fn an_answer_of_an_older_generation_is_dropped() {
        let mut checks = Checks::new();
        let first = checks.request(0, 1, ());
        let generation = first[0].1;
        assert_eq!(checks.finished(&0, generation + 1), (false, Vec::new()));
        assert_eq!(checks.finished(&0, generation - 1), (false, Vec::new()));
        assert_eq!(checks.running(), 1);
        assert!(checks.finished(&0, generation).0);
    }

    #[test]
    fn a_cancelled_running_key_is_removed_when_its_answer_comes() {
        let mut checks = Checks::new();
        let first = checks.request(0, 1, ());
        checks.cancel(&0);
        assert_eq!(checks.pending(), 1);
        assert_eq!(checks.finished(&0, first[0].1), (false, Vec::new()));
        assert_eq!(checks.pending(), 0);
    }
}
