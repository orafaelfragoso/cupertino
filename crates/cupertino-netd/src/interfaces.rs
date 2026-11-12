//! The D-Bus objects: `org.cupertino.Network1` and the subset of
//! `org.freedesktop.NetworkManager` that apps poll for the connection state.
//!
//! The NetworkManager names, signatures and enum numbering follow its D-Bus API documentation
//! (<https://networkmanager.dev/docs/api/latest/>). Only `State`, `Connectivity`,
//! `NetworkingEnabled`, `WirelessEnabled`, `PrimaryConnectionType`, `CheckConnectivity` and the
//! `StateChanged` signal exist; everything else of NetworkManager is absent.
//!
//! Writes (the radio switch, scan, connect, disconnect) need a caller in the active seat session;
//! reads are open to every caller.

use std::sync::Arc;

use cupertino_diagnostic_events::{self as diag, AddressKind, Severity, Value};
use cupertino_netlink::Security;
use zbus::fdo;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;

use crate::auth::Decision;
use crate::daemon::Shared;
use crate::engine::{Command, CommandError};
use crate::nm;

pub const NETWORK_NAME: &str = "org.cupertino.Network1";
pub const NETWORK_PATH: &str = "/org/cupertino/Network1";
pub const MANAGER_NAME: &str = "org.freedesktop.NetworkManager";
pub const MANAGER_PATH: &str = "/org/freedesktop/NetworkManager";

/// Why a caller may not write.
enum Refusal {
    /// The caller is not in the active seat session.
    Denied(String),
    /// The caller could not be checked.
    Failed(String),
}

/// Maps the caller's bus name to its process through `GetConnectionCredentials`, asked on the
/// daemon's second connection ([`Shared::lookup`]), and asks the authorizer. The pid comes from the
/// bus daemon, not from the message, so the caller cannot choose it.
async fn authorize(shared: &Shared, header: Option<&Header<'_>>) -> Result<(), Refusal> {
    let failed =
        |what: &str, error: &dyn std::fmt::Display| Refusal::Failed(format!("{what}: {error}"));
    let sender = header
        .and_then(Header::sender)
        .ok_or_else(|| Refusal::Failed("the call has no sender".to_owned()))?;
    let credentials = shared
        .lookup()
        .credentials(sender)
        .await
        .map_err(|e| failed("reading the caller's credentials", &e))?;
    let pid = credentials
        .process_id()
        .ok_or_else(|| Refusal::Failed("the bus did not report the caller's process".to_owned()))?;
    match shared.authorizer().authorize(pid) {
        Ok(Decision::Allowed) => Ok(()),
        Ok(Decision::Denied(reason)) => Err(Refusal::Denied(reason)),
        Err(error) => Err(failed(
            "asking whether the caller may change the network settings",
            &error,
        )),
    }
}

/// Records `network.request.failed`: `method` of `interface` was answered with the D-Bus error `error`. An SSID the
/// call named is kept as its salted hash.
fn request_failed(interface: &'static str, method: &'static str, error: &str, ssid: Option<&str>) {
    let mut event = diag::event(
        Severity::Warn,
        "network.request.failed",
        "a request to the network service failed",
    )
    .field("interface", Value::Public(interface))
    .field("method", Value::Public(method))
    .field("error", Value::Public(error));
    if let Some(ssid) = ssid {
        event = event.field("ssid", Value::Address(AddressKind::Ssid, ssid));
    }
    event.emit();
}

/// `result` of `method` of `org.cupertino.Network1`, its error recorded.
fn answered<T>(
    method: &'static str,
    ssid: Option<&str>,
    result: Result<T, NetworkError>,
) -> Result<T, NetworkError> {
    if let Err(error) = &result {
        request_failed(
            NETWORK_NAME,
            method,
            zbus::DBusError::name(error).as_str(),
            ssid,
        );
    }
    result
}

/// The errors of `org.cupertino.Network1`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Network1")]
pub enum NetworkError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The caller is not in a session that is active on a seat.
    NotAuthorized(String),
    /// The machine has no Wi-Fi device.
    NoWifiDevice(String),
    /// Wi-Fi is switched off or blocked.
    WifiUnavailable(String),
    /// The last scan did not list the network.
    UnknownNetwork(String),
    /// The network needs a passphrase and none is saved or given.
    PassphraseRequired(String),
    /// The stack cannot connect to a network with this security.
    UnsupportedSecurity(String),
    /// Carrying the request out failed, or the caller could not be checked.
    Failed(String),
}

impl From<Refusal> for NetworkError {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Denied(reason) => Self::NotAuthorized(reason),
            Refusal::Failed(reason) => Self::Failed(reason),
        }
    }
}

impl From<CommandError> for NetworkError {
    fn from(error: CommandError) -> Self {
        let message = error.to_string();
        match error {
            CommandError::NoWifiDevice => Self::NoWifiDevice(message),
            CommandError::WifiUnavailable => Self::WifiUnavailable(message),
            CommandError::UnknownNetwork(_) => Self::UnknownNetwork(message),
            CommandError::PassphraseRequired(_) => Self::PassphraseRequired(message),
            CommandError::UnsupportedSecurity(_) => Self::UnsupportedSecurity(message),
            CommandError::Failed(_) => Self::Failed(message),
        }
    }
}

/// A device on the bus: index, name, kind, state, SSID, failure and address (`10.0.0.9/24`). The
/// strings are empty when the device has none.
pub type DeviceRecord = (u32, String, String, String, String, String, String);

/// A network on the bus: SSID, signal in dBm (`i32::MIN` when the driver gave none), security,
/// whether credentials are saved, whether the device is connected to it.
pub type NetworkRecord = (String, i32, String, bool, bool);

/// `org.cupertino.Network1`.
///
/// The service controls one Wi-Fi device, the first by index; other devices are listed in
/// `Devices`.
pub struct NetworkObject(pub Arc<Shared>);

#[zbus::interface(name = "org.cupertino.Network1")]
impl NetworkObject {
    /// Asks for a scan. The list changes (`NetworksChanged`) when it ends.
    async fn scan(&self, #[zbus(header)] header: Header<'_>) -> Result<(), NetworkError> {
        let result = async {
            authorize(&self.0, Some(&header)).await?;
            Ok(self.0.submit(Command::Scan).await?)
        }
        .await;
        answered("Scan", None, result)
    }

    /// The networks of the last scan, strongest first.
    fn get_networks(&self) -> Vec<NetworkRecord> {
        self.0
            .snapshot()
            .networks
            .iter()
            .map(|network| {
                (
                    network.ssid.to_text(),
                    network.signal_dbm.unwrap_or(i32::MIN),
                    security_name(network.security).to_owned(),
                    network.known,
                    network.connected,
                )
            })
            .collect()
    }

    /// Connects to a network of the last scan. `passphrase` may be empty for an open network or
    /// one with saved credentials. Returns when the attempt has begun; the outcome is the device's
    /// state, and `ConnectionFailed` when it fails.
    async fn connect(
        &self,
        #[zbus(header)] header: Header<'_>,
        ssid: String,
        passphrase: String,
    ) -> Result<(), NetworkError> {
        let named = ssid.clone();
        let result = async {
            authorize(&self.0, Some(&header)).await?;
            let passphrase = (!passphrase.is_empty()).then_some(passphrase);
            Ok(self.0.submit(Command::Connect { ssid, passphrase }).await?)
        }
        .await;
        answered("Connect", Some(&named), result)
    }

    /// The saved networks, in range or not: the name and whether the device joins it by itself.
    fn get_known_networks(&self) -> Vec<(String, bool)> {
        self.0.snapshot().known.clone()
    }

    /// Takes the saved credentials of a network away, so the device no longer joins it.
    async fn forget(
        &self,
        #[zbus(header)] header: Header<'_>,
        ssid: String,
    ) -> Result<(), NetworkError> {
        let named = ssid.clone();
        let result = async {
            authorize(&self.0, Some(&header)).await?;
            Ok(self.0.submit(Command::Forget(ssid)).await?)
        }
        .await;
        answered("Forget", Some(&named), result)
    }

    /// Lets the device join a saved network by itself, or stops it.
    async fn set_auto_join(
        &self,
        #[zbus(header)] header: Header<'_>,
        ssid: String,
        on: bool,
    ) -> Result<(), NetworkError> {
        let named = ssid.clone();
        let result = async {
            authorize(&self.0, Some(&header)).await?;
            Ok(self.0.submit(Command::SetAutoJoin { ssid, on }).await?)
        }
        .await;
        answered("SetAutoJoin", Some(&named), result)
    }

    async fn disconnect(&self, #[zbus(header)] header: Header<'_>) -> Result<(), NetworkError> {
        let result = async {
            authorize(&self.0, Some(&header)).await?;
            Ok(self.0.submit(Command::Disconnect).await?)
        }
        .await;
        answered("Disconnect", None, result)
    }

    /// Emitted when the list of networks changes.
    #[zbus(signal)]
    pub async fn networks_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    /// Emitted when a device cannot connect or loses its connection. `reason` is one of
    /// `wrong-passphrase`, `network-not-found`, `connection-failed`, `connection-lost`,
    /// `no-address`.
    #[zbus(signal)]
    pub async fn connection_failed(
        emitter: &SignalEmitter<'_>,
        device: &str,
        reason: &str,
    ) -> zbus::Result<()>;

    /// Whether Wi-Fi is switched on (the software switch of the radio).
    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.0.snapshot().wireless_enabled
    }

    #[zbus(property)]
    async fn set_wireless_enabled(
        &self,
        value: bool,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::Result<()> {
        set_wireless(&self.0, value, header, NETWORK_NAME).await
    }

    #[zbus(property)]
    fn devices(&self) -> Vec<DeviceRecord> {
        self.0
            .snapshot()
            .devices
            .iter()
            .map(|device| {
                (
                    device.ifindex,
                    device.name.clone(),
                    device.kind.name().to_owned(),
                    device.state.name().to_owned(),
                    device.ssid.clone().unwrap_or_default(),
                    device
                        .failure
                        .map(|failure| failure.name().to_owned())
                        .unwrap_or_default(),
                    device
                        .address
                        .map(|(address, prefix)| format!("{address}/{prefix}"))
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    /// `disconnected`, `connecting`, `connected-local` (an address, no default route) or `connected`.
    #[zbus(property)]
    fn state(&self) -> String {
        nm::state(&self.0.snapshot()).name().to_owned()
    }
}

/// The name of a security on the bus.
fn security_name(security: Security) -> &'static str {
    match security {
        Security::Open => "open",
        Security::Owe => "owe",
        Security::Wep => "wep",
        Security::Wpa => "wpa",
        Security::Wpa2 => "wpa2",
        Security::Wpa3 => "wpa3",
        Security::Wpa2Wpa3 => "wpa2-wpa3",
        Security::Enterprise => "enterprise",
        Security::Unsupported => "unsupported",
    }
}

/// The write behind both `WirelessEnabled` properties, of `interface`; a refusal is recorded.
async fn set_wireless(
    shared: &Shared,
    value: bool,
    header: Option<Header<'_>>,
    interface: &'static str,
) -> zbus::Result<()> {
    let result = async {
        authorize(shared, header.as_ref())
            .await
            .map_err(|refusal| match refusal {
                Refusal::Denied(reason) => fdo::Error::AccessDenied(reason),
                Refusal::Failed(reason) => fdo::Error::Failed(reason),
            })?;
        shared
            .submit(Command::SetWirelessEnabled(value))
            .await
            .map_err(|error| fdo::Error::Failed(error.to_string()))
    }
    .await;
    if let Err(error) = &result {
        request_failed(
            interface,
            "WirelessEnabled",
            zbus::DBusError::name(error).as_str(),
            None,
        );
    }
    result?;
    Ok(())
}

/// `org.freedesktop.NetworkManager`, the subset apps poll.
pub struct Manager(pub Arc<Shared>);

#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Manager {
    /// There is no connectivity probe: the answer is the current value of `Connectivity`.
    fn check_connectivity(&self) -> u32 {
        nm::connectivity(&self.0.snapshot()) as u32
    }

    /// `NMState`.
    #[zbus(property)]
    fn state(&self) -> u32 {
        nm::state(&self.0.snapshot()) as u32
    }

    /// `NMConnectivityState`.
    #[zbus(property)]
    fn connectivity(&self) -> u32 {
        nm::connectivity(&self.0.snapshot()) as u32
    }

    /// Networking cannot be switched off as a whole: always true.
    #[zbus(property)]
    fn networking_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn wireless_enabled(&self) -> bool {
        self.0.snapshot().wireless_enabled
    }

    #[zbus(property)]
    async fn set_wireless_enabled(
        &self,
        value: bool,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::Result<()> {
        set_wireless(&self.0, value, header, MANAGER_NAME).await
    }

    /// `802-3-ethernet`, `802-11-wireless`, or empty without a primary connection.
    #[zbus(property)]
    fn primary_connection_type(&self) -> String {
        nm::primary_connection_type(&self.0.snapshot()).to_owned()
    }

    /// `StateChanged(u state)`.
    #[zbus(signal, name = "StateChanged")]
    pub async fn state_signal(emitter: &SignalEmitter<'_>, state: u32) -> zbus::Result<()>;
}
