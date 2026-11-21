//! The live services: `org.cupertino.Network1` (else NetworkManager, [`nm`]),
//! `org.cupertino.Bluetooth1` (else BlueZ, [`bluez`]), `org.cupertino.Power1` and `org.cupertino.LoginManager1` with
//! UPower's display device on the system bus, `org.cupertino.NotificationCenter1` on
//! the session bus. The Sound pane talks to Audio1 through its own asynchronous client. Each connection is made on first use and
//! again after a failure, so a daemon that starts later is picked up; properties are read every time, never cached.
//! Every call here waits for its service, so the window makes none of them itself: they run as tasks of Kit's I/O pool
//! (`shell::desk`), and a call that can wait long hands back the [`Job`] that waits, for a task of its own.

use crate::hardware::{Hardware, Sources};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::ObjectPath;

use crate::agent::{Agent, AGENT_PATH};
use crate::bluez::{self, BluezAgent};
use crate::nm;
use crate::pane::PaneId;

use crate::live::{
    display_name, Battery, BatteryExtras, BatteryState, Bluetooth, ChargePoint, Focus, Monitor,
    Mouse, Reading, Vpn, Wifi,
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
/// The server's display configuration (D14), which the Displays pane reads and changes.
pub const DISPLAYS_NAME: &str = "org.cupertino.Displays1";
pub const DISPLAYS_PATH: &str = "/org/cupertino/Displays1";
pub const LOGIN_PATH: &str = "/org/cupertino/LoginManager1";
/// The prefix of sessiond's refusals of `SetSystemAccessibility`.
const LOGIN_ERROR: &str = "org.cupertino.LoginManager1.Error.";
/// The Bluetooth daemon's refusal to replace another client's live agent.
const AGENT_EXISTS: &str = "org.cupertino.Bluetooth1.AgentExists";

/// The power profiles service, whose power-saver profile is the Battery pane's low power mode.
pub const PROFILES_NAME: &str = "net.hadess.PowerProfiles";
pub const PROFILES_PATH: &str = "/net/hadess/PowerProfiles";
/// The keyboard backlight of the power service.
const KEYBOARD_BACKLIGHT_NAME: &str = "org.freedesktop.UPower.KbdBacklight";
pub const KEYBOARD_BACKLIGHT_PATH: &str = "/org/freedesktop/UPower/KbdBacklight";
/// How far back the charge history is asked for, and how coarsely: ten days in points ten minutes apart.
const HISTORY_SPAN: u32 = 10 * 24 * 60 * 60;
const HISTORY_STEP: u32 = 600;

/// The lowest raw brightness the slider writes: 1, so it never turns the panel off (the control panel's floor,
/// approximate).
const BRIGHTNESS_FLOOR: u32 = 1;

/// One bus, connected on first use.
pub struct Bus {
    endpoint: Endpoint,
    connection: Option<Connection>,
}

/// Where a [`Bus`] connects.
#[derive(Clone)]
enum Endpoint {
    System,
    Session,
    Address(String),
    /// A test bus handed over connected, which can't be reached again.
    Gone,
}

impl Bus {
    /// Where this bus is for a connection of the window's loop ([`Reach`]): a test's bus is reached on its own
    /// connection.
    pub fn reach(&self) -> Reach {
        Reach {
            endpoint: self.endpoint.clone(),
            connection: Arc::new(Mutex::new(
                self.connection.as_ref().map(|c| c.inner().clone()),
            )),
        }
    }

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

/// A bus for the tasks of the window's loop (the watches, the Sound pane's client): one asynchronous connection, made
/// by the first task that needs it and again once it closed, shared by the clones.
#[derive(Clone)]
pub struct Reach {
    endpoint: Endpoint,
    connection: Arc<Mutex<Option<zbus::Connection>>>,
}

impl Reach {
    /// The connection, made first if there is none open.
    pub fn connect(
        &self,
    ) -> impl std::future::Future<Output = zbus::Result<zbus::Connection>> + Send + 'static {
        let shared = self.connection.clone();
        let endpoint = self.endpoint.clone();
        async move {
            let open = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .filter(|c| !c.is_closed());
            if let Some(connection) = open {
                return Ok(connection);
            }
            let connection = match endpoint {
                Endpoint::Session => zbus::Connection::session().await,
                Endpoint::System => zbus::Connection::system().await,
                Endpoint::Address(address) => {
                    zbus::connection::Builder::address(address.as_str())?
                        .build()
                        .await
                }
                Endpoint::Gone => Err(zbus::Error::Failure("the test bus is gone".into())),
            }?;
            *shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(connection.clone());
            Ok(connection)
        }
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

/// What serves the Wi-Fi and Network panes and the Bluetooth pane: the discovery the control panel and the menu bar
/// share.
pub use cupertino_radios::{BluetoothBackend, NetworkBackend};

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
        if self.network != Some(NetworkBackend::NetworkManager) {
            hidden.push(PaneId::Vpn);
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

/// What the Displays pane says when no display server answers.
const NO_COMPOSITOR: &str = "The display server does not answer";

/// One display as `org.cupertino.Displays1.ListDisplays` gives it: connector, description, whether it is on, its
/// position in points, its mode (width and height in device pixels, refresh rate in millihertz), its scale, its
/// transform, its modes (width, height, refresh rate) and its physical size in millimetres (`(0, 0)` when unknown).
pub type DisplayRecord = (
    String,
    String,
    bool,
    i32,
    i32,
    i32,
    i32,
    u32,
    f64,
    String,
    Vec<(i32, i32, u32)>,
    (u32, u32),
);

/// Whether a transform of `Displays1` turns the display by 90 or 270 degrees.
fn swaps_sides(transform: &str) -> bool {
    matches!(transform, "90" | "270" | "flipped-90" | "flipped-270")
}

/// The displays that are on, from `Displays1`'s records: a display that is off or has no mode is left out.
pub fn monitors(records: Vec<DisplayRecord>) -> Vec<Monitor> {
    records
        .into_iter()
        .filter_map(
            |(
                connector,
                description,
                enabled,
                x,
                y,
                width,
                height,
                refresh,
                scale,
                transform,
                modes,
                mm,
            )| {
                let mode = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
                if !enabled || mode.0 == 0 || mode.1 == 0 {
                    return None;
                }
                let rotated = swaps_sides(&transform);
                let scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };
                let points = |pixels: u32| (f64::from(pixels) / scale).round() as u32;
                let logical = if rotated {
                    (points(mode.1), points(mode.0))
                } else {
                    (points(mode.0), points(mode.1))
                };
                let mut refresh_rates: Vec<u32> = modes
                    .iter()
                    .filter(|(w, h, r)| (*w, *h) == (width, height) && *r > 0)
                    .map(|(_, _, r)| *r)
                    .collect();
                refresh_rates.sort_unstable();
                refresh_rates.dedup();
                let title = crate::about::display_title(&crate::about::Display {
                    description: Some(&description),
                    name: Some(&connector),
                    ..Default::default()
                })
                .unwrap_or_else(|| "Display".to_owned());
                Some(Monitor {
                    title,
                    mode,
                    refresh_mhz: (refresh > 0).then_some(refresh),
                    scale,
                    connector: Some(connector),
                    physical_mm: (mm.0 > 0 && mm.1 > 0).then_some(mm),
                    logical: Some(logical),
                    rotated,
                    position: Some((x, y)),
                    refresh_rates,
                })
            },
        )
        .collect()
}

/// What a display's row says when the server did not change it.
const REFRESH_NOT_CHANGED: &str = "The refresh rate could not be changed.";
const NOT_MOVED: &str = "The display could not be moved.";
const SIZE_NOT_CHANGED: &str = "The display size could not be changed.";

/// A display change refused or failed: the reason goes to the log, `sentence` to the row.
fn display_failure(e: &zbus::Error, sentence: &str) -> String {
    log_once(&format!("{sentence} {e}"));
    sentence.to_owned()
}

/// A new Wi-Fi reading with what the window added to the one it shows, `keep`: a join in progress ends when its
/// network is connected or a device reports a failure.
pub fn carry_wifi(reading: Reading<Wifi>, keep: Option<&Wifi>) -> Reading<Wifi> {
    match (reading, keep) {
        (Reading::Ready(mut w), Some(previous)) => {
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
            Reading::Ready(w)
        }
        (other, _) => other,
    }
}

/// A new Bluetooth reading with what the window added to the one it shows, `keep`: whether another client's agent
/// pairs, the failures shown, and the connections asked for, which mark their devices connecting.
pub fn carry_bluetooth(
    reading: Reading<Bluetooth>,
    keep: Option<&Bluetooth>,
) -> Reading<Bluetooth> {
    reading.map(|b| {
        let mut b = Bluetooth {
            agent_elsewhere: keep.is_some_and(|k| k.agent_elsewhere),
            failure: keep.and_then(|k| k.failure.clone()),
            asked_connect: keep.map(|k| k.asked_connect.clone()).unwrap_or_default(),
            device_failure: keep.and_then(|k| k.device_failure.clone()),
            ..b
        };
        b.mark_connecting();
        b
    })
}

/// A call that waits for its service (a pairing, a device, a join, a display's mode), made by a task of its own:
/// the services go on meanwhile. Its error is the row's sentence.
pub type Job = Box<dyn FnOnce() -> Result<(), String> + Send>;

/// The live services and their connections.
pub struct Services {
    pub system: Bus,
    pub session: Bus,
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

    // ---- reading ---------------------------------------------------------------------------------------------------

    /// Whether the hardware facts are this machine's, so devices coming and going are heard ([`crate::hotplug`]).
    pub fn reads_hardware(&self) -> bool {
        self.hardware.is_some()
    }

    /// Look up which services are on the system bus, running or activatable. `org.cupertino.Network1` comes before
    /// NetworkManager, whose name the network daemon also claims. When the bus does not answer the last answer stays.
    /// Which services back the panes, and the hardware facts.
    pub fn detect(&mut self) -> Backends {
        self.backends.hardware = self
            .hardware
            .as_ref()
            .map(Hardware::read)
            .unwrap_or_default();
        let hardware = self.backends.hardware;
        let result = self
            .system
            .connection()
            .and_then(cupertino_radios::detect)
            .map(|found| Backends {
                network: found.network,
                bluetooth: found.bluetooth,
                hardware,
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
            "The notification center is not running",
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

    /// The displays that are on, as the server's `Displays1` lists them on the session bus.
    pub fn displays(&mut self) -> Reading<Vec<Monitor>> {
        let result = self
            .session
            .proxy(DISPLAYS_NAME, DISPLAYS_PATH, DISPLAYS_NAME)
            .and_then(|p| p.call::<_, _, Vec<DisplayRecord>>("ListDisplays", &()));
        reading(self.session.checked(result), NO_COMPOSITOR).map(monitors)
    }

    /// `method` of `Displays1` with `body`; a refusal is logged and the row says `sentence`.
    fn set_displays<B>(
        &mut self,
        method: &'static str,
        body: &B,
        sentence: &str,
    ) -> Result<(), String>
    where
        B: zbus::export::serde::Serialize + zbus::zvariant::DynamicType,
    {
        let result = self
            .session
            .proxy(DISPLAYS_NAME, DISPLAYS_PATH, DISPLAYS_NAME)
            .and_then(|p| p.call::<_, _, ()>(method, body));
        self.session
            .checked(result)
            .map_err(|e| display_failure(&e, sentence))
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

    /// The VPN connections NetworkManager holds; none are listed while another service manages the network.
    pub fn vpns(&mut self) -> Reading<Vec<Vpn>> {
        if !self.network_manager() {
            return Reading::Unavailable("VPN connections are managed by NetworkManager".into());
        }
        let result = self.system.connection().and_then(nm::vpns);
        reading(self.system.checked(result), "NetworkManager is not running")
    }

    /// The battery's health, charge history and power-saving profile. `with_history` reads the charge history too,
    /// which is long: the pane asks for it when it opens and now and then.
    pub fn battery_extras(&mut self, keep: &BatteryExtras, with_history: bool) -> BatteryExtras {
        let mut extras = BatteryExtras {
            history: keep.history.clone(),
            ..BatteryExtras::default()
        };
        let device = self
            .system
            .proxy(UPOWER_NAME, UPOWER_DISPLAY_DEVICE, UPOWER_DEVICE)
            .and_then(|p| {
                let full: f64 = p.get_property("EnergyFull")?;
                let design: f64 = p.get_property("EnergyFullDesign")?;
                let rate: f64 = p.get_property("EnergyRate")?;
                let history: Option<Vec<(u32, f64, u32)>> = if with_history {
                    p.call("GetHistory", &("charge", HISTORY_SPAN, HISTORY_STEP))
                        .ok()
                } else {
                    None
                };
                Ok((full, design, rate, history))
            });
        if let Ok((full, design, rate, history)) = self.system.checked(device) {
            extras.health =
                (design > 0.0 && full > 0.0).then(|| (full / design * 100.0).min(100.0));
            extras.rate_watts = (rate.abs() > 0.0).then_some(rate.abs());
            if let Some(rows) = history {
                extras.history = rows
                    .into_iter()
                    .map(|(time, percent, state)| ChargePoint {
                        time: i64::from(time),
                        percent,
                        charging: state == 1,
                    })
                    .collect();
            }
        }
        let profile = self
            .system
            .proxy(PROFILES_NAME, PROFILES_PATH, PROFILES_NAME)
            .and_then(|p| p.get_property::<String>("ActiveProfile"));
        extras.low_power = self
            .system
            .checked(profile)
            .ok()
            .map(|p| p == "power-saver");
        extras
    }

    /// The keyboard backlight's level, 0 to 1; `None` without one.
    pub fn keyboard_backlight(&mut self) -> Option<f64> {
        let result = self
            .system
            .proxy(
                UPOWER_NAME,
                KEYBOARD_BACKLIGHT_PATH,
                KEYBOARD_BACKLIGHT_NAME,
            )
            .and_then(|p| {
                let max: i32 = p.call("GetMaxBrightness", &())?;
                let now: i32 = p.call("GetBrightness", &())?;
                Ok((now, max))
            });
        match self.system.checked(result) {
            Ok((now, max)) if max > 0 => Some(f64::from(now) / f64::from(max)),
            _ => None,
        }
    }

    /// The Wi-Fi radio, its devices and networks as the service reports them; what the window adds to them
    /// (a join in progress, its failure) is carried over by [`carry_wifi`].
    pub fn wifi(&mut self) -> Reading<Wifi> {
        // before the first detection the Cupertino daemon is asked, as its calls are
        let backend = self.backends.network.unwrap_or(NetworkBackend::Cupertino);
        let result = self.system.connection().and_then(|c| backend.wifi(c));
        reading(self.system.checked(result), "Wi-Fi is not available")
    }

    /// The Bluetooth adapter and its devices as the service reports them; what the window adds to them (the agent
    /// elsewhere, the failures, the connections asked for) is carried over by [`carry_bluetooth`].
    pub fn bluetooth(&mut self) -> Reading<Bluetooth> {
        let backend = self
            .backends
            .bluetooth
            .unwrap_or(BluetoothBackend::Cupertino);
        let pairing = self
            .pairing
            .lock()
            .expect("no thread panics while pairing")
            .clone();
        let result = self
            .system
            .connection()
            .and_then(|c| backend.adapter(c, &pairing));
        reading(self.system.checked(result), "Bluetooth is not available").map(|adapter| {
            Bluetooth {
                available: adapter.available,
                powered: adapter.powered,
                discovering: adapter.discovering,
                devices: adapter.devices,
                ..Bluetooth::default()
            }
        })
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
            Call::ForgetNetwork(ssid) if self.network_manager() => self.on_system(|c| nm::forget(c, ssid)),
            Call::SetAutoJoin { ssid, on } if self.network_manager() => self.on_system(|c| nm::set_auto_join(c, ssid, *on)),
            Call::SetVpn { id, on } => self.on_system(|c| nm::set_vpn(c, id, *on)),
            Call::ForgetNetwork(ssid) => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.call::<_, _, ()>("Forget", &(ssid.as_str(),))),
            Call::SetAutoJoin { ssid, on } => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.call::<_, _, ()>("SetAutoJoin", &(ssid.as_str(), *on))),
            Call::SetLowPower(on) => system(self, PROFILES_NAME, PROFILES_PATH, PROFILES_NAME, &|p| p.set_property("ActiveProfile", if *on { "power-saver" } else { "balanced" }).map_err(zbus::Error::from)),
            Call::SetKeyboardBacklight(level) => system(self, UPOWER_NAME, KEYBOARD_BACKLIGHT_PATH, KEYBOARD_BACKLIGHT_NAME, &|p| {
                let max: i32 = p.call("GetMaxBrightness", &())?;
                p.call::<_, _, ()>("SetBrightness", &(((level.clamp(0.0, 1.0) * f64::from(max)).round() as i32),))
            }),
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
            Call::SetBrightness(level) => system(self, POWER_NAME, POWER_PATH, POWER_NAME, &|p| {
                let max: u32 = p.call("GetMaxBrightness", &())?;
                let raw = ((level.clamp(0.0, 1.0) * f64::from(max)).round() as u32).clamp(BRIGHTNESS_FLOOR.min(max), max);
                p.call::<_, _, ()>("SetBrightness", &(raw,))
            }),
            Call::SetDisplayRefresh { connector, mhz } => self.set_displays("SetRefreshRate", &(connector.as_str(), *mhz), REFRESH_NOT_CHANGED),
            Call::SetDisplayPosition { connector, x, y } => self.set_displays("SetPosition", &(connector.as_str(), *x, *y), NOT_MOVED),
            Call::SetDisplayScale { connector, scale } => self.set_displays("SetScale", &(connector.as_str(), *scale), SIZE_NOT_CHANGED),
            Call::SetWirelessEnabled(on) => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.set_property("WirelessEnabled", *on).map_err(zbus::Error::from)),
            Call::Join { ssid, passphrase } => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.call::<_, _, ()>("Connect", &(ssid.as_str(), passphrase.as_ref().map_or("", |p| p.as_str())))),
            Call::Disconnect => system(self, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME, &|p| p.call::<_, _, ()>("Disconnect", &())),
            Call::SetBluetoothPowered(on) => system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.set_property("Powered", *on).map_err(zbus::Error::from)),
            Call::Pair(_) => panic!("a pairing waits for the agent's sheet, so it runs as a task of its own: Services::pair"),
            Call::Connect(id) => system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.call::<_, _, ()>("Connect", &(id.as_str(),))),
            Call::DisconnectDevice(id) => system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.call::<_, _, ()>("Disconnect", &(id.as_str(),))),
            Call::Remove { id, confirmed } => {
                assert!(*confirmed, "a device is removed only after its confirmation");
                system(self, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME, &|p| p.call::<_, _, ()>("Remove", &(id.as_str(),)))
            }
        }
    }

    /// The [`Job`] that makes `call` when it can take long: a device's connection, disconnection or removal, a join of
    /// a saved network, a display's mode or place. `None` when `call` is quick and is made with [`Self::call`];
    /// `Some(Err)` when its service can't be reached.
    pub fn long_call(&mut self, call: &Call) -> Option<Result<Job, String>> {
        type BusCall = Box<dyn FnOnce(&Connection) -> zbus::Result<()> + Send>;
        let on_bus = |s: &mut Self, f: BusCall| -> Result<Job, String> {
            let connection = s.system.connection().cloned();
            let connection = s.system.checked(connection).map_err(|e| failure(&e))?;
            Ok(Box::new(move || f(&connection).map_err(|e| failure(&e))))
        };
        let on_proxy = |s: &mut Self, method: &'static str, id: String| -> Result<Job, String> {
            let proxy = s
                .system
                .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME);
            let proxy = s.system.checked(proxy).map_err(|e| failure(&e))?;
            Ok(Box::new(move || {
                proxy
                    .call::<_, _, ()>(method, &(id.as_str(),))
                    .map_err(|e| failure(&e))
            }))
        };
        let on_displays = |s: &mut Self| -> Result<Proxy<'static>, String> {
            let proxy = s.session.proxy(DISPLAYS_NAME, DISPLAYS_PATH, DISPLAYS_NAME);
            s.session.checked(proxy).map_err(|e| failure(&e))
        };
        let job: Result<Job, String> = match call.clone() {
            Call::Connect(id) if self.bluez() => {
                on_bus(self, Box::new(move |c| bluez::connect(c, &id)))
            }
            Call::DisconnectDevice(id) if self.bluez() => {
                on_bus(self, Box::new(move |c| bluez::disconnect(c, &id)))
            }
            Call::Remove { id, confirmed } if self.bluez() => {
                assert!(confirmed, "a device is removed only after its confirmation");
                on_bus(self, Box::new(move |c| bluez::remove(c, &id)))
            }
            Call::Connect(id) => on_proxy(self, "Connect", id),
            Call::DisconnectDevice(id) => on_proxy(self, "Disconnect", id),
            Call::Remove { id, confirmed } => {
                assert!(confirmed, "a device is removed only after its confirmation");
                on_proxy(self, "Remove", id)
            }
            Call::Join {
                ssid,
                passphrase: None,
            } if self.network_manager() => on_bus(self, Box::new(move |c| nm::join(c, &ssid, ""))),
            Call::SetDisplayRefresh { connector, mhz } => on_displays(self).map(|p| -> Job {
                Box::new(move || {
                    p.call::<_, _, ()>("SetRefreshRate", &(connector.as_str(), mhz))
                        .map_err(|e| display_failure(&e, REFRESH_NOT_CHANGED))
                })
            }),
            Call::SetDisplayPosition { connector, x, y } => on_displays(self).map(|p| -> Job {
                Box::new(move || {
                    p.call::<_, _, ()>("SetPosition", &(connector.as_str(), x, y))
                        .map_err(|e| display_failure(&e, NOT_MOVED))
                })
            }),
            Call::SetDisplayScale { connector, scale } => on_displays(self).map(|p| -> Job {
                Box::new(move || {
                    p.call::<_, _, ()>("SetScale", &(connector.as_str(), scale))
                        .map_err(|e| display_failure(&e, SIZE_NOT_CHANGED))
                })
            }),
            _ => return None,
        };
        Some(job)
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

    /// The [`Job`] that pairs device `id`: `Pair` returns only when the pairing is over, and meanwhile the daemon asks
    /// the agent, whose sheet needs the window.
    pub fn pair(&mut self, id: &str) -> Result<Job, String> {
        let id = id.to_string();
        if self.bluez() {
            let device = self
                .system
                .connection()
                .and_then(|c| bluez::pairing(c, &id));
            let device = self.system.checked(device).map_err(|e| failure(&e))?;
            let pairing = self.pairing.clone();
            pairing
                .lock()
                .expect("no thread panics while pairing")
                .insert(id.clone());
            return Ok(Box::new(move || {
                let result = bluez::pair(&device).map_err(|e| failure(&e));
                pairing
                    .lock()
                    .expect("no thread panics while pairing")
                    .remove(&id);
                result
            }));
        }
        let proxy = self
            .system
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME);
        let proxy = self.system.checked(proxy).map_err(|e| failure(&e))?;
        Ok(Box::new(move || {
            proxy
                .call::<_, _, ()>("Pair", &(id.as_str(),))
                .map_err(|e| failure(&e))
        }))
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

    /// What asks sessiond whether this person may set the machine's accessibility defaults (each opening of the
    /// Accessibility pane, never kept), for a task of its own: false when sessiond says no, fails or is not running.
    pub fn can_set_system_accessibility(
        &mut self,
    ) -> Result<Box<dyn FnOnce() -> bool + Send>, String> {
        let proxy = self.system.proxy(LOGIN_NAME, LOGIN_PATH, LOGIN_NAME);
        let proxy = self.system.checked(proxy).map_err(|e| failure(&e))?;
        Ok(Box::new(move || {
            match proxy.call::<_, _, bool>("CanSetSystemAccessibility", &()) {
                Ok(allowed) => allowed,
                Err(e) => {
                    if !absent(&e) {
                        log_once(&e.to_string());
                    }
                    false
                }
            }
        }))
    }

    /// The [`Job`] that makes `options` the machine's accessibility defaults (`SetSystemAccessibility`), for a task of
    /// its own, so the row shows "Saving…" meanwhile; a refusal is the row's sentence ([`login_screen_failure`]).
    pub fn set_system_accessibility(
        &mut self,
        options: HashMap<String, String>,
    ) -> Result<Job, String> {
        let proxy = self.system.proxy(LOGIN_NAME, LOGIN_PATH, LOGIN_NAME);
        let proxy = self
            .system
            .checked(proxy)
            .map_err(|e| login_screen_failure(&e))?;
        Ok(Box::new(move || {
            proxy
                .call::<_, _, ()>("SetSystemAccessibility", &(options,))
                .map_err(|e| login_screen_failure(&e))
        }))
    }

    /// Ask the server for the appearance (`org.cupertino.Session1.SetAppearance`).
    pub fn set_appearance(&mut self, dark: bool) -> Result<(), String> {
        let name = if dark { "dark" } else { "light" };
        let result = self
            .session
            .proxy(SESSION_NAME, SESSION_PATH, SESSION_NAME)
            .and_then(|p| p.call::<_, _, ()>("SetAppearance", &(name,)));
        self.session
            .checked(result)
            .map_err(|e| format!("The appearance could not be changed: {}", failure(&e)))
    }

    /// Ask the server to set one key of `accessibility.conf` (`org.cupertino.Session1.SetAccessibility`).
    pub fn set_accessibility(&mut self, key: &str, value: &str) -> Result<(), String> {
        let result = self
            .session
            .proxy(SESSION_NAME, SESSION_PATH, SESSION_NAME)
            .and_then(|p| p.call::<_, _, ()>("SetAccessibility", &(key, value)));
        self.session
            .checked(result)
            .map_err(|e| format!("The setting could not be changed: {}", failure(&e)))
    }
}

impl Default for Services {
    fn default() -> Self {
        Self::new()
    }
}
