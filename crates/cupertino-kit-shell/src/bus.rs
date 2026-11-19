//! The host bus's message catalogue, in-process & bridged delivery, and session-bus identity (`D20`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::sync::Mutex;

use cupertino_kit::core::Command;
use cupertino_peer::component::{resolve_component, ComponentCaller};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::context;
use crate::{outputs::OutputId, transaction, ComponentName};

/// Maximum queued messages per subscriber before the oldest message is dropped (`shell-host.md#the-host-bus`).
pub const MAX_SUBSCRIBER_QUEUE: usize = 256;

/// Maximum encoded size of one bridged host-bus frame in bytes (`shell-host.md#the-host-bus`).
pub const MAX_BRIDGE_FRAME_BYTES: usize = 64 * 1024;

/// What one component publishes to the others.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Message {
    /// The shared host reports that the server gave this component up.
    ComponentStopped { component: ComponentName },
    /// Open the panel or close it if it is open.
    TogglePanel {
        panel: Panel,
        output: Option<OutputId>,
    },
    /// Whether a panel is visible, and where.
    PanelShown {
        panel: Panel,
        shown: bool,
        output: Option<OutputId>,
    },
    /// Change Do Not Disturb for the selected duration.
    SetDoNotDisturb { on: bool, lifetime: Option<String> },
    /// The current state and the available duration identifiers and titles.
    DoNotDisturbChanged {
        on: bool,
        lifetimes: Vec<(String, String)>,
    },
    /// The agent's session menu.
    SessionMenu {
        user_name: String,
        lock: bool,
        log_out: bool,
        switch_user: bool,
        sleep: bool,
        restart: bool,
        shut_down: bool,
    },
    /// Display the level without changing it.
    ShowOsd { kind: OsdKind, level: f64 },
    /// Join the creator's transaction for this scene.
    Transaction {
        id: transaction::Id,
        output: OutputId,
        scene: Scene,
    },
}

/// A panel the shell opens or closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Panel {
    ControlPanel,
    NotificationCenter,
    Launcher,
    Applications,
}

/// The level an on-screen display shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OsdKind {
    Volume,
    Mute,
    Brightness,
    KeyboardBrightness,
}

/// The scene whose participants join a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scene {
    OverviewOpening,
    OverviewClosing,
    StageSwitch,
}

/// The publications a subscriber asks to follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Topic {
    Host,
    Panels,
    DoNotDisturb,
    Session,
    Osd,
    Transactions,
}

impl Message {
    /// The subscribers this publication goes to.
    pub fn topic(&self) -> Topic {
        match self {
            Self::ComponentStopped { .. } => Topic::Host,
            Self::TogglePanel { .. } | Self::PanelShown { .. } => Topic::Panels,
            Self::SetDoNotDisturb { .. } | Self::DoNotDisturbChanged { .. } => Topic::DoNotDisturb,
            Self::SessionMenu { .. } => Topic::Session,
            Self::ShowOsd { .. } => Topic::Osd,
            Self::Transaction { .. } => Topic::Transactions,
        }
    }

    /// Whether a subscriber joining later receives the last publication from this sender.
    pub fn retained(&self) -> bool {
        matches!(
            self,
            Self::PanelShown { .. } | Self::DoNotDisturbChanged { .. } | Self::SessionMenu { .. }
        )
    }
}

/// A publisher as named by the bus: a component's name or `host`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sender {
    Host,
    Component(ComponentName),
}

impl Serialize for Sender {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(match self {
            Self::Host => "host",
            Self::Component(component) => component.as_str(),
        })
    }
}

impl<'de> Deserialize<'de> for Sender {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        match name.as_str() {
            "host" => Ok(Self::Host),
            name => ComponentName::from_name(name)
                .map(Self::Component)
                .ok_or_else(|| serde::de::Error::custom("unknown host bus sender")),
        }
    }
}

/// Wire envelope carried over a host-bus bridge socket.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WireEnvelope {
    pub from: Sender,
    pub message: Message,
}

/// Encode one host-bus envelope with a 4-byte little-endian length prefix, enforcing [`MAX_BRIDGE_FRAME_BYTES`].
pub(crate) fn write_bridge_frame(
    writer: &mut impl Write,
    from: Sender,
    message: &Message,
) -> io::Result<()> {
    let payload = serde_json::to_vec(&WireEnvelope {
        from,
        message: message.clone(),
    })
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if payload.len() > MAX_BRIDGE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "host bus frame {} bytes exceeds {} byte limit",
                payload.len(),
                MAX_BRIDGE_FRAME_BYTES
            ),
        ));
    }
    writer.write_all(&(payload.len() as u32).to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

/// Decode one length-prefixed host-bus frame from `reader`.
pub(crate) fn read_bridge_frame(reader: &mut impl Read) -> io::Result<WireEnvelope> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_BRIDGE_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("host bus frame {len} bytes exceeds {MAX_BRIDGE_FRAME_BYTES} byte limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

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
}

#[derive(Default)]
struct BusHubInner {
    next_id: u64,
    subscribers: Vec<Subscriber>,
    retained: HashMap<RetainedKey, (Sender, Message)>,
}

/// In-process host-bus router shared by all components in a [`crate::hosting::Runtime`].
#[derive(Default)]
pub(crate) struct BusHub {
    inner: Mutex<BusHubInner>,
}

impl BusHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, component: ComponentName) -> u64 {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.next_id += 1;
        let id = inner.next_id;
        inner.subscribers.push(Subscriber {
            id,
            component,
            topics: HashSet::new(),
            queue: VecDeque::new(),
            warned_overflow: false,
        });
        id
    }

    pub fn unregister(&self, sub_id: u64, component: ComponentName) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.subscribers.retain(|s| s.id != sub_id);
        let still_present = inner.subscribers.iter().any(|s| s.component == component);
        if !still_present {
            let sender = Sender::Component(component);
            inner.retained.retain(|k, _| k.sender() != sender);
        }
    }

    pub fn follow(&self, sub_id: u64, topics: &[Topic]) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let retained_snapshot: Vec<(Sender, Message)> = inner
            .retained
            .values()
            .filter(|(_, msg)| topics.contains(&msg.topic()))
            .cloned()
            .collect();
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
        self.deliver(Sender::Component(component), publisher_sub_id, message);
    }

    pub fn publish_from_host(&self, allow_host_topic: bool, message: Message) {
        if message.topic() == Topic::Host && !allow_host_topic {
            tracing::warn!("separate host attempted to publish Topic::Host; dropped");
            return;
        }
        self.deliver(Sender::Host, None, message);
    }

    pub fn deliver_bridged(&self, from: Sender, message: Message) {
        self.deliver(from, None, message);
    }

    fn deliver(&self, from: Sender, skip_sub_id: Option<u64>, message: Message) {
        let topic = message.topic();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    }

    pub fn drain(&self, sub_id: u64) -> Vec<(Sender, Message)> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner
            .subscribers
            .iter_mut()
            .find(|s| s.id == sub_id)
            .map(|s| s.queue.drain(..).collect())
            .unwrap_or_default()
    }
}

fn push_bounded(sub: &mut Subscriber, from: Sender, msg: Message) {
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

/// Publish `message` on the host bus from the calling thread's component.
/// Messages on [`Topic::Host`] are dropped and logged.
pub fn publish(message: Message) {
    context::publish_bus(message);
}

/// Follow `topics` on the host bus; retained messages on those topics arrive immediately.
pub fn follow(topics: &[Topic]) {
    context::follow_bus(topics);
}

/// Open the calling component's session-bus connection (`cupertino_kit::bus::connect`).
pub fn session<M: Send + 'static>(
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> M + Send + 'static,
    closed: M,
) -> Vec<Command<M>> {
    cupertino_kit::bus::connect(cupertino_kit::bus::Bus::Session, connected, closed)
}

/// Resolved identity of a session-bus caller (D20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Caller {
    /// The compositor server (owner of `org.cupertino.Session1`).
    Server,
    /// A host component accepted by the server through its Wayland nonce.
    Component(ComponentName),
    /// Any other caller (including unannounced host connections or applications).
    Other,
}

/// Resolve `sender` on `connection` through `cupertino-peer` (D20).
/// Always awaited on the bus executor, never blocking a component UI thread.
pub async fn caller(connection: &zbus::Connection, sender: &zbus::names::UniqueName<'_>) -> Caller {
    match resolve_component(connection, sender).await {
        ComponentCaller::Server => Caller::Server,
        ComponentCaller::Component(name) => ComponentName::from_name(&name)
            .map(Caller::Component)
            .unwrap_or(Caller::Other),
        ComponentCaller::Other => Caller::Other,
    }
}
