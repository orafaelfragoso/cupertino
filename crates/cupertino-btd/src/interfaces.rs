//! The D-Bus object: `org.cupertino.Bluetooth1` at `/org/cupertino/Bluetooth1`, and the agent
//! interface that the shell implements.
//!
//! The object holds no state of its own: each getter reads the model, so a property never
//! disagrees with the last signal. Reads are open to every caller; every write, and registering an
//! agent, is checked against the caller's login session ([`crate::auth`]).
//!
//! # The agent
//!
//! A client registers an object with [`Bluetooth::register_agent`]; the daemon then calls it at
//! `org.cupertino.Bluetooth1.Agent` on the connection that registered it:
//!
//! - `ConfirmPasskey(t id, a{sv} device, u passkey, b just_works) -> b`: ask the user to confirm.
//!   With `just_works` there is no number to compare. Answer `true` or `false`, or fail with
//!   `org.cupertino.Bluetooth1.Agent.Rejected` to refuse.
//! - `RequestPasskey(t id, a{sv} device) -> u`: ask for the passkey the remote device shows. Fail
//!   with `...Agent.Rejected` to refuse.
//! - `DisplayPasskey(t id, a{sv} device, u passkey, y entered)`: show a passkey the user types on
//!   the remote device, with the digits typed so far.
//! - `Cancel(t id)`: the pairing `id` is over, close any dialog.

use std::sync::Arc;

use cupertino_diagnostic_events::{self as diag, AddressKind, Severity, Value as Field};
use zbus::fdo;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, SerializeDict, Type, Value};
use zbus::Connection;

use crate::auth::Decision;
use crate::daemon::{Agent, Shared};
use crate::device::{DeviceInfo, DeviceRef};
use crate::request::{Request, RequestError};

pub const BLUETOOTH_NAME: &str = "org.cupertino.Bluetooth1";
pub const BLUETOOTH_PATH: &str = "/org/cupertino/Bluetooth1";
pub const AGENT_INTERFACE: &str = "org.cupertino.Bluetooth1.Agent";

/// A device as the bus shows it: a dictionary, so that fields can be added later. A field that is
/// unknown is left out, not filled with a made-up value.
#[derive(Clone, Debug, SerializeDict, Type, Value, OwnedValue)]
#[zvariant(signature = "dict", rename_all = "PascalCase")]
pub struct DeviceView {
    /// `AA:BB:CC:DD:EE:FF/bredr`: the string every method accepts.
    pub id: String,
    pub address: String,
    /// `bredr`, `le-public` or `le-random`.
    pub address_type: String,
    pub name: Option<String>,
    /// What to draw: `phone`, `headphones`, `keyboard`, ... or `unknown`.
    pub icon: String,
    /// The class of device (BR/EDR).
    pub class: Option<u32>,
    /// The GAP appearance (low energy).
    pub appearance: Option<u16>,
    /// dBm, as last reported while discovering.
    pub rssi: Option<i16>,
    pub paired: bool,
    pub bonded: bool,
    pub connected: bool,
    /// A pairing with this device is in progress.
    pub pairing: bool,
    pub uuids: Vec<String>,
}

impl From<&DeviceInfo> for DeviceView {
    fn from(info: &DeviceInfo) -> Self {
        Self {
            id: info.key(),
            address: info.id.address.to_string(),
            address_type: info.id.kind.to_string(),
            name: info.name.clone(),
            icon: info.icon.as_str().to_owned(),
            class: info.class.map(|class| class.value()),
            appearance: info.appearance,
            rssi: info.rssi.map(i16::from),
            paired: info.paired,
            bonded: info.bonded,
            connected: info.connected,
            pairing: info.pairing,
            uuids: info.uuids.iter().map(ToString::to_string).collect(),
        }
    }
}

/// The errors of `org.cupertino.Bluetooth1`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Bluetooth1")]
pub enum BluetoothError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The caller is not in a session that is active on a seat.
    NotAuthorized(String),
    /// An argument is not an address or an object path.
    InvalidArguments(String),
    NoAdapter(String),
    NotPowered(String),
    UnknownDevice(String),
    /// The address names a BR/EDR and a low-energy device; add the address type.
    Ambiguous(String),
    AlreadyPaired(String),
    NotPaired(String),
    AlreadyConnected(String),
    NotConnected(String),
    /// The controller is busy, for instance with another pairing.
    Busy(String),
    ConnectFailed(String),
    /// The user refused the pairing.
    Rejected(String),
    AuthenticationFailed(String),
    Timeout(String),
    Cancelled(String),
    /// The pairing agent is missing or failed.
    Agent(String),
    /// Another client's agent is registered.
    AgentExists(String),
    /// The remote device wants a legacy PIN, which is not supported.
    PinUnsupported(String),
    /// Bluetooth is blocked by a switch.
    Blocked(String),
    NotSupported(String),
    /// The kernel refused the daemon: it lacks the privilege.
    PermissionDenied(String),
    /// The daemon runs read-only.
    ReadOnly(String),
    /// The controller or the socket failed, or the caller could not be checked.
    Failed(String),
}

impl From<RequestError> for BluetoothError {
    fn from(error: RequestError) -> Self {
        let message = error.to_string();
        match error {
            RequestError::ReadOnly => Self::ReadOnly(message),
            RequestError::NoAdapter => Self::NoAdapter(message),
            RequestError::NotPowered => Self::NotPowered(message),
            RequestError::UnknownDevice(_) => Self::UnknownDevice(message),
            RequestError::AmbiguousDevice(_) => Self::Ambiguous(message),
            RequestError::AlreadyPaired => Self::AlreadyPaired(message),
            RequestError::NotPaired => Self::NotPaired(message),
            RequestError::AlreadyConnected => Self::AlreadyConnected(message),
            RequestError::NotConnected => Self::NotConnected(message),
            RequestError::Busy => Self::Busy(message),
            RequestError::ConnectFailed => Self::ConnectFailed(message),
            RequestError::Rejected => Self::Rejected(message),
            RequestError::AuthenticationFailed => Self::AuthenticationFailed(message),
            RequestError::Timeout => Self::Timeout(message),
            RequestError::Cancelled => Self::Cancelled(message),
            RequestError::Agent(_) => Self::Agent(message),
            RequestError::PinUnsupported => Self::PinUnsupported(message),
            RequestError::Blocked => Self::Blocked(message),
            RequestError::NotSupported => Self::NotSupported(message),
            RequestError::PermissionDenied => Self::PermissionDenied(message),
            RequestError::Controller(_) | RequestError::Transport(_) => Self::Failed(message),
        }
    }
}

/// The name of the error a refused property write answers with, as [`fdo::Error::from`] makes it.
fn property_error_name(error: &BluetoothError) -> String {
    match error {
        BluetoothError::NotAuthorized(_) => "org.freedesktop.DBus.Error.AuthFailed".to_owned(),
        BluetoothError::ZBus(_) => zbus::DBusError::name(error).to_string(),
        _ => "org.freedesktop.DBus.Error.Failed".to_owned(),
    }
}

impl From<BluetoothError> for fdo::Error {
    fn from(error: BluetoothError) -> Self {
        match error {
            BluetoothError::NotAuthorized(message) => fdo::Error::AuthFailed(message),
            BluetoothError::ZBus(error) => fdo::Error::from(error),
            other => fdo::Error::Failed(other.to_string()),
        }
    }
}

/// Maps the caller's bus name to its process through `GetConnectionCredentials` and asks the
/// authorizer. The pid comes from the bus daemon, not from the message, so the caller cannot
/// choose it.
async fn authorize(
    shared: &Shared,
    header: &Header<'_>,
    connection: &Connection,
) -> Result<(), BluetoothError> {
    let failed = |what: &str, error: &dyn std::fmt::Display| {
        BluetoothError::Failed(format!("{what}: {error}"))
    };
    let sender = header
        .sender()
        .ok_or_else(|| BluetoothError::Failed("the call has no sender".to_owned()))?;
    let bus = fdo::DBusProxy::new(connection)
        .await
        .map_err(|e| failed("connecting to the bus daemon", &e))?;
    let credentials = bus
        .get_connection_credentials(BusName::from(sender.clone()))
        .await
        .map_err(|e| failed("reading the caller's credentials", &e))?;
    let pid = credentials.process_id().ok_or_else(|| {
        BluetoothError::Failed("the bus did not report the caller's process".to_owned())
    })?;
    match shared.authorizer().authorize(pid) {
        Ok(Decision::Allowed) => Ok(()),
        Ok(Decision::Denied(reason)) => Err(BluetoothError::NotAuthorized(reason)),
        Err(error) => Err(failed(
            "asking whether the caller may change the adapter",
            &error,
        )),
    }
}

fn reference(address: &str) -> Result<DeviceRef, BluetoothError> {
    address
        .parse()
        .map_err(|error| BluetoothError::InvalidArguments(format!("{address}: {error}")))
}

/// Records `bluetooth.request.failed`: `method` was answered with the D-Bus error `error`. A device address the call
/// named is kept as its salted hash.
fn request_failed(method: &'static str, error: &str, address: Option<&str>) {
    let mut event = diag::event(
        Severity::Warn,
        "bluetooth.request.failed",
        "a request to the Bluetooth service failed",
    )
    .field("interface", Field::Public(BLUETOOTH_NAME))
    .field("method", Field::Public(method))
    .field("error", Field::Public(error));
    if let Some(address) = address {
        event = event.field("address", Field::Address(AddressKind::Bluetooth, address));
    }
    event.emit();
}

/// `result` of `method`, its error recorded.
fn answered<T>(
    method: &'static str,
    address: Option<&str>,
    result: Result<T, BluetoothError>,
) -> Result<T, BluetoothError> {
    if let Err(error) = &result {
        request_failed(method, zbus::DBusError::name(error).as_str(), address);
    }
    result
}

/// `org.cupertino.Bluetooth1`.
pub struct Bluetooth(pub Arc<Shared>);

impl Bluetooth {
    /// Authorises the caller, then runs the request.
    async fn write(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        request: Request,
    ) -> Result<(), BluetoothError> {
        authorize(&self.0, header, connection).await?;
        self.0.submit(request).await?;
        Ok(())
    }
}

#[zbus::interface(name = "org.cupertino.Bluetooth1")]
impl Bluetooth {
    /// Starts looking for devices. Stops by itself after a while, and when `stop_discovery` is
    /// called.
    async fn start_discovery(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), BluetoothError> {
        let result = self
            .write(&header, connection, Request::StartDiscovery)
            .await;
        answered("StartDiscovery", None, result)
    }

    async fn stop_discovery(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), BluetoothError> {
        let result = self
            .write(&header, connection, Request::StopDiscovery)
            .await;
        answered("StopDiscovery", None, result)
    }

    /// Pairs with a discovered device, asking the registered agent to confirm. Returns when the
    /// pairing is over. `address` is `AA:BB:CC:DD:EE:FF`, with `/bredr`, `/le-public` or
    /// `/le-random` added when the same address is known twice.
    async fn pair(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        address: &str,
    ) -> Result<(), BluetoothError> {
        let result = async {
            self.write(&header, connection, Request::Pair(reference(address)?))
                .await
        }
        .await;
        answered("Pair", Some(address), result)
    }

    /// Connects to a device and keeps the link up. It does not start any profile.
    async fn connect(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        address: &str,
    ) -> Result<(), BluetoothError> {
        let result = async {
            self.write(&header, connection, Request::Connect(reference(address)?))
                .await
        }
        .await;
        answered("Connect", Some(address), result)
    }

    async fn disconnect(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        address: &str,
    ) -> Result<(), BluetoothError> {
        let result = async {
            self.write(
                &header,
                connection,
                Request::Disconnect(reference(address)?),
            )
            .await
        }
        .await;
        answered("Disconnect", Some(address), result)
    }

    /// Unpairs a device and deletes its keys, or forgets a device that was only discovered.
    async fn remove(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        address: &str,
    ) -> Result<(), BluetoothError> {
        let result = async {
            self.write(&header, connection, Request::Remove(reference(address)?))
                .await
        }
        .await;
        answered("Remove", Some(address), result)
    }

    /// Registers the caller's object at `path` as the pairing agent. Another client's agent that
    /// is still on the bus is not replaced.
    async fn register_agent(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        path: ObjectPath<'_>,
    ) -> Result<(), BluetoothError> {
        let result = async {
            authorize(&self.0, &header, connection).await?;
            let sender = header
                .sender()
                .ok_or_else(|| BluetoothError::Failed("the call has no sender".to_owned()))?
                .clone()
                .into_owned();
            let agent = Agent {
                sender: sender.clone(),
                path: path.into_owned(),
            };
            // Whether the registered agent still has an owner is asked of the bus first: the lock on
            // the slot cannot be held across the call.
            let bus = fdo::DBusProxy::new(connection).await.map_err(|e| {
                BluetoothError::Failed(format!("connecting to the bus daemon: {e}"))
            })?;
            let current = self.0.agent();
            let alive = match &current {
                Some(current) if current.sender != sender => bus
                    .name_has_owner(BusName::from(current.sender.clone()))
                    .await
                    .map_err(|e| {
                        BluetoothError::Failed(format!(
                            "asking the bus about the registered agent: {e}"
                        ))
                    })?,
                _ => false,
            };
            let registered = self.0.register_agent(agent, |slot| match (slot, &current) {
                (None, None) => true,
                (Some(slot), Some(seen)) => {
                    slot.sender == seen.sender && slot.path == seen.path && !alive
                }
                // The slot changed while the bus was asked: do not guess.
                _ => false,
            });
            if registered {
                Ok(())
            } else {
                Err(BluetoothError::AgentExists(
                    "another client's pairing agent is registered".to_owned(),
                ))
            }
        }
        .await;
        answered("RegisterAgent", None, result)
    }

    /// Removes the caller's agent. Fails if the caller has none registered.
    async fn unregister_agent(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), BluetoothError> {
        let result = async {
            authorize(&self.0, &header, connection).await?;
            let sender = header
                .sender()
                .ok_or_else(|| BluetoothError::Failed("the call has no sender".to_owned()))?;
            if self.0.unregister_agent(sender) {
                Ok(())
            } else {
                Err(BluetoothError::InvalidArguments(
                    "the caller has no pairing agent registered".to_owned(),
                ))
            }
        }
        .await;
        answered("UnregisterAgent", None, result)
    }

    #[zbus(signal)]
    pub async fn device_added(emitter: &SignalEmitter<'_>, device: DeviceView) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn device_changed(
        emitter: &SignalEmitter<'_>,
        device: DeviceView,
    ) -> zbus::Result<()>;

    /// `id` is the device's `Id`.
    #[zbus(signal)]
    pub async fn device_removed(emitter: &SignalEmitter<'_>, id: &str) -> zbus::Result<()>;

    /// An adapter is ready. Without one, every write fails with `NoAdapter`.
    #[zbus(property)]
    fn available(&self) -> bool {
        self.0.props().available
    }

    /// Whether the adapter is on. Writing it turns the adapter on or off.
    #[zbus(property)]
    fn powered(&self) -> bool {
        self.0.props().powered
    }

    #[zbus(property)]
    async fn set_powered(
        &self,
        powered: bool,
        #[zbus(header)] header: Option<Header<'_>>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), BluetoothError> {
        let result = async {
            // A setter has no header when the daemon itself sets the property, which it never does.
            let header = header.ok_or_else(|| {
                BluetoothError::Failed("the property was set without a call".to_owned())
            })?;
            self.write(&header, connection, Request::SetPowered(powered))
                .await
        }
        .await;
        if let Err(error) = &result {
            // a property's caller sees the error as `fdo::Error` makes it
            request_failed("Powered", &property_error_name(error), None);
        }
        result
    }

    #[zbus(property)]
    fn discovering(&self) -> bool {
        self.0.props().discovering
    }

    /// The devices known on the default adapter: paired, connected and recently discovered.
    #[zbus(property)]
    fn devices(&self) -> Vec<DeviceView> {
        self.0.devices().iter().map(DeviceView::from).collect()
    }
}
