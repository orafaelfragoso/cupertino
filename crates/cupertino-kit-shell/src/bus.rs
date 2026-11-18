//! The host bus's message catalogue. A bridge carries the same values as a local publication.

use crate::{outputs::OutputId, transaction, ComponentName};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
