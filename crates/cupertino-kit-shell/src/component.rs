use serde::{Deserialize, Deserializer, Serialize};

/// A name from the server's component table. Its wire form is that name, never an index in the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ComponentName(&'static str);

impl ComponentName {
    /// The menu bar.
    pub const MENUBAR: Self = Self("menubar");
    /// The dock and application switcher.
    pub const DOCK: Self = Self("dock");
    /// The desktop items.
    pub const DESKTOP: Self = Self("desktop");
    /// The window manager and stage strip.
    pub const WINDOWMANAGER: Self = Self("windowmanager");
    /// The overview.
    pub const OVERVIEW: Self = Self("overview");
    /// The launcher.
    pub const LAUNCHER: Self = Self("launcher");
    /// Banners and the notification list.
    pub const NOTIFICATIONS: Self = Self("notifications");
    /// The control panel.
    pub const CONTROLCENTER: Self = Self("controlcenter");
    /// The on-screen displays.
    pub const OSD: Self = Self("osd");
    /// The session agent.
    pub const AGENT: Self = Self("agent");
    /// The portal's choosers.
    pub const DIALOGS: Self = Self("dialogs");

    /// The name as the server sends it.
    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// `None` for a name absent from this build's table.
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::MENUBAR,
            Self::DOCK,
            Self::DESKTOP,
            Self::WINDOWMANAGER,
            Self::OVERVIEW,
            Self::LAUNCHER,
            Self::NOTIFICATIONS,
            Self::CONTROLCENTER,
            Self::OSD,
            Self::AGENT,
            Self::DIALOGS,
        ]
        .into_iter()
        .find(|n| n.as_str() == name)
    }
}

impl<'de> Deserialize<'de> for ComponentName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::from_name(&String::deserialize(deserializer)?)
            .ok_or_else(|| serde::de::Error::custom("unknown component name"))
    }
}
