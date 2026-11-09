//! The daemon: owns the bus name, runs the [`Model`] against a [`Transport`], and turns what the
//! model reports into D-Bus signals and calls to the pairing agent.
//!
//! Two threads touch the model, always under one lock that is never held across an `await`:
//!
//! - the host thread in [`Daemon::run`] waits for packets from the kernel and feeds them in;
//! - the bus executor runs the method calls, which feed in a request and wait for its answer.
//!
//! The model's effects are carried out under that lock, in order. What the bus must hear (property
//! changes, signals, agent calls) is queued there too and sent by one task, so that two threads
//! cannot emit news out of order.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zbus::blocking::connection::Builder;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::UniqueName;
use zbus::zvariant::ObjectPath;
use zbus::Connection;

use crate::auth::Authorizer;
use crate::device::DeviceInfo;
use cupertino_diagnostic_events::{self as diag, AddressKind, Severity, Value};

use crate::error::{start_failed, Error, Result};
use crate::interfaces::{Bluetooth, DeviceView, AGENT_INTERFACE, BLUETOOTH_NAME, BLUETOOTH_PATH};
use crate::keys::KeyStore;
use crate::model::Model;
use crate::pairing::{AgentReply, PairingId, Prompt};
use crate::request::{
    AdapterProps, AgentAnswer, AgentCall, Change, Effect, Request, RequestError, Ticket,
};
use crate::transport::Transport;

/// The longest the host thread waits for the kernel before it looks at the clock again. A bus call
/// can add a deadline while the thread waits, and the thread is not woken for it. approximate: a
/// timeout fires up to this much late, which no timeout here (30 s and longer) can tell.
const LOOP_TICK: Duration = Duration::from_secs(1);

/// Which bus to serve on.
pub enum Bus {
    /// The system bus, where the daemon runs as root.
    System,
    /// The session bus, to run unprivileged.
    Session,
    /// A bus at an explicit address (tests).
    Address(String),
}

/// Everything the daemon needs to start.
pub struct Config {
    pub bus: Bus,
    /// Where the pairing keys are stored.
    pub keys_dir: PathBuf,
    /// Run without changing anything: the model sends no writing command and refuses requests.
    pub read_only: bool,
    /// Decides who may write.
    pub authorizer: Box<dyn Authorizer>,
    pub transport: Arc<dyn Transport>,
}

/// The pairing agent a client registered: the connection that did and the object to call.
#[derive(Clone, Debug)]
pub struct Agent {
    pub sender: UniqueName<'static>,
    pub path: ObjectPath<'static>,
}

/// News for the bus, in the order the model produced it.
enum Delivery {
    Adapter {
        old: AdapterProps,
        new: AdapterProps,
    },
    DeviceAdded(DeviceInfo),
    DeviceChanged(DeviceInfo),
    DeviceRemoved(String),
    Agent {
        agent: Option<Agent>,
        call: AgentCall,
    },
}

struct Core {
    model: Model,
    store: KeyStore,
    /// The callers waiting for an answer.
    waiters: HashMap<Ticket, async_channel::Sender<std::result::Result<(), RequestError>>>,
    next_ticket: u64,
    agent: Option<Agent>,
}

/// State shared between the host thread and the bus objects.
pub struct Shared {
    core: Mutex<Core>,
    transport: Arc<dyn Transport>,
    authorizer: Box<dyn Authorizer>,
    started: Instant,
    deliveries: async_channel::Sender<Delivery>,
}

impl Shared {
    fn core(&self) -> MutexGuard<'_, Core> {
        self.core
            .lock()
            .expect("no thread panics while holding the model")
    }

    pub fn authorizer(&self) -> &dyn Authorizer {
        self.authorizer.as_ref()
    }

    /// The monotonic time the model runs on.
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn props(&self) -> AdapterProps {
        self.core().model.props()
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.core().model.devices()
    }

    /// The registered agent, if any.
    pub fn agent(&self) -> Option<Agent> {
        self.core().agent.clone()
    }

    /// Replaces the registered agent when `replace` accepts the old one (`None` when there is
    /// none); returns whether the new one is registered. Taking the lock once keeps two callers
    /// from both seeing the slot free.
    pub fn register_agent(
        &self,
        agent: Agent,
        replace: impl FnOnce(Option<&Agent>) -> bool,
    ) -> bool {
        let mut core = self.core();
        if replace(core.agent.as_ref()) {
            core.agent = Some(agent);
            true
        } else {
            false
        }
    }

    /// Clears the agent if it is `sender`'s; returns whether it was.
    pub fn unregister_agent(&self, sender: &UniqueName<'_>) -> bool {
        let mut core = self.core();
        if core
            .agent
            .as_ref()
            .is_some_and(|agent| agent.sender == *sender)
        {
            core.agent = None;
            true
        } else {
            false
        }
    }

    /// Clears the agent if it is still `agent`: it did not answer because it is gone.
    fn drop_agent(&self, agent: &Agent) {
        let mut core = self.core();
        if core
            .agent
            .as_ref()
            .is_some_and(|current| current.sender == agent.sender && current.path == agent.path)
        {
            core.agent = None;
        }
    }

    /// Feeds a request to the model and waits for its answer.
    pub async fn submit(&self, request: Request) -> std::result::Result<(), RequestError> {
        let (waiter, answer) = async_channel::bounded(1);
        {
            let mut core = self.core();
            let now = self.now();
            core.next_ticket += 1;
            let ticket = Ticket(core.next_ticket);
            core.waiters.insert(ticket, waiter);
            let effects = core.model.request(now, ticket, request);
            self.run(&mut core, now, effects, Some(ticket));
        }
        // The model answers every request, at the latest when its deadline passes. A closed
        // channel means the daemon is shutting down.
        answer.recv().await.unwrap_or(Err(RequestError::Cancelled))
    }

    /// The agent's answer to a call.
    fn agent_answered(&self, pairing: PairingId, reply: AgentReply) {
        let mut core = self.core();
        let now = self.now();
        let effects = core.model.agent_answered(AgentAnswer { pairing, reply });
        self.run(&mut core, now, effects, None);
    }

    /// Carries out the effects of the model, and the effects of those, in order. `origin` is the
    /// request that caused them: a command that cannot be sent fails it at once instead of after
    /// its deadline.
    fn run(&self, core: &mut Core, now: Duration, effects: Vec<Effect>, origin: Option<Ticket>) {
        let mut queue: VecDeque<Effect> = effects.into();
        while let Some(effect) = queue.pop_front() {
            match effect {
                Effect::Send { index, command } => {
                    if let Err(error) = self.transport.send(index, &command) {
                        match origin {
                            Some(ticket) => answer(
                                core,
                                ticket,
                                Err(RequestError::Transport(error.to_string())),
                            ),
                            None => eprintln!("cupertino-btd: sending {command:?}: {error}"),
                        }
                    }
                }
                Effect::OpenLink {
                    adapter,
                    device,
                    ticket,
                    ..
                } => {
                    if let Err(error) = self.transport.open_link(adapter, device) {
                        queue.extend(core.model.link_failed(ticket, error.to_string()));
                    }
                }
                Effect::CloseLink { adapter, device } => self.transport.close_link(adapter, device),
                // A file that cannot be read leaves the adapter unset-up: writing the keys of a
                // later pairing over a file that could not be read would destroy it.
                Effect::LoadKeys { index, address } => match core.store.load(address) {
                    Ok(keys) => queue.extend(core.model.keys_loaded(now, index, keys)),
                    Err(error) => {
                        eprintln!("cupertino-btd: adapter {address} stays unavailable: {error}");
                        diag::event(
                            Severity::Error,
                            "bluetooth.backend.start_failed",
                            "the Bluetooth service could not start",
                        )
                        .field("step", Value::Public("keys"))
                        .field("kind", Value::Public("keys"))
                        .field(
                            "adapter",
                            Value::Address(AddressKind::Bluetooth, &address.to_string()),
                        )
                        .field("error", Value::Public(&error.to_string()))
                        .emit();
                    }
                },
                // The pairing happened in the controller; the daemon says so and carries on.
                Effect::SaveKeys { address, keys } => {
                    if let Err(error) = core.store.save(address, &keys) {
                        eprintln!("cupertino-btd: the keys of adapter {address} are not saved and are lost at the next restart: {error}");
                    }
                }
                Effect::Complete { ticket, result } => answer(core, ticket, result),
                Effect::Changed(change) => self.deliver(match change {
                    Change::Adapter { old, new } => Delivery::Adapter { old, new },
                    Change::DeviceAdded(info) => Delivery::DeviceAdded(info),
                    Change::DeviceChanged(info) => Delivery::DeviceChanged(info),
                    Change::DeviceRemoved(key) => Delivery::DeviceRemoved(key),
                }),
                Effect::Agent(call) => self.deliver(Delivery::Agent {
                    agent: core.agent.clone(),
                    call,
                }),
                Effect::Log(message) => eprintln!("cupertino-btd: {message}"),
            }
        }
    }

    /// Queues news for the task that sends it.
    fn deliver(&self, delivery: Delivery) {
        if self.deliveries.try_send(delivery).is_err() {
            eprintln!("cupertino-btd: the task that sends signals has stopped");
        }
    }
}

/// Answers the caller waiting for `ticket`, if one still waits.
fn answer(core: &mut Core, ticket: Ticket, result: std::result::Result<(), RequestError>) {
    // A caller that gave up has dropped its end; there is nobody to tell.
    if let Some(waiter) = core.waiters.remove(&ticket) {
        let _ = waiter.try_send(result);
    }
}

/// The running daemon.
pub struct Daemon {
    // Kept so the connection, and the task that sends signals, live as long as the daemon.
    _connection: zbus::blocking::Connection,
    shared: Arc<Shared>,
}

impl Daemon {
    /// Serves the object, takes the bus name and asks the kernel which controllers exist. Fails
    /// with [`Error::NameTaken`] when another process owns the name. A failure is recorded as
    /// `bluetooth.backend.start_failed` (step `bus`).
    pub fn start(config: Config) -> Result<Self> {
        Self::serve(config).inspect_err(|error| start_failed("bus", error))
    }

    fn serve(config: Config) -> Result<Self> {
        let (deliveries, queue) = async_channel::unbounded();
        let shared = Arc::new(Shared {
            core: Mutex::new(Core {
                model: Model::new(config.read_only),
                store: KeyStore::new(config.keys_dir),
                waiters: HashMap::new(),
                next_ticket: 0,
                agent: None,
            }),
            transport: config.transport,
            authorizer: config.authorizer,
            started: Instant::now(),
            deliveries,
        });
        let builder = match &config.bus {
            Bus::System => Builder::system(),
            Bus::Session => Builder::session(),
            Bus::Address(address) => Builder::address(address.as_str()),
        }
        .map_err(Error::bus("connecting to the bus"))?;
        let connection = builder
            .serve_at(BLUETOOTH_PATH, Bluetooth(shared.clone()))
            .map_err(Error::bus("exporting the object"))?
            .build()
            .map_err(Error::bus("building the connection"))?;
        let reply = connection
            .request_name_with_flags(BLUETOOTH_NAME, RequestNameFlags::DoNotQueue.into())
            .map_err(Error::bus("requesting the bus name"))?;
        match reply {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
            RequestNameReply::Exists | RequestNameReply::InQueue => {
                return Err(Error::NameTaken(BLUETOOTH_NAME))
            }
        }
        connection
            .inner()
            .executor()
            .spawn(
                send_deliveries(connection.inner().clone(), shared.clone(), queue),
                "bluetooth signals",
            )
            .detach();

        {
            let mut core = shared.core();
            let effects = core.model.start();
            let now = shared.now();
            shared.run(&mut core, now, effects, None);
        }
        Ok(Self {
            _connection: connection,
            shared,
        })
    }

    /// Reads packets from the kernel and feeds them to the model, for good. Returns only with the
    /// error that stopped it.
    pub fn run(&self) -> Result<Infallible> {
        loop {
            let wait = {
                let core = self.shared.core();
                let until = core
                    .model
                    .next_deadline()
                    .map(|deadline| deadline.saturating_sub(self.shared.now()));
                until.map_or(LOOP_TICK, |until| until.min(LOOP_TICK))
            };
            let drained = self.shared.transport.receive(Some(wait))?;
            for error in &drained.errors {
                eprintln!("cupertino-btd: malformed packet: {error}");
            }
            let mut core = self.shared.core();
            let now = self.shared.now();
            let mut effects = Vec::new();
            for packet in drained.packets {
                effects.extend(core.model.on_packet(now, packet));
            }
            effects.extend(core.model.tick(now));
            self.shared.run(&mut core, now, effects, None);
        }
    }
}

/// Sends the queued news, one item at a time, so that it reaches the bus in the order it happened.
async fn send_deliveries(
    connection: Connection,
    shared: Arc<Shared>,
    queue: async_channel::Receiver<Delivery>,
) {
    while let Ok(delivery) = queue.recv().await {
        let result = match delivery {
            Delivery::Agent { agent, call } => {
                // The agent can take as long as the user does; the next item must not wait.
                connection
                    .executor()
                    .spawn(
                        call_agent(connection.clone(), shared.clone(), agent, call),
                        "pairing agent call",
                    )
                    .detach();
                Ok(())
            }
            other => emit(&connection, other).await,
        };
        if let Err(error) = result {
            eprintln!("cupertino-btd: {error}");
        }
    }
}

/// Emits the property changes and the signal for one item.
async fn emit(connection: &Connection, delivery: Delivery) -> Result<()> {
    let emitting = Error::bus("emitting a signal");
    let object = connection
        .object_server()
        .interface::<_, Bluetooth>(BLUETOOTH_PATH)
        .await
        .map_err(emitting)?;
    let interface = object.get().await;
    let emitter = object.signal_emitter();
    match delivery {
        Delivery::Adapter { old, new } => {
            if old.available != new.available {
                interface
                    .available_changed(emitter)
                    .await
                    .map_err(emitting)?;
            }
            if old.powered != new.powered {
                interface.powered_changed(emitter).await.map_err(emitting)?;
            }
            if old.discovering != new.discovering {
                interface
                    .discovering_changed(emitter)
                    .await
                    .map_err(emitting)?;
            }
            // Devices come and go with the adapter.
            if old.available != new.available {
                interface.devices_changed(emitter).await.map_err(emitting)?;
            }
        }
        Delivery::DeviceAdded(info) => {
            interface.devices_changed(emitter).await.map_err(emitting)?;
            Bluetooth::device_added(emitter, DeviceView::from(&info))
                .await
                .map_err(emitting)?;
        }
        Delivery::DeviceChanged(info) => {
            interface.devices_changed(emitter).await.map_err(emitting)?;
            Bluetooth::device_changed(emitter, DeviceView::from(&info))
                .await
                .map_err(emitting)?;
        }
        Delivery::DeviceRemoved(key) => {
            interface.devices_changed(emitter).await.map_err(emitting)?;
            Bluetooth::device_removed(emitter, &key)
                .await
                .map_err(emitting)?;
        }
        Delivery::Agent { .. } => unreachable!("agent calls are not emitted"),
    }
    Ok(())
}

/// The error name an agent answers with to refuse. The agent interface documents it.
const AGENT_REJECTED: &str = "org.cupertino.Bluetooth1.Agent.Rejected";

/// Asks the agent what `call` needs and hands the answer to the model. With no agent, a question
/// is answered as failed so the pairing ends and says why.
async fn call_agent(
    connection: Connection,
    shared: Arc<Shared>,
    agent: Option<Agent>,
    call: AgentCall,
) {
    let pairing = call.pairing;
    let device = DeviceView::from(&call.device);
    let id = pairing.0;
    let asks = matches!(call.prompt, Prompt::Confirm { .. } | Prompt::RequestPasskey);
    let Some(agent) = agent else {
        if asks {
            shared.agent_answered(
                pairing,
                AgentReply::Failed("no pairing agent is registered".to_owned()),
            );
        }
        return;
    };
    let proxy = match zbus::Proxy::new(
        &connection,
        agent.sender.clone(),
        agent.path.clone(),
        AGENT_INTERFACE,
    )
    .await
    {
        Ok(proxy) => proxy,
        Err(error) => return agent_failed(&shared, &agent, pairing, asks, error),
    };
    let reply = match call.prompt {
        Prompt::Confirm {
            passkey,
            just_works,
        } => proxy
            .call::<_, _, bool>("ConfirmPasskey", &(id, &device, passkey, just_works))
            .await
            .map(AgentReply::Confirm),
        Prompt::RequestPasskey => proxy
            .call::<_, _, u32>("RequestPasskey", &(id, &device))
            .await
            .map(|passkey| AgentReply::Passkey(Some(passkey))),
        Prompt::DisplayPasskey { passkey, entered } => proxy
            .call::<_, _, ()>("DisplayPasskey", &(id, &device, passkey, entered))
            .await
            .map(|()| AgentReply::Confirm(true)),
        Prompt::Cancel => proxy
            .call::<_, _, ()>("Cancel", &(id,))
            .await
            .map(|()| AgentReply::Confirm(true)),
    };
    match reply {
        Ok(reply) if asks => shared.agent_answered(pairing, reply),
        Ok(_) => {}
        // The user said no.
        Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == AGENT_REJECTED && asks => {
            let refusal = if matches!(call.prompt, Prompt::Confirm { .. }) {
                AgentReply::Confirm(false)
            } else {
                AgentReply::Passkey(None)
            };
            shared.agent_answered(pairing, refusal);
        }
        Err(error) => agent_failed(&shared, &agent, pairing, asks, error),
    }
}

/// The agent could not be asked, or failed. A vanished owner means the agent is gone for good.
fn agent_failed(
    shared: &Shared,
    agent: &Agent,
    pairing: PairingId,
    asks: bool,
    error: zbus::Error,
) {
    let gone = matches!(&error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" || name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner");
    if gone {
        shared.drop_agent(agent);
    }
    eprintln!(
        "cupertino-btd: the pairing agent {} failed: {error}",
        agent.sender
    );
    if asks {
        shared.agent_answered(pairing, AgentReply::Failed(error.to_string()));
    }
}
