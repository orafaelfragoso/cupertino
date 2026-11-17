//! What the services say about the radios, in the words of the Cupertino daemons whichever service answered.

/// The longest name a row or a prompt shows, in characters.
pub const MAX_NAME: usize = 128;

/// A name a device or a radio supplies, as a label: without control characters and the bidirectional marks, embeddings,
/// overrides and isolates (which could reorder the text around it), the ends trimmed, cut to [`MAX_NAME`] characters.
pub fn display_name(name: &str) -> String {
    name.chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_NAME)
        .collect()
}

/// A row of `Network1.Devices`: index, name, kind, state, SSID, failure, address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub index: u32,
    pub name: String,
    /// `ethernet` or `wifi`.
    pub kind: String,
    pub state: String,
    pub ssid: String,
    pub failure: String,
    pub address: String,
}

/// A row of `Network1.GetNetworks`: SSID, signal (dBm, `i32::MIN` unknown), security, saved, connected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// The network's key for the service, which a Join names: the SSID as the service gives it.
    pub ssid: String,
    /// The SSID as a label ([`display_name`]).
    pub name: String,
    pub signal: i32,
    pub security: String,
    pub saved: bool,
    pub connected: bool,
}

impl Network {
    /// A network that needs a passphrase to join: secured and not saved (§6.4).
    pub fn needs_passphrase(&self) -> bool {
        self.security != "open" && !self.saved
    }
}

/// A network the machine has saved, in range or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownNetwork {
    /// The network's key for the service ([`Network::ssid`]).
    pub ssid: String,
    pub name: String,
    /// Whether the machine joins it by itself when it is in range; `None` when the service does not say.
    pub auto_join: Option<bool>,
    pub in_range: bool,
    pub connected: bool,
    pub secured: bool,
}

/// A VPN connection the system holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vpn {
    /// The connection's identifier for the service (its UUID).
    pub id: String,
    pub name: String,
    /// What kind of VPN it is, as a label.
    pub kind: String,
    pub active: bool,
    /// An attempt to connect or disconnect is under way.
    pub busy: bool,
}

/// `org.cupertino.Network1`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Wifi {
    /// The saved networks, in range or not; empty when the service does not list them.
    pub known: Vec<KnownNetwork>,
    /// Whether the service can forget a saved network.
    pub can_forget: bool,
    pub enabled: bool,
    pub devices: Vec<Device>,
    /// The daemon's overall state.
    pub state: String,
    pub networks: Vec<Network>,
    /// The network a Join is waiting for, until the device connects or the attempt fails.
    pub joining: Option<String>,
    /// The text of the last failed attempt or refused write.
    pub failure: Option<String>,
}

impl Wifi {
    pub fn has_wifi_device(&self) -> bool {
        self.devices.iter().any(|d| d.kind == "wifi")
    }
}

/// One Bluetooth device of `Bluetooth1.Devices`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtDevice {
    pub id: String,
    pub address: String,
    pub name: Option<String>,
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
    pub pairing: bool,
    /// A connection was asked for and has not been made or failed yet.
    pub connecting: bool,
}

/// What a device's row says about its link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connected,
    Connecting,
    Pairing,
    NotConnected,
}

impl LinkState {
    pub fn label(self) -> &'static str {
        match self {
            LinkState::Connected => "Connected",
            LinkState::Connecting => "Connecting…",
            LinkState::Pairing => "Pairing…",
            LinkState::NotConnected => "Not Connected",
        }
    }

    /// The device is busy: its actions wait until this is over.
    pub fn busy(self) -> bool {
        matches!(self, LinkState::Connecting | LinkState::Pairing)
    }
}

impl BtDevice {
    /// The name, else what the device is and the end of its address.
    pub fn title(&self) -> String {
        device_title(self.name.as_deref(), &self.icon, &self.address)
    }

    /// A pairing says so first, then a link that is up, then one being made.
    pub fn state(&self) -> LinkState {
        if self.pairing {
            LinkState::Pairing
        } else if self.connected {
            LinkState::Connected
        } else if self.connecting {
            LinkState::Connecting
        } else {
            LinkState::NotConnected
        }
    }
}

/// What a device is, by the icon name its daemon gives it (either daemon's names), for a device without a name.
pub fn device_kind(icon: &str) -> &'static str {
    match icon {
        "computer" => "Computer",
        "tablet" | "input-tablet" => "Tablet",
        "phone" => "Phone",
        "watch" => "Watch",
        "display" | "video-display" => "Display",
        "headphones" | "audio-headphones" => "Headphones",
        "headset" | "audio-headset" => "Headset",
        "speaker" => "Speaker",
        "audio" | "audio-card" | "multimedia-player" => "Audio Device",
        "keyboard" | "input-keyboard" => "Keyboard",
        "mouse" | "input-mouse" => "Mouse",
        "trackpad" => "Trackpad",
        "gamepad" | "input-gaming" => "Game Controller",
        "remote-control" => "Remote Control",
        "camera" | "camera-photo" | "camera-video" => "Camera",
        "printer" => "Printer",
        "scanner" => "Scanner",
        _ => "Bluetooth Device",
    }
}

/// The last two bytes of an address (`AA:BB:CC:DD:EE:FF` gives `EE:FF`): enough to tell two unnamed devices of a kind
/// apart.
fn short_address(address: &str) -> String {
    let parts: Vec<&str> = address.split(':').collect();
    match parts.as_slice() {
        [.., a, b] if parts.len() >= 3 => format!("{a}:{b}"),
        _ => address.to_string(),
    }
}

/// A device's label: its name when it has one, else its kind and the end of its address (`Keyboard (EE:FF)`), never
/// the address alone.
pub fn device_title(name: Option<&str>, icon: &str, address: &str) -> String {
    match name.map(display_name).filter(|n| !n.is_empty()) {
        Some(name) => name,
        None => format!(
            "{} ({})",
            device_kind(icon),
            short_address(&display_name(address))
        ),
    }
}

/// The Wi-Fi radio and its first device, without the list of networks: what a switch and its caption need.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Link {
    pub enabled: bool,
    /// The first Wi-Fi device; `None` on a machine without one.
    pub device: Option<Device>,
}

impl Link {
    /// The network the device is connected to, as a label.
    pub fn network(&self) -> Option<&str> {
        self.device
            .as_ref()
            .filter(|d| d.state == "connected" && !d.ssid.is_empty())
            .map(|d| d.ssid.as_str())
    }
}

/// The Bluetooth adapter and its devices.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Adapter {
    /// Whether there is an adapter at all.
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    pub devices: Vec<BtDevice>,
}
