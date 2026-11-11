//! The panes of the minimum set (§3) and what the sidebar and the search know
//! of them: title, symbol, enclosure colour, how their settings reach the system, and the authored search entries.
//!
//! Pane ids are ours (kebab-case; also the `--pane` argument). The symbol and enclosure colour of each row are the
//! reference's mapped to the asset directory's `symbol.* names; the Battery enclosure
//! colour is not known (approximate: green). Section titles and index words are original English text (the
//! reference's `*.searchTerms` are not copied, §5).

/// One pane of the minimum set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PaneId {
    Wifi,
    Bluetooth,
    Network,
    Battery,
    Accessibility,
    Appearance,
    ControlPanel,
    DesktopDock,
    Displays,
    Wallpaper,
    Sound,
    Notifications,
    Focus,
    LockScreen,
    Privacy,
    Keyboard,
    Mouse,
    Trackpad,
}

/// The fill of a row's icon enclosure: a system colour of the design tokens (names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enclosure {
    Blue,
    Black,
    Gray,
    Pink,
    Indigo,
    Green,
    Red,
}

/// How a pane's settings reach the running system (§2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// A daemon or the server serves a runtime interface: the change applies at once.
    Live,
    /// A file the owning process reads; it applies when the process reloads it.
    File,
}

/// What the sidebar shows of a pane and what the search finds in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pane {
    pub id: PaneId,
    pub title: &'static str,
    /// The `symbol.*` template symbol inside the enclosure.
    pub symbol: &'static str,
    pub enclosure: Enclosure,
    pub tier: Tier,
    /// The pane's section titles, each findable by the search.
    pub sections: &'static [&'static str],
    /// Further words the search finds the pane by.
    pub words: &'static [&'static str],
}

const fn pane(
    id: PaneId,
    title: &'static str,
    symbol: &'static str,
    enclosure: Enclosure,
    tier: Tier,
    sections: &'static [&'static str],
    words: &'static [&'static str],
) -> Pane {
    Pane {
        id,
        title,
        symbol,
        enclosure,
        tier,
        sections,
        words,
    }
}

/// Every pane of the minimum set.
pub const PANES: [Pane; 18] = [
    pane(
        PaneId::Wifi,
        "Wi-Fi",
        "symbol.wifi.fill",
        Enclosure::Blue,
        Tier::Live,
        &["Known Networks", "Other Networks"],
        &["wireless", "internet", "join", "password", "hotspot"],
    ),
    pane(
        PaneId::Bluetooth,
        "Bluetooth",
        "symbol.bluetooth.fill",
        Enclosure::Blue,
        Tier::Live,
        &["My Devices", "Nearby Devices"],
        &["pair", "wireless", "headphones", "connect"],
    ),
    pane(
        PaneId::Network,
        "Network",
        "symbol.network.fill",
        Enclosure::Blue,
        Tier::Live,
        &["Devices"],
        &["ethernet", "connection", "address", "interface"],
    ),
    pane(
        PaneId::Battery,
        "Battery",
        "symbol.battery.fill",
        Enclosure::Green,
        Tier::Live,
        &["Battery Level", "System Sleep"],
        &["power", "charge", "energy", "adapter"],
    ),
    pane(
        PaneId::Accessibility,
        "Accessibility",
        "symbol.accessibility.fill",
        Enclosure::Blue,
        Tier::Live,
        &[
            "Display",
            "Login Screen",
            "Screen Reader",
            "Pointer",
            "Keyboard",
        ],
        &[
            "screen reader",
            "read aloud",
            "speech",
            "reduce motion",
            "transparency",
            "contrast",
            "colour filter",
            "cursor",
            "pointer size",
            "shake mouse pointer to locate",
            "sticky keys",
            "slow keys",
            "mouse keys",
        ],
    ),
    pane(
        PaneId::Appearance,
        "Appearance",
        "symbol.appearance.fill",
        Enclosure::Black,
        Tier::Live,
        &["Appearance"],
        &["dark", "light", "mode", "theme"],
    ),
    pane(
        PaneId::ControlPanel,
        "Control Panel",
        "symbol.control-panel.fill",
        Enclosure::Gray,
        Tier::Live,
        &["Clock Options"],
        &["clock", "date", "time", "seconds", "menu bar", "24-hour"],
    ),
    pane(
        PaneId::DesktopDock,
        "Desktop & Dock",
        "symbol.desktop.fill",
        Enclosure::Black,
        Tier::Live,
        &["Dock", "Hot Corners"],
        &[
            "size",
            "magnification",
            "position",
            "minimize",
            "hide",
            "recent applications",
        ],
    ),
    pane(
        PaneId::Displays,
        "Displays",
        "symbol.sun.fill",
        Enclosure::Blue,
        Tier::Live,
        &["Resolution", "Brightness"],
        &["refresh rate", "rotation", "scale", "monitor"],
    ),
    pane(
        PaneId::Wallpaper,
        "Wallpaper",
        "symbol.photo.fill",
        Enclosure::Blue,
        Tier::Live,
        &["Pictures", "Folders"],
        &[
            "background",
            "desktop",
            "picture",
            "photo",
            "image",
            "add folder",
            "add photo",
        ],
    ),
    pane(
        PaneId::Sound,
        "Sound",
        "symbol.speaker.high.fill",
        Enclosure::Pink,
        Tier::Live,
        &["Output"],
        &["volume", "mute", "speakers", "audio"],
    ),
    pane(
        PaneId::Notifications,
        "Notifications",
        "symbol.bell.fill",
        Enclosure::Red,
        Tier::File,
        &["Lock Screen", "Application Notifications"],
        &[
            "notifications",
            "lock screen",
            "previews",
            "show previews",
            "banners",
            "alerts",
        ],
    ),
    pane(
        PaneId::Focus,
        "Focus",
        "symbol.moon.fill",
        Enclosure::Indigo,
        Tier::Live,
        &["Do Not Disturb"],
        &["notifications", "silence", "quiet"],
    ),
    pane(
        PaneId::LockScreen,
        "Lock Screen",
        "symbol.lock-screen.fill",
        Enclosure::Black,
        Tier::File,
        &["Display Sleep", "Password"],
        &["lock", "inactive", "screen off"],
    ),
    pane(
        PaneId::Privacy,
        "Privacy & Security",
        "symbol.privacy.fill",
        Enclosure::Blue,
        Tier::File,
        &["Clipboard Access", "Desktop Control", "Screen Capture"],
        &[
            "clipboard",
            "copy",
            "paste",
            "clipboard manager",
            "clipboard history",
            "data control",
            "automation",
            "remote control",
            "agent",
            "screenshot",
        ],
    ),
    pane(
        PaneId::Keyboard,
        "Keyboard",
        "symbol.keyboard.fill",
        Enclosure::Gray,
        Tier::Live,
        &["Key Repeat", "Modifier Keys", "Function Key"],
        &["repeat rate", "delay", "caps lock", "fn", "accents"],
    ),
    pane(
        PaneId::Mouse,
        "Mouse",
        "symbol.mouse.fill",
        Enclosure::Gray,
        Tier::Live,
        &["Tracking", "Scrolling"],
        &[
            "speed",
            "tracking speed",
            "acceleration",
            "pointer acceleration",
            "mouse resolution",
            "dpi",
            "natural scrolling",
            "pointer",
        ],
    ),
    pane(
        PaneId::Trackpad,
        "Trackpad",
        "symbol.trackpad.fill",
        Enclosure::Gray,
        Tier::Live,
        &["Tracking", "Scrolling"],
        &["speed", "tracking speed", "natural scrolling", "touchpad"],
    ),
];

impl PaneId {
    /// The pane with this id: every id has one.
    pub fn pane(self) -> &'static Pane {
        PANES
            .iter()
            .find(|p| p.id == self)
            .expect("every pane id is in PANES")
    }

    /// The title the window shows: the Battery row is "Energy Saver" on a machine without a battery
    /// (the row switches by predicate), "Battery" while that is not known.
    pub fn title(self, has_battery: Option<bool>) -> &'static str {
        match (self, has_battery) {
            (PaneId::Battery, Some(false)) => "Energy Saver",
            (pane, _) => pane.pane().title,
        }
    }

    /// The kebab-case id of the `--pane` argument.
    pub fn key(self) -> &'static str {
        match self {
            PaneId::Wifi => "wifi",
            PaneId::Bluetooth => "bluetooth",
            PaneId::Network => "network",
            PaneId::Battery => "battery",
            PaneId::Accessibility => "accessibility",
            PaneId::Appearance => "appearance",
            PaneId::ControlPanel => "control-panel",
            PaneId::DesktopDock => "desktop-dock",
            PaneId::Displays => "displays",
            PaneId::Wallpaper => "wallpaper",
            PaneId::Sound => "sound",
            PaneId::Notifications => "notifications",
            PaneId::Focus => "focus",
            PaneId::LockScreen => "lock-screen",
            PaneId::Privacy => "privacy",
            PaneId::Keyboard => "keyboard",
            PaneId::Mouse => "mouse",
            PaneId::Trackpad => "trackpad",
        }
    }

    /// The pane of a `--pane` argument; `None` for an unknown id.
    pub fn from_key(key: &str) -> Option<PaneId> {
        PANES.iter().map(|p| p.id).find(|id| id.key() == key)
    }
}

/// Every `symbol.*` name the sidebar draws, which the process requires of the asset directory at start.
pub fn symbols() -> Vec<&'static str> {
    PANES.iter().map(|p| p.symbol).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pane_has_one_entry_and_a_round_tripping_key() {
        for p in &PANES {
            assert_eq!(
                PANES.iter().filter(|q| q.id == p.id).count(),
                1,
                "{:?}",
                p.id
            );
            assert_eq!(PaneId::from_key(p.id.key()), Some(p.id));
            assert!(
                p.symbol.starts_with("symbol.") && p.symbol.ends_with(".fill"),
                "a tile wears a filled symbol: {}",
                p.symbol
            );
            assert!(!p.sections.is_empty(), "{} has a findable section", p.title);
        }
        assert_eq!(
            PaneId::from_key("general"),
            None,
            "a reference pane with no backend is not a pane"
        );
        assert_eq!(PaneId::Keyboard.pane().title, "Keyboard");
        assert_eq!(
            (
                PaneId::Battery.title(Some(false)),
                PaneId::Battery.title(Some(true)),
                PaneId::Battery.title(None)
            ),
            ("Energy Saver", "Battery", "Battery")
        );
    }
}
