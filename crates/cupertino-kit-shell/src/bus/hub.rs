//! The host bus inside one host: each component's queue, the retained publications, and the hand-off of every
//! publication to the bridges ([`super::bridge`]) in the order the queues see it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

use super::bridge::{Bridges, Came};
use super::{Message, Panel, Sender, Topic, MAX_SUBSCRIBER_QUEUE};
use crate::ComponentName;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum RetainedKey {
    PanelShown(Sender, Panel),
    DoNotDisturbChanged(Sender),
    SessionMenu(Sender),
}

impl RetainedKey {
    fn for_message(from: Sender, msg: &Message) -> Option<Self> {
        match msg {
            Message::PanelShown { panel, .. } => Some(Self::PanelShown(from, *panel)),
            Message::DoNotDisturbChanged { .. } => Some(Self::DoNotDisturbChanged(from)),
            Message::SessionMenu { .. } => Some(Self::SessionMenu(from)),
            _ => None,
        }
    }

    fn sender(self) -> Sender {
        match self {
            Self::PanelShown(s, _) | Self::DoNotDisturbChanged(s) | Self::SessionMenu(s) => s,
        }
    }
}

struct Subscriber {
    id: u64,
    component: ComponentName,
    topics: HashSet<Topic>,
    queue: VecDeque<(Sender, Message)>,
    warned_overflow: bool,
    /// The component's loop waiting for its queue to fill ([`BusHub::poll_drain`]).
    waker: Option<Waker>,
}

#[derive(Default)]
pub(super) struct BusHubInner {
    next_id: u64,
    subscribers: Vec<Subscriber>,
    retained: HashMap<RetainedKey, (Sender, Message)>,
}

impl BusHubInner {
    /// The retained publications whose publisher `keep` accepts.
    pub(super) fn retained(&self, keep: impl Fn(Sender) -> bool) -> Vec<(Sender, Message)> {
        self.retained
            .values()
            .filter(|(from, _)| keep(*from))
            .cloned()
            .collect()
    }
}

/// In-process host-bus router shared by all components in a [`crate::hosting::Runtime`], and the hub of its bridges.
#[derive(Default)]
pub(crate) struct BusHub {
    inner: Mutex<BusHubInner>,
    bridges: Bridges,
}

impl BusHub {
    /// The bridges to the other hosts.
    pub(crate) fn bridges(&self) -> &Bridges {
        &self.bridges
    }

    /// The bus's state, locked. Lock order: this lock before the bridges'.
    pub(super) fn lock(&self) -> MutexGuard<'_, BusHubInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn register(&self, component: ComponentName) -> u64 {
        let mut inner = self.lock();
        inner.next_id += 1;
        let id = inner.next_id;
        inner.subscribers.push(Subscriber {
            id,
            component,
            topics: HashSet::new(),
            queue: VecDeque::new(),
            warned_overflow: false,
            waker: None,
        });
        id
    }

    /// Takes subscriber `sub_id` of `component` off the bus; the component's retained publications go when no other
    /// subscriber of it is left. A subscriber already off the bus (cut off by `Running::shut` before its thread ended)
    /// changes nothing: what is retained of the component now came from another instance, as one in a separate host.
    pub fn unregister(&self, sub_id: u64, component: ComponentName) {
        let mut inner = self.lock();
        let before = inner.subscribers.len();
        inner.subscribers.retain(|s| s.id != sub_id);
        if inner.subscribers.len() == before {
            return;
        }
        let still_present = inner.subscribers.iter().any(|s| s.component == component);
        if !still_present {
            let sender = Sender::Component(component);
            inner.retained.retain(|k, _| k.sender() != sender);
        }
    }

    /// Drops the retained publications of `sender`, a publisher that stopped.
    pub(crate) fn forget(&self, sender: Sender) {
        self.lock().retained.retain(|k, _| k.sender() != sender);
    }

    pub fn follow(&self, sub_id: u64, topics: &[Topic]) {
        let mut inner = self.lock();
        let retained_snapshot = inner.retained(|_| true);
        if let Some(sub) = inner.subscribers.iter_mut().find(|s| s.id == sub_id) {
            let mut newly_followed = Vec::new();
            for &topic in topics {
                if sub.topics.insert(topic) {
                    newly_followed.push(topic);
                }
            }
            for (from, msg) in retained_snapshot {
                if newly_followed.contains(&msg.topic()) && from != Sender::Component(sub.component)
                {
                    push_bounded(sub, from, msg);
                }
            }
        }
    }

    /// Whether subscriber `sub_id` is still on the bus: a component that was shut is not.
    pub(crate) fn is_registered(&self, sub_id: u64) -> bool {
        self.lock().subscribers.iter().any(|s| s.id == sub_id)
    }

    pub fn publish_from_component(
        &self,
        component: ComponentName,
        publisher_sub_id: Option<u64>,
        message: Message,
    ) {
        if message.topic() == Topic::Host {
            tracing::warn!(
                component = component.as_str(),
                "component attempted to publish Topic::Host; dropped"
            );
            return;
        }
        if publisher_sub_id.is_some_and(|id| !self.is_registered(id)) {
            // a component that was shut publishes nothing (`Running::shut`)
            return;
        }
        self.deliver(
            Sender::Component(component),
            publisher_sub_id,
            message,
            Came::Here,
        );
    }

    pub fn publish_from_host(&self, allow_host_topic: bool, message: Message) {
        if message.topic() == Topic::Host && !allow_host_topic {
            tracing::warn!("separate host attempted to publish Topic::Host; dropped");
            return;
        }
        self.deliver(Sender::Host, None, message, Came::Here);
    }

    /// A publication that came over a bridge, already checked by it.
    pub(super) fn deliver_bridged(&self, from: Sender, message: Message, came: Came) {
        self.deliver(from, None, message, came);
    }

    fn deliver(&self, from: Sender, skip_sub_id: Option<u64>, message: Message, came: Came) {
        let topic = message.topic();
        let mut inner = self.lock();
        if let Some(key) = RetainedKey::for_message(from, &message) {
            inner.retained.insert(key, (from, message.clone()));
        }
        for sub in &mut inner.subscribers {
            if Some(sub.id) == skip_sub_id {
                continue;
            }
            if from == Sender::Component(sub.component) {
                continue;
            }
            if sub.topics.contains(&topic) {
                push_bounded(sub, from, message.clone());
            }
        }
        // under the bus's lock, so every bridge sees publications in the order the queues do
        self.bridges.forward(from, &message, came);
    }

    /// The publications waiting in `sub_id`'s queue, or `Pending` with `cx`'s task woken by the next one. A subscriber
    /// that left the bus waits for good.
    pub(crate) fn poll_drain(
        &self,
        sub_id: u64,
        cx: &mut Context<'_>,
    ) -> Poll<Vec<(Sender, Message)>> {
        let mut inner = self.lock();
        let Some(sub) = inner.subscribers.iter_mut().find(|s| s.id == sub_id) else {
            return Poll::Pending;
        };
        if sub.queue.is_empty() {
            sub.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        Poll::Ready(sub.queue.drain(..).collect())
    }
}

fn push_bounded(sub: &mut Subscriber, from: Sender, msg: Message) {
    if let Some(waker) = sub.waker.take() {
        waker.wake();
    }
    if sub.queue.len() >= MAX_SUBSCRIBER_QUEUE {
        sub.queue.pop_front();
        if !sub.warned_overflow {
            sub.warned_overflow = true;
            tracing::warn!(
                component = sub.component.as_str(),
                "host bus subscriber queue reached {MAX_SUBSCRIBER_QUEUE}; dropping oldest message"
            );
        }
    }
    sub.queue.push_back((from, msg));
}
