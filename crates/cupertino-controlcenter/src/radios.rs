//! The Wi-Fi radio and the Bluetooth adapter through whichever service owns them (`cupertino-radios`): the Cupertino
//! network and Bluetooth daemons, else NetworkManager and BlueZ, the same discovery and readings as Settings and the
//! menu bar.
//!
//! Both backends share one connection to the system bus, made on the first use and again after a failure, so a bus
//! or a service that starts later is picked up. The services are looked up at every reading and nothing is cached. The
//! services' change signals are followed on that connection: [`Radios::on_change`] hears when a reading is out of
//! date, so the panel shows a radio switched elsewhere without waiting for its next periodic reading.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cupertino_radios::{refusal, BluetoothBackend, NetworkBackend};
use zbus::blocking::{connection, Connection};

use crate::backend::{BackendError, Bluetooth, Wifi};
use crate::model::{Device, Network, WifiStatus};

/// What a tile says when no service serves its radio.
const NO_NETWORK_SERVICE: &str = "no network service is running";
const NO_BLUETOOTH_SERVICE: &str = "no Bluetooth service is running";
/// What a tile says on a machine without the radio.
const NO_WIFI_DEVICE: &str = "there is no Wi-Fi device";
const NO_ADAPTER: &str = "there is no Bluetooth adapter";

type OnChange = Arc<dyn Fn() + Send + Sync>;

struct Shared {
    /// The bus's address; `None` takes the system bus of the environment.
    address: Option<String>,
    connection: Option<Connection>,
    on_change: Option<OnChange>,
}

/// The system bus the radios are read on.
#[derive(Clone)]
pub struct Radios {
    shared: Arc<Mutex<Shared>>,
}

impl Radios {
    /// The radios of the system bus; nothing is contacted until the first use.
    pub fn new() -> Self {
        Self::on(None)
    }

    /// The radios of the bus at `address`.
    pub fn at(address: &str) -> Self {
        Self::on(Some(address.to_owned()))
    }

    fn on(address: Option<String>) -> Self {
        Radios {
            shared: Arc::new(Mutex::new(Shared {
                address,
                connection: None,
                on_change: None,
            })),
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .expect("no thread panics holding the radios' bus")
    }

    /// Call `on_change` whenever a signal of the services says the radios changed, or a service started or stopped.
    /// It is called from a thread of the bus and must not wait.
    pub fn on_change(&self, on_change: impl Fn() + Send + Sync + 'static) {
        let on_change: OnChange = Arc::new(on_change);
        let mut shared = self.shared();
        if let Some(connection) = &shared.connection {
            follow(connection, &on_change);
        }
        shared.on_change = Some(on_change);
    }

    /// The Wi-Fi radio.
    pub fn wifi(&self) -> RadioWifi {
        RadioWifi {
            radios: self.clone(),
            keys: HashMap::new(),
        }
    }

    /// The Bluetooth adapter.
    pub fn bluetooth(&self) -> RadioBluetooth {
        RadioBluetooth {
            radios: self.clone(),
        }
    }

    fn connection(&self) -> zbus::Result<Connection> {
        let mut shared = self.shared();
        if let Some(connection) = &shared.connection {
            return Ok(connection.clone());
        }
        let connection = match &shared.address {
            Some(address) => connection::Builder::address(address.as_str())?,
            None => connection::Builder::system()?,
        }
        .build()?;
        if let Some(on_change) = &shared.on_change {
            follow(&connection, on_change);
        }
        shared.connection = Some(connection.clone());
        Ok(connection)
    }

    /// Run `f` on the bus; a failure of the bus itself forgets the connection, so the next use connects again.
    fn with<T>(&self, f: impl FnOnce(&Connection) -> zbus::Result<T>) -> zbus::Result<T> {
        let result = self.connection().and_then(|connection| f(&connection));
        if let Err(zbus::Error::InputOutput(_) | zbus::Error::Address(_)) = &result {
            self.shared().connection = None;
        }
        result
    }

    fn network(&self) -> Result<(Connection, NetworkBackend), BackendError> {
        let (connection, found) = self.with(|c| Ok((c.clone(), cupertino_radios::detect(c)?)))?;
        Ok((connection, found.network.ok_or(NO_NETWORK_SERVICE)?))
    }

    fn adapter(&self) -> Result<(Connection, BluetoothBackend), BackendError> {
        let (connection, found) = self.with(|c| Ok((c.clone(), cupertino_radios::detect(c)?)))?;
        Ok((connection, found.bluetooth.ok_or(NO_BLUETOOTH_SERVICE)?))
    }

    /// The service's answer to a call: its own text when it refused, and a lost bus is connected again.
    fn answered<T>(&self, result: zbus::Result<T>) -> Result<T, BackendError> {
        result.map_err(|e| {
            if let zbus::Error::InputOutput(_) = &e {
                self.shared().connection = None;
            }
            refusal(&e).into()
        })
    }
}

impl Default for Radios {
    fn default() -> Self {
        Self::new()
    }
}

/// Follow the radios' signals on `connection`; one that cannot be followed is still read at the panel's periodic
/// reading.
fn follow(connection: &Connection, on_change: &OnChange) {
    let followed = zbus::block_on(cupertino_radios::watch::follow(
        connection.inner(),
        on_change.clone(),
    ));
    if let Err(e) = followed {
        eprintln!("cupertino-controlcenter: cannot follow the radios' change signals: {e}");
    }
}

/// The Wi-Fi radio: the switch, the network the first Wi-Fi device is connected to, and the networks in range.
pub struct RadioWifi {
    radios: Radios,
    /// The service's key of each network of the last list, by the name the list shows.
    keys: HashMap<String, String>,
}

impl Wifi for RadioWifi {
    fn read(&mut self) -> Result<WifiStatus, BackendError> {
        let (connection, backend) = self.radios.network()?;
        let link = self.radios.answered(backend.link(&connection))?;
        if link.device.is_none() {
            return Err(NO_WIFI_DEVICE.into());
        }
        Ok(WifiStatus {
            enabled: link.enabled,
            network: link.network().map(str::to_owned),
        })
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), BackendError> {
        let (connection, backend) = self.radios.network()?;
        self.radios
            .answered(backend.set_wireless_enabled(&connection, enabled))
    }

    /// The networks in range, after asking for a scan, whose list arrives with a later reading.
    fn networks(&mut self) -> Result<Vec<Network>, BackendError> {
        let (connection, backend) = self.radios.network()?;
        // a refused or busy scan still leaves the last list to show
        let _ = backend.scan(&connection);
        let wifi = self.radios.answered(backend.wifi(&connection))?;
        self.keys.clear();
        let mut networks = Vec::new();
        for network in wifi.networks {
            // two networks that read the same could not be told apart in the list: the stronger one is shown
            if network.name.is_empty() || self.keys.contains_key(&network.name) {
                continue;
            }
            self.keys.insert(network.name.clone(), network.ssid);
            networks.push(Network {
                ssid: network.name,
                bars: (network.signal != i32::MIN).then(|| Network::bars_of(network.signal)),
                secured: network.security != "open",
                known: network.saved,
                connected: network.connected,
            });
        }
        Ok(networks)
    }

    /// Join with no passphrase: an open network, or one whose credentials the service holds.
    fn join(&mut self, ssid: &str) -> Result<(), BackendError> {
        let (connection, backend) = self.radios.network()?;
        let key = self.keys.get(ssid).map_or(ssid, String::as_str);
        self.radios.answered(backend.join(&connection, key, ""))
    }
}

/// The Bluetooth adapter: the switch and the paired devices.
pub struct RadioBluetooth {
    radios: Radios,
}

impl RadioBluetooth {
    fn state(&self) -> Result<cupertino_radios::Adapter, BackendError> {
        let (connection, backend) = self.radios.adapter()?;
        let adapter = self
            .radios
            .answered(backend.adapter(&connection, &HashSet::new()))?;
        if !adapter.available {
            return Err(NO_ADAPTER.into());
        }
        Ok(adapter)
    }
}

impl Bluetooth for RadioBluetooth {
    fn read(&mut self) -> Result<bool, BackendError> {
        Ok(self.state()?.powered)
    }

    fn set_powered(&mut self, powered: bool) -> Result<(), BackendError> {
        let (connection, backend) = self.radios.adapter()?;
        self.radios
            .answered(backend.set_powered(&connection, powered))
    }

    /// The paired devices, by name.
    fn devices(&mut self) -> Result<Vec<Device>, BackendError> {
        let mut devices: Vec<Device> = self
            .state()?
            .devices
            .into_iter()
            .filter(|d| d.paired)
            .map(|d| Device {
                name: d.title(),
                connected: d.connected,
                id: d.id,
            })
            .collect();
        devices.sort_by_key(|d| d.name.to_lowercase());
        Ok(devices)
    }

    /// Connect or disconnect. BlueZ answers only once the link is up or has failed, which takes seconds, so its call
    /// runs on a thread of its own: the outcome shows when the device's change signal, or the call's end, has the
    /// panel read again, and a failure goes to the log.
    fn set_connected(&mut self, id: &str, connected: bool) -> Result<(), BackendError> {
        let (connection, backend) = self.radios.adapter()?;
        let call = move |connection: &Connection, id: &str| {
            if connected {
                backend.connect(connection, id)
            } else {
                backend.disconnect(connection, id)
            }
        };
        if backend == BluetoothBackend::Cupertino {
            return self.radios.answered(call(&connection, id));
        }
        let id = id.to_owned();
        let on_change = self.radios.shared().on_change.clone();
        std::thread::Builder::new()
            .name("bluetooth-link".into())
            .spawn(move || {
                if let Err(e) = call(&connection, &id) {
                    let action = if connected { "connect" } else { "disconnect" };
                    eprintln!(
                        "cupertino-controlcenter: bluetooth device: cannot {action} {id}: {}",
                        refusal(&e)
                    );
                }
                if let Some(on_change) = on_change {
                    on_change();
                }
            })
            .map_err(|e| format!("cannot start the device's call: {e}"))?;
        Ok(())
    }
}
