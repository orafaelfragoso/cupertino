//! The host bus's message catalogue, in-process & bridged delivery, and session-bus identity (`D20`).

pub(crate) mod bridge;
mod hub;
mod identity;

use cupertino_kit::core::Command;
use cupertino_peer::component::{resolve_component, ComponentCaller};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::context;
use crate::{outputs::OutputId, transaction, ComponentName};

pub use bridge::{BRIDGE_VERSION, MAX_BRIDGE_QUEUE};
pub(crate) use hub::BusHub;

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

/// Publish `message` on the host bus from the calling thread's component.
/// Messages on [`Topic::Host`] are dropped and logged.
pub fn publish(message: Message) {
    context::publish_bus(message);
}

/// Follow `topics` on the host bus; retained messages on those topics arrive immediately.
pub fn follow(topics: &[Topic]) {
    context::follow_bus(topics);
}

/// The component's connection to the session bus, as `cupertino_kit::bus::connect` gives one, on a socket the host
/// can shut (`hosting::Running::shut`). The crate announces the connection's unique name on the component's Wayland
/// connection and proves it with the nonce the server answers (`org.cupertino.Session1.AnnounceComponent`, D20);
/// [`crate::Event::BusIdentity`] says how that went, and `connected` gets the connection after it, accepted or not,
/// so no call of the component precedes its identity. On a thread that runs no host component (a program of its own)
/// the connection is made as Kit makes it, and nothing is announced.
pub fn session<M: Send + 'static>(
    connected: impl FnOnce(zbus::Result<zbus::Connection>) -> M + Send + 'static,
    closed: M,
) -> Vec<Command<M>> {
    identity::session(connected, closed)
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
        // a program the server started itself is no component
        ComponentCaller::Program(_) | ComponentCaller::Other => Caller::Other,
    }
}
