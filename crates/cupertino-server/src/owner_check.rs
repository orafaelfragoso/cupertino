//! Whether a Wayland client owns a bus name: the one check behind every bus name a client hands the server for another
//! process to use (the menus a window announces). A request names the client and the name; the D-Bus worker asks the
//! bus daemon ([`crate::dbus::OwnerQuery`]) and the answer comes back to the loop as an [`OwnerChecked`] for the caller
//! of its key. The match itself (pidfds on both sides, the sandbox instance) stays behind this module.
//!
//! Requests are keyed by what they are for ([`OwnerKey`]), and the latest request of a key wins. At most one check of a
//! key is on the worker: a request made while one is out waits in its place, replacing any request already waiting,
//! and is sent when the check out returns, whose answer is dropped. A key thus has one check out and one waiting at
//! most, however often a client asks. The answer of a cancelled key is dropped as well.

use std::collections::HashMap;

use cupertino_peer::ProcessHandle;
use smithay::reexports::calloop::channel::{self, Event, Sender};
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::Client;

use crate::dbus::OwnerQuery;
pub use crate::dbus::OwnerRefusal;
use crate::state::{ClientState, Cupertino};

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

/// A request to send.
struct Request {
    generation: u64,
    name: String,
    /// The client's process, from its connection.
    client: Option<ProcessHandle>,
}

/// The checks of one key.
struct Slot {
    /// The generation of the check on the worker.
    checking: u64,
    /// The latest request, made while that check was out.
    waiting: Option<Request>,
    cancelled: bool,
}

/// The answer the worker sends back to the loop.
struct Reply {
    key: OwnerKey,
    generation: u64,
    verdict: Result<String, OwnerRefusal>,
}

/// The keys with a check on the worker.
pub struct OwnerChecks {
    slots: HashMap<OwnerKey, Slot>,
    /// The generation of the latest request of any key.
    generations: u64,
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
            slots: HashMap::new(),
            generations: 0,
            replies,
        })
    }

    /// How many keys have a check on the worker.
    pub fn in_flight(&self) -> usize {
        self.slots.len()
    }
}

impl Cupertino {
    /// Check who owns `check.name`; the answer comes as an [`OwnerChecked`] for `check.key`, unless a newer request of
    /// the key or its cancellation comes first.
    pub fn check_owner(&mut self, check: OwnerCheck) {
        let OwnerCheck { key, name, client } = check;
        let checks = &mut self.owner_checks;
        checks.generations += 1;
        let request = Request {
            generation: checks.generations,
            name,
            client: client
                .get_data::<ClientState>()
                .and_then(|data| data.peer.as_ref()?.try_clone().ok()),
        };
        if let Some(slot) = checks.slots.get_mut(&key) {
            slot.waiting = Some(request);
            slot.cancelled = false;
            return;
        }
        checks.slots.insert(
            key.clone(),
            Slot {
                checking: request.generation,
                waiting: None,
                cancelled: false,
            },
        );
        self.send_owner_check(key, request);
    }

    /// Drop the answer to the requests of `key` made so far.
    pub fn cancel_owner_check(&mut self, key: &OwnerKey) {
        if let Some(slot) = self.owner_checks.slots.get_mut(key) {
            slot.waiting = None;
            slot.cancelled = true;
        }
    }

    fn send_owner_check(&mut self, key: OwnerKey, request: Request) {
        let Request {
            generation,
            name,
            client,
        } = request;
        let refusal = match (client, &self.dbus) {
            (None, _) => OwnerRefusal::NoClientProcess,
            (Some(_), None) => OwnerRefusal::NoBus,
            (Some(client), Some(dbus)) => {
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
                if dbus.check_owner(OwnerQuery {
                    name,
                    client,
                    reply,
                }) {
                    return;
                }
                OwnerRefusal::NoBus
            }
        };
        self.owner_replied(key, generation, Err(refusal));
    }

    /// The check of generation `generation` of `key` returned: the request waiting is sent in its place, or the answer
    /// goes to the caller unless the key was cancelled.
    fn owner_replied(
        &mut self,
        key: OwnerKey,
        generation: u64,
        verdict: Result<String, OwnerRefusal>,
    ) {
        let Some(slot) = self.owner_checks.slots.get_mut(&key) else {
            return;
        };
        if slot.checking != generation {
            return;
        }
        if let Some(waiting) = slot.waiting.take() {
            slot.checking = waiting.generation;
            return self.send_owner_check(key, waiting);
        }
        let cancelled = self
            .owner_checks
            .slots
            .remove(&key)
            .is_some_and(|slot| slot.cancelled);
        if !cancelled {
            self.owner_checked(OwnerChecked { key, verdict });
        }
    }

    /// Hand an answer to the caller of its key.
    fn owner_checked(&mut self, checked: OwnerChecked) {
        match checked.key {
            OwnerKey::MenuAddress(surface) => self.menu_owner_checked(surface, checked.verdict),
        }
    }
}
