//! The services behind the bar's own status items: `org.cupertino.Network1` (`cupertino-netd`), the UPower display
//! device and manager (`cupertino-powerd`), `org.cupertino.Bluetooth1` (`cupertino-btd`) on the system bus, and the
//! do-not-disturb state of `org.cupertino.NotificationCenter1` (`cupertino-notifications`) on the session bus.
//!
//! One task reads every service into an [`Extras`] and sends it to the process whenever it differs from the last one
//! sent. It reads again after any signal on the services' objects (property changes, the network list, devices), when a
//! service's bus name changes owner (a daemon that starts later is picked up), on [`ExtrasBus::refresh`], and after
//! every action. Properties are read from the service every time, never from a cache. A service that cannot be read
//! hides its item; why is reported on stderr once per new reason.

use crate::extras::{
    BatteryState, BluetoothDevice, BluetoothState, ExtraAction, Extras, FocusLifetime, FocusState,
    SessionCaps, SessionState, WifiNetwork, WifiState,
};
use futures_util::StreamExt;
use std::collections::HashMap;
use std::future::Future;
use std::sync::mpsc;
use std::sync::Arc;
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

/// `org.freedesktop.UPower.Device.State` of a charging battery.
const UPOWER_STATE_CHARGING: u32 = 1;

/// A row of `Devices` of `org.cupertino.Network1`: index, name, kind, state, SSID, failure, address.
type DeviceRecord = (u32, String, String, String, String, String, String);
/// A row of `GetNetworks`: SSID, signal (dBm, `i32::MIN` when unknown), security, saved, connected.
type NetworkRecord = (String, i32, String, bool, bool);

enum Command {
    Refresh,
    Act(ExtraAction),
    Agent(SessionAgent),
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

/// Handle on the extras task.
pub struct ExtrasBus {
    commands: async_channel::Sender<Command>,
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
        let (commands, rx) = async_channel::unbounded();
        let (out, events) = mpsc::channel();
        let executor = session.executor().clone();
        executor
            .spawn(
                run(session, system, commands.clone(), rx, out, Arc::new(wake)),
                "cupertino-extras",
            )
            .detach();
        (ExtrasBus { commands }, events)
    }

    /// Read every service again (the duration rows change with the hour).
    pub fn refresh(&self) -> Result<(), WorkerGone> {
        self.commands
            .try_send(Command::Refresh)
            .map_err(|_| WorkerGone)
    }

    /// Name the process that is the session agent; the system menu is read again.
    pub fn session_agent(&self, agent: SessionAgent) -> Result<(), WorkerGone> {
        self.commands
            .try_send(Command::Agent(agent))
            .map_err(|_| WorkerGone)
    }

    pub fn act(&self, action: ExtraAction) -> Result<(), WorkerGone> {
        self.commands
            .try_send(Command::Act(action))
            .map_err(|_| WorkerGone)
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
}

async fn run(
    session: Connection,
    system: impl Future<Output = zbus::Result<Connection>>,
    commands: async_channel::Sender<Command>,
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
            follow(system, name, path, &commands).await;
        }
    }
    follow(&session, CENTRE_NAME, CENTRE_PATH, &commands).await;
    follow(&session, LOGIN_NAME, LOGIN_PATH, &commands).await;

    let mut reasons = Reasons::default();
    let mut last: Option<Extras> = None;
    let mut agent = SessionAgent::None;
    loop {
        let previous = last
            .as_ref()
            .and_then(|e| e.session.as_ref())
            .map(|s| s.caps);
        let extras = read_all(system.as_ref(), &session, agent, previous, &mut reasons).await;
        if last.as_ref() != Some(&extras) {
            // a closed channel means the process is shutting down
            if out.send(extras.clone()).is_err() {
                return;
            }
            wake();
            last = Some(extras);
        }
        match rx.recv().await {
            Ok(Command::Refresh) => {}
            Ok(Command::Agent(named)) => agent = named,
            Ok(Command::Act(action)) => {
                if let Err(e) = act(system.as_ref(), &session, &action).await {
                    eprintln!("cupertino-menubar: {action:?} failed: {e}");
                }
            }
            Err(_) => return,
        }
        // collapse a burst of signals into one read
        while let Ok(command) = rx.try_recv() {
            match command {
                Command::Act(action) => {
                    if let Err(e) = act(system.as_ref(), &session, &action).await {
                        eprintln!("cupertino-menubar: {action:?} failed: {e}");
                    }
                }
                Command::Agent(named) => agent = named,
                Command::Refresh => {}
            }
        }
    }
}

/// Queue a refresh for every signal on `path` and every owner change of `name`.
async fn follow(
    conn: &Connection,
    name: &'static str,
    path: &'static str,
    commands: &async_channel::Sender<Command>,
) {
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
        let commands = commands.clone();
        conn.executor()
            .spawn(
                async move {
                    while stream.next().await.is_some() {
                        if commands.send(Command::Refresh).await.is_err() {
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
