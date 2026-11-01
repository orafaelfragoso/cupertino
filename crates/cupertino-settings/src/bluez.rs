//! The Bluetooth pane on BlueZ, when `cupertino-btd` is not running: the same [`Bluetooth`] reading and calls as
//! `org.cupertino.Bluetooth1` on the first adapter, and the pairing agent as an `org.bluez.Agent1`. A device's id is its
//! object path.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, Proxy};
use zbus::message::Header;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

use crate::agent::{from_owner, Agent, AgentEvent, AGENT_PATH};
use crate::live::{display_name, Bluetooth, BtDevice};
use crate::sheet::{Answer, Prompt};

pub const NAME: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const AGENT_MANAGER: &str = "org.bluez.AgentManager1";
const AGENT_MANAGER_PATH: &str = "/org/bluez";
/// The agent answers questions and shows numbers, so every pairing method can be offered.
const CAPABILITY: &str = "KeyboardDisplay";

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn proxy<'a>(connection: &Connection, path: &str, interface: &'a str) -> zbus::Result<Proxy<'a>> {
    Builder::new(connection)
        .destination(NAME)?
        .path(path.to_owned())?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
}

fn objects(connection: &Connection) -> zbus::Result<Objects> {
    proxy(connection, "/", "org.freedesktop.DBus.ObjectManager")?.call("GetManagedObjects", &())
}

/// The adapter the pane works with: the first by path, as `bluetoothctl` picks it.
fn adapter(objects: &Objects) -> Option<&OwnedObjectPath> {
    objects
        .iter()
        .filter(|(_, interfaces)| interfaces.contains_key(ADAPTER))
        .map(|(path, _)| path)
        .min_by(|a, b| a.as_str().cmp(b.as_str()))
}

fn text(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

fn flag(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    properties.get(key).and_then(|v| bool::try_from(v).ok())
}

/// The reading the pane shows; `pairing` are the devices whose `Pair` has not returned.
pub fn bluetooth(connection: &Connection, pairing: &HashSet<String>) -> zbus::Result<Bluetooth> {
    let objects = objects(connection)?;
    let Some(adapter_path) = adapter(&objects) else {
        return Ok(Bluetooth {
            available: false,
            powered: false,
            discovering: false,
            devices: Vec::new(),
            agent_elsewhere: false,
            failure: None,
        });
    };
    let adapter = &objects[adapter_path][ADAPTER];
    let mut devices: Vec<BtDevice> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let d = interfaces.get(DEVICE)?;
            let on: OwnedObjectPath = d.get("Adapter")?.try_clone().ok()?.try_into().ok()?;
            if on != *adapter_path {
                return None;
            }
            Some(BtDevice {
                id: path.to_string(),
                address: text(d, "Address")?,
                name: text(d, "Name").map(|n| display_name(&n)),
                icon: text(d, "Icon").unwrap_or_default(),
                paired: flag(d, "Paired")?,
                connected: flag(d, "Connected")?,
                pairing: pairing.contains(path.as_str()),
            })
        })
        .collect();
    devices.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Bluetooth {
        available: true,
        powered: flag(adapter, "Powered").unwrap_or(false),
        discovering: flag(adapter, "Discovering").unwrap_or(false),
        devices,
        agent_elsewhere: false,
        failure: None,
    })
}

fn the_adapter(connection: &Connection) -> zbus::Result<Proxy<'static>> {
    let objects = objects(connection)?;
    let path = adapter(&objects).ok_or_else(|| {
        zbus::Error::from(zbus::fdo::Error::Failed(
            "There is no Bluetooth adapter.".into(),
        ))
    })?;
    proxy(connection, path.as_str(), ADAPTER)
}

/// A device of the adapter by its id; any other path is refused, so a call never reaches another object.
fn device(connection: &Connection, id: &str) -> zbus::Result<Proxy<'static>> {
    let objects = objects(connection)?;
    let known = ObjectPath::try_from(id)
        .ok()
        .and_then(|path| objects.get(&OwnedObjectPath::from(path)))
        .is_some_and(|interfaces| interfaces.contains_key(DEVICE));
    if !known {
        return Err(zbus::fdo::Error::Failed("The device is no longer available.".into()).into());
    }
    proxy(connection, id, DEVICE)
}

pub fn set_powered(connection: &Connection, on: bool) -> zbus::Result<()> {
    the_adapter(connection)?
        .set_property("Powered", on)
        .map_err(zbus::Error::from)
}

pub fn discovery(connection: &Connection, on: bool) -> zbus::Result<()> {
    let method = if on {
        "StartDiscovery"
    } else {
        "StopDiscovery"
    };
    the_adapter(connection)?.call(method, &())
}

pub fn connect(connection: &Connection, id: &str) -> zbus::Result<()> {
    device(connection, id)?.call("Connect", &())
}

pub fn disconnect(connection: &Connection, id: &str) -> zbus::Result<()> {
    device(connection, id)?.call("Disconnect", &())
}

pub fn remove(connection: &Connection, id: &str) -> zbus::Result<()> {
    device(connection, id)?;
    let path = ObjectPath::try_from(id).map_err(zbus::Error::from)?;
    the_adapter(connection)?.call("RemoveDevice", &(path,))
}

/// The device to pair, checked; [`pair`] runs on the pairing's own thread.
pub fn pairing(connection: &Connection, id: &str) -> zbus::Result<Proxy<'static>> {
    device(connection, id)
}

/// Pair, then trust the device, so its services connect again later without asking (as BlueZ's own clients do).
pub fn pair(device: &Proxy<'_>) -> zbus::Result<()> {
    device.call::<_, _, ()>("Pair", &())?;
    device
        .set_property("Trusted", true)
        .map_err(zbus::Error::from)
}

pub fn register_agent(connection: &Connection) -> zbus::Result<()> {
    let path = ObjectPath::try_from(AGENT_PATH).expect("a valid object path");
    let manager = proxy(connection, AGENT_MANAGER_PATH, AGENT_MANAGER)?;
    manager.call::<_, _, ()>("RegisterAgent", &(&path, CAPABILITY))?;
    manager.call("RequestDefaultAgent", &(&path,))
}

pub fn unregister_agent(connection: &Connection) -> zbus::Result<()> {
    let path = ObjectPath::try_from(AGENT_PATH).expect("a valid object path");
    proxy(connection, AGENT_MANAGER_PATH, AGENT_MANAGER)?.call("UnregisterAgent", &(path,))
}

/// The agent's answers to BlueZ, `org.bluez.Error.Rejected` and `org.bluez.Error.Canceled`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
pub enum BluezError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

/// The pairing agent of the pane as BlueZ's `org.bluez.Agent1`. BlueZ asks about one pairing at a time and names no
/// pairing in `Cancel`, so each question gets an id of its own and a cancel closes the open one.
pub struct BluezAgent {
    agent: Agent,
    next: AtomicU64,
    /// The open question: its device, its id, and whether it only shows a passkey (repeated as digits are typed).
    open: Mutex<Option<(String, u64, bool)>>,
}

impl BluezAgent {
    pub fn new(agent: Agent) -> Self {
        BluezAgent {
            agent,
            next: AtomicU64::new(1),
            open: Mutex::new(None),
        }
    }

    fn id(&self, device: &str, display: bool) -> u64 {
        let mut open = self
            .open
            .lock()
            .expect("no thread panics holding the question");
        if let Some((d, id, true)) = &*open {
            if display && d == device {
                return *id;
            }
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        *open = Some((device.to_string(), id, display));
        id
    }

    async fn ask(
        &self,
        header: &Header<'_>,
        connection: &zbus::Connection,
        device: &ObjectPath<'_>,
        prompt: Prompt,
    ) -> Result<Answer, BluezError> {
        from_owner(header, connection, NAME)
            .await
            .map_err(BluezError::Rejected)?;
        let id = self.id(device.as_str(), false);
        Ok(self
            .agent
            .ask(id, title(connection, device).await, prompt)
            .await)
    }
}

/// The name a prompt shows for a device: its alias, which BlueZ sets to the name or the address.
async fn title(connection: &zbus::Connection, device: &ObjectPath<'_>) -> String {
    let alias = async {
        zbus::proxy::Builder::<zbus::Proxy<'_>>::new(connection)
            .destination(NAME)?
            .path(device.to_owned())?
            .interface(DEVICE)?
            .cache_properties(CacheProperties::No)
            .build()
            .await?
            .get_property::<String>("Alias")
            .await
    };
    alias
        .await
        .map(|a| display_name(&a))
        .unwrap_or_else(|_: zbus::Error| "Unknown device".to_string())
}

fn refused() -> BluezError {
    BluezError::Rejected("the user refused the pairing".into())
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl BluezAgent {
    async fn release(&self) {}

    /// A legacy PIN, typed as the six-digit code.
    async fn request_pin_code(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        device: ObjectPath<'_>,
    ) -> Result<String, BluezError> {
        match self
            .ask(&header, connection, &device, Prompt::Request)
            .await?
        {
            Answer::Passkey(p) => Ok(format!("{p:06}")),
            Answer::Reject | Answer::Confirm => Err(refused()),
        }
    }

    async fn request_passkey(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        device: ObjectPath<'_>,
    ) -> Result<u32, BluezError> {
        match self
            .ask(&header, connection, &device, Prompt::Request)
            .await?
        {
            Answer::Passkey(p) => Ok(p),
            Answer::Reject | Answer::Confirm => Err(refused()),
        }
    }

    async fn request_confirmation(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        device: ObjectPath<'_>,
        passkey: u32,
    ) -> Result<(), BluezError> {
        let prompt = Prompt::Confirm {
            passkey: Some(passkey),
        };
        match self.ask(&header, connection, &device, prompt).await? {
            Answer::Confirm => Ok(()),
            Answer::Reject | Answer::Passkey(_) => Err(refused()),
        }
    }

    /// A pairing with no number (just works).
    async fn request_authorization(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        device: ObjectPath<'_>,
    ) -> Result<(), BluezError> {
        let prompt = Prompt::Confirm { passkey: None };
        match self.ask(&header, connection, &device, prompt).await? {
            Answer::Confirm => Ok(()),
            Answer::Reject | Answer::Passkey(_) => Err(refused()),
        }
    }

    /// A service of a device that is not trusted; the pane trusts what it pairs, so anything else is refused.
    async fn authorize_service(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        _device: ObjectPath<'_>,
        _uuid: String,
    ) -> Result<(), BluezError> {
        from_owner(&header, connection, NAME)
            .await
            .map_err(BluezError::Rejected)?;
        Err(BluezError::Rejected("the device is not trusted".into()))
    }

    async fn display_passkey(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        device: ObjectPath<'_>,
        passkey: u32,
        entered: u16,
    ) -> Result<(), BluezError> {
        from_owner(&header, connection, NAME)
            .await
            .map_err(BluezError::Rejected)?;
        let id = self.id(device.as_str(), true);
        (self.agent.on_event)(AgentEvent::Prompt {
            id,
            device: title(connection, &device).await,
            prompt: Prompt::Display {
                passkey,
                entered: u8::try_from(entered).unwrap_or(u8::MAX),
            },
            answer: None,
        });
        Ok(())
    }

    /// A legacy PIN to type on the device, shown when it is a six-digit code; another one is refused.
    async fn display_pin_code(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
        device: ObjectPath<'_>,
        pincode: String,
    ) -> Result<(), BluezError> {
        let passkey = (pincode.len() == 6)
            .then(|| pincode.parse::<u32>().ok())
            .flatten()
            .ok_or_else(|| BluezError::Rejected("only a six-digit code can be shown".into()))?;
        self.display_passkey(header, connection, device, passkey, 0)
            .await
    }

    async fn cancel(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), BluezError> {
        from_owner(&header, connection, NAME)
            .await
            .map_err(BluezError::Rejected)?;
        let open = self
            .open
            .lock()
            .expect("no thread panics holding the question")
            .take();
        if let Some((_, id, _)) = open {
            (self.agent.on_event)(AgentEvent::Cancel(id));
        }
        Ok(())
    }
}
