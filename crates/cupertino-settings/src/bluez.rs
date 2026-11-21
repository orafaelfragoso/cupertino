//! The Bluetooth pane on BlueZ, when `cupertino-btd` is not running: the adapter's reading and calls
//! (`cupertino_radios::bluez`, the same as `org.cupertino.Bluetooth1` on the first adapter), and the pane's pairing
//! agent as an `org.bluez.Agent1`. A device's id is its object path.
#![expect(
    clippy::disallowed_methods,
    reason = "called by the services, on the I/O pool (shell::desk)"
)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

pub use cupertino_radios::bluez::{
    connect, disconnect, discovery, pair, pairing, proxy, read, remove, set_powered, DEVICE, NAME,
};
use zbus::blocking::Connection;
use zbus::message::Header;
use zbus::proxy::CacheProperties;
use zbus::zvariant::ObjectPath;

use crate::agent::{from_owner, Agent, AgentEvent, AGENT_PATH};
use crate::live::device_title;
use crate::sheet::{Answer, Prompt};

const AGENT_MANAGER: &str = "org.bluez.AgentManager1";
const AGENT_MANAGER_PATH: &str = "/org/bluez";
/// The agent answers questions and shows numbers, so every pairing method can be offered.
const CAPABILITY: &str = "KeyboardDisplay";

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

/// The name a prompt shows for a device, as its row shows it ([`device_title`]): its name, else its kind and the end of
/// its address.
async fn title(connection: &zbus::Connection, device: &ObjectPath<'_>) -> String {
    let read = async {
        let proxy = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(connection)
            .destination(NAME)?
            .path(device.to_owned())?
            .interface(DEVICE)?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        let address = proxy.get_property::<String>("Address").await?;
        let name = proxy.get_property::<String>("Name").await.ok();
        let icon = proxy
            .get_property::<String>("Icon")
            .await
            .unwrap_or_default();
        Ok::<_, zbus::Error>(device_title(name.as_deref(), &icon, &address))
    };
    read.await.unwrap_or_else(|_| "Unknown device".to_string())
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
