//! The Bluetooth pairing agent (§6.4): an object the Bluetooth pane registers with `org.cupertino.Bluetooth1` while it
//! is shown. The daemon asks it during a pairing (`org.cupertino.Bluetooth1.Agent`, the `cupertino-btd` README); each
//! question becomes a sheet and waits for the user's answer, so the bus is never blocked by the window.

use std::collections::HashMap;

use zbus::message::Header;
use zbus::zvariant::OwnedValue;
use zbus::{fdo, Connection};

use crate::sheet::{Answer, Prompt};

/// The daemon whose questions the agent answers.
const BLUETOOTH_NAME: &str = "org.cupertino.Bluetooth1";

/// Where the agent object lives on the connection that registered it.
pub const AGENT_PATH: &str = "/org/cupertino/Settings1/Agent";

/// What the daemon asked the agent.
#[derive(Debug)]
pub enum AgentEvent {
    /// A prompt for the user; `answer` takes the reply of a question (`None` for a passkey to show only). Dropping it
    /// refuses.
    Prompt {
        id: u64,
        device: String,
        prompt: Prompt,
        answer: Option<async_channel::Sender<Answer>>,
    },
    /// Pairing `id` is over: its sheet closes.
    Cancel(u64),
}

/// The agent's refusal, `org.cupertino.Bluetooth1.Agent.Rejected` (the daemon's name for "the user said no").
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Bluetooth1.Agent")]
pub enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
}

pub struct Agent {
    pub on_event: Box<dyn Fn(AgentEvent) + Send + Sync>,
}

/// The name a prompt shows for the daemon's device record: its name, else its address.
fn device_title(device: &HashMap<String, OwnedValue>) -> String {
    let text = |k: &str| {
        device
            .get(k)
            .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
    };
    text("Name")
        .or_else(|| text("Address"))
        .unwrap_or_else(|| "Unknown device".to_string())
}

/// Accept a call only from the current owner of `name`, the Bluetooth daemon: any client of the bus can call the agent's
/// object, and a forged question would put a pairing prompt (and the user's answer) in another client's hands. The owner
/// is looked up per call, so a restarted daemon is followed; calls are rare (one per pairing step). The refusal's text.
pub(crate) async fn from_owner(
    header: &Header<'_>,
    connection: &Connection,
    name: &'static str,
) -> Result<(), String> {
    let sender = header.sender().ok_or("the call has no sender")?;
    let owner = fdo::DBusProxy::new(connection)
        .await
        .map_err(|e| e.to_string())?
        .get_name_owner(name.try_into().expect("a valid bus name"))
        .await
        .map_err(|e| format!("the Bluetooth daemon is not on the bus: {e}"))?;
    if owner.as_str() == sender.as_str() {
        Ok(())
    } else {
        Err(format!("{sender} is not the Bluetooth daemon"))
    }
}

async fn from_daemon(header: &Header<'_>, connection: &Connection) -> Result<(), AgentError> {
    from_owner(header, connection, BLUETOOTH_NAME)
        .await
        .map_err(AgentError::Rejected)
}

impl Agent {
    /// Ask the user `prompt` about `device` and wait for the answer; a closed sheet refuses.
    pub(crate) async fn ask(&self, id: u64, device: String, prompt: Prompt) -> Answer {
        let (answer, reply) = async_channel::bounded(1);
        (self.on_event)(AgentEvent::Prompt {
            id,
            device,
            prompt,
            answer: Some(answer),
        });
        reply.recv().await.unwrap_or(Answer::Reject)
    }
}

#[zbus::interface(name = "org.cupertino.Bluetooth1.Agent")]
impl Agent {
    /// Accept pairing `id`; `passkey` is the number to compare unless `just_works`.
    async fn confirm_passkey(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        id: u64,
        device: HashMap<String, OwnedValue>,
        passkey: u32,
        just_works: bool,
    ) -> Result<bool, AgentError> {
        from_daemon(&header, connection).await?;
        match self
            .ask(
                id,
                device_title(&device),
                Prompt::Confirm {
                    passkey: (!just_works).then_some(passkey),
                },
            )
            .await
        {
            Answer::Confirm => Ok(true),
            Answer::Reject | Answer::Passkey(_) => {
                Err(AgentError::Rejected("the user refused the pairing".into()))
            }
        }
    }

    /// The passkey the device shows, typed by the user.
    async fn request_passkey(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        id: u64,
        device: HashMap<String, OwnedValue>,
    ) -> Result<u32, AgentError> {
        from_daemon(&header, connection).await?;
        match self.ask(id, device_title(&device), Prompt::Request).await {
            Answer::Passkey(p) => Ok(p),
            Answer::Reject | Answer::Confirm => {
                Err(AgentError::Rejected("the user refused the pairing".into()))
            }
        }
    }

    /// Show `passkey` to type on the device; `entered` digits so far.
    async fn display_passkey(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        id: u64,
        device: HashMap<String, OwnedValue>,
        passkey: u32,
        entered: u8,
    ) -> Result<(), AgentError> {
        from_daemon(&header, connection).await?;
        (self.on_event)(AgentEvent::Prompt {
            id,
            device: device_title(&device),
            prompt: Prompt::Display { passkey, entered },
            answer: None,
        });
        Ok(())
    }

    async fn cancel(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        id: u64,
    ) -> Result<(), AgentError> {
        from_daemon(&header, connection).await?;
        (self.on_event)(AgentEvent::Cancel(id));
        Ok(())
    }
}
