//! The real backends over D-Bus: Wi-Fi through `cupertino-netd`, Bluetooth through `cupertino-btd`, the display
//! backlight and the battery through `cupertino-powerd` (all on the system bus), and the overlay and the focus mode
//! (the do-not-disturb switch of `cupertino-notifications`) on the session bus.
//! Each connection is made on the first use and again after a failure, so a daemon that starts later is picked up.
//! Properties are read from the service every time, never from a cache.

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;

use crate::backend::{
    BackendError, BatteryReader, Bluetooth, Brightness, FocusMode, OverlayKind, OverlayNotifier,
    Wifi,
};
use crate::model::{Battery, WifiStatus};

const NETWORK_NAME: &str = "org.cupertino.Network1";
const NETWORK_PATH: &str = "/org/cupertino/Network1";
const BLUETOOTH_NAME: &str = "org.cupertino.Bluetooth1";
const BLUETOOTH_PATH: &str = "/org/cupertino/Bluetooth1";
const POWER_NAME: &str = "org.cupertino.Power1";
const POWER_PATH: &str = "/org/cupertino/Power1";
const UPOWER_NAME: &str = "org.freedesktop.UPower";
const UPOWER_DISPLAY_DEVICE_PATH: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const UPOWER_DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";
const OSD_NAME: &str = "org.cupertino.Osd1";
const OSD_PATH: &str = "/org/cupertino/Osd1";
const CENTRE_NAME: &str = "org.cupertino.NotificationCenter1";
const CENTRE_PATH: &str = "/org/cupertino/NotificationCenter1";

/// `State` value of a charging battery in the UPower interface the power daemon serves.
const UPOWER_STATE_CHARGING: u32 = 1;

/// Lowest raw display brightness a slider reaches: 1, so the slider never turns the screen off (approximate, the
/// same floor as the overlay's key presses; the design maps 0.0 to the panel's device minimum).
const DISPLAY_FLOOR: u32 = 1;

/// One bus connection, made when first needed.
struct Bus {
    session: bool,
    connection: Option<Connection>,
}

impl Bus {
    fn system() -> Self {
        Self {
            session: false,
            connection: None,
        }
    }

    fn session() -> Self {
        Self {
            session: true,
            connection: None,
        }
    }

    fn proxy(
        &mut self,
        name: &'static str,
        path: &'static str,
        interface: &'static str,
    ) -> zbus::Result<Proxy<'static>> {
        if self.connection.is_none() {
            self.connection = Some(if self.session {
                Connection::session()?
            } else {
                Connection::system()?
            });
        }
        let connection = self.connection.as_ref().expect("connected above");
        Builder::new(connection)
            .destination(name)?
            .path(path)?
            .interface(interface)?
            .cache_properties(CacheProperties::No)
            .build()
    }

    /// Forget the connection after a failure so the next call connects again.
    fn failed<T, E: Into<BackendError>>(
        &mut self,
        result: Result<T, E>,
    ) -> Result<T, BackendError> {
        result.map_err(|e| {
            self.connection = None;
            e.into()
        })
    }
}

/// The Wi-Fi radio of `org.cupertino.Network1`: the `WirelessEnabled` property and the first connected Wi-Fi device.
pub struct NetdWifi {
    bus: Bus,
}

impl NetdWifi {
    /// A Wi-Fi backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self { bus: Bus::system() }
    }
}

impl Default for NetdWifi {
    fn default() -> Self {
        Self::new()
    }
}

/// A row of the `Devices` property: index, name, kind, state, SSID, failure, address.
type DeviceRecord = (u32, String, String, String, String, String, String);

impl Wifi for NetdWifi {
    fn read(&mut self) -> Result<WifiStatus, BackendError> {
        let result = self
            .bus
            .proxy(NETWORK_NAME, NETWORK_PATH, NETWORK_NAME)
            .and_then(|proxy| {
                let enabled: bool = proxy.get_property("WirelessEnabled")?;
                let devices: Vec<DeviceRecord> = proxy.get_property("Devices")?;
                let network = devices
                    .into_iter()
                    .find(|device| device.2 == "wifi" && device.3 == "connected")
                    .map(|device| device.4)
                    .filter(|ssid| !ssid.is_empty());
                Ok(WifiStatus { enabled, network })
            });
        self.bus.failed(result)
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(NETWORK_NAME, NETWORK_PATH, NETWORK_NAME)
            .map(|proxy| proxy.set_property("WirelessEnabled", enabled));
        // a refused write is the service's answer, not a bus failure, but reconnecting is harmless
        match self.bus.failed(result) {
            Ok(written) => Ok(written?),
            Err(e) => Err(e),
        }
    }
}

/// The Bluetooth adapter of `org.cupertino.Bluetooth1`: the `Available` and `Powered` properties.
pub struct BtdBluetooth {
    bus: Bus,
}

impl BtdBluetooth {
    /// A Bluetooth backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self { bus: Bus::system() }
    }
}

impl Default for BtdBluetooth {
    fn default() -> Self {
        Self::new()
    }
}

impl Bluetooth for BtdBluetooth {
    fn read(&mut self) -> Result<bool, BackendError> {
        let result = self
            .bus
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
            .and_then(|proxy| {
                let available: bool = proxy.get_property("Available")?;
                if !available {
                    return Ok(Err("the Bluetooth daemon has no adapter".into()));
                }
                Ok(Ok(proxy.get_property::<bool>("Powered")?))
            });
        self.bus.failed(result)?
    }

    fn set_powered(&mut self, powered: bool) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
            .map(|proxy| proxy.set_property("Powered", powered));
        match self.bus.failed(result) {
            Ok(written) => Ok(written?),
            Err(e) => Err(e),
        }
    }
}

/// The do-not-disturb switch of `org.cupertino.NotificationCenter1`: the `DoNotDisturb` property and `SetDoNotDisturb`.
pub struct NotificationFocusMode {
    bus: Bus,
}

impl NotificationFocusMode {
    /// A focus mode backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self {
            bus: Bus::session(),
        }
    }
}

impl Default for NotificationFocusMode {
    fn default() -> Self {
        Self::new()
    }
}

impl FocusMode for NotificationFocusMode {
    fn read(&mut self) -> Result<bool, BackendError> {
        let result = self
            .bus
            .proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
            .and_then(|proxy| proxy.get_property::<bool>("DoNotDisturb"));
        self.bus.failed(result)
    }

    fn set(&mut self, on: bool) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
            .and_then(|proxy| proxy.call::<_, _, ()>("SetDoNotDisturb", &(on,)));
        self.bus.failed(result)
    }
}

/// The display backlight of `org.cupertino.Power1`, in the daemon's raw units mapped linearly onto 0 to 1.
pub struct PowerdBrightness {
    bus: Bus,
}

impl PowerdBrightness {
    /// A brightness backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self { bus: Bus::system() }
    }
}

impl Default for PowerdBrightness {
    fn default() -> Self {
        Self::new()
    }
}

impl Brightness for PowerdBrightness {
    fn read(&mut self) -> Result<f64, BackendError> {
        let result = self
            .bus
            .proxy(POWER_NAME, POWER_PATH, POWER_NAME)
            .and_then(|proxy| {
                let current: u32 = proxy.call("GetBrightness", &())?;
                let max: u32 = proxy.call("GetMaxBrightness", &())?;
                Ok(if max == 0 {
                    Err("the display backlight reports a maximum of 0".into())
                } else {
                    Ok(f64::from(current) / f64::from(max))
                })
            });
        self.bus.failed(result)?
    }

    fn set(&mut self, level: f64) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(POWER_NAME, POWER_PATH, POWER_NAME)
            .and_then(|proxy| {
                let max: u32 = proxy.call("GetMaxBrightness", &())?;
                if max == 0 {
                    return Ok(Err("the display backlight reports a maximum of 0".into()));
                }
                let raw = (level.clamp(0.0, 1.0) * f64::from(max)).round() as u32;
                proxy.call::<_, _, ()>(
                    "SetBrightness",
                    &(raw.clamp(DISPLAY_FLOOR.min(max), max),),
                )?;
                Ok(Ok(()))
            });
        self.bus.failed(result)?
    }
}

/// The battery of the power daemon's display device (all batteries as one).
pub struct PowerdBattery {
    bus: Bus,
}

impl PowerdBattery {
    /// A battery backend; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self { bus: Bus::system() }
    }
}

impl Default for PowerdBattery {
    fn default() -> Self {
        Self::new()
    }
}

impl BatteryReader for PowerdBattery {
    fn read(&mut self) -> Result<Option<Battery>, BackendError> {
        let result = self
            .bus
            .proxy(
                UPOWER_NAME,
                UPOWER_DISPLAY_DEVICE_PATH,
                UPOWER_DEVICE_INTERFACE,
            )
            .and_then(|proxy| {
                if !proxy.get_property::<bool>("IsPresent")? {
                    return Ok(None);
                }
                let percentage: f64 = proxy.get_property("Percentage")?;
                let state: u32 = proxy.get_property("State")?;
                Ok(Some(Battery {
                    percent: percentage.round().clamp(0.0, 100.0) as u8,
                    charging: state == UPOWER_STATE_CHARGING,
                }))
            });
        self.bus.failed(result)
    }
}

/// The overlay's `Show` on the session bus.
pub struct SessionOverlay {
    bus: Bus,
}

impl SessionOverlay {
    /// An overlay notifier; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self {
            bus: Bus::session(),
        }
    }
}

impl Default for SessionOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayNotifier for SessionOverlay {
    fn show(&mut self, kind: OverlayKind, level: f64) -> Result<(), BackendError> {
        let result = self
            .bus
            .proxy(OSD_NAME, OSD_PATH, OSD_NAME)
            .and_then(|proxy| proxy.call::<_, _, ()>("Show", &(kind.name(), level)));
        self.bus.failed(result)
    }
}
