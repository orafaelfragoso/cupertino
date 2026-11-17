//! What the panes of live services show (§2 T1, §6.4): the readings of the network,
//! Bluetooth and power daemons, the notification centre and the mixer, filled by `services` (feature `app`) and read by
//! `panes`. A reading is unknown before its first read, or the reason it is unavailable (no daemon on the bus, no
//! device), which the pane shows instead of an empty list (ST-19).

use cupertino_defaults::accessibility::AccessibilitySettings;

pub use cupertino_radios::{
    device_kind, device_title, display_name, BtDevice, Device, KnownNetwork, LinkState, Network,
    Vpn, Wifi, MAX_NAME,
};

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
    /// The groups (0 the packs', then each folder) whose listing has not come back yet.
    pub loading: std::collections::BTreeSet<usize>,
    /// The pictures whose thumbnail could not be made, and why.
    pub unreadable: std::collections::BTreeMap<std::path::PathBuf, crate::controls::Unreadable>,
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

/// A sound output the system can play to (a PipeWire sink).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// The id `wpctl set-default` takes.
    pub id: u32,
    pub name: String,
    /// Whether applications play to it now.
    pub default: bool,
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

/// A network interface as the kernel and the resolver describe it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    /// `ethernet`, `wifi` or `other`.
    pub kind: String,
    pub mac: Option<String>,
    /// The kernel's operational state (`up`, `down`, …).
    pub state: String,
    /// The IPv4 address with its prefix length, as `192.168.1.20/24`.
    pub ipv4: Option<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    /// The link's speed in megabits per second, when the driver reports one.
    pub speed: Option<u32>,
}

/// One point of the battery's charge history.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChargePoint {
    /// Seconds since the epoch.
    pub time: i64,
    /// The charge, 0 to 100.
    pub percent: f64,
    pub charging: bool,
}

/// What the Battery pane shows besides the charge itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryExtras {
    /// What the battery holds when full now against when it was new, in per cent; `None` when the supply does not say.
    pub health: Option<f64>,
    /// The power flowing in or out, in watts.
    pub rate_watts: Option<f64>,
    /// The recorded charge, oldest first, for the last ten days.
    pub history: Vec<ChargePoint>,
    /// Whether the power-saving profile is on; `None` when no service offers one.
    pub low_power: Option<bool>,
}

/// An action on one device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceAction {
    Connect,
    Disconnect,
    Forget,
    Pair,
}

/// An action on a device that failed: shown on that device's row until the next action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceFailure {
    pub id: String,
    pub action: DeviceAction,
    /// What the service said.
    pub reason: String,
}

impl DeviceFailure {
    /// What failed, then why: `Could not connect. Timed out.`
    pub fn text(&self) -> String {
        let lead = match self.action {
            DeviceAction::Connect => "Could not connect.",
            DeviceAction::Disconnect => "Could not disconnect.",
            DeviceAction::Forget => "Could not forget this device.",
            DeviceAction::Pair => "Could not pair.",
        };
        let reason = self.reason.trim();
        let mut chars = reason.chars();
        let Some(first) = chars.next() else {
            return lead.to_string();
        };
        let stop = if reason.ends_with(['.', '!', '?', '…']) {
            ""
        } else {
            "."
        };
        format!("{lead} {}{}{stop}", first.to_uppercase(), chars.as_str())
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
    /// The devices whose connection the pane asked for and whose call has not returned: they show as connecting with a
    /// daemon that does not say so itself.
    pub asked_connect: Vec<String>,
    /// The last failed action on a device.
    pub device_failure: Option<DeviceFailure>,
}

impl Bluetooth {
    /// The pane asked to connect device `id`: it shows as connecting until the call returns.
    pub fn connect_asked(&mut self, id: &str) {
        if !self.asked_connect.iter().any(|a| a == id) {
            self.asked_connect.push(id.to_string());
        }
        self.mark_connecting();
    }

    /// The connect call for `id` returned.
    pub fn connect_returned(&mut self, id: &str) {
        self.asked_connect.retain(|a| a != id);
    }

    /// Mark the devices the pane is connecting, over what the daemon said.
    pub fn mark_connecting(&mut self) {
        for d in &mut self.devices {
            d.connecting |= self.asked_connect.contains(&d.id);
        }
    }

    /// A new action starts: the errors of the last ones go.
    pub fn clear_failures(&mut self) {
        self.failure = None;
        self.device_failure = None;
    }
}

/// The machine's accessibility defaults for the login screen and new users (`org.cupertino.LoginManager1`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoginScreen {
    /// The machine's file as the login screen reads it; `None` without one or with one the readers refuse.
    pub current: Option<AccessibilitySettings>,
    /// A write waits for its reply.
    pub saving: bool,
}

/// A connected display.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    /// Make and model, else the description, else the connector name.
    pub title: String,
    /// The current mode in device pixels.
    pub mode: (u32, u32),
    pub refresh_mhz: Option<u32>,
    /// Device pixels per logical point.
    pub scale: f64,
    /// The connector name (`eDP-1`), which output configurations name the display by; `None` when the server gave
    /// none.
    pub connector: Option<String>,
    /// The size of the picture in millimetres, `None` when unknown.
    pub physical_mm: Option<(u32, u32)>,
    /// The size the desktop has on it, in points (turned with the display); `None` when the server gave none.
    pub logical: Option<(u32, u32)>,
    /// Turned by 90 or 270 degrees.
    pub rotated: bool,
    /// Where the display's top-left corner lies in the desktop, in points.
    pub position: Option<(i32, i32)>,
    /// The refresh rates it has at its current size, in millihertz, ascending; empty when the server does not list
    /// them.
    pub refresh_rates: Vec<u32>,
}

impl Monitor {
    /// The display's "looks like" choices at its current mode; empty when its physical size is unknown, so its
    /// density is.
    pub fn catalogue(&self) -> cupertino_displays::Catalogue {
        let built_in = self
            .connector
            .as_deref()
            .is_some_and(cupertino_displays::is_built_in);
        self.physical_mm
            .and_then(|mm| cupertino_displays::Panel::from_physical(self.mode, mm, self.rotated))
            .map(|panel| {
                cupertino_displays::catalogue(
                    &panel.built_in(built_in),
                    self.refresh_mhz.unwrap_or(0),
                )
            })
            .unwrap_or(cupertino_displays::Catalogue {
                choices: Vec::new(),
                default: None,
            })
    }

    /// The points the desktop has on this display at `choice`, turned with the display like `logical`: a choice is
    /// sized on the mode as the display scans it out, so a display turned by 90 or 270 degrees shows it with its sides
    /// swapped.
    pub fn choice_logical(&self, choice: &cupertino_displays::Candidate) -> (u32, u32) {
        let (w, h) = (choice.mode.width, choice.mode.height);
        if self.rotated {
            (h, w)
        } else {
            (w, h)
        }
    }
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
    /// The sound outputs, empty when there is no PipeWire daemon or none is listed.
    pub outputs: Vec<Output>,
    /// The connected displays, as the compositor announces them.
    pub displays: Reading<Vec<Monitor>>,
    /// The display backlight, 0 to 1 (`org.cupertino.Power1`).
    pub brightness: Reading<f64>,
    /// `None` inside: no battery (`IsPresent` false).
    pub battery: Reading<Option<Battery>>,
    /// What the machine has, once read: it decides whether there is a battery before the power service answers.
    pub hardware: Option<crate::hardware::Hardware>,
    pub wifi: Reading<Wifi>,
    pub bluetooth: Reading<Bluetooth>,
    /// Shown only after `CanSetSystemAccessibility` said yes at this opening of the Accessibility pane.
    pub login_screen: Option<LoginScreen>,
    /// The installed applications, read when the Privacy or the Notifications pane shows.
    pub apps: Vec<crate::clipboard::Installed>,
    /// The applications whose notifications the centre holds (`NotificationCenter1.Applications`): their settings id
    /// and name, read while the Notifications pane shows.
    pub notifying_apps: Vec<(String, String)>,
    /// A screen reader is installed (`Paths::screen_reader`): without one the Screen reader switch is disabled.
    pub screen_reader_installed: bool,
    /// What the General and Account panes show of the machine, read when one of them opens.
    pub system: Option<Box<crate::system::SystemInfo>>,
    /// The programs that start at login, read when the General pane opens.
    pub login_items: Vec<crate::system::LoginItem>,
    /// The installed web browsers and the index of the default one.
    pub browsers: (Vec<crate::system::Browser>, Option<usize>),
    /// The status icons applications have put in the menu bar, read when the Menu Bar pane opens.
    pub tray: Vec<TrayItem>,
    /// The network interfaces, read while the Wi-Fi or Network pane shows.
    pub interfaces: Vec<Interface>,
    /// The VPN connections, read while the VPN pane shows.
    pub vpns: Reading<Vec<Vpn>>,
    /// The battery's health, history and power-saving profile, read while the Battery pane shows.
    pub battery_extras: BatteryExtras,
    /// The sound inputs, empty without a PipeWire daemon.
    pub inputs: Vec<Output>,
    pub input_volume: Reading<Volume>,
    /// The output balance, -1 (left) to 1 (right); `None` when the sound server does not say.
    pub balance: Option<f64>,
    /// How loud the input is now, 0 to 1, while the Sound pane shows its inputs.
    pub input_level: f64,
    /// The people with an account on this machine, read while the Users & Groups pane shows.
    pub users: Vec<crate::system::User>,
    /// The keyboard's backlight, 0 to 1; `None` without one.
    pub keyboard_backlight: Option<f64>,
    /// The page's tab (Trackpad and Mouse, Sound): not saved anywhere.
    pub tab: usize,
    /// Which span the battery charts show: false the last 24 hours, true the last ten days.
    pub chart_days: bool,
}

/// A status icon an application keeps in the menu bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayItem {
    /// The id `menubar.conf` lists it by: the application's own id for the icon, else its bus name.
    pub id: String,
    pub name: String,
}

impl Default for Live {
    fn default() -> Self {
        Live {
            pictures: Pictures::default(),
            mice: Reading::Unknown,
            focus: Reading::Unknown,
            volume: Reading::Unknown,
            outputs: Vec::new(),
            displays: Reading::Unknown,
            brightness: Reading::Unknown,
            battery: Reading::Unknown,
            hardware: None,
            wifi: Reading::Unknown,
            bluetooth: Reading::Unknown,
            login_screen: None,
            apps: Vec::new(),
            notifying_apps: Vec::new(),
            screen_reader_installed: false,
            system: None,
            login_items: Vec::new(),
            browsers: (Vec::new(), None),
            tray: Vec::new(),
            interfaces: Vec::new(),
            vpns: Reading::Unknown,
            battery_extras: BatteryExtras::default(),
            inputs: Vec::new(),
            input_volume: Reading::Unknown,
            balance: None,
            input_level: 0.0,
            users: Vec::new(),
            keyboard_backlight: None,
            tab: 0,
            chart_days: false,
        }
    }
}

impl Live {
    /// Whether the machine has a battery, once known: the Battery row is titled "Energy Saver" without one. The
    /// kernel's power supplies decide; the power service only when they were not read.
    pub fn has_battery(&self) -> Option<bool> {
        self.hardware
            .map(|h| h.battery)
            .or_else(|| self.battery.ready().map(Option::is_some))
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
