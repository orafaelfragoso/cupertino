//! The Wi-Fi radio and the Bluetooth adapter through whichever service owns them (`cupertino-radios`): the Cupertino
//! network and Bluetooth daemons, else NetworkManager and BlueZ, the same discovery and readings as Settings and the
//! menu bar.
//!
//! The panel's thread never waits for a service. Every reading and switch is carried out on a thread of its own, and
//! so is the connection to the system bus, made on the first use and again after a failure, so a bus or a service
//! that starts later is picked up. A reading the panel asks for is waited for a moment ([`FRESH_WAIT`]), which is
//! longer than services take that answer as they should; one that takes longer leaves the panel with the last
//! reading, or unavailable with the reason while there is none, and [`Radios::on_change`] hears when it arrives. One
//! reading of a kind is under way at a time: a request that comes meanwhile has it taken once more when it ends. A
//! service that does not answer fails its reading after the bounded wait of `cupertino-radios`, and its tile says so.
//!
//! The services are looked up at every reading. Their change signals are followed on the connection:
//! [`Radios::on_change`] hears which radio's reading is out of date, so the panel shows a radio switched elsewhere
//! without waiting for its next periodic reading.

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use cupertino_radios::{refusal, unanswered, BluetoothBackend, NetworkBackend};
use zbus::blocking::{connection, Connection};

pub use cupertino_radios::watch::Radio;

use crate::backend::{BackendError, Bluetooth, Wifi};
use crate::model::{Device, Network, WifiStatus};

/// What a tile says when no service serves its radio.
const NO_NETWORK_SERVICE: &str = "no network service is running";
const NO_BLUETOOTH_SERVICE: &str = "no Bluetooth service is running";
/// What a tile says on a machine without the radio.
const NO_WIFI_DEVICE: &str = "there is no Wi-Fi device";
const NO_ADAPTER: &str = "there is no Bluetooth adapter";
/// What a tile says while its first reading is under way, and once its service did not answer.
const WAITING_FOR_NETWORK: &str = "waiting for the network service";
const WAITING_FOR_BLUETOOTH: &str = "waiting for the Bluetooth service";
const NETWORK_SILENT: &str = "the network service does not answer";
const BLUETOOTH_SILENT: &str = "the Bluetooth service does not answer";

/// How long the panel's thread waits for a reading it asked for, so an open panel shows the radios as they are now
/// when their services answer as they should. A reading that takes longer arrives later.
pub const FRESH_WAIT: Duration = Duration::from_millis(100);
/// How long the panel's thread waits for a service's answer to a switch, so a refusal shows with its reason.
pub const SWITCH_WAIT: Duration = Duration::from_millis(250);

type OnChange = Arc<dyn Fn(Radio) + Send + Sync>;

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
    /// How long the panel's thread waits for a reading and for a switch.
    fresh_wait: Duration,
    switch_wait: Duration,
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
            fresh_wait: FRESH_WAIT,
            switch_wait: SWITCH_WAIT,
        }
    }

    /// These radios with other waits than [`FRESH_WAIT`] and [`SWITCH_WAIT`] for the backends made from now on.
    pub fn waiting(mut self, fresh: Duration, switch: Duration) -> Self {
        self.fresh_wait = fresh;
        self.switch_wait = switch;
        self
    }

    /// The shared state, held only to copy out of it or to store into it: never across a call on the bus.
    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .expect("no thread panics holding the radios' bus")
    }

    /// Call `on_change` with the radio whose reading is out of date: a signal of its service said it changed, the
    /// service started or stopped, or a reading or a switch that the panel did not wait for has ended. It is called
    /// from other threads and must not wait.
    pub fn on_change(&self, on_change: impl Fn(Radio) + Send + Sync + 'static) {
        let on_change: OnChange = Arc::new(on_change);
        let connection = {
            let mut shared = self.shared();
            shared.on_change = Some(on_change.clone());
            shared.connection.clone()
        };
        if let Some(connection) = connection {
            // the bus daemon is asked for the signals, which waits for its answers
            let _ = std::thread::Builder::new()
                .name("radios-follow".into())
                .spawn(move || follow(&connection, &on_change));
        }
    }

    /// Tell the panel that the reading of `radio` is out of date.
    fn changed(&self, radio: Radio) {
        let on_change = self.shared().on_change.clone();
        if let Some(on_change) = on_change {
            on_change(radio);
        }
    }

    /// The Wi-Fi radio.
    pub fn wifi(&self) -> RadioWifi {
        RadioWifi {
            radios: self.clone(),
            status: Latest::new(),
            networks: Latest::new(),
            keys: HashMap::new(),
        }
    }

    /// The Bluetooth adapter.
    pub fn bluetooth(&self) -> RadioBluetooth {
        RadioBluetooth {
            radios: self.clone(),
            adapter: Latest::new(),
        }
    }

    /// The connection, made when there is none. It waits for the bus, so it is called from the readings' and the
    /// switches' threads alone.
    fn connection(&self) -> zbus::Result<Connection> {
        let (existing, address) = {
            let shared = self.shared();
            (shared.connection.clone(), shared.address.clone())
        };
        if let Some(connection) = existing {
            return Ok(connection);
        }
        let connection = match &address {
            Some(address) => connection::Builder::address(address.as_str())?,
            None => connection::Builder::system()?,
        }
        .build()?;
        let on_change = {
            let mut shared = self.shared();
            if let Some(first) = &shared.connection {
                // another thread connected meanwhile: its connection is the one followed
                return Ok(first.clone());
            }
            shared.connection = Some(connection.clone());
            shared.on_change.clone()
        };
        if let Some(on_change) = on_change {
            follow(&connection, &on_change);
        }
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

    fn network(&self) -> Result<(Connection, NetworkBackend), String> {
        let (connection, found) = self
            .with(|c| Ok((c.clone(), cupertino_radios::detect(c)?)))
            .map_err(|e| reason(&e, NETWORK_SILENT))?;
        Ok((connection, found.network.ok_or(NO_NETWORK_SERVICE)?))
    }

    fn adapter(&self) -> Result<(Connection, BluetoothBackend), String> {
        let (connection, found) = self
            .with(|c| Ok((c.clone(), cupertino_radios::detect(c)?)))
            .map_err(|e| reason(&e, BLUETOOTH_SILENT))?;
        Ok((connection, found.bluetooth.ok_or(NO_BLUETOOTH_SERVICE)?))
    }

    /// The service's answer to a call: its own text when it refused, `silent` when it did not answer, and a lost bus
    /// is connected again.
    fn answered<T>(&self, result: zbus::Result<T>, silent: &str) -> Result<T, String> {
        result.map_err(|e| {
            if let zbus::Error::InputOutput(_) = &e {
                self.shared().connection = None;
            }
            reason(&e, silent)
        })
    }

    /// Carry out `switch` on a thread of its own and wait for its outcome ([`SWITCH_WAIT`]). One that takes longer is
    /// reported as `silent`; when it ends after all, a failure goes to the log and the panel reads `radio` again.
    fn switch(
        &self,
        radio: Radio,
        silent: &str,
        switch: impl FnOnce(&Radios) -> Result<(), String> + Send + 'static,
    ) -> Result<(), BackendError> {
        let (done, outcome) = mpsc::channel();
        let radios = self.clone();
        std::thread::Builder::new()
            .name("radios-switch".into())
            .spawn(move || {
                let result = switch(&radios);
                if let Err(mpsc::SendError(late)) = done.send(result) {
                    if let Err(e) = late {
                        eprintln!("cupertino-controlcenter: a radio's switch failed late: {e}");
                    }
                    radios.changed(radio);
                }
            })
            .map_err(|e| format!("cannot start the switch's call: {e}"))?;
        match outcome.recv_timeout(self.switch_wait) {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(silent.into()),
        }
    }
}

impl Default for Radios {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a call failed, for a tile: `silent` when the service did not answer, else the service's or the bus's words.
fn reason(error: &zbus::Error, silent: &str) -> String {
    if unanswered(error) {
        silent.to_owned()
    } else {
        refusal(error)
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

/// What a reading gave: the value, or why there is none.
type Outcome<T> = Result<T, String>;

struct LatestState<T> {
    /// The last reading that ended.
    last: Option<Outcome<T>>,
    /// The reading the panel was last given.
    given: Option<Outcome<T>>,
    /// A reading is under way.
    reading: bool,
    /// A reading was asked for while one was under way: it is taken once more when that one ends.
    again: bool,
    /// How many readings ended.
    ended: u64,
    /// How many callers wait for a reading to end.
    waiting: u32,
}

/// The last reading of one kind, taken on threads of their own, one at a time.
struct Latest<T> {
    state: Mutex<LatestState<T>>,
    arrived: Condvar,
}

impl<T: Clone + PartialEq + Send + 'static> Latest<T> {
    fn new() -> Arc<Self> {
        Arc::new(Latest {
            state: Mutex::new(LatestState {
                last: None,
                given: None,
                reading: false,
                again: false,
                ended: 0,
                waiting: 0,
            }),
            arrived: Condvar::new(),
        })
    }

    fn state(&self) -> MutexGuard<'_, LatestState<T>> {
        self.state
            .lock()
            .expect("no thread panics holding a reading")
    }

    /// The last reading, and note that the panel has it; `pending` while there is none.
    fn give(state: &mut LatestState<T>, pending: &str) -> Outcome<T> {
        state.given = state.last.clone();
        state
            .last
            .clone()
            .unwrap_or_else(|| Err(pending.to_owned()))
    }

    /// Ask for a reading by `read` and wait `wait` for it. When one is under way already the last reading is given
    /// at once. `late` is called when a reading the caller did not wait for ends with something else than the caller
    /// was given.
    fn fresh(
        self: &Arc<Self>,
        wait: Duration,
        pending: &str,
        read: impl Fn() -> Outcome<T> + Send + 'static,
        late: impl Fn() + Send + 'static,
    ) -> Outcome<T> {
        let wanted = {
            let mut state = self.state();
            if state.reading {
                state.again = true;
                return Self::give(&mut state, pending);
            }
            state.reading = true;
            // counted before the reading starts, so one that ends at once is not told as late
            state.waiting += 1;
            state.ended + 1
        };
        let latest = self.clone();
        let started = std::thread::Builder::new()
            .name("radios-reading".into())
            .spawn(move || loop {
                let outcome = read();
                let tell = {
                    let mut state = latest.state();
                    state.last = Some(outcome);
                    state.ended += 1;
                    if state.again {
                        state.again = false;
                        None
                    } else {
                        state.reading = false;
                        Some(state.waiting == 0 && state.last != state.given)
                    }
                };
                latest.arrived.notify_all();
                match tell {
                    None => continue,
                    Some(true) => late(),
                    Some(false) => {}
                }
                break;
            });
        let state = self.state();
        if let Err(e) = started {
            let mut state = state;
            state.reading = false;
            state.waiting -= 1;
            return Err(format!("cannot start a reading: {e}"));
        }
        let (mut state, _) = self
            .arrived
            .wait_timeout_while(state, wait, |state| state.ended < wanted)
            .expect("no thread panics holding a reading");
        state.waiting -= 1;
        Self::give(&mut state, pending)
    }
}

/// A network of the list with the service's key for it.
type Listed = (Network, String);

/// The Wi-Fi radio: the switch, the network the first Wi-Fi device is connected to, and the networks in range.
pub struct RadioWifi {
    radios: Radios,
    status: Arc<Latest<WifiStatus>>,
    networks: Arc<Latest<Vec<Listed>>>,
    /// The service's key of each network of the last list, by the name the list shows.
    keys: HashMap<String, String>,
}

/// Read the radio and the network of its first Wi-Fi device.
fn read_status(radios: &Radios) -> Outcome<WifiStatus> {
    let (connection, backend) = radios.network()?;
    let link = radios.answered(backend.link(&connection), NETWORK_SILENT)?;
    if link.device.is_none() {
        return Err(NO_WIFI_DEVICE.to_owned());
    }
    Ok(WifiStatus {
        enabled: link.enabled,
        network: link.network().map(str::to_owned),
    })
}

/// Read the networks in range, after asking for a scan, whose list arrives with a later reading.
fn read_networks(radios: &Radios) -> Outcome<Vec<Listed>> {
    let (connection, backend) = radios.network()?;
    // a refused or busy scan still leaves the last list to show
    let _ = backend.scan(&connection);
    let wifi = radios.answered(backend.wifi(&connection), NETWORK_SILENT)?;
    let mut listed: Vec<Listed> = Vec::new();
    for network in wifi.networks {
        // two networks that read the same could not be told apart in the list: the stronger one is shown
        if network.name.is_empty() || listed.iter().any(|(shown, _)| shown.ssid == network.name) {
            continue;
        }
        listed.push((
            Network {
                ssid: network.name,
                bars: (network.signal != i32::MIN).then(|| Network::bars_of(network.signal)),
                secured: network.security != "open",
                known: network.saved,
                connected: network.connected,
            },
            network.ssid,
        ));
    }
    Ok(listed)
}

impl Wifi for RadioWifi {
    fn read(&mut self) -> Result<WifiStatus, BackendError> {
        let (radios, late) = (self.radios.clone(), self.radios.clone());
        self.status
            .fresh(
                self.radios.fresh_wait,
                WAITING_FOR_NETWORK,
                move || read_status(&radios),
                move || late.changed(Radio::Network),
            )
            .map_err(Into::into)
    }

    fn set_enabled(&mut self, enabled: bool) -> Result<(), BackendError> {
        self.radios
            .switch(Radio::Network, NETWORK_SILENT, move |radios| {
                let (connection, backend) = radios.network()?;
                radios.answered(
                    backend.set_wireless_enabled(&connection, enabled),
                    NETWORK_SILENT,
                )
            })
    }

    /// The networks in range. Each reading asks for a scan, whose list arrives with a later one.
    fn networks(&mut self) -> Result<Vec<Network>, BackendError> {
        let (radios, late) = (self.radios.clone(), self.radios.clone());
        let listed = self.networks.fresh(
            self.radios.fresh_wait,
            WAITING_FOR_NETWORK,
            move || read_networks(&radios),
            move || late.changed(Radio::Network),
        )?;
        self.keys = listed
            .iter()
            .map(|(network, key)| (network.ssid.clone(), key.clone()))
            .collect();
        Ok(listed.into_iter().map(|(network, _)| network).collect())
    }

    /// Join with no passphrase: an open network, or one whose credentials the service holds.
    fn join(&mut self, ssid: &str) -> Result<(), BackendError> {
        let key = self.keys.get(ssid).map_or(ssid, String::as_str).to_owned();
        self.radios
            .switch(Radio::Network, NETWORK_SILENT, move |radios| {
                let (connection, backend) = radios.network()?;
                radios.answered(backend.join(&connection, &key, ""), NETWORK_SILENT)
            })
    }
}

/// The Bluetooth adapter: the switch and the paired devices.
pub struct RadioBluetooth {
    radios: Radios,
    adapter: Arc<Latest<cupertino_radios::Adapter>>,
}

/// Read the adapter and its devices.
fn read_adapter(radios: &Radios) -> Outcome<cupertino_radios::Adapter> {
    let (connection, backend) = radios.adapter()?;
    let adapter = radios.answered(
        backend.adapter(&connection, &HashSet::new()),
        BLUETOOTH_SILENT,
    )?;
    if !adapter.available {
        return Err(NO_ADAPTER.to_owned());
    }
    Ok(adapter)
}

impl RadioBluetooth {
    fn state(&self) -> Result<cupertino_radios::Adapter, BackendError> {
        let (radios, late) = (self.radios.clone(), self.radios.clone());
        self.adapter
            .fresh(
                self.radios.fresh_wait,
                WAITING_FOR_BLUETOOTH,
                move || read_adapter(&radios),
                move || late.changed(Radio::Bluetooth),
            )
            .map_err(Into::into)
    }
}

impl Bluetooth for RadioBluetooth {
    fn read(&mut self) -> Result<bool, BackendError> {
        Ok(self.state()?.powered)
    }

    fn set_powered(&mut self, powered: bool) -> Result<(), BackendError> {
        self.radios
            .switch(Radio::Bluetooth, BLUETOOTH_SILENT, move |radios| {
                let (connection, backend) = radios.adapter()?;
                radios.answered(backend.set_powered(&connection, powered), BLUETOOTH_SILENT)
            })
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

    /// Connect or disconnect, without waiting: a service may answer only once the link is up or has failed, which
    /// takes seconds. The outcome shows when the device's change signal, or the call's end, has the panel read
    /// again, and a failure goes to the log.
    fn set_connected(&mut self, id: &str, connected: bool) -> Result<(), BackendError> {
        let (radios, id) = (self.radios.clone(), id.to_owned());
        std::thread::Builder::new()
            .name("bluetooth-link".into())
            .spawn(move || {
                let done = radios.adapter().and_then(|(connection, backend)| {
                    let call = if connected {
                        backend.connect(&connection, &id)
                    } else {
                        backend.disconnect(&connection, &id)
                    };
                    radios.answered(call, BLUETOOTH_SILENT)
                });
                if let Err(e) = done {
                    let action = if connected { "connect" } else { "disconnect" };
                    eprintln!(
                        "cupertino-controlcenter: bluetooth device: cannot {action} {id}: {e}"
                    );
                }
                radios.changed(Radio::Bluetooth);
            })
            .map_err(|e| format!("cannot start the device's call: {e}"))?;
        Ok(())
    }
}
