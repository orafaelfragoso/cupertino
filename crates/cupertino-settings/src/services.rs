//! The live services (feature `app`): `org.cupertino.Network1` (else NetworkManager, [`nm`]),
//! `org.cupertino.Bluetooth1` (else BlueZ, [`bluez`]), `org.cupertino.Power1` and `org.cupertino.LoginManager1` with
//! UPower's display device on the system bus, `org.cupertino.NotificationCenter1` on
//! the session bus, and the default PipeWire sink through `cupertino-pipewire`, else the mixer's master through
//! `cupertino-alsa` (in-process, as the control panel does, §1). Each connection is made on first use and
//! again after a failure, so a daemon that starts later is picked up; properties are read every time, never cached.

use crate::hardware::{Hardware, Sources};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cupertino_pipewire::{Target, VolumeWorker, Wpctl};
use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedValue};

use crate::agent::{Agent, AGENT_PATH};
use crate::bluez::{self, BluezAgent};
use crate::nm;
use crate::pane::PaneId;

use crate::live::{
    display_name, Battery, BatteryState, Bluetooth, BtDevice, Device, Focus, Monitor, Mouse,
    Network, Output, Reading, Volume, Wifi,
};
use crate::panes::Call;

pub const NETWORK_NAME: &str = "org.cupertino.Network1";
pub const NETWORK_PATH: &str = "/org/cupertino/Network1";
pub const BLUETOOTH_NAME: &str = "org.cupertino.Bluetooth1";
pub const BLUETOOTH_PATH: &str = "/org/cupertino/Bluetooth1";
pub const POWER_NAME: &str = "org.cupertino.Power1";
pub const POWER_PATH: &str = "/org/cupertino/Power1";
pub const UPOWER_NAME: &str = "org.freedesktop.UPower";
pub const UPOWER_DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
pub const UPOWER_DEVICE: &str = "org.freedesktop.UPower.Device";
pub const CENTRE_NAME: &str = "org.cupertino.NotificationCenter1";
pub const SESSION_NAME: &str = "org.cupertino.Session1";
pub const SESSION_PATH: &str = "/org/cupertino/Session1";
pub const CENTRE_PATH: &str = "/org/cupertino/NotificationCenter1";
pub const LOGIN_NAME: &str = "org.cupertino.LoginManager1";
pub const LOGIN_PATH: &str = "/org/cupertino/LoginManager1";
/// The prefix of sessiond's refusals of `SetSystemAccessibility`.
const LOGIN_ERROR: &str = "org.cupertino.LoginManager1.Error.";
/// The Bluetooth daemon's refusal to replace another client's live agent.
const AGENT_EXISTS: &str = "org.cupertino.Bluetooth1.AgentExists";

/// The lowest raw brightness the slider writes: 1, so it never turns the panel off (the control panel's floor,
/// approximate).
const BRIGHTNESS_FLOOR: u32 = 1;

/// One bus, connected on first use.
pub struct Bus {
    endpoint: Endpoint,
    connection: Option<Connection>,
}

/// Where a [`Bus`] connects.
enum Endpoint {
    System,
    Session,
    Address(String),
    /// A test bus handed over connected, which can't be reached again.
    Gone,
}

impl Bus {
    pub fn system() -> Self {
        Bus {
            endpoint: Endpoint::System,
            connection: None,
        }
    }

    pub fn session() -> Self {
        Bus {
            endpoint: Endpoint::Session,
            connection: None,
        }
    }

    /// The bus at a D-Bus `address`, connected on first use.
    pub fn address(address: &str) -> Self {
        Bus {
            endpoint: Endpoint::Address(address.to_owned()),
            connection: None,
        }
    }

    /// A bus at `address` (the tests' private buses).
    pub fn at(connection: Connection) -> Self {
        Bus {
            endpoint: Endpoint::Gone,
            connection: Some(connection),
        }
    }

    pub fn connection(&mut self) -> zbus::Result<&Connection> {
        if self.connection.is_none() {
            self.connection = Some(match &self.endpoint {
                Endpoint::System => Connection::system(),
                Endpoint::Session => Connection::session(),
                Endpoint::Address(address) => {
                    zbus::blocking::connection::Builder::address(address.as_str())?.build()
                }
                Endpoint::Gone => Err(zbus::Error::Failure("the test bus is gone".into())),
            }?);
        }
        Ok(self.connection.as_ref().expect("connected above"))
    }

    pub fn proxy(
        &mut self,
        name: &'static str,
        path: &'static str,
        interface: &'static str,
    ) -> zbus::Result<Proxy<'static>> {
        let connection = self.connection()?;
        Builder::new(connection)
            .destination(name)?
            .path(path)?
            .interface(interface)?
            .cache_properties(CacheProperties::No)
            .build()
    }

    /// Forget the connection after a failure of the bus itself, so the next use connects again.
    fn checked<T>(&mut self, result: zbus::Result<T>) -> zbus::Result<T> {
        if let Err(zbus::Error::InputOutput(_) | zbus::Error::Address(_)) = &result {
            self.connection = None;
        }
        result
    }
}

/// The most distinct messages [`log_once`] remembers.
const LOG_ONCE_MAX: usize = 256;

/// Write `detail` to the log the first time it comes, so a failure that repeats on every refresh is logged once.
pub fn log_once(detail: &str) {
    static SEEN: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
        std::sync::Mutex::new(None);
    let mut seen = SEEN.lock().expect("no thread panics while logging");
    let seen = seen.get_or_insert_with(Default::default);
    // messages with a varying part must not grow the set without bound: a full set starts over
    if seen.len() >= LOG_ONCE_MAX {
        seen.clear();
    }
    if seen.insert(detail.to_string()) {
        eprintln!("cupertino-settings: {detail}");
    }
}

/// What the pane says when the bus or a device failed: the problem stays in the log, never on screen.
pub const SERVICE_UNAVAILABLE: &str = "The service is not available.";

/// The standard errors a service answers with its own plain text (the Cupertino daemons refuse with these).
/// The network daemon refuses an unauthorized change with `AccessDenied` and a sentence; the bus daemon's own policy
/// denial says "Rejected send message ..." (dbus-daemon) or "Sender is not authorized to send message" / "Receiver is
/// not authorized to receive message" (dbus-broker) and is told apart by that text.
const SERVICE_STANDARD: [&str; 4] = [
    "org.freedesktop.DBus.Error.Failed",
    "org.freedesktop.DBus.Error.InvalidArgs",
    "org.freedesktop.DBus.Error.NotSupported",
    "org.freedesktop.DBus.Error.AccessDenied",
];

/// How the bus daemons word a policy denial.
const BUS_POLICY_TEXTS: [&str; 3] = [
    "Rejected send message",
    "Sender is not authorized to send message",
    "Receiver is not authorized to receive message",
];

/// The name of the D-Bus error `e` carries, if it came over the bus.
fn error_name(e: &zbus::Error) -> Option<String> {
    match e {
        zbus::Error::MethodError(name, _, _) => Some(name.to_string()),
        zbus::Error::FDO(e) => Some(zbus::DBusError::name(e.as_ref()).to_string()),
        _ => None,
    }
}

/// Whether a service itself answered with an error whose text is meant for the pane: a name of its own, or one of
/// [`SERVICE_STANDARD`]. The bus daemon's errors (no reply, access denied, timeout, limits) and the connection's are not.
fn from_daemon(e: &zbus::Error) -> bool {
    let text = e.to_string();
    !BUS_POLICY_TEXTS.iter().any(|t| text.contains(t))
        && error_name(e).is_some_and(|name| {
            !name.starts_with("org.freedesktop.DBus.Error.")
                || SERVICE_STANDARD.contains(&name.as_str())
        })
}

/// The text of a service failure for the pane: the service's own message when it sent one; a problem of the bus
/// daemon or the connection is logged and shown as [`SERVICE_UNAVAILABLE`].
pub fn failure(e: &zbus::Error) -> String {
    if !from_daemon(e) {
        log_once(&e.to_string());
        return SERVICE_UNAVAILABLE.to_string();
    }
    match e {
        zbus::Error::MethodError(name, message, _) => error_text(name.as_str(), message.as_deref()),
        zbus::Error::FDO(e) => error_text(
            zbus::DBusError::name(e.as_ref()).as_str(),
            zbus::DBusError::description(e.as_ref()),
        ),
        other => other.to_string(),
    }
}

/// The message of a bus error, else the last part of its name.
fn error_text(name: &str, message: Option<&str>) -> String {
    match message {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => name.rsplit('.').next().unwrap_or(name).to_string(),
    }
}

/// The row's sentence for a refused `SetSystemAccessibility`, by the refusal's name: sessiond's message is for the log.
/// Any other failure is logged and shown as [`SERVICE_UNAVAILABLE`].
pub fn login_screen_failure(e: &zbus::Error) -> String {
    let name = error_name(e);
    let sentence = match name.as_deref().and_then(|n| n.strip_prefix(LOGIN_ERROR)) {
        Some("NotAdministrator") => "Only an administrator can change the login screen.",
        Some("NotAtConsole") => "Change this at the computer itself.",
        Some("TooManyWrites") => "Wait a moment, then try again.",
        Some("InvalidArgs") => "These settings can't be used for the login screen.",
        _ => {
            log_once(&e.to_string());
            SERVICE_UNAVAILABLE
        }
    };
    sentence.to_string()
}

/// The bus's errors for a name nobody owns.
const ABSENT: [&str; 2] = [
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.NameHasNoOwner",
];

/// Whether `e` says the service is not on the bus at all.
fn absent(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => ABSENT.contains(&name.as_str()),
        zbus::Error::FDO(e) => matches!(
            e.as_ref(),
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)
        ),
        _ => false,
    }
}

/// A reading from `result`: the value, or the pane's reason (`missing` when the service is not on the bus).
fn reading<T>(result: zbus::Result<T>, missing: &str) -> Reading<T> {
    match result {
        Ok(v) => Reading::Ready(v),
        Err(e) if absent(&e) => Reading::Unavailable(missing.to_string()),
        Err(e) if !from_daemon(&e) => {
            log_once(&e.to_string());
            Reading::Unavailable(missing.to_string())
        }
        Err(e) => Reading::Unavailable(failure(&e)),
    }
}

/// What serves the Wi-Fi and Network panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkBackend {
    Cupertino,
    NetworkManager,
}

/// What serves the Bluetooth pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BluetoothBackend {
    Cupertino,
    BlueZ,
}

/// The services on the system bus that the network and Bluetooth panes can use, and the hardware the Trackpad pane
/// needs; a pane without them is hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backends {
    pub network: Option<NetworkBackend>,
    pub bluetooth: Option<BluetoothBackend>,
    pub hardware: Hardware,
}

impl Backends {
    /// The panes no service backs.
    pub fn hidden(&self) -> Vec<PaneId> {
        let mut hidden = Vec::new();
        if self.network.is_none() {
            hidden.extend([PaneId::Wifi, PaneId::Network]);
        }
        if self.bluetooth.is_none() {
            hidden.push(PaneId::Bluetooth);
        }
        if !self.hardware.touchpad {
            hidden.push(PaneId::Trackpad);
        }
        hidden
    }
}

/// A row of `Network1.Devices`.
type DeviceRecord = (u32, String, String, String, String, String, String);
/// A row of `Network1.GetNetworks`.
type NetworkRecord = (String, i32, String, bool, bool);

/// The live services and their connections.
pub struct Services {
    pub system: Bus,
    pub session: Bus,
    mixer: Option<cupertino_alsa::Master>,
    alsa_dir: PathBuf,
    wpctl: Wpctl,
    /// The sink's readings and the slider's writes, in the background so neither a drag nor the pane's refresh waits
    /// for `wpctl`.
    worker: VolumeWorker,
    pipewire: bool,
    backends: Backends,
    /// The BlueZ devices whose `Pair` has not returned (the Cupertino daemon reports its own).
    pairing: Arc<Mutex<HashSet<String>>>,
    /// Where the hardware facts are read; `None` (tests) is a machine without a battery or a touchpad.
    hardware: Option<Sources>,
}

impl Services {
    pub fn new() -> Self {
        Services {
            system: Bus::system(),
            session: Bus::session(),
            mixer: None,
            alsa_dir: PathBuf::from(cupertino_alsa::DEVICE_DIR),
            wpctl: Wpctl::default(),
            worker: sound_worker(&Wpctl::default()),
            pipewire: true,
            backends: Backends::default(),
            pairing: Arc::default(),
            hardware: Some(Sources::default()),
        }
    }

    /// Services on given buses (tests).
    pub fn on(system: Bus, session: Bus) -> Self {
        Services {
            system,
            session,
            mixer: None,
            alsa_dir: PathBuf::from(cupertino_alsa::DEVICE_DIR),
            wpctl: Wpctl::default(),
            worker: sound_worker(&Wpctl::default()),
            pipewire: true,
            backends: Backends::default(),
            pairing: Arc::default(),
            hardware: None,
        }
    }

    /// The hardware facts read from `sources` (tests).
    pub fn with_hardware(self, hardware: Sources) -> Self {
        Services {
            hardware: Some(hardware),
            ..self
        }
    }

    /// Read and change the sound output through `wpctl`, else the mixer below `alsa_dir` (tests).
    pub fn with_sound(self, wpctl: Wpctl, alsa_dir: PathBuf) -> Self {
        Services {
            worker: sound_worker(&wpctl),
            wpctl,
            alsa_dir,
            ..self
        }
    }

    /// Wait until the sound changes asked are written and the readings asked for are done (tests).
    pub fn wait_sound(&self) {
        self.worker.wait_idle();
    }

    // ---- reading ---------------------------------------------------------------------------------------------------

    /// Look up which services are on the system bus, running or activatable. `org.cupertino.Network1` comes before
    /// NetworkManager, whose name the network daemon also claims. When the bus does not answer the last answer stays.
    pub fn detect(&mut self) -> Backends {
        self.backends.hardware = self
            .hardware
            .as_ref()
            .map(Hardware::read)
            .unwrap_or_default();
        let hardware = self.backends.hardware;
        let result = self.system.connection().and_then(|c| {
            let bus = zbus::blocking::fdo::DBusProxy::new(c)?;
            let mut names = bus.list_names()?;
            names.extend(bus.list_activatable_names()?);
            let has = |name: &str| names.iter().any(|n| n.as_str() == name);
            Ok(Backends {
                network: if has(NETWORK_NAME) {
                    Some(NetworkBackend::Cupertino)
                } else if has(nm::NAME) {
                    Some(NetworkBackend::NetworkManager)
                } else {
                    None
                },
                bluetooth: if has(BLUETOOTH_NAME) {
                    Some(BluetoothBackend::Cupertino)
                } else if has(bluez::NAME) {
                    Some(BluetoothBackend::BlueZ)
                } else {
                    None
                },
                hardware,
            })
        });
        match self.system.checked(result) {
            Ok(backends) => self.backends = backends,
            Err(e) => log_once(&format!("cannot list the services of the system bus: {e}")),
        }
        self.backends
    }

    fn network_manager(&self) -> bool {
        self.backends.network == Some(NetworkBackend::NetworkManager)
    }

    fn bluez(&self) -> bool {
        self.backends.bluetooth == Some(BluetoothBackend::BlueZ)
    }

    /// `f` on the system bus's connection, the error text for the row when it fails.
    fn on_system(&mut self, f: impl FnOnce(&Connection) -> zbus::Result<()>) -> Result<(), String> {
        let result = self.system.connection().and_then(f);
        self.system.checked(result).map_err(|e| failure(&e))
    }

    /// Every key of `accessibility.conf` as the server holds it (`org.cupertino.Session1.Accessibility`).
    pub fn accessibility(&mut self) -> Reading<HashMap<String, String>> {
        let result = self
            .session
            .proxy(SESSION_NAME, SESSION_PATH, SESSION_NAME)
            .and_then(|p| p.get_property::<HashMap<String, String>>("Accessibility"));
        reading(self.session.checked(result), "The session is not running")
    }

    /// Whether the session is dark (`org.cupertino.Session1.Appearance`); `None` when the session cannot tell.
    pub fn dark(&mut self) -> Option<bool> {
        let result = self
            .session
            .proxy(SESSION_NAME, SESSION_PATH, SESSION_NAME)
            .and_then(|p| p.get_property::<String>("Appearance"));
        self.session
            .checked(result)
            .ok()
            .map(|appearance| appearance == "dark")
    }

    /// The connected mice, as the server lists them (`org.cupertino.Session1.Mice`: key, name, resolution, stored).
    pub fn mice(&mut self) -> Reading<Vec<Mouse>> {
        let result = self
            .session
            .proxy(SESSION_NAME, SESSION_PATH, SESSION_NAME)
            .and_then(|p| p.get_property::<Vec<(String, String, u32, bool)>>("Mice"));
        reading(self.session.checked(result), "The session is not running").map(|mice| {
            mice.into_iter()
                .map(|(key, name, dpi, stored)| Mouse {
                    key,
                    name,
                    dpi,
                    stored,
                })
                .collect()
        })
    }

    pub fn focus(&mut self) -> Reading<Focus> {
        let result = self
            .session
            .proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
            .and_then(|p| {
                Ok(Focus {
                    on: p.get_property("DoNotDisturb")?,
                    lifetimes: p.get_property("FocusLifetimes")?,
                })
            });
        reading(
            self.session.checked(result),
            "The notification centre is not running",
        )
    }

    /// The status icons applications keep in the menu bar now, each with the id `menubar.conf` lists it by (the
    /// item's own `Id`, else its bus name) and the title it shows; empty while no host is running.
    pub fn tray_items(&mut self) -> Vec<crate::live::TrayItem> {
        let registered: zbus::Result<Vec<String>> = self
            .session
            .proxy(
                "org.kde.StatusNotifierWatcher",
                "/StatusNotifierWatcher",
                "org.kde.StatusNotifierWatcher",
            )
            .and_then(|p| p.get_property("RegisteredStatusNotifierItems"));
        let registered = match self.session.checked(registered) {
            Ok(items) => items,
            Err(_) => return Vec::new(),
        };
        let Ok(connection) = self.session.connection().cloned() else {
            return Vec::new();
        };
        let mut out: Vec<crate::live::TrayItem> = Vec::new();
        for entry in registered {
            if entry.is_empty() || entry.starts_with('/') {
                continue;
            }
            let (service, path) = match entry.find('/') {
                Some(i) => (entry[..i].to_owned(), entry[i..].to_owned()),
                None => (entry.clone(), "/StatusNotifierItem".to_owned()),
            };
            let proxy: zbus::Result<Proxy<'static>> = (|| {
                Builder::new(&connection)
                    .destination(service.clone())?
                    .path(path)?
                    .interface("org.kde.StatusNotifierItem")?
                    .cache_properties(CacheProperties::No)
                    .build()
            })();
            let Ok(proxy) = proxy else { continue };
            let app_id: String = proxy.get_property("Id").unwrap_or_default();
            let title: String = proxy.get_property("Title").unwrap_or_default();
            let id = if app_id.is_empty() { service } else { app_id };
            let name = display_name(&if title.is_empty() { id.clone() } else { title });
            if !out.iter().any(|t| t.id == id) {
                out.push(crate::live::TrayItem { id, name });
            }
        }
        out.sort_by(|a, b| crate::sidebar::standard_compare(&a.name, &b.name));
        out
    }

    /// The applications whose notifications the centre holds now, as (settings id, name); empty while it is not
    /// running or does not answer.
    pub fn notifying_apps(&mut self) -> Vec<(String, String)> {
        let result = self
            .session
            .proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
            .and_then(|p| p.call::<_, _, Vec<(String, String)>>("Applications", &()));
        match self.session.checked(result) {
            Ok(apps) => apps,
            Err(e) => {
                log_once(&format!("the notification centre's applications: {e}"));
                Vec::new()
            }
        }
    }

    /// The connected displays with a current mode: one short connection asks the server.
    pub fn displays(&self) -> Reading<Vec<Monitor>> {
        let outputs = cupertino_wayland_client::Connection::connect()
            .map_err(|e| e.to_string())
            .and_then(|mut conn| {
                conn.roundtrip().map_err(|e| e.to_string())?;
                Ok(conn.outputs())
            });
        match outputs {
            Ok(outputs) => Reading::Ready(
                outputs
                    .into_iter()
                    .filter_map(|o| {
                        let mode = o.mode?;
                        let title = crate::about::display_title(&crate::about::Display {
                            make: o.make.as_deref(),
                            model: o.model.as_deref(),
                            description: o.description.as_deref(),
                            name: o.name.as_deref(),
                            ..Default::default()
                        })
                        .unwrap_or_else(|| "Display".to_owned());
                        Some(Monitor {
                            title,
                            mode,
                            refresh_mhz: o.refresh_mhz,
                            scale: o.effective_scale().unwrap_or_else(|| o.scale.factor()),
                            connector: o.name.clone(),
                            physical_mm: o.physical_size_mm,
                            logical: o.geometry.map(|g| (g.width, g.height)),
                            rotated: o.transform.swaps_sides(),
                        })
                    })
                    .collect(),
            ),
            Err(e) => {
                log_once(&format!("the displays could not be listed: {e}"));
                Reading::Unavailable("The display server does not answer".into())
            }
        }
    }

    pub fn brightness(&mut self) -> Reading<f64> {
        let result = self
            .system
            .proxy(POWER_NAME, POWER_PATH, POWER_NAME)
            .and_then(|p| {
                let current: u32 = p.call("GetBrightness", &())?;
                let max: u32 = p.call("GetMaxBrightness", &())?;
                Ok((current, max))
            });
        match reading(
            self.system.checked(result),
            "This display has no brightness control",
        ) {
            Reading::Ready((_, 0)) => {
                Reading::Unavailable("This display has no brightness control".into())
            }
            Reading::Ready((current, max)) => Reading::Ready(f64::from(current) / f64::from(max)),
            Reading::Unavailable(r) => Reading::Unavailable(r),
            Reading::Unknown => Reading::Unknown,
        }
    }

    pub fn battery(&mut self) -> Reading<Option<Battery>> {
        let result = self
            .system
            .proxy(UPOWER_NAME, UPOWER_DISPLAY_DEVICE, UPOWER_DEVICE)
            .and_then(|p| {
                if !p.get_property::<bool>("IsPresent")? {
                    return Ok(None);
                }
                Ok(Some(Battery {
                    percent: p.get_property("Percentage")?,
                    state: BatteryState::from_upower(p.get_property("State")?),
                    time_to_empty: p.get_property("TimeToEmpty")?,
                    time_to_full: p.get_property("TimeToFull")?,
                }))
            });
        reading(
            self.system.checked(result),
            "The power service is not running",
        )
    }

    pub fn wifi(&mut self, keep: Option<&Wifi>) -> Reading<Wifi> {
        let result = if self.network_manager() {
            self.system.connection().and_then(nm::wifi)
        } else {
            self.wifi_daemon()
        };
        match reading(self.system.checked(result), "Wi-Fi is not available") {
            Reading::Ready(mut w) => {
                // a Join in progress ends when its network is connected or a device reports a failure
                if let Some(previous) = keep {
                    w.joining = previous.joining.clone().filter(|ssid| {
                        !w.networks.iter().any(|n| &n.ssid == ssid && n.connected)
                            && w.devices.iter().all(|d| d.failure.is_empty())
                    });
                    w.failure = previous.failure.clone().or_else(|| {
                        w.devices
                            .iter()
                            .find(|d| !d.failure.is_empty() && previous.joining.is_some())
                            .map(|d| d.failure.clone())
                    });
                }
                Reading::Ready(w)
            }
            other => other,
        }
    }

    fn wifi_daemon(&mut self) -> zbus::Result<Wifi> {
        self.system
            .proxy(NETWORK_NAME, NETWORK_PATH, NETWORK_NAME)
            .and_then(|p| {
                let devices: Vec<DeviceRecord> = p.get_property("Devices")?;
                let networks: Vec<NetworkRecord> = p.call("GetNetworks", &())?;
                Ok(Wifi {
                    enabled: p.get_property("WirelessEnabled")?,
                    devices: devices
                        .into_iter()
                        .map(
                            |(index, name, kind, state, ssid, failure, address)| Device {
                                index,
                                name,
                                kind,
                                state,
                                ssid: display_name(&ssid),
                                failure,
                                address,
                            },
                        )
                        .collect(),
                    state: p.get_property("State")?,
                    networks: networks
                        .into_iter()
                        .map(|(ssid, signal, security, saved, connected)| Network {
                            name: display_name(&ssid),
                            ssid,
                            signal,
                            security,
                            saved,
                            connected,
                        })
                        .collect(),
                    joining: None,
                    failure: None,
                })
            })
    }

    pub fn bluetooth(&mut self, keep: Option<&Bluetooth>) -> Reading<Bluetooth> {
        let result = if self.bluez() {
            let pairing = self
                .pairing
                .lock()
                .expect("no thread panics while pairing")
                .clone();
            self.system
                .connection()
                .and_then(|c| bluez::bluetooth(c, &pairing))
        } else {
            self.system
                .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
                .and_then(|p| {
                    let devices: Vec<HashMap<String, OwnedValue>> = p.get_property("Devices")?;
                    Ok(Bluetooth {
                        available: p.get_property("Available")?,
                        powered: p.get_property("Powered")?,
                        discovering: p.get_property("Discovering")?,
                        devices: devices.iter().filter_map(bt_device).collect(),
                        agent_elsewhere: false,
                        failure: None,
                    })
                })
        };
        reading(self.system.checked(result), "Bluetooth is not available").map(|b| Bluetooth {
            agent_elsewhere: keep.is_some_and(|k| k.agent_elsewhere),
            failure: keep.and_then(|k| k.failure.clone()),
            ..b
        })
    }

    /// The default PipeWire sink; when no PipeWire daemon answers, the mixer's master, which the changes after this
    /// reading then go to as well.
    /// After the first, a reading is the worker's last, and the next is asked for in the background.
    pub fn volume(&mut self) -> Reading<Volume> {
        self.worker.request_read(true);
        let sink = match self.worker.latest() {
            Some(read) => read.volume,
            None => self
                .wpctl
                .volume(Target::DefaultSink)
                .map_err(|e| e.to_string()),
        };
        if let Ok(sink) = sink {
            self.pipewire = true;
            return Reading::Ready(Volume {
                level: sink.level,
                muted: sink.muted,
            });
        }
        self.pipewire = false;
        match self.with_mixer(|m| {
            let level = m.level()?;
            Ok(Volume {
                level,
                muted: m.muted()?.unwrap_or(level <= 0.0),
            })
        }) {
            Ok(v) => Reading::Ready(v),
            Err(e) => {
                log_once(&format!("no sound output: {e}"));
                Reading::Unavailable("No sound output is available".into())
            }
        }
    }

    /// The PipeWire sinks, in `wpctl status` order; none when the volume is not on PipeWire or `wpctl` does not answer.
    /// After the first reading, the sinks of the worker's last one; none until the worker has listed them.
    pub fn outputs(&mut self) -> Vec<Output> {
        if !self.pipewire {
            return Vec::new();
        }
        let sinks = match self.worker.latest() {
            Some(read) => read.sinks.unwrap_or_else(|| Ok(Vec::new())),
            None => self.wpctl.sinks().map_err(|e| e.to_string()),
        };
        match sinks {
            Ok(sinks) => sinks
                .into_iter()
                .map(|s| Output {
                    id: s.id,
                    name: s.name,
                    default: s.default,
                })
                .collect(),
            Err(e) => {
                log_once(&format!("the sound outputs could not be listed: {e}"));
                Vec::new()
            }
        }
    }

    fn with_mixer<T>(
        &mut self,
        action: impl FnOnce(&cupertino_alsa::Master) -> cupertino_alsa::Result<T>,
    ) -> cupertino_alsa::Result<T> {
        if self.mixer.is_none() {
            self.mixer = Some(cupertino_alsa::Master::open_default(
                &self.alsa_dir,
                cupertino_alsa::OpenMode::ReadWrite,
            )?);
        }
        let result = action(self.mixer.as_ref().expect("opened above"));
        if result.is_err() {
            self.mixer = None;
        }
        result
    }

    // ---- writing ---------------------------------------------------------------------------------------------------

    /// Carry out `call`; the error text for the row when it is refused or fails.
    pub fn call(&mut self, call: &Call) -> Result<(), String> {
        let system =
            |s: &mut Self, name, path, iface, f: &dyn Fn(&Proxy<'static>) -> zbus::Result<()>| {
                let result = s.system.proxy(name, path, iface).and_then(|p| f(&p));
                s.system.checked(result).map_err(|e| failure(&e))
            };
        match call {
            Call::SetWirelessEnabled(on) if self.network_manager() => self.on_system(|c| nm::set_wireless_enabled(c, *on)),
            Call::Join { ssid, passphrase } if self.network_manager() => self.on_system(|c| nm::join(c, ssid, passphrase.as_ref().map_or("", |p| p.as_str()))),
            Call::Disconnect if self.network_manager() => self.on_system(nm::disconnect),
            Call::SetBluetoothPowered(on) if self.bluez() => self.on_system(|c| bluez::set_powered(c, *on)),
            Call::Connect(id) if self.bluez() => self.on_system(|c| bluez::connect(c, id)),
            Call::DisconnectDevice(id) if self.bluez() => self.on_system(|c| bluez::disconnect(c, id)),
            Call::Remove { id, confirmed } if self.bluez() => {
                assert!(*confirmed, "a device is removed only after its confirmation");
                self.on_system(|c| bluez::remove(c, id))
            }
            Call::SetDoNotDisturb(on) => {
                let result = self.session.proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME).and_then(|p| p.call::<_, _, ()>("SetDoNotDisturb", &(*on,)));
                self.session.checked(result).map_err(|e| failure(&e))
            }
            Call::SetDoNotDisturbFor(id) => {
                let result = self.session.proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME).and_then(|p| p.call::<_, _, ()>("SetDoNotDisturbFor", &(id.as_str(),)));
                self.session.checked(result).map_err(|e| failure(&e))
            }
            Call::SetVolume { level, muted } if self.pipewire => self
                .worker
                .set(cupertino_pipewire::Volume {
                    level: *level,
                    muted: *muted,
                })
                .map_err(|e| {
                    log_once(&format!("the volume could not be changed: {e}"));
                    "The volume could not be changed.".to_string()
                }),
            Call::SetVolume { level, muted } => self
                .with_mixer(|m| {
                    m.set_level(*level)?;
                    if m.muted()?.is_some_and(|current| current != *muted) {
                        m.set_muted(*muted)?;
                    }
                    Ok(())
                })
                .map_err(|e| {
                    log_once(&format!("the volume could not be changed: {e}"));
                    "The volume could not be changed.".to_string()
                }),
            Call::SetOutput(id) => {
                let changed = self.wpctl.set_default_sink(*id);
                self.worker.request_read(true);
                changed.map_err(|e| {
                    log_once(&format!("the sound output could not be changed: {e}"));
                    "The sound output could not be changed.".to_string()
                })
            }
            Call::SetBrightness(level) => system(self, POWER_NAME, POWER_PATH, POWER_NAME, &|p| {
                let max: u32 = p.call("GetMaxBrightness", &())?;
                let raw = ((level.clamp(0.0, 1.0) * f64::from(max)).round() as u32).clamp(BRIGHTNESS_FLOOR.min(max), max);
                p.call::<_, _, ()>("SetBrightness", &(raw,))
            }),
            Call::SetDisplayScale { connector, scale } => crate::output_config::set_scale(connector, *scale).map_err(|e| {
                log_once(&format!("the display size could not be changed: {e}"));
                "The display size could not be changed.".to_string()
            }),
            Call::SetWirelessEnabled(on) => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.set_property("WirelessEnabled", *on).map_err(zbus::Error::from)),
            Call::Join { ssid, passphrase } => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.call::<_, _, ()>("Connect", &(ssid.as_str(), passphrase.as_ref().map_or("", |p| p.as_str())))),
            Call::Disconnect => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.call::<_, _, ()>("Disconnect", &())),
            Call::SetBluetoothPowered(on) => system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.set_property("Powered", *on).map_err(zbus::Error::from)),
            Call::Pair(_) => panic!("a pairing waits for the agent's sheet, so it runs on its own thread: Services::pair"),
            Call::Connect(id) => system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.call::<_, _, ()>("Connect", &(id.as_str(),))),
            Call::DisconnectDevice(id) => system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.call::<_, _, ()>("Disconnect", &(id.as_str(),))),
            Call::Remove { id, confirmed } => {
                assert!(*confirmed, "a device is removed only after its confirmation");
                system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.call::<_, _, ()>("Remove", &(id.as_str(),)))
            }
        }
    }

    /// Ask the Wi-Fi device to scan (the pane open and every 10 s while the radio is on, approximate).
    pub fn scan(&mut self) -> Result<(), String> {
        if self.network_manager() {
            return self.on_system(nm::scan);
        }
        let result = self
            .system
            .proxy(NETWORK_NAME, NETWORK_PATH, NETWORK_NAME)
            .and_then(|p| p.call::<_, _, ()>("Scan", &()));
        self.system.checked(result).map_err(|e| failure(&e))
    }

    /// Serve `agent` at [`AGENT_PATH`] and register it with the Bluetooth daemon (the pane open, §6.4). `Ok(false)`:
    /// another client's agent is registered (`AgentExists`), so pairing questions go to its dialog.
    pub fn register_agent(&mut self, agent: Agent) -> Result<bool, String> {
        let connection = self.system.connection().map_err(|e| failure(&e))?.clone();
        if self.bluez() {
            connection
                .object_server()
                .at(AGENT_PATH, BluezAgent::new(agent))
                .map_err(|e| failure(&e))?;
            let result = bluez::register_agent(&connection);
            if result.is_err() {
                connection
                    .object_server()
                    .remove::<BluezAgent, _>(AGENT_PATH)
                    .map_err(|e| failure(&e))?;
            }
            return self
                .system
                .checked(result)
                .map(|()| true)
                .map_err(|e| failure(&e));
        }
        connection
            .object_server()
            .at(AGENT_PATH, agent)
            .map_err(|e| failure(&e))?;
        let path = ObjectPath::try_from(AGENT_PATH).expect("a valid object path");
        let result = self
            .system
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
            .and_then(|p| p.call::<_, _, ()>("RegisterAgent", &(path,)));
        match self.system.checked(result) {
            Ok(()) => Ok(true),
            Err(e) => {
                connection
                    .object_server()
                    .remove::<Agent, _>(AGENT_PATH)
                    .map_err(|e| failure(&e))?;
                match &e {
                    zbus::Error::MethodError(name, _, _) if name.as_str() == AGENT_EXISTS => {
                        Ok(false)
                    }
                    _ => Err(failure(&e)),
                }
            }
        }
    }

    /// Unregister the agent and stop serving it (the pane left or the window closed).
    pub fn unregister_agent(&mut self) -> Result<(), String> {
        if self.bluez() {
            let unregistered = self.on_system(bluez::unregister_agent);
            let connection = self.system.connection().map_err(|e| failure(&e))?;
            connection
                .object_server()
                .remove::<BluezAgent, _>(AGENT_PATH)
                .map_err(|e| failure(&e))?;
            return unregistered;
        }
        let result = self
            .system
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
            .and_then(|p| p.call::<_, _, ()>("UnregisterAgent", &()));
        let unregistered = self.system.checked(result).map_err(|e| failure(&e));
        let connection = self.system.connection().map_err(|e| failure(&e))?;
        connection
            .object_server()
            .remove::<Agent, _>(AGENT_PATH)
            .map_err(|e| failure(&e))?;
        unregistered
    }

    /// Pair device `id` on a thread of its own: `Pair` returns only when the pairing is over, and meanwhile the daemon
    /// asks the agent, whose sheet needs the window. `done` gets the outcome on that thread.
    pub fn pair(
        &mut self,
        id: &str,
        done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) -> Result<(), String> {
        if self.bluez() {
            let device = self.system.connection().and_then(|c| bluez::pairing(c, id));
            let device = self.system.checked(device).map_err(|e| failure(&e))?;
            let pairing = self.pairing.clone();
            let id = id.to_string();
            pairing
                .lock()
                .expect("no thread panics while pairing")
                .insert(id.clone());
            let (shared, key) = (pairing.clone(), id.clone());
            return std::thread::Builder::new()
                .name("bluetooth-pair".into())
                .spawn(move || {
                    let result = bluez::pair(&device).map_err(|e| failure(&e));
                    shared
                        .lock()
                        .expect("no thread panics while pairing")
                        .remove(&key);
                    done(result)
                })
                .map(drop)
                .map_err(|e| {
                    pairing
                        .lock()
                        .expect("no thread panics while pairing")
                        .remove(&id);
                    log_once(&format!("cannot start the pairing: {e}"));
                    "The pairing could not be started.".to_string()
                });
        }
        let proxy = self
            .system
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME);
        let proxy = self.system.checked(proxy).map_err(|e| failure(&e))?;
        let id = id.to_string();
        std::thread::Builder::new()
            .name("bluetooth-pair".into())
            .spawn(move || {
                done(
                    proxy
                        .call::<_, _, ()>("Pair", &(id.as_str(),))
                        .map_err(|e| failure(&e)),
                )
            })
            .map_err(|e| {
                log_once(&format!("cannot start the pairing: {e}"));
                "The pairing could not be started.".to_string()
            })?;
        Ok(())
    }

    /// Start or stop Bluetooth discovery (the pane open, §6.4).
    pub fn discovery(&mut self, on: bool) -> Result<(), String> {
        if self.bluez() {
            return self.on_system(|c| bluez::discovery(c, on));
        }
        let method = if on {
            "StartDiscovery"
        } else {
            "StopDiscovery"
        };
        let result = self
            .system
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
            .and_then(|p| p.call::<_, _, ()>(method, &()));
        self.system.checked(result).map_err(|e| failure(&e))
    }

    /// Ask sessiond on a thread of its own whether this person may set the machine's accessibility defaults (each
    /// opening of the Accessibility pane, never kept). `done` gets the answer: false when sessiond says no, fails or
    /// is not running.
    pub fn can_set_system_accessibility(
        &mut self,
        done: impl FnOnce(bool) + Send + 'static,
    ) -> Result<(), String> {
        let proxy = self.system.proxy(LOGIN_NAME, LOGIN_PATH, LOGIN_NAME);
        let proxy = self.system.checked(proxy).map_err(|e| failure(&e))?;
        std::thread::Builder::new()
            .name("login-screen".into())
            .spawn(move || {
                done(
                    match proxy.call::<_, _, bool>("CanSetSystemAccessibility", &()) {
                        Ok(allowed) => allowed,
                        Err(e) => {
                            if !absent(&e) {
                                log_once(&e.to_string());
                            }
                            false
                        }
                    },
                )
            })
            .map_err(|e| {
                log_once(&format!("cannot ask for the login screen: {e}"));
                SERVICE_UNAVAILABLE.to_string()
            })?;
        Ok(())
    }

    /// Make `options` the machine's accessibility defaults (`SetSystemAccessibility`) on a thread of its own, so the
    /// row shows "Saving…" meanwhile. `done` gets the outcome: the row's sentence for a refusal
    /// ([`login_screen_failure`]).
    pub fn set_system_accessibility(
        &mut self,
        options: HashMap<String, String>,
        done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) -> Result<(), String> {
        let proxy = self.system.proxy(LOGIN_NAME, LOGIN_PATH, LOGIN_NAME);
        let proxy = self
            .system
            .checked(proxy)
            .map_err(|e| login_screen_failure(&e))?;
        std::thread::Builder::new()
            .name("login-screen".into())
            .spawn(move || {
                done(
                    proxy
                        .call::<_, _, ()>("SetSystemAccessibility", &(options,))
                        .map_err(|e| login_screen_failure(&e)),
                )
            })
            .map_err(|e| {
                log_once(&format!("cannot change the login screen: {e}"));
                SERVICE_UNAVAILABLE.to_string()
            })?;
        Ok(())
    }
}

impl Default for Services {
    fn default() -> Self {
        Self::new()
    }
}

/// A device of `Bluetooth1.Devices`; one without its always-present keys is skipped.
fn bt_device(d: &HashMap<String, OwnedValue>) -> Option<BtDevice> {
    let text = |k: &str| {
        d.get(k)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
    };
    let flag = |k: &str| d.get(k).and_then(|v| bool::try_from(v).ok());
    Some(BtDevice {
        id: text("Id")?,
        address: text("Address")?,
        name: text("Name").map(|n| display_name(&n)),
        icon: text("Icon")?,
        paired: flag("Paired")?,
        connected: flag("Connected")?,
        pairing: flag("Pairing")?,
    })
}

/// The default sink's worker, its first reading (with the sinks) already asked for.
fn sound_worker(wpctl: &Wpctl) -> VolumeWorker {
    let worker = VolumeWorker::new(wpctl.clone(), Target::DefaultSink);
    worker.request_read(true);
    worker
}
