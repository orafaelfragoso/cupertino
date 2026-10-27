//! What the panes of live services show (§2 T1, §6.4): the readings of the network,
//! Bluetooth and power daemons, the notification centre and the mixer, filled by `services` (feature `app`) and read by
//! `panes`. A reading is unknown before its first read, or the reason it is unavailable (no daemon on the bus, no
//! device), which the pane shows instead of an empty list (ST-19).

/// One service's state.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading<T> {
    Unknown,
    Unavailable(String),
    Ready(T),
}

impl<T> Reading<T> {
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Reading<U> {
        match self {
            Reading::Ready(v) => Reading::Ready(f(v)),
            Reading::Unknown => Reading::Unknown,
            Reading::Unavailable(r) => Reading::Unavailable(r),
        }
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Reading::Ready(v) => Some(v),
            Reading::Unknown | Reading::Unavailable(_) => None,
        }
    }

    /// The reason to show while the reading is not there: "Checking…" before the first read.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Reading::Ready(_) => None,
            Reading::Unknown => Some("Checking…"),
            Reading::Unavailable(r) => Some(r),
        }
    }
}

/// Do Not Disturb and the durations it can be turned on for (`org.cupertino.NotificationCenter1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focus {
    pub on: bool,
    /// `FocusLifetimes`: identifier and title, in menu order.
    pub lifetimes: Vec<(String, String)>,
}

/// The pictures of the Wallpaper pane: those of the asset packs and those of the folders the user added.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pictures {
    pub bundled: Vec<cupertino_assets::WallpaperFile>,
    pub folders: Vec<(std::path::PathBuf, Vec<cupertino_assets::WallpaperFile>)>,
    /// The picture the desktop shows when the settings name none (the first pack's default).
    pub default: Option<std::path::PathBuf>,
}

/// A connected mouse and its resolution, as the server resolved it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mouse {
    /// Its key in `mouse_dpi` (`cupertino_defaults::compositor::mouse_dpi_key`).
    pub key: String,
    pub name: String,
    /// The resolution in effect, in dots per inch.
    pub dpi: u32,
    /// Whether the user stored it (else the hardware database's or the default).
    pub stored: bool,
}

/// The output volume of the mixer's master.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub level: f64,
    pub muted: bool,
}

/// What the battery is doing (UPower `State`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryState {
    Charging,
    Discharging,
    Empty,
    Full,
    Unknown,
}

impl BatteryState {
    /// The UPower `State` value (1 charging, 2 discharging, 3 empty, 4 fully charged, 5 and 6 pending).
    pub fn from_upower(state: u32) -> Self {
        match state {
            1 | 5 => BatteryState::Charging,
            2 | 6 => BatteryState::Discharging,
            3 => BatteryState::Empty,
            4 => BatteryState::Full,
            _ => BatteryState::Unknown,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            BatteryState::Charging => "Charging",
            BatteryState::Discharging => "On battery",
            BatteryState::Empty => "Empty",
            BatteryState::Full => "Charged",
            BatteryState::Unknown => "Unknown",
        }
    }
}

/// The battery of UPower's display device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    pub percent: f64,
    pub state: BatteryState,
    /// Seconds; 0 when UPower does not know.
    pub time_to_empty: i64,
    pub time_to_full: i64,
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
    pub ssid: String,
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

/// `org.cupertino.Network1`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Wifi {
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
}

impl BtDevice {
    /// The name, else the address.
    pub fn title(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.address)
    }
}

/// `org.cupertino.Bluetooth1`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bluetooth {
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
    pub devices: Vec<BtDevice>,
    /// Another client's agent holds pairing (`AgentExists`).
    pub agent_elsewhere: bool,
    pub failure: Option<String>,
}

/// Every live reading.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    /// The pictures the Wallpaper pane lists.
    pub pictures: Pictures,
    /// The connected mice (`org.cupertino.Session1.Mice`), in the server's order.
    pub mice: Reading<Vec<Mouse>>,
    pub focus: Reading<Focus>,
    pub volume: Reading<Volume>,
    /// The display backlight, 0 to 1 (`org.cupertino.Power1`).
    pub brightness: Reading<f64>,
    /// `None` inside: no battery (`IsPresent` false).
    pub battery: Reading<Option<Battery>>,
    pub wifi: Reading<Wifi>,
    pub bluetooth: Reading<Bluetooth>,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            pictures: Pictures::default(),
            mice: Reading::Unknown,
            focus: Reading::Unknown,
            volume: Reading::Unknown,
            brightness: Reading::Unknown,
            battery: Reading::Unknown,
            wifi: Reading::Unknown,
            bluetooth: Reading::Unknown,
        }
    }
}

impl Live {
    /// Whether the machine has a battery, once known: the Battery row is titled "Energy Saver" without one.
    pub fn has_battery(&self) -> Option<bool> {
        self.battery.ready().map(Option::is_some)
    }
}

/// A duration in seconds as "2 h 05 min" / "42 min".
pub fn duration_text(seconds: i64) -> String {
    let minutes = (seconds.max(0) + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, m) => format!("{h} h {m:02} min"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_states_and_durations() {
        assert_eq!(Reading::<bool>::Unknown.reason(), Some("Checking…"));
        assert_eq!(
            Reading::<bool>::Unavailable("Wi-Fi is not available".into()).reason(),
            Some("Wi-Fi is not available")
        );
        assert_eq!(Reading::Ready(true).reason(), None);
        assert_eq!(
            [1, 2, 3, 4, 5, 0].map(BatteryState::from_upower),
            [
                BatteryState::Charging,
                BatteryState::Discharging,
                BatteryState::Empty,
                BatteryState::Full,
                BatteryState::Charging,
                BatteryState::Unknown
            ]
        );
        assert_eq!(
            (duration_text(2520), duration_text(7500)),
            ("42 min".to_string(), "2 h 05 min".to_string())
        );
        let mut live = Live::default();
        assert_eq!(live.has_battery(), None);
        live.battery = Reading::Ready(None);
        assert_eq!(live.has_battery(), Some(false));
        let open = Network {
            ssid: "cafe".into(),
            signal: -50,
            security: "open".into(),
            saved: false,
            connected: false,
        };
        assert!(!open.needs_passphrase());
        assert!(Network {
            security: "wpa2".into(),
            ..open.clone()
        }
        .needs_passphrase());
        assert!(!Network {
            security: "wpa2".into(),
            saved: true,
            ..open
        }
        .needs_passphrase());
    }
}
