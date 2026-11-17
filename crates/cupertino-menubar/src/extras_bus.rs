//! The services behind the bar's own status items: the network and the Bluetooth adapter through whichever service
//! owns them (`cupertino-radios`: `cupertino-netd` and `cupertino-btd`, else NetworkManager and BlueZ, the discovery
//! and readings Settings and the control panel use), the UPower display device and manager (`cupertino-powerd`) on
//! the system bus, and the do-not-disturb state of `org.cupertino.NotificationCenter1` (`cupertino-notifications`) on
//! the session bus. It also reads whether the control panel (`org.cupertino.ControlPanel1`,
//! `cupertino-controlcenter`) and the notification centre's list are open and on which output, so the bar highlights
//! the item that opened them.
//!
//! One task reads every service into an [`Extras`] and sends it to the process whenever it differs from the last one
//! sent. It reads again after any signal on the services' objects (property changes, the network list, devices), when a
//! service's bus name changes owner (a daemon that starts later is picked up), on [`ExtrasBus::refresh`], and after
//! every action. Properties are read from the service every time, never from a cache. A service that cannot be read
//! hides its item; why is reported on stderr once per new reason.
//!
//! The radios are read and changed with calls that wait for their answers, each on a thread of its own, so the
//! task's executor, which also carries the session bus, never waits for the system bus. Each radio has one reading
//! under way at a time ([`Reader`]); a request that comes meanwhile is kept, the newest one, and read when that one
//! ends. The task waits a moment for a reading it started ([`Waits::fresh`]), which services that answer as they
//! should need less than; a reading that takes longer leaves the item as it was and is taken when it ends, and one
//! still unanswered after [`Waits::silent`] hides its item until it ends. The other items are read meanwhile.
//!
//! A signal of a radio's service has that radio read again, not the other. On NetworkManager the bar's reading is the
//! radio and its device alone; the networks in range are read when that reading differs from what the bar shows,
//! when the Wi-Fi menu opens (it asks for a scan), once more when the scan has had time to end, and with every
//! change for [`LIST_FRESH`] after that.
//!
//! The task's queue is bounded ([`QUEUE`]). Any peer on a bus can send this process signals, so a refresh is queued
//! only when none waits already: the read that a waiting refresh causes starts after the new signal arrived, so it
//! sees what the signal announced. The session agent is a value the next read takes, not a queued command, so a
//! newer agent replaces an older one and is never lost. An action waits in the queue; should the queue be full (the
//! task is stuck on a service that does not answer), the action is dropped and reported rather than queued without
//! bound.

use crate::extras::{
    BatteryState, BluetoothDevice, BluetoothState, ExtraAction, Extras, FocusLifetime, FocusState,
    OpenPanels, Passphrase, SessionCaps, SessionState, WifiLink, WifiNetwork, WifiState,
};
use cupertino_radios::watch::Radio;
use cupertino_radios::{BluetoothBackend, NetworkBackend};
use futures_util::future::{select, Either};
use futures_util::StreamExt;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zbus::message::Type;
use zbus::names::UniqueName;
use zbus::proxy::{Builder, CacheProperties};
use zbus::{Connection, MatchRule, MessageStream, Proxy};

use crate::worker::WorkerGone;

const LOGIN_NAME: &str = "org.cupertino.Login1";
const LOGIN_PATH: &str = "/org/cupertino/Login1";
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

/// How many commands may wait for the task: at most one refresh, the rest actions.
pub const QUEUE: usize = 16;

/// How long the networks in range are kept current after the Wi-Fi menu asked for them.
pub const LIST_FRESH: Duration = Duration::from_secs(30);
/// How long after a scan was asked for the networks in range are read once more: a scan ends without a signal.
pub const SCAN_TIME: Duration = Duration::from_secs(4);

/// How long the task waits for the radios.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Waits {
    /// For a reading it just started, so the items show the radios as they are now.
    pub fresh: Duration,
    /// After this long a reading that is still under way hides its item: its service does not answer.
    pub silent: Duration,
}

impl Default for Waits {
    fn default() -> Self {
        Waits {
            fresh: Duration::from_millis(150),
            silent: cupertino_radios::CALL_TIMEOUT,
        }
    }
}

/// What of the radios is due for a reading ([`Queue::due`]).
const DUE_NETWORK: u8 = 1;
const DUE_BLUETOOTH: u8 = 2;
/// The networks in range are wanted with the network's reading.
const DUE_LIST: u8 = 4;

enum Command {
    Refresh,
    Act(ExtraAction),
    /// Join a network with a passphrase.
    Join {
        ssid: String,
        passphrase: Passphrase,
    },
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
    /// The radios whose reading is out of date (`DUE_NETWORK`, `DUE_BLUETOOTH`, `DUE_LIST`), until the task takes
    /// them.
    due: Arc<AtomicU8>,
    /// The session agent the next read takes.
    agent: Arc<Mutex<SessionAgent>>,
}

impl Queue {
    fn new() -> (Queue, async_channel::Receiver<Command>) {
        let (commands, rx) = async_channel::bounded(QUEUE);
        let queue = Queue {
            commands,
            refresh_waiting: Arc::new(AtomicBool::new(false)),
            due: Arc::new(AtomicU8::new(0)),
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

    /// Note that the radios of `due` are out of date, and ask for a read.
    fn radios_due(&self, due: u8) -> Result<(), WorkerGone> {
        self.due.fetch_or(due, Ordering::AcqRel);
        self.refresh()
    }

    /// The radios that became due since the last call.
    fn take_due(&self) -> u8 {
        self.due.swap(0, Ordering::AcqRel)
    }

    /// The task took a refresh: the next signal queues another.
    fn took_refresh(&self) {
        self.refresh_waiting.store(false, Ordering::Release);
    }

    /// Queue a join with a passphrase; the passphrase is not printed when it is dropped. Fails only when the task is
    /// gone.
    fn join(&self, ssid: String, passphrase: Passphrase) -> Result<(), WorkerGone> {
        match self.commands.try_send(Command::Join { ssid, passphrase }) {
            Ok(()) => Ok(()),
            Err(async_channel::TrySendError::Full(_)) => Ok(()),
            Err(async_channel::TrySendError::Closed(_)) => Err(WorkerGone),
        }
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
        Self::with_waits(session, system, Waits::default(), wake)
    }

    /// [`ExtrasBus::with_system`] waiting for the radios as `waits` says.
    pub fn with_waits(
        session: Connection,
        system: impl Future<Output = zbus::Result<Connection>> + Send + 'static,
        waits: Waits,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (ExtrasBus, mpsc::Receiver<Extras>) {
        let (queue, rx) = Queue::new();
        let (out, events) = mpsc::channel();
        let executor = session.executor().clone();
        executor
            .spawn(
                run(
                    session,
                    system,
                    waits,
                    queue.clone(),
                    rx,
                    out,
                    Arc::new(wake),
                ),
                "cupertino-extras",
            )
            .detach();
        (ExtrasBus { queue }, events)
    }

    /// Read every service again (the duration rows change with the hour).
    pub fn refresh(&self) -> Result<(), WorkerGone> {
        self.queue.radios_due(DUE_NETWORK | DUE_BLUETOOTH)
    }

    /// Name the process that is the session agent; the system menu is read again.
    pub fn session_agent(&self, agent: SessionAgent) -> Result<(), WorkerGone> {
        self.queue.set_agent(agent)
    }

    /// Join the network `ssid` with `passphrase`. The outcome shows in [`WifiLink`]: the device's state, its failure,
    /// and what the service answered when it refused the request.
    pub fn join(&self, ssid: String, passphrase: Passphrase) -> Result<(), WorkerGone> {
        self.queue.join(ssid, passphrase)
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
    waits: Waits,
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
            (UPOWER_NAME, UPOWER_PATH),
            (UPOWER_NAME, UPOWER_DISPLAY_DEVICE_PATH),
        ] {
            follow(system, name, path, &queue).await;
        }
        let radios = queue.clone();
        let followed = cupertino_radios::watch::follow(
            system,
            Arc::new(move |radio| {
                let _ = radios.radios_due(match radio {
                    Radio::Network => DUE_NETWORK,
                    Radio::Bluetooth => DUE_BLUETOOTH,
                });
            }),
        )
        .await;
        if let Err(e) = followed {
            eprintln!("cupertino-menubar: cannot follow the network and Bluetooth services: {e}");
        }
    }
    // the radios' calls wait for their answers, so they run on threads of their own with this handle
    let radios: Option<zbus::blocking::Connection> = system.clone().map(Into::into);
    follow(&session, CENTRE_NAME, CENTRE_PATH, &queue).await;
    follow(&session, PANEL_NAME, PANEL_PATH, &queue).await;
    follow(&session, LOGIN_NAME, LOGIN_PATH, &queue).await;

    let mut reasons = Reasons::default();
    let mut last: Option<Extras> = None;
    // what the service answered to the last join the bar asked for, when it refused
    let mut refused: (u32, Option<String>) = (0, None);
    let mut reads = radios.clone().map(|system| RadioReads::new(system, waits));
    // the first reading is of everything
    queue
        .due
        .fetch_or(DUE_NETWORK | DUE_BLUETOOTH, Ordering::AcqRel);
    loop {
        let previous = last
            .as_ref()
            .and_then(|e| e.session.as_ref())
            .map(|s| s.caps);
        if let Some(reads) = reads.as_mut() {
            reads.read(&queue).await;
        }
        let extras = read_all(
            system.as_ref(),
            reads.as_ref(),
            &session,
            queue.agent(),
            previous,
            &refused,
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
        // a reading under way that hides its item when it is not answered, or the list due after a scan, wakes
        // the task without a command
        let wake_at = reads.as_ref().and_then(RadioReads::next_wake);
        let command = match wake_at {
            None => rx.recv().await,
            Some(at) => match select(pin!(rx.recv()), pin!(async_io::Timer::at(at))).await {
                Either::Left((command, _)) => command,
                Either::Right(_) => continue,
            },
        };
        let Ok(command) = command else {
            return;
        };
        // collapse a burst of commands into one read
        for command in std::iter::once(command).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            match command {
                Command::Act(action) => {
                    if let Err(e) = act(radios.as_ref(), &session, &action, &queue).await {
                        eprintln!("cupertino-menubar: {action:?} failed: {e}");
                    }
                }
                Command::Join { ssid, passphrase } => {
                    refused = (
                        refused.0 + 1,
                        join(radios.as_ref(), ssid, passphrase).await.err(),
                    );
                    queue.due.fetch_or(DUE_NETWORK, Ordering::AcqRel);
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
    reads: Option<&RadioReads>,
    session: &Connection,
    agent: SessionAgent,
    previous: Option<SessionCaps>,
    refused: &(u32, Option<String>),
    reasons: &mut Reasons,
) -> Extras {
    let mut extras = Extras {
        lock_without_agent: agent != SessionAgent::None,
        ..Extras::default()
    };
    if let Some(system) = system {
        extras.battery = reasons.note("battery", absent_if_not_running(read_battery(system).await));
    }
    if let Some(reads) = reads {
        extras.wifi = reasons.note("Wi-Fi", reads.wifi());
        extras.wifi_link = reads.link();
        extras.wifi_link.refused = refused.1.clone();
        extras.wifi_link.joins = refused.0;
        extras.bluetooth = reasons.note("Bluetooth", reads.bluetooth());
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

/// Run `f`, which waits for the system bus, on a thread of its own, and wait for it without holding the executor.
async fn off_thread<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (done, result) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("cupertino-extras-radios".into())
        .spawn(move || {
            let _ = done.send_blocking(f());
        })
        .map_err(|e| format!("cannot start a thread for the system bus: {e}"))?;
    result
        .recv()
        .await
        .map_err(|_| "the thread for the system bus ended without an answer".to_owned())
}

/// One reading at a time on a thread of its own.
struct Reader<T> {
    /// The reading under way: where its result arrives, and when it started.
    under_way: Option<(async_channel::Receiver<T>, Instant)>,
    /// The task waits for the reading under way: it takes the result itself, so the thread need not wake it.
    awaited: Arc<AtomicBool>,
}

impl<T: Send + 'static> Reader<T> {
    fn new() -> Self {
        Reader {
            under_way: None,
            awaited: Arc::new(AtomicBool::new(false)),
        }
    }

    fn busy(&self) -> bool {
        self.under_way.is_some()
    }

    /// Start `read` on a thread; the task is woken through `queue` when it ends, unless it waits for it then.
    fn start(&mut self, queue: &Queue, read: impl FnOnce() -> T + Send + 'static) {
        let (done, result) = async_channel::bounded(1);
        let (queue, awaited) = (queue.clone(), self.awaited.clone());
        let started = std::thread::Builder::new()
            .name("cupertino-extras-radios".into())
            .spawn(move || {
                let _ = done.send_blocking(read());
                if !awaited.load(Ordering::Acquire) {
                    let _ = queue.refresh();
                }
            });
        match started {
            Ok(_) => self.under_way = Some((result, Instant::now())),
            Err(e) => eprintln!("cupertino-menubar: cannot start a thread for the system bus: {e}"),
        }
    }

    /// The result of the reading under way, waiting `wait` for it; `None` while it has not ended.
    async fn take(&mut self, wait: Duration) -> Option<T> {
        let (result, _) = self.under_way.as_ref()?;
        let mut taken = result.try_recv().ok();
        if taken.is_none() && !wait.is_zero() {
            self.awaited.store(true, Ordering::Release);
            if let Either::Left((Ok(value), _)) =
                select(pin!(result.recv()), pin!(async_io::Timer::after(wait))).await
            {
                taken = Some(value);
            }
            self.awaited.store(false, Ordering::Release);
            // a reading that ended between the wait and the flag woke nobody
            taken = taken.or_else(|| result.try_recv().ok());
        }
        // a thread that ended without a result (it panicked) is no longer under way either
        if taken.is_some() || result.is_closed() {
            self.under_way = None;
        }
        taken
    }

    /// When the reading under way, unanswered until then, counts as silent.
    fn silent_at(&self, after: Duration) -> Option<Instant> {
        self.under_way.as_ref().map(|(_, started)| *started + after)
    }
}

/// What a reading gave: the item's state (`None` without a service or the radio), or why it could not be read.
type Read<T> = Result<Option<T>, String>;

/// What the bar shows of the network: the item's state and the Wi-Fi device's.
#[derive(Clone, Default)]
struct NetworkRead {
    wifi: Option<WifiState>,
    link: WifiLink,
}

/// What a light reading compares: the radio's switch and its device's state, network and failure.
type LinkKey = (bool, String, String, String);

impl NetworkRead {
    fn key(&self) -> Option<LinkKey> {
        let wifi = self.wifi.as_ref()?;
        Some((
            wifi.enabled,
            self.link.state.clone(),
            self.link.ssid.clone(),
            self.link.failure.clone(),
        ))
    }
}

/// The radios' readings: the last of each, the one under way, and what was asked for meanwhile.
struct RadioReads {
    system: zbus::blocking::Connection,
    waits: Waits,
    network: Reader<Option<Result<NetworkRead, String>>>,
    bluetooth: Reader<Read<BluetoothState>>,
    /// A reading of the network is due, and whether with the networks in range: the newest request, kept while one
    /// is under way.
    network_due: Option<bool>,
    bluetooth_due: bool,
    last_network: Option<Result<NetworkRead, String>>,
    last_bluetooth: Option<Read<BluetoothState>>,
    /// Until when every reading of the network takes the networks in range.
    list_until: Option<Instant>,
    /// When the networks in range are read once more, after a scan.
    list_again: Option<Instant>,
}

impl RadioReads {
    fn new(system: zbus::blocking::Connection, waits: Waits) -> Self {
        RadioReads {
            system,
            waits,
            network: Reader::new(),
            bluetooth: Reader::new(),
            network_due: None,
            bluetooth_due: false,
            last_network: None,
            last_bluetooth: None,
            list_until: None,
            list_again: None,
        }
    }

    /// Take the readings that ended, start the ones that are due, and wait a moment for those.
    async fn read(&mut self, queue: &Queue) {
        let now = Instant::now();
        let due = queue.take_due();
        if due & DUE_LIST != 0 {
            self.list_until = Some(now + LIST_FRESH);
            self.list_again = Some(now + SCAN_TIME);
        }
        let mut list = due & DUE_LIST != 0;
        if self.list_again.is_some_and(|at| at <= now) {
            self.list_again = None;
            list = true;
        }
        if list || due & DUE_NETWORK != 0 {
            let listed = list || self.list_until.is_some_and(|until| now < until);
            self.network_due = Some(self.network_due.unwrap_or(false) | listed);
        }
        self.bluetooth_due |= due & DUE_BLUETOOTH != 0;

        if let Some(read) = self.network.take(Duration::ZERO).await {
            self.took_network(read);
        }
        if let Some(read) = self.bluetooth.take(Duration::ZERO).await {
            self.last_bluetooth = Some(read);
        }
        let mut started = (false, false);
        if !self.network.busy() {
            if let Some(list) = self.network_due.take() {
                let system = self.system.clone();
                let shown = self
                    .last_network
                    .as_ref()
                    .and_then(|last| last.as_ref().ok())
                    .and_then(NetworkRead::key)
                    .filter(|_| !list);
                self.network
                    .start(queue, move || read_network(&system, shown.as_ref()));
                started.0 = self.network.busy();
            }
        }
        if !self.bluetooth.busy() && std::mem::take(&mut self.bluetooth_due) {
            let system = self.system.clone();
            self.bluetooth.start(queue, move || read_bluetooth(&system));
            started.1 = self.bluetooth.busy();
        }
        if started.0 {
            if let Some(read) = self.network.take(self.waits.fresh).await {
                self.took_network(read);
            }
        }
        if started.1 {
            // what is left of the moment the network's reading was given
            let left = self.waits.fresh.saturating_sub(now.elapsed());
            if let Some(read) = self
                .bluetooth
                .take(left.max(Duration::from_millis(1)))
                .await
            {
                self.last_bluetooth = Some(read);
            }
        }
    }

    /// A reading of the network ended: `None` when the radio and its device are as the bar shows them.
    fn took_network(&mut self, read: Option<Result<NetworkRead, String>>) {
        if let Some(read) = read {
            self.last_network = Some(read);
        }
    }

    /// When the task has something to do without a command: a reading under way turns silent, or the networks in
    /// range are due once more.
    fn next_wake(&self) -> Option<Instant> {
        let now = Instant::now();
        [
            self.network.silent_at(self.waits.silent),
            self.bluetooth.silent_at(self.waits.silent),
        ]
        .into_iter()
        .flatten()
        // one that is silent already has hidden its item
        .filter(|at| *at > now)
        .chain(self.list_again)
        .min()
    }

    fn silent<T: Send + 'static>(&self, reader: &Reader<T>) -> bool {
        reader
            .silent_at(self.waits.silent)
            .is_some_and(|at| at <= Instant::now())
    }

    /// The Wi-Fi item: the last reading, hidden while the service does not answer.
    fn wifi(&self) -> zbus::Result<Option<WifiState>> {
        if self.silent(&self.network) {
            return Err(zbus::Error::Failure(
                "the network service does not answer".to_owned(),
            ));
        }
        match &self.last_network {
            None => Ok(None),
            Some(Ok(read)) => Ok(read.wifi.clone()),
            Some(Err(e)) => Err(zbus::Error::Failure(e.clone())),
        }
    }

    /// The Wi-Fi device's state; empty when nothing answers.
    fn link(&self) -> WifiLink {
        match &self.last_network {
            Some(Ok(read)) if !self.silent(&self.network) => read.link.clone(),
            _ => WifiLink::default(),
        }
    }

    /// The Bluetooth item: the last reading, hidden while the service does not answer.
    fn bluetooth(&self) -> zbus::Result<Option<BluetoothState>> {
        if self.silent(&self.bluetooth) {
            return Err(zbus::Error::Failure(
                "the Bluetooth service does not answer".to_owned(),
            ));
        }
        match &self.last_bluetooth {
            None => Ok(None),
            Some(Ok(read)) => Ok(read.clone()),
            Some(Err(e)) => Err(zbus::Error::Failure(e.clone())),
        }
    }
}

/// A reading's failure as the item's reason; a service that is not running hides its item without one.
fn reason<T: Default>(error: zbus::Error) -> Result<T, String> {
    if not_running(&error) {
        Ok(T::default())
    } else if cupertino_radios::unanswered(&error) {
        Err("its service does not answer".to_owned())
    } else {
        Err(error.to_string())
    }
}

/// Read the network from the service that owns it now. On NetworkManager the radio and its device are read first,
/// and when they are as `shown` (what the bar shows) the networks in range are left as they are: `None`. Without
/// `shown` the networks are read too.
fn read_network(
    system: &zbus::blocking::Connection,
    shown: Option<&LinkKey>,
) -> Option<Result<NetworkRead, String>> {
    let backend = match cupertino_radios::detect(system) {
        Ok(found) => found.network,
        Err(e) => return Some(reason(e)),
    };
    let Some(backend) = backend else {
        return Some(Ok(NetworkRead::default()));
    };
    if let (NetworkBackend::NetworkManager, Some(shown)) = (backend, shown) {
        match backend.link(system) {
            Ok(link) => {
                let key = link.device.as_ref().map(|device| {
                    (
                        link.enabled,
                        device.state.clone(),
                        device.ssid.clone(),
                        device.failure.clone(),
                    )
                });
                if key.as_ref() == Some(shown) {
                    return None;
                }
            }
            Err(e) => return Some(reason(e)),
        }
    }
    let wifi = match backend.wifi(system) {
        Ok(wifi) => wifi,
        Err(e) => return Some(reason(e)),
    };
    let mut link = WifiLink::default();
    let device = wifi.devices.iter().find(|device| device.kind == "wifi");
    if let Some(device) = device {
        link.state = device.state.clone();
        link.ssid = device.ssid.clone();
        link.failure = device.failure.clone();
    }
    let state = device.is_some().then(|| WifiState {
        enabled: wifi.enabled,
        networks: wifi
            .networks
            .into_iter()
            // a network whose name is not text has no label here; Settings lists it
            .filter(|network| !network.ssid.starts_with('\0'))
            .map(|network| WifiNetwork {
                ssid: network.ssid,
                signal_dbm: (network.signal != i32::MIN).then_some(network.signal),
                security: network.security,
                known: network.saved,
                connected: network.connected,
            })
            .collect(),
    });
    Some(Ok(NetworkRead { wifi: state, link }))
}

/// Read the Bluetooth adapter from the service that owns it now.
fn read_bluetooth(system: &zbus::blocking::Connection) -> Read<BluetoothState> {
    let backend = match cupertino_radios::detect(system) {
        Ok(found) => found.bluetooth,
        Err(e) => return reason(e),
    };
    let Some(backend) = backend else {
        return Ok(None);
    };
    match backend.adapter(system, &HashSet::new()) {
        Err(e) => reason(e),
        Ok(adapter) => Ok(adapter.available.then(|| BluetoothState {
            powered: adapter.powered,
            devices: adapter
                .devices
                .into_iter()
                .map(|device| BluetoothDevice {
                    id: device.id,
                    address: device.address,
                    name: device.name,
                    paired: device.paired,
                    connected: device.connected,
                })
                .collect(),
        })),
    }
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

fn no_system_bus() -> zbus::Error {
    zbus::Error::Failure("there is no system bus".to_owned())
}

fn no_service(what: &str) -> zbus::Error {
    zbus::Error::Failure(format!("no {what} service is running"))
}

/// The network service of `system` now.
fn network(system: &zbus::blocking::Connection) -> zbus::Result<NetworkBackend> {
    cupertino_radios::detect(system)?
        .network
        .ok_or_else(|| no_service("network"))
}

/// The Bluetooth service of `system` now.
fn bluetooth(system: &zbus::blocking::Connection) -> zbus::Result<BluetoothBackend> {
    cupertino_radios::detect(system)?
        .bluetooth
        .ok_or_else(|| no_service("Bluetooth"))
}

/// Ask the service to join `ssid` with `passphrase`; the text of its refusal when it refuses.
async fn join(
    system: Option<&zbus::blocking::Connection>,
    ssid: String,
    passphrase: Passphrase,
) -> Result<(), String> {
    let system = system
        .ok_or_else(|| "There is no connection to the network service.".to_owned())?
        .clone();
    off_thread(move || {
        network(&system)
            .and_then(|backend| backend.join(&system, &ssid, passphrase.as_str()))
            .map_err(|e| cupertino_radios::refusal(&e))
    })
    .await?
}

/// Carry out `call` on a thread of its own without waiting for its answer, so the bar's other items are read
/// meanwhile; a failure is reported, and the radios of `due` are read again when it ends.
fn on_radios(
    system: Option<&zbus::blocking::Connection>,
    queue: &Queue,
    due: u8,
    what: &'static str,
    call: impl FnOnce(&zbus::blocking::Connection) -> zbus::Result<()> + Send + 'static,
) -> zbus::Result<()> {
    let (system, queue) = (system.ok_or_else(no_system_bus)?.clone(), queue.clone());
    std::thread::Builder::new()
        .name("cupertino-extras-radios".into())
        .spawn(move || {
            if let Err(e) = call(&system) {
                eprintln!(
                    "cupertino-menubar: cannot {what}: {}",
                    cupertino_radios::refusal(&e)
                );
            }
            let _ = queue.radios_due(due);
        })
        .map(drop)
        .map_err(|e| zbus::Error::Failure(format!("cannot start a thread for the system bus: {e}")))
}

/// Connect or disconnect the Bluetooth device `id` without waiting: a service may answer only once the link is up or
/// has failed, which takes seconds, and the bar's other items are read meanwhile. The outcome is read when the
/// device's change signal comes and again when the call ends; a failure is reported.
fn link_device(
    system: Option<&zbus::blocking::Connection>,
    id: &str,
    connect: bool,
    queue: &Queue,
) -> zbus::Result<()> {
    let (system, id, queue) = (
        system.ok_or_else(no_system_bus)?.clone(),
        id.to_owned(),
        queue.clone(),
    );
    std::thread::Builder::new()
        .name("cupertino-extras-link".into())
        .spawn(move || {
            let done = bluetooth(&system).and_then(|backend| {
                if connect {
                    backend.connect(&system, &id)
                } else {
                    backend.disconnect(&system, &id)
                }
            });
            if let Err(e) = done {
                let action = if connect { "connect" } else { "disconnect" };
                eprintln!(
                    "cupertino-menubar: cannot {action} the Bluetooth device {id}: {}",
                    cupertino_radios::refusal(&e)
                );
            }
            let _ = queue.radios_due(DUE_BLUETOOTH);
        })
        .map(drop)
        .map_err(|e| zbus::Error::Failure(format!("cannot start the device's call: {e}")))
}

async fn act(
    system: Option<&zbus::blocking::Connection>,
    session: &Connection,
    action: &ExtraAction,
    queue: &Queue,
) -> zbus::Result<()> {
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
            let on = *on;
            // a radio switched on scans: the networks it finds are read when the scan has had time to end
            let due = if on {
                DUE_NETWORK | DUE_LIST
            } else {
                DUE_NETWORK
            };
            on_radios(system, queue, due, "switch Wi-Fi", move |c| {
                network(c)?.set_wireless_enabled(c, on)
            })
        }
        ExtraAction::ToggleWifiList | ExtraAction::PromptNetwork(_) => Ok(()),
        ExtraAction::CancelJoin => {
            on_radios(system, queue, DUE_NETWORK, "leave the network", |c| {
                network(c)?.disconnect(c)
            })
        }
        // the Wi-Fi menu opens: the networks in range are read now, and again when the scan has had time to end
        ExtraAction::ScanWifi => {
            let _ = queue.radios_due(DUE_NETWORK | DUE_LIST);
            on_radios(system, queue, 0, "scan for networks", |c| {
                network(c)?.scan(c)
            })
        }
        ExtraAction::JoinNetwork(ssid) => {
            let ssid = ssid.clone();
            on_radios(system, queue, DUE_NETWORK, "join the network", move |c| {
                network(c)?.join(c, &ssid, "")
            })
        }
        ExtraAction::SetBluetoothPowered(on) => {
            let on = *on;
            on_radios(system, queue, DUE_BLUETOOTH, "switch Bluetooth", move |c| {
                bluetooth(c)?.set_powered(c, on)
            })
        }
        ExtraAction::ConnectDevice(id) => link_device(system, id, true, queue),
        ExtraAction::DisconnectDevice(id) => link_device(system, id, false, queue),
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
