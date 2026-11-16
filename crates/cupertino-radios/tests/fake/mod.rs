//! Stand-ins for NetworkManager and BlueZ on a test bus (feature `fake`), for the tests of the crates that read the
//! radios: enough of each service's objects for the readings and the calls of this crate, with the signals the real
//! services send when their state changes. Each keeps what it was asked ([`FakeNetworkManager::calls`],
//! [`FakeBluez::calls`]) and can be changed from outside, as another program or the radio itself would. [`Silent`] owns
//! a service's name and answers nothing, as a service that hangs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::blocking::connection::Builder;
use zbus::blocking::Connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{fdo, interface};

use crate::{bluez, nm};

fn path(p: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(p).expect("a valid object path")
}

// ---- NetworkManager ------------------------------------------------------------------------------------------------

/// The fake's Wi-Fi device and its wired one.
pub const WLAN: &str = "/org/freedesktop/NetworkManager/Devices/1";
pub const ETH: &str = "/org/freedesktop/NetworkManager/Devices/2";

/// The networks in range: SSID, strength (0 to 100), whether it is secured (WPA2 personal) and whether a connection
/// is saved for it.
pub const NETWORKS: [(&str, u8, bool, bool); 3] = [
    ("Home", 80, true, true),
    ("Cafe", 60, false, false),
    ("Office", 40, true, false),
];

/// `NM_DEVICE_STATE_UNAVAILABLE`, `_DISCONNECTED`, `_ACTIVATED` and `_FAILED`.
const UNAVAILABLE: u32 = 20;
const DISCONNECTED: u32 = 30;
const ACTIVATED: u32 = 100;
const FAILED: u32 = 120;

struct Net {
    enabled: bool,
    wifi_device: bool,
    /// The index in [`NETWORKS`] of the network the Wi-Fi device is connected to.
    active: Option<usize>,
    /// The Wi-Fi device's state.
    state: u32,
    calls: Vec<String>,
    /// How many times the devices and the access points were asked for.
    readings: usize,
    listings: usize,
    /// The service answers no reading from now on.
    hung: bool,
}

type SharedNet = Arc<Mutex<Net>>;

fn net(shared: &SharedNet) -> std::sync::MutexGuard<'_, Net> {
    shared.lock().expect("no test panics holding the fake")
}

fn access_point(index: usize) -> OwnedObjectPath {
    path(&format!(
        "/org/freedesktop/NetworkManager/AccessPoint/{}",
        index + 1
    ))
}

fn access_point_index(ap: &ObjectPath<'_>) -> Option<usize> {
    let number: usize = ap.as_str().rsplit('/').next()?.parse().ok()?;
    number.checked_sub(1).filter(|i| *i < NETWORKS.len())
}

/// Tell the bus the manager's state changed, as NetworkManager does after every change of a link.
async fn announce(connection: &zbus::Connection) {
    connection
        .emit_signal(None::<&str>, nm::PATH, nm::NAME, "StateChanged", &(70u32,))
        .await
        .expect("the fake's bus takes its signal");
}

struct Manager(SharedNet);

impl Manager {
    /// Never ends once the service hangs.
    async fn answering(&self) {
        if net(&self.0).hung {
            std::future::pending::<()>().await;
        }
    }
}

#[interface(name = "org.freedesktop.NetworkManager")]
impl Manager {
    #[zbus(property)]
    async fn wireless_enabled(&self) -> bool {
        self.answering().await;
        net(&self.0).enabled
    }

    #[zbus(property)]
    fn set_wireless_enabled(&mut self, on: bool) {
        let mut net = net(&self.0);
        net.calls.push(format!("WirelessEnabled={on}"));
        net.enabled = on;
        net.active = None;
        net.state = if on { DISCONNECTED } else { UNAVAILABLE };
    }

    /// `NM_STATE_CONNECTED_GLOBAL` while a network is joined, else `NM_STATE_DISCONNECTED`.
    #[zbus(property)]
    fn state(&self) -> u32 {
        if net(&self.0).active.is_some() {
            70
        } else {
            20
        }
    }

    #[zbus(property)]
    fn active_connections(&self) -> Vec<OwnedObjectPath> {
        Vec::new()
    }

    async fn get_devices(&self) -> Vec<OwnedObjectPath> {
        self.answering().await;
        net(&self.0).readings += 1;
        if net(&self.0).wifi_device {
            vec![path(WLAN), path(ETH)]
        } else {
            vec![path(ETH)]
        }
    }

    async fn activate_connection(
        &self,
        #[zbus(connection)] bus: &zbus::Connection,
        connection: ObjectPath<'_>,
        device: ObjectPath<'_>,
        ap: ObjectPath<'_>,
    ) -> fdo::Result<OwnedObjectPath> {
        self.activate(
            format!("ActivateConnection({connection},{device},{ap})"),
            &ap,
        )?;
        announce(bus).await;
        Ok(path("/org/freedesktop/NetworkManager/ActiveConnection/1"))
    }

    async fn add_and_activate_connection(
        &self,
        #[zbus(connection)] bus: &zbus::Connection,
        settings: HashMap<String, HashMap<String, OwnedValue>>,
        device: ObjectPath<'_>,
        ap: ObjectPath<'_>,
    ) -> fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        let psk = settings
            .get("802-11-wireless-security")
            .and_then(|s| s.get("psk"))
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok());
        self.activate(
            format!("AddAndActivateConnection({psk:?},{device},{ap})"),
            &ap,
        )?;
        announce(bus).await;
        Ok((
            path("/org/freedesktop/NetworkManager/Settings/9"),
            path("/org/freedesktop/NetworkManager/ActiveConnection/1"),
        ))
    }
}

impl Manager {
    fn activate(&self, call: String, ap: &ObjectPath<'_>) -> fdo::Result<()> {
        let mut net = net(&self.0);
        net.calls.push(call);
        let index = access_point_index(ap)
            .ok_or_else(|| fdo::Error::Failed("No such access point.".into()))?;
        net.active = Some(index);
        net.state = ACTIVATED;
        Ok(())
    }
}

struct Device {
    net: SharedNet,
    wifi: bool,
}

#[interface(name = "org.freedesktop.NetworkManager.Device")]
impl Device {
    #[zbus(property)]
    fn interface(&self) -> String {
        if self.wifi { "wlan0" } else { "eth0" }.to_string()
    }

    /// `NM_DEVICE_TYPE_WIFI` or `NM_DEVICE_TYPE_ETHERNET`.
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        if self.wifi {
            2
        } else {
            1
        }
    }

    #[zbus(property)]
    fn state(&self) -> u32 {
        if self.wifi {
            net(&self.net).state
        } else {
            ACTIVATED
        }
    }

    #[zbus(property)]
    fn state_reason(&self) -> (u32, u32) {
        let state = self.state();
        (state, if state == FAILED { 7 } else { 0 })
    }

    #[zbus(property)]
    fn hw_address(&self) -> String {
        if self.wifi {
            "AA:BB:CC:DD:EE:01"
        } else {
            "AA:BB:CC:DD:EE:02"
        }
        .to_string()
    }

    async fn disconnect(&self, #[zbus(connection)] bus: &zbus::Connection) {
        {
            let mut net = net(&self.net);
            net.calls.push("Disconnect".to_string());
            if self.wifi {
                net.active = None;
                net.state = DISCONNECTED;
            }
        }
        announce(bus).await;
    }
}

struct Wireless(SharedNet);

#[interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl Wireless {
    #[zbus(property)]
    fn active_access_point(&self) -> OwnedObjectPath {
        net(&self.0).active.map_or_else(|| path("/"), access_point)
    }

    fn get_all_access_points(&self) -> Vec<OwnedObjectPath> {
        net(&self.0).listings += 1;
        if net(&self.0).enabled {
            (0..NETWORKS.len()).map(access_point).collect()
        } else {
            Vec::new()
        }
    }

    fn request_scan(&self, _options: HashMap<String, OwnedValue>) {
        net(&self.0).calls.push("RequestScan".to_string());
    }
}

struct AccessPoint(usize);

#[interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl AccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        NETWORKS[self.0].0.as_bytes().to_vec()
    }

    #[zbus(property)]
    fn strength(&self) -> u8 {
        NETWORKS[self.0].1
    }

    /// `NM_802_11_AP_FLAGS_PRIVACY` on a secured network.
    #[zbus(property)]
    fn flags(&self) -> u32 {
        u32::from(NETWORKS[self.0].2)
    }

    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        0
    }

    /// The pre-shared key bit of a WPA2 personal network.
    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        if NETWORKS[self.0].2 {
            0x100
        } else {
            0
        }
    }
}

struct Saved;

#[interface(name = "org.freedesktop.NetworkManager.Settings")]
impl Saved {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        NETWORKS
            .iter()
            .enumerate()
            .filter(|(_, n)| n.3)
            .map(|(i, _)| saved_connection(i))
            .collect()
    }
}

fn saved_connection(index: usize) -> OwnedObjectPath {
    path(&format!(
        "/org/freedesktop/NetworkManager/Settings/{}",
        index + 1
    ))
}

struct SavedConnection(usize);

#[interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl SavedConnection {
    fn get_settings(&self) -> HashMap<String, HashMap<String, OwnedValue>> {
        let value =
            |v: Value<'_>| OwnedValue::try_from(v).expect("a value without file descriptors");
        let (ssid, _, secured, _) = NETWORKS[self.0];
        let mut settings = HashMap::from([
            (
                "connection".to_string(),
                HashMap::from([("type".to_string(), value("802-11-wireless".into()))]),
            ),
            (
                "802-11-wireless".to_string(),
                HashMap::from([("ssid".to_string(), value(ssid.as_bytes().to_vec().into()))]),
            ),
        ]);
        if secured {
            settings.insert(
                "802-11-wireless-security".to_string(),
                HashMap::from([("key-mgmt".to_string(), value("wpa-psk".into()))]),
            );
        }
        settings
    }
}

/// A NetworkManager with one wired device and, unless made wired, one Wi-Fi device that sees [`NETWORKS`]; the radio
/// starts enabled and joined to nothing.
pub struct FakeNetworkManager {
    connection: Connection,
    net: SharedNet,
}

impl FakeNetworkManager {
    /// Serve on a connection built from `builder` (a test bus's), taking NetworkManager's name.
    pub fn serve(builder: Builder<'static>) -> zbus::Result<Self> {
        Self::serve_with(builder, true)
    }

    /// As [`FakeNetworkManager::serve`], on a machine without a Wi-Fi device.
    pub fn serve_wired(builder: Builder<'static>) -> zbus::Result<Self> {
        Self::serve_with(builder, false)
    }

    fn serve_with(builder: Builder<'static>, wifi_device: bool) -> zbus::Result<Self> {
        let net = Arc::new(Mutex::new(Net {
            enabled: true,
            wifi_device,
            active: None,
            state: DISCONNECTED,
            calls: Vec::new(),
            readings: 0,
            listings: 0,
            hung: false,
        }));
        let device = |wifi| Device {
            net: net.clone(),
            wifi,
        };
        let mut builder = builder
            .serve_at(nm::PATH, Manager(net.clone()))?
            .serve_at(ETH, device(false))?
            .serve_at(nm::SETTINGS_PATH, Saved)?;
        if wifi_device {
            builder = builder
                .serve_at(WLAN, device(true))?
                .serve_at(WLAN, Wireless(net.clone()))?;
        }
        for (index, network) in NETWORKS.iter().enumerate() {
            builder = builder.serve_at(access_point(index), AccessPoint(index))?;
            if network.3 {
                builder = builder.serve_at(saved_connection(index), SavedConnection(index))?;
            }
        }
        Ok(FakeNetworkManager {
            connection: builder.name(nm::NAME)?.build()?,
            net,
        })
    }

    /// What the service was asked, oldest first.
    pub fn calls(&self) -> Vec<String> {
        net(&self.net).calls.clone()
    }

    /// How many times the radio and its devices were read from the service.
    pub fn readings(&self) -> usize {
        net(&self.net).readings
    }

    /// How many times the access points in range were listed.
    pub fn listings(&self) -> usize {
        net(&self.net).listings
    }

    /// The service hangs: it keeps its name and answers no reading from now on.
    pub fn hang(&self) {
        net(&self.net).hung = true;
    }

    /// The service sends its signal though nothing a reading shows has changed.
    pub fn announce(&self) {
        self.changed(|_| {});
    }

    /// Whether the radio is enabled.
    pub fn enabled(&self) -> bool {
        net(&self.net).enabled
    }

    /// The network the Wi-Fi device is joined to.
    pub fn joined(&self) -> Option<&'static str> {
        net(&self.net).active.map(|i| NETWORKS[i].0)
    }

    fn changed(&self, change: impl FnOnce(&mut Net)) {
        change(&mut net(&self.net));
        zbus::block_on(announce(self.connection.inner()));
    }

    /// The radio is switched from elsewhere.
    pub fn set_enabled(&self, on: bool) {
        self.changed(|net| {
            net.enabled = on;
            net.active = None;
            net.state = if on { DISCONNECTED } else { UNAVAILABLE };
        });
    }

    /// The device joins the network `ssid` of [`NETWORKS`] by itself, or leaves its network.
    pub fn set_joined(&self, ssid: Option<&str>) {
        let active = ssid.map(|ssid| {
            NETWORKS
                .iter()
                .position(|n| n.0 == ssid)
                .expect("a network of NETWORKS")
        });
        self.changed(|net| {
            net.active = active;
            net.state = if active.is_some() {
                ACTIVATED
            } else {
                DISCONNECTED
            };
        });
    }

    /// The device's attempt to join fails.
    pub fn fail(&self) {
        self.changed(|net| {
            net.active = None;
            net.state = FAILED;
        });
    }
}

// ---- BlueZ ---------------------------------------------------------------------------------------------------------

pub const ADAPTER: &str = "/org/bluez/hci0";
/// A paired device with a name, and one never paired.
pub const HEADPHONES: &str = "/org/bluez/hci0/dev_AA_00_00_00_00_01";
pub const KEYBOARD: &str = "/org/bluez/hci0/dev_AA_00_00_00_00_02";

struct Bt {
    powered: bool,
    /// The devices that are connected, by path.
    connected: Vec<&'static str>,
    /// What a `Connect` is refused with.
    refusal: Option<String>,
    calls: Vec<String>,
    /// How many times the adapter's state was read.
    readings: usize,
}

type SharedBt = Arc<Mutex<Bt>>;

fn bt(shared: &SharedBt) -> std::sync::MutexGuard<'_, Bt> {
    shared.lock().expect("no test panics holding the fake")
}

struct BtAdapter(SharedBt);

#[interface(name = "org.bluez.Adapter1")]
impl BtAdapter {
    #[zbus(property)]
    fn powered(&self) -> bool {
        let mut bt = bt(&self.0);
        bt.readings += 1;
        bt.powered
    }

    #[zbus(property)]
    fn set_powered(&mut self, on: bool) {
        let mut bt = bt(&self.0);
        bt.calls.push(format!("Powered={on}"));
        bt.powered = on;
        if !on {
            bt.connected.clear();
        }
    }

    #[zbus(property)]
    fn discovering(&self) -> bool {
        false
    }
}

struct BtDevice {
    bt: SharedBt,
    path: &'static str,
    name: &'static str,
    address: &'static str,
    icon: &'static str,
    paired: bool,
}

#[interface(name = "org.bluez.Device1")]
impl BtDevice {
    #[zbus(property)]
    fn address(&self) -> String {
        self.address.to_string()
    }

    #[zbus(property)]
    fn name(&self) -> String {
        self.name.to_string()
    }

    #[zbus(property)]
    fn icon(&self) -> String {
        self.icon.to_string()
    }

    #[zbus(property)]
    fn paired(&self) -> bool {
        self.paired
    }

    #[zbus(property)]
    fn connected(&self) -> bool {
        bt(&self.bt).connected.contains(&self.path)
    }

    #[zbus(property)]
    fn adapter(&self) -> OwnedObjectPath {
        path(ADAPTER)
    }

    async fn connect(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> fdo::Result<()> {
        {
            let mut bt = bt(&self.bt);
            bt.calls.push(format!("Connect({})", self.path));
            if let Some(refusal) = &bt.refusal {
                return Err(fdo::Error::Failed(refusal.clone()));
            }
            if !bt.connected.contains(&self.path) {
                bt.connected.push(self.path);
            }
        }
        self.connected_changed(&emitter).await?;
        Ok(())
    }

    async fn disconnect(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        {
            let mut bt = bt(&self.bt);
            bt.calls.push(format!("Disconnect({})", self.path));
            bt.connected.retain(|p| *p != self.path);
        }
        self.connected_changed(&emitter).await?;
        Ok(())
    }
}

/// A BlueZ with, unless made without one, a powered adapter, the paired [`HEADPHONES`] and the unpaired [`KEYBOARD`],
/// neither connected.
pub struct FakeBluez {
    connection: Connection,
    bt: SharedBt,
}

impl FakeBluez {
    /// Serve on a connection built from `builder` (a test bus's), taking BlueZ's name.
    pub fn serve(builder: Builder<'static>) -> zbus::Result<Self> {
        Self::serve_with(builder, true)
    }

    /// As [`FakeBluez::serve`], on a machine without an adapter.
    pub fn serve_without_adapter(builder: Builder<'static>) -> zbus::Result<Self> {
        Self::serve_with(builder, false)
    }

    fn serve_with(builder: Builder<'static>, adapter: bool) -> zbus::Result<Self> {
        let bt = Arc::new(Mutex::new(Bt {
            powered: true,
            connected: Vec::new(),
            refusal: None,
            calls: Vec::new(),
            readings: 0,
        }));
        let mut builder = builder.serve_at("/", fdo::ObjectManager)?;
        if adapter {
            let device = |path, name, address, icon, paired| BtDevice {
                bt: bt.clone(),
                path,
                name,
                address,
                icon,
                paired,
            };
            builder = builder
                .serve_at(ADAPTER, BtAdapter(bt.clone()))?
                .serve_at(
                    HEADPHONES,
                    device(
                        HEADPHONES,
                        "Headphones",
                        "AA:00:00:00:00:01",
                        "audio-headphones",
                        true,
                    ),
                )?
                .serve_at(
                    KEYBOARD,
                    device(
                        KEYBOARD,
                        "Keyboard",
                        "AA:00:00:00:00:02",
                        "input-keyboard",
                        false,
                    ),
                )?;
        }
        Ok(FakeBluez {
            connection: builder.name(bluez::NAME)?.build()?,
            bt,
        })
    }

    /// What the service was asked, oldest first.
    pub fn calls(&self) -> Vec<String> {
        bt(&self.bt).calls.clone()
    }

    pub fn powered(&self) -> bool {
        bt(&self.bt).powered
    }

    /// How many times the adapter's state was read from the service.
    pub fn readings(&self) -> usize {
        bt(&self.bt).readings
    }

    /// The device at `path` is heard again while the adapter discovers: its signal strength changes, as it does
    /// several times a second, and nothing else.
    pub fn advertise(&self, path: &str, rssi: i16) {
        let changed: HashMap<&str, Value<'_>> = HashMap::from([("RSSI", Value::from(rssi))]);
        self.connection
            .emit_signal(
                None::<&str>,
                path,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(bluez::DEVICE, changed, Vec::<&str>::new()),
            )
            .expect("the fake's bus takes its signal");
    }

    /// Whether the device at `path` is connected.
    pub fn connected(&self, path: &str) -> bool {
        bt(&self.bt).connected.contains(&path)
    }

    /// Every later `Connect` is refused with `reason`.
    pub fn refuse_connect(&self, reason: &str) {
        bt(&self.bt).refusal = Some(reason.to_string());
    }

    fn property_changed(&self, path: &str, interface: &str, name: &str, value: bool) {
        let changed: HashMap<&str, Value<'_>> = HashMap::from([(name, Value::from(value))]);
        self.connection
            .emit_signal(
                None::<&str>,
                path,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(interface, changed, Vec::<&str>::new()),
            )
            .expect("the fake's bus takes its signal");
    }

    /// The adapter is switched from elsewhere.
    pub fn set_powered(&self, on: bool) {
        {
            let mut bt = bt(&self.bt);
            bt.powered = on;
            if !on {
                bt.connected.clear();
            }
        }
        self.property_changed(ADAPTER, bluez::ADAPTER, "Powered", on);
    }

    /// The device at `path` ([`HEADPHONES`] or [`KEYBOARD`]) connects or goes by itself.
    pub fn set_connected(&self, path: &str, on: bool) {
        let path = [HEADPHONES, KEYBOARD]
            .into_iter()
            .find(|p| *p == path)
            .expect("a device of the fake");
        {
            let mut bt = bt(&self.bt);
            bt.connected.retain(|p| *p != path);
            if on {
                bt.connected.push(path);
            }
        }
        self.property_changed(path, bluez::DEVICE, "Connected", on);
    }
}

// ---- A service that hangs ------------------------------------------------------------------------------------------

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

struct SilentManager;

#[interface(name = "org.freedesktop.NetworkManager")]
impl SilentManager {
    #[zbus(property)]
    async fn wireless_enabled(&self) -> bool {
        std::future::pending().await
    }

    #[zbus(property)]
    async fn set_wireless_enabled(&mut self, _on: bool) {
        std::future::pending().await
    }

    #[zbus(property)]
    async fn state(&self) -> u32 {
        std::future::pending().await
    }

    async fn get_devices(&self) -> Vec<OwnedObjectPath> {
        std::future::pending().await
    }
}

struct SilentObjects;

#[interface(name = "org.freedesktop.DBus.ObjectManager")]
impl SilentObjects {
    async fn get_managed_objects(&self) -> ManagedObjects {
        std::future::pending().await
    }
}

/// A service that owns its name and never answers a reading, as one that hangs.
pub struct Silent {
    _connection: Connection,
}

impl Silent {
    /// A NetworkManager that answers nothing, on a connection built from `builder`.
    pub fn network_manager(builder: Builder<'static>) -> zbus::Result<Self> {
        Ok(Silent {
            _connection: builder
                .serve_at(nm::PATH, SilentManager)?
                .name(nm::NAME)?
                .build()?,
        })
    }

    /// A BlueZ that answers nothing, on a connection built from `builder`.
    pub fn bluez(builder: Builder<'static>) -> zbus::Result<Self> {
        Ok(Silent {
            _connection: builder
                .serve_at("/", SilentObjects)?
                .name(bluez::NAME)?
                .build()?,
        })
    }
}
