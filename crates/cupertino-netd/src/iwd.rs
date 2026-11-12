//! The [`Supplicant`] over iwd's D-Bus API (`net.connman.iwd`, version 3.x).
//!
//! - Devices, networks and saved networks come from the service's object manager at `/.
//! - `Device.Powered`, `Station.Scan`, `Station.Disconnect` and `Network.Connect` are the calls.
//! - The passphrase of a network that has no saved profile reaches iwd through an `Agent` this
//!   process exports and registers with `AgentManager`: iwd calls `RequestPassphrase` during
//!   `Network.Connect`. The agent answers only the connection that owns iwd's name: the bus
//!   policy lets every user reach this process, so any other caller that named the network
//!   while a connection waits would be handed its passphrase.
//! - `Station.State` changes become [`SupplicantEvent`]s. A watcher thread reads them, and every
//!   `Network.Connect` runs on its own thread, because iwd answers only when the attempt ends.
//!   A change counts only when iwd's name owner sent it: the match rule's sender keeps out
//!   nothing addressed to this process directly, and a forged `disconnected` would drop the
//!   lease.
//!
//! iwd answers every failed handshake with the same `net.connman.iwd.Failed`, so a wrong
//! passphrase cannot be told from other failures. A failure after the agent supplied a
//! passphrase is reported as [`ConnectFailure::WrongPassphrase`] (approximate).

use std::collections::HashMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use cupertino_netlink::Ssid;
use cupertino_peer::signal::{from_owner, Owner};
use rustix::event::{eventfd, EventfdFlags};
use rustix::io::{read, write, Errno};
use zbus::blocking::fdo::{ObjectManagerProxy, PropertiesProxy};
use zbus::blocking::{Connection, Proxy};
use zbus::fdo;
use zbus::message::{Header, Type as MessageType};
use zbus::names::InterfaceName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::supplicant::{
    ConnectFailure, Interface, KnownNetwork, NetworkClass, Supplicant, SupplicantEvent,
};

const SERVICE: &str = "net.connman.iwd";
const ROOT: &str = "/";
const AGENT_MANAGER_PATH: &str = "/net/connman/iwd";
const AGENT_MANAGER: &str = "net.connman.iwd.AgentManager";
const DEVICE: &str = "net.connman.iwd.Device";
const STATION: &str = "net.connman.iwd.Station";
const NETWORK: &str = "net.connman.iwd.Network";
const KNOWN_NETWORK: &str = "net.connman.iwd.KnownNetwork";
/// Where this process exports its agent.
pub const AGENT_PATH: &str = "/org/cupertino/Network1/Agent";

type Properties = HashMap<String, OwnedValue>;
type Objects = HashMap<OwnedObjectPath, HashMap<zbus::names::OwnedInterfaceName, Properties>>;

/// What the agent may hand to iwd: one network, one passphrase.
struct Pending {
    /// The iwd network the passphrase is for; any other network is refused.
    network: OwnedObjectPath,
    /// Zeroed when it is dropped unused; the copy handed to iwd is the reply's.
    passphrase: Option<Zeroizing<String>>,
    /// iwd asked and was given the passphrase.
    supplied: bool,
}

type SharedPending = Arc<Mutex<Option<Pending>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .expect("no thread panics while holding the lock")
}

/// The errors an agent method answers with (`net.connman.iwd.Agent.Error.*`).
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "net.connman.iwd.Agent.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The request is refused.
    Canceled(String),
}

/// `net.connman.iwd.Agent`.
struct Agent {
    pending: SharedPending,
    /// iwd's connection, as the watcher follows it: asking the bus here, in the call, would stop the object server, and
    /// with it the connection, while calls from other users filled its queue.
    iwd: Owner,
}

#[zbus::interface(name = "net.connman.iwd.Agent")]
impl Agent {
    /// iwd stopped using the agent.
    fn release(&self) {}

    /// The pending passphrase, for iwd only.
    fn request_passphrase(
        &self,
        network: ObjectPath<'_>,
        #[zbus(header)] header: Header<'_>,
    ) -> std::result::Result<String, AgentError> {
        if !self.iwd.is(header.sender()) {
            return Err(AgentError::Canceled("only iwd may ask".to_owned()));
        }
        let mut pending = lock(&self.pending);
        match pending.as_mut() {
            Some(pending) if pending.network.as_ref() == network => match pending.passphrase.take()
            {
                Some(mut passphrase) => {
                    pending.supplied = true;
                    Ok(std::mem::take(&mut *passphrase))
                }
                None => Err(AgentError::Canceled("there is no passphrase".to_owned())),
            },
            _ => Err(AgentError::Canceled(
                "no connection to that network was requested".to_owned(),
            )),
        }
    }

    fn request_private_key_passphrase(
        &self,
        _network: ObjectPath<'_>,
    ) -> std::result::Result<String, AgentError> {
        Err(AgentError::Canceled(
            "private key passphrases are not supported".to_owned(),
        ))
    }

    fn request_user_name_and_password(
        &self,
        _network: ObjectPath<'_>,
    ) -> std::result::Result<(String, String), AgentError> {
        Err(AgentError::Canceled(
            "user names and passwords are not supported".to_owned(),
        ))
    }

    fn request_user_password(
        &self,
        _network: ObjectPath<'_>,
        _user: String,
    ) -> std::result::Result<String, AgentError> {
        Err(AgentError::Canceled(
            "user passwords are not supported".to_owned(),
        ))
    }

    /// iwd withdrew the request.
    fn cancel(&self, _reason: String) {}
}

/// Whether the call failed because nobody owns `net.connman.iwd`.
fn is_missing(error: &zbus::Error) -> bool {
    let missing = |name: &str| {
        name == "org.freedesktop.DBus.Error.ServiceUnknown"
            || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
    };
    match error {
        zbus::Error::MethodError(name, _, _) => missing(name.as_str()),
        zbus::Error::FDO(error) => matches!(
            **error,
            fdo::Error::ServiceUnknown(_) | fdo::Error::NameHasNoOwner(_)
        ),
        _ => false,
    }
}

/// The adapter for `map_err` on a call to iwd.
fn call_error(what: &'static str) -> impl Fn(zbus::Error) -> Error + Copy {
    move |source| {
        if is_missing(&source) {
            Error::SupplicantMissing
        } else {
            Error::Bus {
                what,
                source: Box::new(source),
            }
        }
    }
}

fn text(properties: &Properties, name: &str) -> Option<String> {
    properties
        .get(name)
        .and_then(|value| <&str>::try_from(&**value).ok())
        .map(str::to_owned)
}

fn class_name(class: NetworkClass) -> Option<&'static str> {
    match class {
        NetworkClass::Open => Some("open"),
        NetworkClass::Wep => Some("wep"),
        NetworkClass::Psk => Some("psk"),
        NetworkClass::Enterprise => Some("8021x"),
        NetworkClass::Unsupported => None,
    }
}

fn class_of(name: &str) -> NetworkClass {
    match name {
        "open" => NetworkClass::Open,
        "wep" => NetworkClass::Wep,
        "psk" => NetworkClass::Psk,
        "8021x" => NetworkClass::Enterprise,
        _ => NetworkClass::Unsupported,
    }
}

/// What the background threads send to [`IwdSupplicant::events`].
type Report = Result<SupplicantEvent>;

/// Wakes the event loop: adds one to the counter of `fd`.
fn wake(fd: &OwnedFd) {
    // A non-blocking eventfd refuses a write only when the counter would pass 2^64 - 2, which
    // takes that many wakes without one read.
    write(fd, &1u64.to_ne_bytes())
        .expect("the counter of an eventfd that is read does not overflow");
}

/// iwd as a [`Supplicant`].
pub struct IwdSupplicant {
    connection: Connection,
    pending: SharedPending,
    /// Interface names to indexes, filled by [`Supplicant::power_on`]. The watcher reads it.
    interfaces: Arc<Mutex<HashMap<String, u32>>>,
    reports: Receiver<Report>,
    sender: Sender<Report>,
    waker: Arc<OwnedFd>,
}

impl IwdSupplicant {
    /// Exports the agent and starts the watcher. Does not need iwd to be running yet.
    pub fn new(connection: Connection) -> Result<Self> {
        let pending: SharedPending = Arc::new(Mutex::new(None));
        let (sender, reports) = channel();
        let waker = Arc::new(
            eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC)
                .map_err(Error::syscall("eventfd"))?,
        );
        let interfaces = Arc::new(Mutex::new(HashMap::new()));
        let this = Self {
            connection,
            pending,
            interfaces,
            reports,
            sender,
            waker,
        };
        let iwd = this.watch()?;
        this.connection
            .object_server()
            .at(
                AGENT_PATH,
                Agent {
                    pending: this.pending.clone(),
                    iwd,
                },
            )
            .map_err(Error::bus("exporting the agent"))?;
        Ok(this)
    }

    fn proxy(&self, path: &str, interface: &'static str) -> Result<Proxy<'static>> {
        Proxy::new(&self.connection, SERVICE, path.to_owned(), interface)
            .map_err(Error::bus("creating a proxy"))
    }

    fn objects(&self) -> Result<Objects> {
        let manager = ObjectManagerProxy::builder(&self.connection)
            .destination(SERVICE)
            .and_then(|builder| builder.path(ROOT))
            .and_then(|builder| builder.build())
            .map_err(Error::bus("creating the object manager proxy"))?;
        manager
            .get_managed_objects()
            .map_err(|error| call_error("listing the supplicant's objects")(error.into()))
    }

    /// The object path of the device with this interface name.
    fn device_path(&self, name: &str) -> Result<OwnedObjectPath> {
        self.objects()?
            .into_iter()
            .find(|(_, interfaces)| {
                interfaces.iter().any(|(interface, properties)| {
                    interface.as_str() == DEVICE
                        && text(properties, "Name").as_deref() == Some(name)
                })
            })
            .map(|(path, _)| path)
            .ok_or_else(|| Error::SupplicantDevice(name.to_owned()))
    }

    /// The object path of the saved profile of a network.
    fn known_path(&self, ssid: &Ssid, class: NetworkClass) -> Result<OwnedObjectPath> {
        let wanted_type = class_name(class).ok_or_else(|| {
            Error::SupplicantNetwork(format!("{} has no saved profile", ssid.to_text()))
        })?;
        self.objects()?
            .into_iter()
            .find(|(_, interfaces)| {
                interfaces.iter().any(|(interface, properties)| {
                    interface.as_str() == KNOWN_NETWORK
                        && text(properties, "Name")
                            .is_some_and(|name| name.as_bytes() == ssid.as_bytes())
                        && text(properties, "Type").as_deref() == Some(wanted_type)
                })
            })
            .map(|(path, _)| path)
            .ok_or_else(|| {
                Error::SupplicantNetwork(format!("{} has no saved profile", ssid.to_text()))
            })
    }

    /// Registers the agent. iwd answers `AlreadyExists` when it is registered, which is the goal.
    fn register_agent(&self) -> Result<()> {
        let manager = self.proxy(AGENT_MANAGER_PATH, AGENT_MANAGER)?;
        let path = ObjectPath::try_from(AGENT_PATH).map_err(|error| Error::Bus {
            what: "naming the agent",
            source: Box::new(error.into()),
        })?;
        match manager.call_method("RegisterAgent", &(path,)) {
            Ok(_) => Ok(()),
            Err(zbus::Error::MethodError(name, _, _))
                if name.as_str() == "net.connman.iwd.AlreadyExists" =>
            {
                Ok(())
            }
            Err(error) => Err(call_error("registering the agent")(error)),
        }
    }

    /// Reads `Station.State` changes and turns them into events, on a thread of its own. Returns the owner of iwd's name
    /// that the watcher follows.
    fn watch(&self) -> Result<Owner> {
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(SERVICE)
            .and_then(|rule| rule.interface("org.freedesktop.DBus.Properties"))
            .and_then(|rule| rule.member("PropertiesChanged"))
            .map(|rule| rule.build())
            .map_err(Error::bus("building the match rule"))?;
        let messages = from_owner(&self.connection, SERVICE, rule, |_| {
            eprintln!("cupertino-netd: ignored a state change that iwd did not send")
        })
        .map_err(Error::bus("subscribing to state changes"))?;
        let iwd = messages.owner();
        let (connection, interfaces, sender, waker) = (
            self.connection.clone(),
            self.interfaces.clone(),
            self.sender.clone(),
            self.waker.clone(),
        );
        thread::Builder::new()
            .name("iwd-watcher".to_owned())
            .spawn(move || {
                // station path to interface index: the paths do not change while iwd runs
                let mut known: HashMap<String, u32> = HashMap::new();
                for message in messages {
                    let report = message
                        .map_err(Error::bus("reading a state change"))
                        .and_then(|message| {
                            station_event(&connection, &interfaces, &mut known, &message)
                        });
                    match report {
                        Ok(None) => {}
                        Ok(Some(event)) => {
                            if sender.send(Ok(event)).is_ok() {
                                wake(&waker);
                            }
                        }
                        Err(error) => {
                            if sender.send(Err(error)).is_ok() {
                                wake(&waker);
                            }
                        }
                    }
                }
            })
            .map_err(|source| Error::Io {
                path: "iwd-watcher".into(),
                source,
            })?;
        Ok(iwd)
    }

    /// The iwd network of the device with this name, type and SSID.
    fn network_path(
        &self,
        device: &OwnedObjectPath,
        ssid: &Ssid,
        class: NetworkClass,
    ) -> Result<OwnedObjectPath> {
        let wanted_type = class_name(class).ok_or_else(|| {
            Error::SupplicantNetwork(format!(
                "{} has an authentication the supplicant cannot start",
                ssid.to_text()
            ))
        })?;
        self.objects()?
            .into_iter()
            .find(|(_, interfaces)| {
                interfaces.iter().any(|(interface, properties)| {
                    interface.as_str() == NETWORK
                        && text(properties, "Name")
                            .is_some_and(|name| name.as_bytes() == ssid.as_bytes())
                        && text(properties, "Type").as_deref() == Some(wanted_type)
                        && properties
                            .get("Device")
                            .and_then(|value| ObjectPath::try_from(&**value).ok())
                            .is_some_and(|path| path == device.as_ref())
                })
            })
            .map(|(path, _)| path)
            .ok_or_else(|| Error::SupplicantNetwork(ssid.to_text()))
    }
}

/// The event for a `PropertiesChanged` signal, if it changes `Station.State`.
fn station_event(
    connection: &Connection,
    interfaces: &Mutex<HashMap<String, u32>>,
    known: &mut HashMap<String, u32>,
    message: &zbus::Message,
) -> Result<Option<SupplicantEvent>> {
    let (interface, changed, _invalidated): (String, HashMap<String, OwnedValue>, Vec<String>) =
        message
            .body()
            .deserialize()
            .map_err(Error::bus("reading PropertiesChanged"))?;
    if interface != STATION {
        return Ok(None);
    }
    let Some(state) = changed
        .get("State")
        .and_then(|value| <&str>::try_from(&**value).ok())
    else {
        return Ok(None);
    };
    let Some(path) = message.header().path().map(|path| path.to_string()) else {
        return Ok(None);
    };
    let ifindex = match known.get(&path) {
        Some(ifindex) => *ifindex,
        None => {
            let properties = PropertiesProxy::builder(connection)
                .destination(SERVICE)
                .and_then(|builder| builder.path(path.as_str()))
                .and_then(|builder| builder.build())
                .map_err(Error::bus("creating a properties proxy"))?;
            let name: OwnedValue = properties
                .get(InterfaceName::from_static_str_unchecked(DEVICE), "Name")
                .map_err(|error| call_error("reading the device name")(error.into()))?;
            let name = String::try_from(name).map_err(|error| Error::Bus {
                what: "reading the device name",
                source: Box::new(error.into()),
            })?;
            // a device that power_on never named is not one this daemon manages
            let Some(ifindex) = lock(interfaces).get(&name).copied() else {
                return Ok(None);
            };
            known.insert(path.clone(), ifindex);
            ifindex
        }
    };
    match state {
        "connecting" => Ok(Some(SupplicantEvent::Connecting { ifindex })),
        "connected" => {
            let properties = PropertiesProxy::builder(connection)
                .destination(SERVICE)
                .and_then(|builder| builder.path(path.as_str()))
                .and_then(|builder| builder.build())
                .map_err(Error::bus("creating a properties proxy"))?;
            let network: OwnedValue = properties
                .get(
                    InterfaceName::from_static_str_unchecked(STATION),
                    "ConnectedNetwork",
                )
                .map_err(|error| call_error("reading the connected network")(error.into()))?;
            let network = OwnedObjectPath::try_from(network).map_err(|error| Error::Bus {
                what: "reading the connected network",
                source: Box::new(error.into()),
            })?;
            let properties = PropertiesProxy::builder(connection)
                .destination(SERVICE)
                .and_then(|builder| builder.path(network.as_str()))
                .and_then(|builder| builder.build())
                .map_err(Error::bus("creating a properties proxy"))?;
            let name: OwnedValue = properties
                .get(InterfaceName::from_static_str_unchecked(NETWORK), "Name")
                .map_err(|error| call_error("reading the network name")(error.into()))?;
            let name = String::try_from(name).map_err(|error| Error::Bus {
                what: "reading the network name",
                source: Box::new(error.into()),
            })?;
            Ok(Some(SupplicantEvent::Connected {
                ifindex,
                ssid: Ssid(name.into_bytes()),
            }))
        }
        "disconnected" => Ok(Some(SupplicantEvent::Disconnected { ifindex })),
        // "disconnecting" and "roaming" change nothing the model tracks
        _ => Ok(None),
    }
}

impl Supplicant for IwdSupplicant {
    fn power_on(&mut self, interface: Interface<'_>) -> Result<()> {
        let device = self.device_path(interface.name)?;
        lock(&self.interfaces).insert(interface.name.to_owned(), interface.index);
        let properties = PropertiesProxy::builder(&self.connection)
            .destination(SERVICE)
            .and_then(|builder| builder.path(device.as_str()))
            .and_then(|builder| builder.build())
            .map_err(Error::bus("creating a properties proxy"))?;
        properties
            .set(
                InterfaceName::from_static_str_unchecked(DEVICE),
                "Powered",
                Value::Bool(true),
            )
            .map_err(|error| call_error("powering the device on")(error.into()))
    }

    fn scan(&mut self, interface: Interface<'_>) -> Result<()> {
        let device = self.device_path(interface.name)?;
        self.proxy(device.as_str(), STATION)?
            .call_method("Scan", &())
            .map(drop)
            .map_err(call_error("scanning"))
    }

    fn known_networks(&mut self, _interface: Interface<'_>) -> Result<Vec<KnownNetwork>> {
        Ok(self
            .objects()?
            .into_values()
            .filter_map(|interfaces| {
                interfaces
                    .into_iter()
                    .find(|(interface, _)| interface.as_str() == KNOWN_NETWORK)
            })
            .filter_map(|(_, properties)| {
                Some(KnownNetwork {
                    ssid: Ssid(text(&properties, "Name")?.into_bytes()),
                    class: class_of(&text(&properties, "Type")?),
                    // iwd's default for a profile that does not say
                    auto_connect: properties
                        .get("AutoConnect")
                        .and_then(|value| bool::try_from(&**value).ok())
                        .unwrap_or(true),
                })
            })
            .collect())
    }

    fn connect(
        &mut self,
        interface: Interface<'_>,
        ssid: &Ssid,
        class: NetworkClass,
        passphrase: Option<&str>,
    ) -> Result<()> {
        self.register_agent()?;
        let device = self.device_path(interface.name)?;
        let network = self.network_path(&device, ssid, class)?;
        *lock(&self.pending) = Some(Pending {
            network: network.clone(),
            passphrase: passphrase.map(|passphrase| Zeroizing::new(passphrase.to_owned())),
            supplied: false,
        });
        let proxy = self.proxy(network.as_str(), NETWORK)?;
        let (pending, sender, waker, ifindex) = (
            self.pending.clone(),
            self.sender.clone(),
            self.waker.clone(),
            interface.index,
        );
        thread::Builder::new()
            .name("iwd-connect".to_owned())
            .spawn(move || {
                let result = proxy.call_method("Connect", &());
                let supplied = lock(&pending)
                    .take()
                    .is_some_and(|pending| pending.supplied);
                let report = match result {
                    Ok(_) => return,
                    // the user (or a newer request) cancelled the attempt: nothing failed
                    Err(zbus::Error::MethodError(name, _, _))
                        if name.as_str() == "net.connman.iwd.Aborted" =>
                    {
                        return
                    }
                    Err(zbus::Error::MethodError(name, _, _))
                        if matches!(
                            name.as_str(),
                            "net.connman.iwd.NotFound" | "net.connman.iwd.NotAvailable"
                        ) =>
                    {
                        Ok(SupplicantEvent::Failed {
                            ifindex,
                            reason: ConnectFailure::NotFound,
                        })
                    }
                    Err(zbus::Error::MethodError(name, _, _))
                        if name.as_str() == "net.connman.iwd.Failed" && supplied =>
                    {
                        Ok(SupplicantEvent::Failed {
                            ifindex,
                            reason: ConnectFailure::WrongPassphrase,
                        })
                    }
                    Err(zbus::Error::MethodError(..)) => Ok(SupplicantEvent::Failed {
                        ifindex,
                        reason: ConnectFailure::Failed,
                    }),
                    // the bus failed: the attempt's outcome is unknown, so say so and fail it
                    Err(error) => {
                        if sender.send(Err(call_error("connecting")(error))).is_ok() {
                            wake(&waker);
                        }
                        Ok(SupplicantEvent::Failed {
                            ifindex,
                            reason: ConnectFailure::Failed,
                        })
                    }
                };
                if sender.send(report).is_ok() {
                    wake(&waker);
                }
            })
            .map_err(|source| Error::Io {
                path: "iwd-connect".into(),
                source,
            })?;
        Ok(())
    }

    fn forget(&mut self, ssid: &Ssid, class: NetworkClass) -> Result<()> {
        let path = self.known_path(ssid, class)?;
        self.proxy(path.as_str(), KNOWN_NETWORK)?
            .call_method("Forget", &())
            .map(drop)
            .map_err(call_error("forgetting a network"))
    }

    fn set_auto_connect(&mut self, ssid: &Ssid, class: NetworkClass, on: bool) -> Result<()> {
        let path = self.known_path(ssid, class)?;
        let properties = PropertiesProxy::builder(&self.connection)
            .destination(SERVICE)
            .and_then(|builder| builder.path(path.as_str()))
            .and_then(|builder| builder.build())
            .map_err(Error::bus("creating a properties proxy"))?;
        let result = properties.set(
            InterfaceName::from_static_str_unchecked(KNOWN_NETWORK),
            "AutoConnect",
            Value::Bool(on),
        );
        result.map_err(|error| call_error("changing a network's auto-join")(error.into()))
    }

    fn disconnect(&mut self, interface: Interface<'_>) -> Result<()> {
        let device = self.device_path(interface.name)?;
        self.proxy(device.as_str(), STATION)?
            .call_method("Disconnect", &())
            .map(drop)
            .map_err(call_error("disconnecting"))
    }

    fn events(&mut self) -> Vec<Result<SupplicantEvent>> {
        // Clear the counter before reading the channel: an event that arrives in between wakes the
        // loop again.
        let mut counter = [0u8; 8];
        let cleared = match read(&*self.waker, &mut counter) {
            // the counter was zero: another call took the wake already
            Ok(_) | Err(Errno::AGAIN) => None,
            Err(errno) => Some(Err(Error::Syscall {
                call: "read of the supplicant's eventfd",
                errno,
            })),
        };
        cleared.into_iter().chain(self.reports.try_iter()).collect()
    }

    fn as_fd(&self) -> Option<BorrowedFd<'_>> {
        Some(self.waker.as_fd())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const STATION_PATH: &str = "/net/connman/iwd/0/3";
    const NETWORK_PATH: &str = "/net/connman/iwd/0/3/5361666548_psk";

    struct FakeDevice;

    #[zbus::interface(name = "net.connman.iwd.Device")]
    impl FakeDevice {
        #[zbus(property)]
        fn name(&self) -> String {
            "wlan0".to_owned()
        }
    }

    fn iwd(bus: &cupertino_test_bus::PrivateBus) -> Connection {
        bus.blocking_builder()
            .name(SERVICE)
            .unwrap()
            .serve_at(STATION_PATH, FakeDevice)
            .unwrap()
            .build()
            .unwrap()
    }

    fn request_passphrase(from: &Connection, to: &Connection) -> zbus::Result<String> {
        let to = to.unique_name().unwrap().to_string();
        let network = ObjectPath::try_from(NETWORK_PATH).unwrap();
        from.call_method(
            Some(to.as_str()),
            AGENT_PATH,
            Some("net.connman.iwd.Agent"),
            "RequestPassphrase",
            &(network,),
        )?
        .body()
        .deserialize()
    }

    #[test]
    fn only_iwd_is_handed_the_passphrase() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let iwd = iwd(&bus);
        let netd = bus.connect();
        let supplicant = IwdSupplicant::new(netd.clone()).unwrap();
        *lock(&supplicant.pending) = Some(Pending {
            network: OwnedObjectPath::try_from(NETWORK_PATH).unwrap(),
            passphrase: Some(Zeroizing::new("correct horse".to_owned())),
            supplied: false,
        });
        // another user's program on the bus, while the connection waits
        let peer = bus.connect();
        assert!(matches!(
            request_passphrase(&peer, &netd),
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "net.connman.iwd.Agent.Error.Canceled"
        ));
        assert!(!lock(&supplicant.pending).as_ref().unwrap().supplied);
        assert_eq!(request_passphrase(&iwd, &netd).unwrap(), "correct horse");
    }

    #[test]
    fn only_iwd_says_a_station_disconnected() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let iwd = iwd(&bus);
        let netd = bus.connect();
        let mut supplicant = IwdSupplicant::new(netd.clone()).unwrap();
        lock(&supplicant.interfaces).insert("wlan0".to_owned(), 3);
        let disconnected = |from: &Connection, to: Option<&str>| {
            let changed = HashMap::from([("State", Value::from("disconnected"))]);
            from.emit_signal(
                to,
                STATION_PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &(STATION, changed, Vec::<String>::new()),
            )
            .unwrap()
        };
        let events = |supplicant: &mut IwdSupplicant, within: Duration| {
            let deadline = Instant::now() + within;
            loop {
                let events: Vec<String> = supplicant
                    .events()
                    .into_iter()
                    .map(|event| format!("{event:?}"))
                    .collect();
                if !events.is_empty() || Instant::now() > deadline {
                    return events;
                }
                thread::sleep(Duration::from_millis(10));
            }
        };
        let to = netd.unique_name().unwrap().to_string();
        let peer = bus.connect();
        for to in [Some(to.as_str()), None] {
            disconnected(&peer, to);
        }
        assert!(
            events(&mut supplicant, Duration::from_millis(300)).is_empty(),
            "a forged disconnection is ignored"
        );
        disconnected(&iwd, None);
        assert_eq!(
            events(&mut supplicant, Duration::from_secs(5)),
            [format!(
                "{:?}",
                Ok::<_, Error>(SupplicantEvent::Disconnected { ifindex: 3 })
            )]
        );
    }
}
