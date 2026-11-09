//! The services behind the bar's own status items: `org.cupertino.Network1` (`cupertino-netd`), the UPower display
//! device and manager (`cupertino-powerd`), `org.cupertino.Bluetooth1` (`cupertino-btd`) on the system bus, and the
//! do-not-disturb state of `org.cupertino.NotificationCenter1` (`cupertino-notifications`) on the session bus. It also
//! reads whether the control panel (`org.cupertino.ControlPanel1`, `cupertino-controlcenter`) and the notification
//! centre's list are open and on which output, so the bar highlights the item that opened them.
//!
//! One task reads every service into an [`Extras`] and sends it to the process whenever it differs from the last one
//! sent. It reads again after any signal on the services' objects (property changes, the network list, devices), when a
//! service's bus name changes owner (a daemon that starts later is picked up), on [`ExtrasBus::refresh`], and after
//! every action. Properties are read from the service every time, never from a cache. A service that cannot be read
//! hides its item; why is reported on stderr once per new reason.
//!
//! The task's queue is bounded ([`QUEUE`]). Any peer on a bus can send this process signals, so a refresh is queued
//! only when none waits already: the read that a waiting refresh causes starts after the new signal arrived, so it
//! sees what the signal announced. The session agent is a value the next read takes, not a queued command, so a
//! newer agent replaces an older one and is never lost. An action waits in the queue; should the queue be full (the
//! task is stuck on a service that does not answer), the action is dropped and reported rather than queued without
//! bound.

use crate::extras::{
    BatteryState, BluetoothDevice, BluetoothState, ExtraAction, Extras, FocusLifetime, FocusState,
    OpenPanels, SessionCaps, SessionState, WifiNetwork, WifiState,
};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use zbus::message::Type;
use zbus::names::UniqueName;
use zbus::proxy::{Builder, CacheProperties};
use zbus::zvariant::OwnedValue;
use zbus::{Connection, MatchRule, MessageStream, Proxy};

use crate::worker::WorkerGone;

const LOGIN_NAME: &str = "org.cupertino.Login1";
const LOGIN_PATH: &str = "/org/cupertino/Login1";
const NETWORK_NAME: &str = "org.cupertino.Network1";
const NETWORK_PATH: &str = "/org/cupertino/Network1";
const BLUETOOTH_NAME: &str = "org.cupertino.Bluetooth1";
const BLUETOOTH_PATH: &str = "/org/cupertino/Bluetooth1";
const UPOWER_NAME: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const UPOWER_DISPLAY_DEVICE_PATH: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const UPOWER_DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";
const CENTRE_NAME: &str = "org.cupertino.NotificationCenter1";
const CENTRE_PATH: &str = "/org/cupertino/NotificationCenter1";
const PANEL_NAME: &str = "org.cupertino.ControlPanel1";
const PANEL_PATH: &str = "/org/cupertino/ControlPanel1";

/// `org.freedesktop.UPower.Device.State` of a charging battery.
const UPOWER_STATE_CHARGING: u32 = 1;

/// A row of `Devices` of `org.cupertino.Network1`: index, name, kind, state, SSID, failure, address.
type DeviceRecord = (u32, String, String, String, String, String, String);
/// A row of `GetNetworks`: SSID, signal (dBm, `i32::MIN` when unknown), security, saved, connected.
type NetworkRecord = (String, i32, String, bool, bool);

/// How many commands may wait for the task: at most one refresh, the rest actions.
pub const QUEUE: usize = 16;

enum Command {
    Refresh,
    Act(ExtraAction),
}

/// Which process may answer as the session agent, as the server names it. Nothing is trusted until it does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionAgent {
    #[default]
    None,
    /// The owner of `org.cupertino.Login1` must be this process.
    Process(u32),
    /// A development run: whoever owns the name.
    Any,
}

/// The task's queue, as the process and the signal followers fill it.
#[derive(Clone)]
struct Queue {
    commands: async_channel::Sender<Command>,
    /// A refresh waits in `commands`: another would read nothing newer.
    refresh_waiting: Arc<AtomicBool>,
    /// The session agent the next read takes.
    agent: Arc<Mutex<SessionAgent>>,
}

impl Queue {
    fn new() -> (Queue, async_channel::Receiver<Command>) {
        let (commands, rx) = async_channel::bounded(QUEUE);
        let queue = Queue {
            commands,
            refresh_waiting: Arc::new(AtomicBool::new(false)),
            agent: Arc::new(Mutex::new(SessionAgent::None)),
        };
        (queue, rx)
    }

    /// Ask for a read, unless one waits already. Fails only when the task is gone.
    fn refresh(&self) -> Result<(), WorkerGone> {
        if self.refresh_waiting.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        match self.commands.try_send(Command::Refresh) {
            Ok(()) => Ok(()),
            // a full queue reads after each of its commands anyway
            Err(async_channel::TrySendError::Full(_)) => {
                self.refresh_waiting.store(false, Ordering::Release);
                Ok(())
            }
            Err(async_channel::TrySendError::Closed(_)) => Err(WorkerGone),
        }
    }

    /// The task took a refresh: the next signal queues another.
    fn took_refresh(&self) {
        self.refresh_waiting.store(false, Ordering::Release);
    }

    /// Queue an action; one that finds the queue full is dropped and reported. Fails only when the task is gone.
    fn act(&self, action: ExtraAction) -> Result<(), WorkerGone> {
        match self.commands.try_send(Command::Act(action)) {
            Ok(()) => Ok(()),
            Err(async_channel::TrySendError::Full(Command::Act(action))) => {
                eprintln!("cupertino-menubar: {action:?} dropped: the services do not answer");
                Ok(())
            }
            Err(async_channel::TrySendError::Full(_)) => Ok(()),
            Err(async_channel::TrySendError::Closed(_)) => Err(WorkerGone),
        }
    }

    fn set_agent(&self, agent: SessionAgent) -> Result<(), WorkerGone> {
        *self.agent.lock().unwrap_or_else(|e| e.into_inner()) = agent;
        self.refresh()
    }

    fn agent(&self) -> SessionAgent {
        *self.agent.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Handle on the extras task.
pub struct ExtrasBus {
    queue: Queue,
}

impl ExtrasBus {
    /// Start the task on the executor of `session`, with the system bus; `wake` is called after every new [`Extras`].
    pub fn start(
        session: Connection,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (ExtrasBus, mpsc::Receiver<Extras>) {
        Self::with_system(session, Connection::system(), wake)
    }

    /// [`ExtrasBus::start`] with the system bus `system` connects to.
    pub fn with_system(
        session: Connection,
        system: impl Future<Output = zbus::Result<Connection>> + Send + 'static,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (ExtrasBus, mpsc::Receiver<Extras>) {
        let (queue, rx) = Queue::new();
        let (out, events) = mpsc::channel();
        let executor = session.executor().clone();
        executor
            .spawn(
                run(session, system, queue.clone(), rx, out, Arc::new(wake)),
                "cupertino-extras",
            )
            .detach();
        (ExtrasBus { queue }, events)
    }

    /// Read every service again (the duration rows change with the hour).
    pub fn refresh(&self) -> Result<(), WorkerGone> {
        self.queue.refresh()
    }

    /// Name the process that is the session agent; the system menu is read again.
    pub fn session_agent(&self, agent: SessionAgent) -> Result<(), WorkerGone> {
        self.queue.set_agent(agent)
    }

    /// Carry out an action; dropped and reported when the queue is full.
    pub fn act(&self, action: ExtraAction) -> Result<(), WorkerGone> {
        self.queue.act(action)
    }
}

/// Reports a reason once, until a read succeeds or fails for another reason.
#[derive(Default)]
struct Reasons(HashMap<&'static str, String>);

impl Reasons {
    fn note<T>(&mut self, what: &'static str, result: zbus::Result<Option<T>>) -> Option<T> {
        match result {
            Ok(value) => {
                self.0.remove(what);
                value
            }
            Err(e) => {
                let reason = e.to_string();
                if self.0.get(what) != Some(&reason) {
                    eprintln!("cupertino-menubar: the {what} item is hidden: {reason}");
                    self.0.insert(what, reason);
                }
                None
            }
        }
    }

    /// As [`Reasons::note`], for whether a panel is open: one that cannot be read is taken as closed.
    fn note_panel(
        &mut self,
        what: &'static str,
        result: zbus::Result<Option<String>>,
    ) -> Option<String> {
        match result {
            Ok(value) => {
                self.0.remove(what);
                value
            }
            Err(e) => {
                let reason = e.to_string();
                if self.0.get(what) != Some(&reason) {
                    eprintln!(
                        "cupertino-menubar: cannot tell whether the {what} is open: {reason}"
                    );
                    self.0.insert(what, reason);
                }
                None
            }
        }
    }
}

async fn run(
    session: Connection,
    system: impl Future<Output = zbus::Result<Connection>>,
    queue: Queue,
    rx: async_channel::Receiver<Command>,
    out: mpsc::Sender<Extras>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let system = match system.await {
        Ok(system) => Some(system),
        Err(e) => {
            eprintln!(
                "cupertino-menubar: no system bus, so no Wi-Fi, battery or Bluetooth items: {e}"
            );
            None
        }
    };
    if let Some(system) = &system {
        for (name, path) in [
            (NETWORK_NAME, NETWORK_PATH),
            (BLUETOOTH_NAME, BLUETOOTH_PATH),
            (UPOWER_NAME, UPOWER_PATH),
            (UPOWER_NAME, UPOWER_DISPLAY_DEVICE_PATH),
        ] {
            follow(system, name, path, &queue).await;
        }
    }
    follow(&session, CENTRE_NAME, CENTRE_PATH, &queue).await;
    follow(&session, PANEL_NAME, PANEL_PATH, &queue).await;
    follow(&session, LOGIN_NAME, LOGIN_PATH, &queue).await;

    let mut reasons = Reasons::default();
    let mut last: Option<Extras> = None;
    loop {
        let previous = last
            .as_ref()
            .and_then(|e| e.session.as_ref())
            .map(|s| s.caps);
        let extras = read_all(
            system.as_ref(),
            &session,
            queue.agent(),
            previous,
            &mut reasons,
        )
        .await;
        if last.as_ref() != Some(&extras) {
            // a closed channel means the process is shutting down
            if out.send(extras.clone()).is_err() {
                return;
            }
            wake();
            last = Some(extras);
        }
        let Ok(command) = rx.recv().await else {
            return;
        };
        // collapse a burst of commands into one read
        for command in std::iter::once(command).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match command {
                Command::Act(action) => {
                    if let Err(e) = act(system.as_ref(), &session, &action).await {
                        eprintln!("cupertino-menubar: {action:?} failed: {e}");
                    }
                }
                Command::Refresh => queue.took_refresh(),
            }
        }
    }
}

/// Queue a refresh for every signal on `path` and every owner change of `name`.
async fn follow(conn: &Connection, name: &'static str, path: &'static str, queue: &Queue) {
    let rules = [
        MatchRule::builder()
            .msg_type(Type::Signal)
            .path(path)
            .map(|b| b.build()),
        MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.DBus")
            .and_then(|b| b.interface("org.freedesktop.DBus"))
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg(0, name))
            .map(|b| b.build()),
    ];
    for rule in rules {
        let stream = match rule {
            Ok(rule) => MessageStream::for_match_rule(rule, conn, None).await,
            Err(e) => Err(e),
        };
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("cupertino-menubar: cannot follow {name}{path}: {e}");
                continue;
            }
        };
        let queue = queue.clone();
        conn.executor()
            .spawn(
                async move {
                    while stream.next().await.is_some() {
                        if queue.refresh().is_err() {
                            break;
                        }
                    }
                },
                "cupertino-extras-follow",
            )
            .detach();
    }
}

async fn proxy(
    conn: &Connection,
    name: &'static str,
    path: &'static str,
    interface: &'static str,
) -> zbus::Result<Proxy<'static>> {
    Builder::new(conn)
        .destination(name)?
        .path(path)?
        .interface(interface)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// Whether `error` says nobody owns the name: the service is not running, which hides its item without a report.
fn not_running(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, _, _) if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" || name.as_str() == "org.freedesktop.DBus.Error.NameHasNoOwner")
}

/// `Ok(None)` for a service that is not running.
fn absent_if_not_running<T>(result: zbus::Result<Option<T>>) -> zbus::Result<Option<T>> {
    match result {
        Err(e) if not_running(&e) => Ok(None),
        other => other,
    }
}

async fn read_all(
    system: Option<&Connection>,
    session: &Connection,
    agent: SessionAgent,
    previous: Option<SessionCaps>,
    reasons: &mut Reasons,
) -> Extras {
    let mut extras = Extras {
        lock_without_agent: agent != SessionAgent::None,
        ..Extras::default()
    };
    if let Some(system) = system {
        extras.wifi = reasons.note("Wi-Fi", absent_if_not_running(read_wifi(system).await));
        extras.battery = reasons.note("battery", absent_if_not_running(read_battery(system).await));
        extras.bluetooth = reasons.note(
            "Bluetooth",
            absent_if_not_running(read_bluetooth(system).await),
        );
    }
    extras.focus = reasons.note("Focus", absent_if_not_running(read_focus(session).await));
    extras.panels = OpenPanels {
        control_panel: reasons.note_panel(
            "control panel",
            absent_if_not_running(
                read_panel(session, PANEL_NAME, PANEL_PATH, "Shown", "Output").await,
            ),
        ),
        centre: reasons.note_panel(
            "notification centre",
            absent_if_not_running(
                read_panel(
                    session,
                    CENTRE_NAME,
                    CENTRE_PATH,
                    "CentreShown",
                    "CentreOutput",
                )
                .await,
            ),
        ),
    };
    extras.session = reasons.note(
        "system menu",
        absent_if_not_running(read_session(session, agent, previous).await),
    );
    extras
}

/// The unique name of the session agent: the owner of its bus name when that is the process the server named. A
/// client that took the name first is not the agent and gets no say in the menu.
async fn agent_owner(
    conn: &Connection,
    agent: SessionAgent,
) -> zbus::Result<Option<UniqueName<'static>>> {
    if agent == SessionAgent::None {
        return Ok(None);
    }
    let bus = zbus::fdo::DBusProxy::new(conn).await?;
    let owner = match bus.get_name_owner(LOGIN_NAME.try_into()?).await {
        Ok(owner) => owner.into_inner(),
        Err(zbus::fdo::Error::NameHasNoOwner(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let SessionAgent::Process(expected) = agent else {
        return Ok(Some(owner));
    };
    let caller = cupertino_peer::identify(conn, &owner).await;
    if caller.pid() == Some(expected) {
        return Ok(Some(owner));
    }
    eprintln!(
        "cupertino-menubar: the owner of {LOGIN_NAME} ({owner}, process {:?}) is not the session agent (process {expected}): the system menu is not read from it",
        caller.pid()
    );
    Ok(None)
}

/// The session agent's entries for the system menu; `None` without the agent. Only `Locked` has to be there: the
/// other properties are newer, and a missing one reads as the default (every entry possible, the name from `$USER`).
async fn read_session(
    conn: &Connection,
    agent: SessionAgent,
    previous: Option<SessionCaps>,
) -> zbus::Result<Option<SessionState>> {
    let Some(owner) = agent_owner(conn, agent).await? else {
        return Ok(None);
    };
    let login: Proxy<'static> = Builder::new(conn)
        .destination(owner)?
        .path(LOGIN_PATH)?
        .interface(LOGIN_NAME)?
        .cache_properties(CacheProperties::No)
        .build()
        .await?;
    let _locked: bool = login.get_property("Locked").await?;
    let user_name: String = login
        .get_property("UserName")
        .await
        .ok()
        .filter(|name: &String| !name.is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_default();
    let capabilities: Option<HashMap<String, bool>> = login.get_property("Capabilities").await.ok();
    Ok(Some(SessionState {
        user_name,
        caps: session_caps(capabilities.as_ref(), previous),
    }))
}

/// `None` without a Wi-Fi device.
async fn read_wifi(conn: &Connection) -> zbus::Result<Option<WifiState>> {
    let network = proxy(conn, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME).await?;
    let devices: Vec<DeviceRecord> = network.get_property("Devices").await?;
    if !devices.iter().any(|device| device.2 == "wifi") {
        return Ok(None);
    }
    let enabled: bool = network.get_property("WirelessEnabled").await?;
    let records: Vec<NetworkRecord> = network.call("GetNetworks", &()).await?;
    let networks = records
        .into_iter()
        .map(|(ssid, signal, security, known, connected)| WifiNetwork {
            ssid,
            signal_dbm: (signal != i32::MIN).then_some(signal),
            security,
            known,
            connected,
        })
        .collect();
    Ok(Some(WifiState { enabled, networks }))
}

/// `None` without a battery.
async fn read_battery(conn: &Connection) -> zbus::Result<Option<BatteryState>> {
    let device = proxy(
        conn,
        UPOWER_NAME,
        UPOWER_DISPLAY_DEVICE_PATH,
        UPOWER_DEVICE_INTERFACE,
    )
    .await?;
    if !device.get_property::<bool>("IsPresent").await? {
        return Ok(None);
    }
    let percentage: f64 = device.get_property("Percentage").await?;
    let state: u32 = device.get_property("State").await?;
    let on_battery: bool = proxy(conn, UPOWER_NAME, UPOWER_PATH, UPOWER_NAME)
        .await?
        .get_property("OnBattery")
        .await?;
    Ok(Some(BatteryState {
        percent: percentage.round().clamp(0.0, 100.0) as u8,
        charging: state == UPOWER_STATE_CHARGING,
        on_battery,
    }))
}

fn field<T: TryFrom<OwnedValue>>(
    device: &HashMap<String, OwnedValue>,
    key: &str,
) -> zbus::Result<Option<T>>
where
    T::Error: std::fmt::Display,
{
    device
        .get(key)
        .map(|value| {
            T::try_from(value.try_clone()?)
                .map_err(|e| zbus::Error::Failure(format!("device field {key}: {e}")))
        })
        .transpose()
}

fn required<T>(value: Option<T>, key: &str) -> zbus::Result<T> {
    value.ok_or_else(|| zbus::Error::Failure(format!("a device has no {key}")))
}

/// `None` without an adapter.
async fn read_bluetooth(conn: &Connection) -> zbus::Result<Option<BluetoothState>> {
    let bluetooth = proxy(conn, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME).await?;
    if !bluetooth.get_property::<bool>("Available").await? {
        return Ok(None);
    }
    let powered: bool = bluetooth.get_property("Powered").await?;
    let views: Vec<HashMap<String, OwnedValue>> = bluetooth.get_property("Devices").await?;
    let devices = views
        .iter()
        .map(|view| {
            Ok(BluetoothDevice {
                id: required(field(view, "Id")?, "Id")?,
                address: required(field(view, "Address")?, "Address")?,
                name: field(view, "Name")?,
                paired: required(field(view, "Paired")?, "Paired")?,
                connected: required(field(view, "Connected")?, "Connected")?,
            })
        })
        .collect::<zbus::Result<_>>()?;
    Ok(Some(BluetoothState { powered, devices }))
}

async fn read_focus(conn: &Connection) -> zbus::Result<Option<FocusState>> {
    let centre = proxy(conn, CENTRE_NAME, CENTRE_PATH, CENTRE_NAME).await?;
    let active: bool = centre.get_property("DoNotDisturb").await?;
    let rows: Vec<(String, String)> = centre.get_property("FocusLifetimes").await?;
    Ok(Some(FocusState {
        active,
        lifetimes: rows
            .into_iter()
            .map(|(id, title)| FocusLifetime { id, title })
            .collect(),
    }))
}

/// The output a panel is open on (its connector name, empty while the panel does not know it), read from its
/// `shown` and `output` properties; `None` while it is closed.
async fn read_panel(
    conn: &Connection,
    name: &'static str,
    path: &'static str,
    shown: &'static str,
    output: &'static str,
) -> zbus::Result<Option<String>> {
    let panel = proxy(conn, name, path, name).await?;
    if !panel.get_property::<bool>(shown).await? {
        return Ok(None);
    }
    Ok(Some(panel.get_property::<String>(output).await?))
}

async fn act(
    system: Option<&Connection>,
    session: &Connection,
    action: &ExtraAction,
) -> zbus::Result<()> {
    let system = || system.ok_or_else(|| zbus::Error::Failure("there is no system bus".to_owned()));
    match action {
        // the server asks the session agent for the menu bar (`SessionAction` over the shell protocol)
        ExtraAction::Session(_) => Err(zbus::Error::Failure(
            "session actions go through the server".to_owned(),
        )),
        ExtraAction::System(_) => Err(zbus::Error::Failure(
            "system menu actions are carried out by the bar".to_owned(),
        )),
        ExtraAction::SetDoNotDisturb(on) => {
            proxy(session, CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
                .await?
                .call("SetDoNotDisturb", &(*on,))
                .await
        }
        ExtraAction::DoNotDisturbFor(id) => {
            proxy(session, CENTRE_NAME, CENTRE_PATH, CENTRE_NAME)
                .await?
                .call("SetDoNotDisturbFor", &(id,))
                .await
        }
        ExtraAction::SetWifiEnabled(on) => {
            Ok(proxy(system()?, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME)
                .await?
                .set_property("WirelessEnabled", *on)
                .await?)
        }
        ExtraAction::JoinNetwork(ssid) => {
            proxy(system()?, NETWORK_NAME, NETWORK_PATH, NETWORK_NAME)
                .await?
                .call("Connect", &(ssid, ""))
                .await
        }
        ExtraAction::SetBluetoothPowered(on) => {
            Ok(
                proxy(system()?, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
                    .await?
                    .set_property("Powered", *on)
                    .await?,
            )
        }
        ExtraAction::ConnectDevice(id) => {
            proxy(system()?, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
                .await?
                .call("Connect", &(id,))
                .await
        }
        ExtraAction::DisconnectDevice(id) => {
            proxy(system()?, BLUETOOTH_NAME, BLUETOOTH_PATH, BLUETOOTH_NAME)
                .await?
                .call("Disconnect", &(id,))
                .await
        }
    }
}

/// The system menu's session entries from the agent's `Capabilities`. A capability the agent does not report is not
/// offered (drawn disabled). A map that could not be read (the agent still starting, a timeout) is no answer: the
/// entries stay as the last read gave them (`previous`), and with none yet every entry is offered, so Lock and Log Out
/// stay reachable; the agent and logind still refuse what is not allowed.
fn session_caps(
    capabilities: Option<&HashMap<String, bool>>,
    previous: Option<SessionCaps>,
) -> SessionCaps {
    let Some(capabilities) = capabilities else {
        return previous.unwrap_or_default();
    };
    let can = |key: &str| capabilities.get(key).copied().unwrap_or(false);
    SessionCaps {
        sleep: can("sleep"),
        restart: can("restart"),
        shut_down: can("shut-down"),
        log_out: can("log-out"),
        switch_user: can("switch-user"),
        lock: can("lock"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_of_refreshes_waits_as_one_and_a_full_queue_drops_actions_not_the_agent() {
        let (queue, rx) = Queue::new();
        for _ in 0..10_000 {
            queue.refresh().unwrap();
        }
        assert_eq!(rx.len(), 1, "one refresh waits for the whole burst");
        // the task takes it: the next signal queues another
        assert!(matches!(rx.try_recv(), Ok(Command::Refresh)));
        queue.took_refresh();
        queue.refresh().unwrap();
        assert_eq!(rx.len(), 1);

        // a stuck task: the queue fills with actions, and stays at its bound
        for _ in 0..QUEUE * 4 {
            queue.act(ExtraAction::SetWifiEnabled(false)).unwrap();
        }
        assert_eq!(rx.len(), QUEUE);
        // a refresh finds it full and is not needed: the queued commands read after them
        queue.took_refresh();
        queue.refresh().unwrap();
        assert_eq!(rx.len(), QUEUE);
        // the agent is never dropped: the next read takes the newest
        queue.set_agent(SessionAgent::Process(7)).unwrap();
        queue.set_agent(SessionAgent::Any).unwrap();
        assert_eq!(queue.agent(), SessionAgent::Any);

        // the task is gone: every call says so
        drop(rx);
        assert!(queue.act(ExtraAction::SetWifiEnabled(true)).is_err());
        queue.took_refresh();
        assert!(queue.refresh().is_err());
    }

    #[test]
    fn an_unreported_capability_is_off_but_an_unreadable_map_offers_everything() {
        let reported: HashMap<String, bool> = [("lock".to_string(), true)].into();
        let caps = session_caps(Some(&reported), None);
        assert!(caps.lock && !caps.log_out && !caps.shut_down);
        assert_eq!(session_caps(None, None), SessionCaps::default());
        assert_eq!(
            session_caps(None, Some(caps)),
            caps,
            "a failed read keeps what the last one gave"
        );
    }
}
