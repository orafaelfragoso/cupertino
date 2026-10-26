//! The live services (feature `app`): `org.cupertino.Network1`, `org.cupertino.Bluetooth1` and `org.cupertino.Power1`
//! with UPower's display device on the system bus, `org.cupertino.NotificationCenter1` on the session bus, and the mixer's
//! master through `cupertino-alsa` (in-process, as the control panel does, §1). Each connection is made on first use and
//! again after a failure, so a daemon that starts later is picked up; properties are read every time, never cached.

use std::collections::HashMap;
use std::path::PathBuf;

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedValue};

use crate::agent::{Agent, AGENT_PATH};

use crate::live::{
    Battery, BatteryState, Bluetooth, BtDevice, Device, Focus, Network, Reading, Volume, Wifi,
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
pub const CENTRE_PATH: &str = "/org/cupertino/NotificationCenter1";
/// The Bluetooth daemon's refusal to replace another client's live agent.
const AGENT_EXISTS: &str = "org.cupertino.Bluetooth1.AgentExists";

/// The lowest raw brightness the slider writes: 1, so it never turns the panel off (the control panel's floor,
/// approximate).
const BRIGHTNESS_FLOOR: u32 = 1;

/// One bus, connected on first use.
pub struct Bus {
    connect: fn() -> zbus::Result<Connection>,
    connection: Option<Connection>,
}

impl Bus {
    pub fn system() -> Self {
        Bus {
            connect: Connection::system,
            connection: None,
        }
    }

    pub fn session() -> Self {
        Bus {
            connect: Connection::session,
            connection: None,
        }
    }

    /// A bus at `address` (the tests' private buses).
    pub fn at(connection: Connection) -> Self {
        Bus {
            connect: || Err(zbus::Error::Failure("the test bus is gone".into())),
            connection: Some(connection),
        }
    }

    pub fn connection(&mut self) -> zbus::Result<&Connection> {
        if self.connection.is_none() {
            self.connection = Some((self.connect)()?);
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

/// The text of a service failure for the pane: the daemon's error name when it sent one, the bus problem otherwise.
fn failure(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, message, _) => error_text(name.as_str(), message.as_deref()),
        // property reads report the daemon's error as a standard one
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
        Err(e) => Reading::Unavailable(failure(&e)),
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
}

impl Services {
    pub fn new() -> Self {
        Services {
            system: Bus::system(),
            session: Bus::session(),
            mixer: None,
            alsa_dir: PathBuf::from(cupertino_alsa::DEVICE_DIR),
        }
    }

    /// Services on given buses (tests).
    pub fn on(system: Bus, session: Bus) -> Self {
        Services {
            system,
            session,
            mixer: None,
            alsa_dir: PathBuf::from(cupertino_alsa::DEVICE_DIR),
        }
    }

    // ---- reading ---------------------------------------------------------------------------------------------------

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
        let result = self
            .system
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
                                ssid,
                                failure,
                                address,
                            },
                        )
                        .collect(),
                    state: p.get_property("State")?,
                    networks: networks
                        .into_iter()
                        .map(|(ssid, signal, security, saved, connected)| Network {
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
            });
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

    pub fn bluetooth(&mut self, keep: Option<&Bluetooth>) -> Reading<Bluetooth> {
        let result = self
            .system
            .proxy(BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
            .and_then(|p| {
                let devices: Vec<HashMap<String, OwnedValue>> = p.get_property("Devices")?;
                Ok(Bluetooth {
                    available: p.get_property("Available")?,
                    powered: p.get_property("Powered")?,
                    discovering: p.get_property("Discovering")?,
                    devices: devices.iter().filter_map(bt_device).collect(),
                    agent_elsewhere: keep.is_some_and(|b| b.agent_elsewhere),
                    failure: keep.and_then(|b| b.failure.clone()),
                })
            });
        reading(self.system.checked(result), "Bluetooth is not available")
    }

    pub fn volume(&mut self) -> Reading<Volume> {
        match self.with_mixer(|m| {
            let level = m.level()?;
            Ok(Volume {
                level,
                muted: m.muted()?.unwrap_or(level <= 0.0),
            })
        }) {
            Ok(v) => Reading::Ready(v),
            Err(e) => Reading::Unavailable(format!("No sound output is available ({e})")),
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
            Call::SetDoNotDisturb(on) => {
                let result = self.session.proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME).and_then(|p| p.call::<_, _, ()>("SetDoNotDisturb", &(*on,)));
                self.session.checked(result).map_err(|e| failure(&e))
            }
            Call::SetDoNotDisturbFor(id) => {
                let result = self.session.proxy(CENTRE_NAME, CENTRE_PATH, CENTRE_NAME).and_then(|p| p.call::<_, _, ()>("SetDoNotDisturbFor", &(id.as_str(),)));
                self.session.checked(result).map_err(|e| failure(&e))
            }
            Call::SetVolume { level, muted } => self
                .with_mixer(|m| {
                    m.set_level(*level)?;
                    if m.muted()?.is_some_and(|current| current != *muted) {
                        m.set_muted(*muted)?;
                    }
                    Ok(())
                })
                .map_err(|e| format!("The volume could not be changed: {e}")),
            Call::SetBrightness(level) => system(self, POWER_NAME, POWER_PATH, POWER_NAME, &|p| {
                let max: u32 = p.call("GetMaxBrightness", &())?;
                let raw = ((level.clamp(0.0, 1.0) * f64::from(max)).round() as u32).clamp(BRIGHTNESS_FLOOR.min(max), max);
                p.call::<_, _, ()>("SetBrightness", &(raw,))
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
            .map_err(|e| format!("cannot start the pairing: {e}"))?;
        Ok(())
    }

    /// Start or stop Bluetooth discovery (the pane open, §6.4).
    pub fn discovery(&mut self, on: bool) -> Result<(), String> {
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
        name: text("Name"),
        icon: text("Icon")?,
        paired: flag("Paired")?,
        connected: flag("Connected")?,
        pairing: flag("Pairing")?,
    })
}
