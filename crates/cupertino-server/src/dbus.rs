//! Session bus worker: relays the lock request (the lock screen's `org.cupertino.Login1`) to the process that owns it,
//! once it has checked that owner, on the connection that owns `org.cupertino.Session1`, the one caller the lock screen
//! takes it from (D20). The calls on the shell components' bus interfaces (the notification centre's, the launcher's and the control panel's
//! toggles, the on-screen displays' media keys) go from the connection that owns `org.cupertino.Session1` instead
//! ([`crate::quit::SessionConnection`]), the one connection each component takes for the server (D20), and so do the
//! session agent's requests ([`crate::session_agent`]).
//!
//! Everything runs on one background thread that drives `zbus` futures; the compositor talks to it
//! through [`DbusHandle::send`]. Without a session bus the worker ends after logging and the
//! requests fail with a warning.
//!
//! The worker also checks who owns a bus name for `crate::owner_check` ([`OwnerQuery`]). It asks only the bus daemon,
//! never the name itself.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use cupertino_peer::{same_instance, ProcessHandle};
use zbus::fdo::ConnectionCredentials;
use zbus::names::{BusName, OwnedUniqueName};

use crate::caller::{identify, Caller};
use crate::input::OsdKey;

/// What the server asks a shell component to toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// The notification centre list of the `notifications` component.
    NotificationCentre,
    /// The panel of the `launcher` component.
    Launcher,
    /// The application browser of the `launcher` component.
    Applications,
    /// The panel of the `controlcenter` component.
    ControlPanel,
}

/// One call the worker makes on a program the server started: the lock request on `cupertino-login`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Lock the session through the lock screen's `org.cupertino.Login1` while no lock screen is connected to ask
    /// over the server's own connection: `Lock`, its one method, for every lock request.
    Lock,
}

/// Arguments of a control method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    Empty,
    /// Direction (+1 or -1) and the fine flag of the overlay's step methods.
    Step(i32, bool),
}

/// Bus name, object path and interface of a shell component's bus interface.
type Interface = (&'static str, &'static str, &'static str);

/// The on-screen displays' interface (`cupertino_osd::control`).
const OSD: Interface = (
    "org.cupertino.Osd1",
    "/org/cupertino/Osd1",
    "org.cupertino.Osd1",
);

/// The notification centre's interface (`cupertino_notifications::control`).
const NOTIFICATION_CENTRE: Interface = (
    "org.cupertino.NotificationCenter1",
    "/org/cupertino/NotificationCenter1",
    "org.cupertino.NotificationCenter1",
);

/// The launcher's interface (`cupertino_launcher::control`).
const LAUNCHER: Interface = (
    "org.cupertino.Launcher1",
    "/org/cupertino/Launcher1",
    "org.cupertino.Launcher1",
);

/// The control panel's interface (`cupertino_controlcenter::control`).
const CONTROL_PANEL: Interface = (
    "org.cupertino.ControlPanel1",
    "/org/cupertino/ControlPanel1",
    "org.cupertino.ControlPanel1",
);

impl Toggle {
    /// The interface of the component that toggles, and the method.
    fn call(self) -> (Interface, &'static str) {
        match self {
            Toggle::NotificationCentre => (NOTIFICATION_CENTRE, "ToggleCentre"),
            Toggle::Launcher => (LAUNCHER, "Toggle"),
            Toggle::Applications => (LAUNCHER, "ToggleApplications"),
            Toggle::ControlPanel => (CONTROL_PANEL, "Toggle"),
        }
    }
}

/// The method and arguments of `org.cupertino.Osd1` for a media key; `fine` is the Shift+Alt step.
fn osd_call(key: OsdKey, fine: bool) -> (&'static str, Body) {
    match key {
        OsdKey::VolumeUp => ("VolumeStep", Body::Step(1, fine)),
        OsdKey::VolumeDown => ("VolumeStep", Body::Step(-1, fine)),
        OsdKey::Mute => ("ToggleMute", Body::Empty),
        OsdKey::MicMute => ("ToggleMicrophoneMute", Body::Empty),
        OsdKey::BrightnessUp => ("BrightnessStep", Body::Step(1, fine)),
        OsdKey::BrightnessDown => ("BrightnessStep", Body::Step(-1, fine)),
        OsdKey::KeyboardBrightnessUp => ("KeyboardBrightnessStep", Body::Step(1, fine)),
        OsdKey::KeyboardBrightnessDown => ("KeyboardBrightnessStep", Body::Step(-1, fine)),
    }
}

/// Calls `method` of the shell component's interface with `body` on `conn`, the connection that owns
/// `org.cupertino.Session1`, in a task of its executor, and logs a failure with `what`.
fn send_to_component(
    conn: &zbus::Connection,
    (name, path, interface): Interface,
    method: &'static str,
    body: Body,
    what: impl std::fmt::Debug + Send + 'static,
) {
    let conn = conn.clone();
    conn.executor()
        .clone()
        .spawn(
            async move {
                let sent = match body {
                    Body::Step(direction, fine) => {
                        conn.call_method(
                            Some(name),
                            path,
                            Some(interface),
                            method,
                            &(direction, fine),
                        )
                        .await
                    }
                    Body::Empty => {
                        conn.call_method(Some(name), path, Some(interface), method, &())
                            .await
                    }
                };
                if let Err(e) = sent {
                    tracing::warn!(?what, name, error = %e, "cannot reach the shell component");
                }
            },
            "cupertino-component-call",
        )
        .detach();
}

impl Request {
    /// Bus name, object path, interface, method and arguments of the call: the lock request goes to the login process.
    fn call(self) -> (&'static str, &'static str, &'static str, &'static str, Body) {
        match self {
            Request::Lock => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "Lock",
                Body::Empty,
            ),
        }
    }
}

/// Who may own `org.cupertino.Login1`: the lock request goes to nobody else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginTrust {
    /// No login process is supervised: no owner of the name is trusted.
    Nobody,
    /// The supervised login process, by its process id.
    Agent(u32),
    /// A development run: the owner of the name is whoever started it.
    Any,
}

impl crate::state::Cupertino {
    /// Whose ownership of `org.cupertino.Login1` is trusted now: the supervised process of the session-lock role.
    pub fn login_trust(&self) -> LoginTrust {
        if self.config.dev_shell {
            return LoginTrust::Any;
        }
        self.supervisor
            .pids_with_role(crate::authorization::ShellRole::SessionLock)
            .first()
            .map_or(LoginTrust::Nobody, |pid| LoginTrust::Agent(*pid))
    }
}

/// A check of the owner of a bus name. The worker asks the bus daemon who owns the name (`GetNameOwner` for a
/// well-known name) and for the owner's credentials, and answers the owner's unique name when it is one sandbox instance
/// with `client` ([`cupertino_peer::same_instance`]).
pub struct OwnerQuery {
    pub name: String,
    /// The Wayland client to match, held from its connection.
    pub client: ProcessHandle,
    /// How many identification threads of the client are alive (`crate::state::ClientState::owner_identifying`).
    pub client_identifying: Identifying,
    /// When the loop answers the check as timed out and drops its answer: a bus that answers after that starts no
    /// identification thread, so a client is not refused as stalled by its own checks the loop gave up on.
    pub expires: Instant,
    pub reply: Box<dyn FnOnce(Result<String, OwnerRefusal>) + Send>,
}

/// Why the owner of a bus name is not taken for the Wayland client.
#[derive(Debug)]
pub enum OwnerRefusal {
    /// No session bus to ask.
    NoBus,
    /// No process is known for the client: a kernel without `SO_PEERPIDFD`, or a connection the server made for a
    /// shell process that has not started (`crate::state::ClientState::process`).
    NoClientProcess,
    NotABusName,
    /// The daemon named no owner: nobody owns the name, or the connection is gone.
    NoOwner(String),
    /// The daemon gave no process descriptor for the owner, or the owner or the client could not be classified or is
    /// gone.
    Unidentified,
    /// The owner is a process outside the client's sandbox instance.
    OtherInstance {
        owner: u32,
        client: u32,
    },
    /// As many identification threads as [`IDENTIFY_LIMIT`] are alive (processes whose `/proc` entries stall reading).
    Busy,
    /// The check did not return in time (`crate::owner_check::DEADLINE`).
    TimedOut,
    /// As many identification threads of the client as [`CLIENT_IDENTIFY_LIMIT`] are alive: its checks are refused
    /// until one ends.
    Stalled,
}

/// What the worker is asked to do.
enum Job {
    /// Make the call on the connection given, after the owner check on the worker's own.
    Call(Request, zbus::Connection),
    Owner(OwnerQuery),
    WatchOwner(&'static str, OwnerChanged),
    /// Run the callback once the name has an owner.
    AwaitOwner(&'static str, Box<dyn FnOnce() + Send>),
    /// The session's variables, and defaults that apply only where the user manager has no value of its own.
    Publish(Vec<(String, String)>, Vec<(String, String)>),
}

/// Runs on a watch thread each time the owner of a watched name may have changed.
pub type OwnerChanged = Box<dyn Fn() + Send>;

/// Handle on the worker thread.
pub struct DbusHandle {
    tx: async_channel::Sender<Job>,
    login: std::sync::Arc<std::sync::Mutex<LoginTrust>>,
    calls: ShellCalls,
}

/// The calls on shell processes queued with [`DbusHandle::send`], and those the worker is done with (made, failed or
/// refused); a test reads them as a barrier on the worker.
#[derive(Clone, Default)]
struct ShellCalls(std::sync::Arc<(AtomicUsize, AtomicUsize)>);

impl DbusHandle {
    /// Start the worker.
    pub fn start() -> Option<DbusHandle> {
        Self::start_on(None)
    }

    /// Start the worker on the bus at `address`, the session bus when `None`.
    pub fn start_on(address: Option<String>) -> Option<DbusHandle> {
        let (tx, rx) = async_channel::unbounded::<Job>();
        let login = std::sync::Arc::new(std::sync::Mutex::new(LoginTrust::Nobody));
        let worker_login = login.clone();
        let identifying = std::sync::Arc::new(AtomicUsize::new(0));
        let calls = ShellCalls::default();
        let worker_calls = calls.clone();
        let spawned = std::thread::Builder::new()
            .name("cupertino-dbus".into())
            .spawn(move || {
                zbus::block_on(worker(address, rx, worker_login, identifying, worker_calls))
            });
        match spawned {
            Ok(_) => Some(DbusHandle { tx, login, calls }),
            Err(e) => {
                tracing::warn!(error = %e, "cannot start the D-Bus worker");
                None
            }
        }
    }

    /// How many calls on shell processes were queued, and how many of them the worker is done with.
    pub fn shell_calls(&self) -> (usize, usize) {
        let (queued, done) = &*self.calls.0;
        (queued.load(Ordering::SeqCst), done.load(Ordering::SeqCst))
    }

    /// Say whose ownership of `org.cupertino.Login1` the worker may trust.
    pub fn set_login(&self, trust: LoginTrust) {
        *self
            .login
            .lock()
            .expect("no thread panics while holding the login trust") = trust;
    }

    /// Queue a call on a shell process, made on `via` (the connection that owns `org.cupertino.Session1`); false when
    /// the worker is gone (no session bus).
    pub fn send(&self, what: Request, via: zbus::Connection) -> bool {
        // counted before the worker can be done with it, so the done count never runs ahead
        self.calls.0 .0.fetch_add(1, Ordering::SeqCst);
        let queued = self.tx.try_send(Job::Call(what, via)).is_ok();
        if !queued {
            self.calls.0 .0.fetch_sub(1, Ordering::SeqCst);
            tracing::warn!(?what, "no session bus: cannot reach the shell process");
        }
        queued
    }

    /// Queue a check of who owns a bus name; false when the worker is gone (no session bus), and the reply is then
    /// never made.
    pub fn check_owner(&self, query: OwnerQuery) -> bool {
        self.tx.try_send(Job::Owner(query)).is_ok()
    }

    /// Put the session's variables on a shared user bus ([`crate::activation_env`]); false when the worker is gone (no
    /// session bus).
    pub fn publish_session(
        &self,
        variables: Vec<(String, String)>,
        defaults: Vec<(String, String)>,
    ) -> bool {
        self.tx.try_send(Job::Publish(variables, defaults)).is_ok()
    }

    /// Call `changed` once the watch is up and then on every `NameOwnerChanged` of `name`, so no change is missed between
    /// a caller's first check and the watch; false when the worker is gone (no session bus).
    pub fn watch_owner(&self, name: &'static str, changed: OwnerChanged) -> bool {
        self.tx.try_send(Job::WatchOwner(name, changed)).is_ok()
    }

    /// Call `owned` on the worker once `name` has an owner, at once when it has one already. It is never called when the
    /// name is not taken while the connection lives; the caller bounds the wait. False when the worker is gone (no
    /// session bus).
    pub fn await_owner(&self, name: &'static str, owned: Box<dyn FnOnce() + Send>) -> bool {
        self.tx.try_send(Job::AwaitOwner(name, owned)).is_ok()
    }
}

/// Return once `name` has an owner. The watch is set up before the first look, so an owner that comes between the two
/// is not missed.
async fn await_owner(conn: &zbus::Connection, name: &'static str) -> zbus::Result<()> {
    use zbus::export::futures_core::Stream;
    let proxy = zbus::fdo::DBusProxy::new(conn).await?;
    let mut changes = proxy
        .receive_name_owner_changed_with_args(&[(0, name)])
        .await?;
    if proxy.name_has_owner(BusName::try_from(name)?).await? {
        return Ok(());
    }
    while let Some(change) =
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut changes).poll_next(cx)).await
    {
        if change.args()?.new_owner().is_some() {
            return Ok(());
        }
    }
    Err(zbus::Error::Failure(format!("the watch of {name} ended")))
}

/// Follow the owner of `name` on a thread of its own, for as long as the connection lives.
fn watch_owner(conn: &zbus::Connection, name: &'static str, changed: SharedWatch) {
    let conn = zbus::blocking::Connection::from(conn.clone());
    let spawned = std::thread::Builder::new()
        .name("cupertino-owner-watch".into())
        .spawn(move || {
            let changes = zbus::blocking::fdo::DBusProxy::new(&conn)
                .and_then(|proxy| proxy.receive_name_owner_changed_with_args(&[(0, name)]));
            let changes = match changes {
                Ok(changes) => changes,
                Err(e) => {
                    tracing::warn!(name, error = %e, "cannot follow the owner of a bus name");
                    return;
                }
            };
            let notify = || (changed.lock().unwrap_or_else(|e| e.into_inner()))();
            notify();
            for _ in changes {
                notify();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(name, error = %e, "cannot start the watch of a bus name");
    }
}

type SharedLoginTrust = std::sync::Arc<std::sync::Mutex<LoginTrust>>;

/// A connection to the bus at `address`, the session bus when `None`.
pub(crate) fn bus(address: Option<&str>) -> zbus::Result<zbus::connection::Builder<'static>> {
    match address {
        Some(address) => zbus::connection::Builder::address(address),
        None => zbus::connection::Builder::session(),
    }
}

/// The credentials the bus gives for `conn` itself; `None` when it does not answer.
async fn own_credentials(conn: &zbus::Connection) -> Option<ConnectionCredentials> {
    let own = conn.unique_name()?.clone();
    zbus::fdo::DBusProxy::new(conn)
        .await
        .ok()?
        .get_connection_credentials(own.into())
        .await
        .ok()
}

/// How long the worker waits before it connects again after the first failure, and the longest it waits.
pub(crate) const RECONNECT_FIRST: std::time::Duration = std::time::Duration::from_millis(250);
pub(crate) const RECONNECT_LONGEST: std::time::Duration = std::time::Duration::from_secs(15);

/// A watch on the owner of a name, shared by the connections that carry it.
type SharedWatch = std::sync::Arc<std::sync::Mutex<OwnerChanged>>;
/// A callback waiting for a name to have an owner, taken by the one that fires it.
type SharedAwait = std::sync::Arc<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>>;
/// Name and value pairs, as the variables and defaults are published.
type Pairs = Vec<(String, String)>;

/// What the worker keeps across connections: what it was asked to follow, so a new connection follows it again.
#[derive(Default)]
struct Kept {
    watches: Vec<(&'static str, SharedWatch)>,
    awaiting: Vec<(&'static str, SharedAwait)>,
    /// The latest variables and defaults published.
    published: Option<(Pairs, Pairs)>,
}

/// How a connection's serving ended.
enum Ended {
    /// The bus went away.
    Closed,
    /// Nobody can send jobs any more.
    NoMoreJobs,
}

/// The output of whichever of two futures completes first.
pub(crate) enum Either<A, B> {
    First(A),
    Second(B),
}

/// Wait for the first of two futures (the first when both are ready).
pub(crate) async fn either<A: std::future::Future, B: std::future::Future>(
    a: A,
    b: B,
) -> Either<A::Output, B::Output> {
    let (mut a, mut b) = (std::pin::pin!(a), std::pin::pin!(b));
    std::future::poll_fn(move |cx| {
        if let std::task::Poll::Ready(v) = a.as_mut().poll(cx) {
            return std::task::Poll::Ready(Either::First(v));
        }
        match b.as_mut().poll(cx) {
            std::task::Poll::Ready(v) => std::task::Poll::Ready(Either::Second(v)),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    })
    .await
}

/// Serve the jobs on the bus, and when the connection drops, connect again after a growing pause, take up what was
/// being followed and go on. While no connection is there, calls are dropped and owner checks answered as without a bus.
async fn worker(
    address: Option<String>,
    rx: async_channel::Receiver<Job>,
    login: SharedLoginTrust,
    identifying: Identifying,
    calls: ShellCalls,
) {
    let mut keep = Kept::default();
    let mut pause = RECONNECT_FIRST;
    let mut first = true;
    loop {
        match async { bus(address.as_deref())?.build().await }.await {
            Ok(conn) => {
                if first {
                    first = false;
                } else {
                    tracing::info!("the session bus is back: the shell toggles, the session interface and the watches resume");
                }
                pause = RECONNECT_FIRST;
                match serve(&conn, &rx, &login, &identifying, &calls, &mut keep).await {
                    Ended::NoMoreJobs => return,
                    Ended::Closed => {
                        tracing::warn!("the session bus connection dropped: connecting again")
                    }
                }
            }
            Err(e) => {
                if first {
                    first = false;
                    tracing::info!(error = %e, "no session bus: the shell toggles and media keys are unavailable until it is there");
                } else {
                    tracing::debug!(error = %e, "the session bus is still not there");
                }
                pause = (pause * 2).min(RECONNECT_LONGEST);
            }
        }
        // wait out the pause; whatever is asked meanwhile is refused or kept for the next connection
        let deadline = Instant::now() + pause;
        // nothing else runs on this thread without a connection, so it can look for jobs between short sleeps
        while Instant::now() < deadline {
            loop {
                match rx.try_recv() {
                    Ok(job) => refuse(job, &mut keep, &calls),
                    Err(async_channel::TryRecvError::Empty) => break,
                    Err(async_channel::TryRecvError::Closed) => return,
                }
            }
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(std::time::Duration::from_millis(50)),
            );
        }
    }
}

/// Deal with a job while there is no connection: a call is over, a check finds no bus, what is to be followed or
/// published waits for the next connection.
fn refuse(job: Job, keep: &mut Kept, calls: &ShellCalls) {
    match job {
        Job::Call(..) => {
            calls.0 .1.fetch_add(1, Ordering::SeqCst);
        }
        Job::Owner(query) => (query.reply)(Err(OwnerRefusal::NoBus)),
        Job::WatchOwner(name, changed) => keep
            .watches
            .push((name, std::sync::Arc::new(std::sync::Mutex::new(changed)))),
        Job::AwaitOwner(name, owned) => keep.awaiting.push((
            name,
            std::sync::Arc::new(std::sync::Mutex::new(Some(owned))),
        )),
        Job::Publish(variables, defaults) => keep.published = Some((variables, defaults)),
    }
}

/// Follow the owner of `name` for `conn`'s lifetime.
fn spawn_await(conn: &zbus::Connection, name: &'static str, owned: SharedAwait) {
    let conn = conn.clone();
    conn.executor()
        .clone()
        .spawn(
            {
                let conn = conn.clone();
                async move {
                    match await_owner(&conn, name).await {
                        Ok(()) => {
                            let fire = owned.lock().unwrap_or_else(|e| e.into_inner()).take();
                            if let Some(fire) = fire {
                                fire();
                            }
                        }
                        Err(e) => tracing::warn!(name, error = %e, "cannot wait for the owner of a bus name"),
                    }
                }
            },
            "cupertino-await-owner",
        )
        .detach();
}

/// Publish the session's variables on `conn`.
fn spawn_publish(
    conn: &zbus::Connection,
    variables: Vec<(String, String)>,
    defaults: Vec<(String, String)>,
) {
    let conn = conn.clone();
    conn.executor()
        .clone()
        .spawn(
            {
                let conn = conn.clone();
                async move {
                    report_publication(
                        crate::activation_env::publish(
                            &conn,
                            &pairs(&variables),
                            &pairs(&defaults),
                        )
                        .await,
                    )
                }
            },
            "cupertino-activation-env",
        )
        .detach();
}

/// Serve the jobs on `conn` until it closes or there are no more jobs; first take up what was kept from earlier
/// connections.
async fn serve(
    conn: &zbus::Connection,
    rx: &async_channel::Receiver<Job>,
    login: &SharedLoginTrust,
    identifying: &Identifying,
    calls: &ShellCalls,
    keep: &mut Kept,
) -> Ended {
    if let Some(notice) = own_credentials(conn)
        .await
        .as_ref()
        .and_then(cupertino_peer::process_fd_notice)
    {
        tracing::warn!("{notice}: the menu bar's menus are not shown");
    }
    for (name, changed) in &keep.watches {
        watch_owner(conn, name, changed.clone());
    }
    keep.awaiting
        .retain(|(_, owned)| owned.lock().unwrap_or_else(|e| e.into_inner()).is_some());
    for (name, owned) in &keep.awaiting {
        spawn_await(conn, name, owned.clone());
    }
    if let Some((variables, defaults)) = keep.published.clone() {
        spawn_publish(conn, variables, defaults);
    }
    loop {
        let job = match either(rx.recv(), conn.closed()).await {
            Either::First(Ok(job)) => job,
            Either::First(Err(_)) => return Ended::NoMoreJobs,
            Either::Second(()) => return Ended::Closed,
        };
        let conn = conn.clone();
        let (login, identifying, calls) = (login.clone(), identifying.clone(), calls.clone());
        let (what, via) = match job {
            Job::Call(what, via) => (what, via),
            Job::WatchOwner(name, changed) => {
                let changed: SharedWatch = std::sync::Arc::new(std::sync::Mutex::new(changed));
                watch_owner(&conn, name, changed.clone());
                keep.watches.push((name, changed));
                continue;
            }
            Job::AwaitOwner(name, owned) => {
                let owned: SharedAwait = std::sync::Arc::new(std::sync::Mutex::new(Some(owned)));
                spawn_await(&conn, name, owned.clone());
                keep.awaiting.push((name, owned));
                continue;
            }
            Job::Publish(variables, defaults) => {
                keep.published = Some((variables.clone(), defaults.clone()));
                spawn_publish(&conn, variables, defaults);
                continue;
            }
            Job::Owner(OwnerQuery {
                name,
                client,
                client_identifying,
                expires,
                reply,
            }) => {
                conn.executor()
                    .clone()
                    .spawn(
                        async move {
                            let threads = (&identifying, &client_identifying);
                            reply(owner_of(&conn, &name, client, threads, expires).await)
                        },
                        "cupertino-owner-check",
                    )
                    .detach();
                continue;
            }
        };
        conn.executor()
            .clone()
            .spawn(
                async move {
                    call(&conn, &via, what, &login).await;
                    calls.0 .1.fetch_add(1, Ordering::SeqCst);
                },
                "cupertino-shell-call",
            )
            .detach();
    }
}

/// Borrowed name and value pairs of `list`.
fn pairs(list: &[(String, String)]) -> Vec<(&str, &str)> {
    list.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect()
}

fn report_publication(result: zbus::Result<crate::activation_env::Outcome>) {
    use crate::activation_env::Outcome;
    match result {
        Ok(Outcome::Applied { defaults }) => {
            tracing::info!(
                target = crate::activation_env::SESSION_TARGET,
                ?defaults,
                "the session's display and desktop name are in the user manager and the bus activation environment"
            );
            crate::activation_env::record_defaults(&defaults);
        }
        Ok(Outcome::AnotherSession) => tracing::warn!(
            "another graphical session of this user is active: its environment is left alone, so services started on the shared bus may use that session's display, and no portal reaches this one"
        ),
        Ok(Outcome::NoSystemd) => tracing::debug!("the session bus has no systemd user manager"),
        Err(e) => tracing::warn!(error = %e, "cannot update the activation environment of the session bus"),
    }
}

/// Where to send a call for the owner of `name`. The login process is looked for by its connection: the owner of
/// `org.cupertino.Login1` must be the supervised process, and the call goes to its unique name, so a client that takes
/// the name later gets nothing. `None`: the owner is not the login process.
async fn destination(
    conn: &zbus::Connection,
    name: &str,
    login: &SharedLoginTrust,
) -> Option<String> {
    if name != LOGIN_NAME {
        return Some(name.to_string());
    }
    let trust = *login
        .lock()
        .expect("no thread panics while holding the login trust");
    let Ok(proxy) = zbus::fdo::DBusProxy::new(conn).await else {
        return Some(name.to_string());
    };
    let Ok(bus_name) = zbus::names::BusName::try_from(name) else {
        return Some(name.to_string());
    };
    let Ok(owner) = proxy.get_name_owner(bus_name).await else {
        // nobody owns it: the call fails the way it did, and the log out request finds nobody to ask
        return Some(name.to_string());
    };
    let caller = identify(conn, &owner).await;
    if !trusts(trust, &caller) {
        tracing::error!(
            name,
            owner = %owner,
            pid = caller.pid(),
            "the owner of the login process's name is not the login process: the request is not sent"
        );
        return None;
    }
    Some(owner.to_string())
}

const LOGIN_NAME: &str = "org.cupertino.Login1";

/// How many identification threads are alive: of all clients, or of one.
pub type Identifying = std::sync::Arc<AtomicUsize>;

/// How many identification threads may be alive at once, counting those still reading after their check timed out.
pub const IDENTIFY_LIMIT: usize = 16;
/// How many identification threads of one client may be alive at once; a client whose identifications stall keeps
/// that many at most.
pub const CLIENT_IDENTIFY_LIMIT: usize = 3;

/// One alive identification thread, counted in all threads and in its client's while it lives.
struct Alive {
    all: Identifying,
    client: Identifying,
}

impl Alive {
    /// Count a new identification thread of a client: refused as stalled at the client's limit and as busy at the
    /// global one.
    fn enter(all: &Identifying, client: &Identifying) -> Result<Alive, OwnerRefusal> {
        let of_client = client.fetch_add(1, Ordering::SeqCst);
        if of_client >= CLIENT_IDENTIFY_LIMIT {
            client.fetch_sub(1, Ordering::SeqCst);
            return Err(OwnerRefusal::Stalled);
        }
        if all.fetch_add(1, Ordering::SeqCst) >= IDENTIFY_LIMIT {
            all.fetch_sub(1, Ordering::SeqCst);
            client.fetch_sub(1, Ordering::SeqCst);
            return Err(OwnerRefusal::Busy);
        }
        if of_client + 1 == CLIENT_IDENTIFY_LIMIT {
            tracing::warn!(
                threads = CLIENT_IDENTIFY_LIMIT,
                "a client has as many owner identifications alive as it may: its bus names are refused until one ends"
            );
        }
        Ok(Alive {
            all: all.clone(),
            client: client.clone(),
        })
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.client.fetch_sub(1, Ordering::SeqCst);
        self.all.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The answer of an identification thread.
type Identified = async_channel::Receiver<Result<(), OwnerRefusal>>;

/// Identify the owner the daemon named in `credentials` against `client` on a thread of its own, reading `proc_root`.
/// Reading `/proc` of a process the client chose can stall for good (a FUSE root), so each identification has a thread
/// that may be left behind, never the worker that makes the shell calls. `all` and `of_client` count the alive threads
/// of all clients and of this one ([`Alive::enter`]).
fn identify_owner(
    (all, of_client): (&Identifying, &Identifying),
    proc_root: PathBuf,
    credentials: ConnectionCredentials,
    client: ProcessHandle,
) -> Result<Identified, OwnerRefusal> {
    let alive = Alive::enter(all, of_client)?;
    let (answer, answered) = async_channel::bounded(1);
    std::thread::Builder::new()
        .name("cupertino-identify".into())
        .spawn(move || {
            let _alive = alive;
            let _ = answer.try_send(identified(&proc_root, &credentials, client));
        })
        .map_err(|_| OwnerRefusal::Busy)?;
    Ok(answered)
}

/// Whether the owner the daemon named in `credentials` is one sandbox instance with `client`.
fn identified(
    proc_root: &Path,
    credentials: &ConnectionCredentials,
    client: ProcessHandle,
) -> Result<(), OwnerRefusal> {
    let owner = Caller::from_process_fd(credentials, proc_root);
    let client = Caller::of_process(client, proc_root);
    let (Caller::Process(owner), Caller::Process(client)) = (&owner, &client) else {
        return Err(OwnerRefusal::Unidentified);
    };
    if !same_instance(owner, client, proc_root) {
        return Err(OwnerRefusal::OtherInstance {
            owner: owner.process.pid,
            client: client.process.pid,
        });
    }
    Ok(())
}

/// The unique name of the owner of `name` when it is one sandbox instance with `client`. Only the bus daemon is asked:
/// the owner of a well-known name, then the owner's credentials, whose `ProcessFD` alone names its process.
async fn owner_of(
    conn: &zbus::Connection,
    name: &str,
    client: ProcessHandle,
    threads: (&Identifying, &Identifying),
    expires: Instant,
) -> Result<String, OwnerRefusal> {
    let name = BusName::try_from(name).map_err(|_| OwnerRefusal::NotABusName)?;
    let no_owner = |e: zbus::fdo::Error| OwnerRefusal::NoOwner(e.to_string());
    let proxy = zbus::fdo::DBusProxy::new(conn)
        .await
        .map_err(|e| OwnerRefusal::NoOwner(e.to_string()))?;
    let unique: OwnedUniqueName = match name {
        BusName::Unique(unique) => unique.into(),
        BusName::WellKnown(_) => proxy.get_name_owner(name).await.map_err(no_owner)?,
    };
    let credentials = proxy
        .get_connection_credentials(BusName::from(unique.as_ref()))
        .await
        .map_err(no_owner)?;
    identify_before(
        threads,
        PathBuf::from("/proc"),
        credentials,
        client,
        expires,
    )?
    .recv()
    .await
    .map_err(|_| OwnerRefusal::Unidentified)??;
    Ok(unique.to_string())
}

/// [`identify_owner`], unless the check is past `expires`: its answer would be dropped, and a thread started for it would
/// count against the client's limit after a slow bus.
fn identify_before(
    threads: (&Identifying, &Identifying),
    proc_root: PathBuf,
    credentials: ConnectionCredentials,
    client: ProcessHandle,
    expires: Instant,
) -> Result<Identified, OwnerRefusal> {
    if Instant::now() >= expires {
        return Err(OwnerRefusal::TimedOut);
    }
    identify_owner(threads, proc_root, credentials, client)
}

/// Whether the owner of the login process's name is the process `trust` names. Only its process id counts, so a process
/// that is not dumpable (the login process hardens itself) is judged like any other.
fn trusts(trust: LoginTrust, owner: &Caller) -> bool {
    match (trust, owner.pid()) {
        (LoginTrust::Any, _) => true,
        (LoginTrust::Agent(expected), Some(pid)) => pid == expected,
        (LoginTrust::Agent(_) | LoginTrust::Nobody, _) => false,
    }
}

/// Make the call of `what` on `via`, once the owner of its name passed the check on `conn`, and log a failure.
async fn call(
    conn: &zbus::Connection,
    via: &zbus::Connection,
    what: Request,
    login: &SharedLoginTrust,
) {
    let (name, path, interface, method, body) = what.call();
    // an owner that is not the lock screen gets nothing
    let Some(destination) = destination(conn, name, login).await else {
        return;
    };
    let destination = Some(destination.as_str());
    let sent = match body {
        Body::Empty => {
            via.call_method(destination, path, Some(interface), method, &())
                .await
        }
        Body::Step(direction, fine) => {
            via.call_method(
                destination,
                path,
                Some(interface),
                method,
                &(direction, fine),
            )
            .await
        }
    };
    if let Err(e) = sent {
        tracing::warn!(?what, name, error = %e, "cannot reach the shell process");
    }
}

impl crate::state::Cupertino {
    /// Ask the notifications component to open or close its centre list.
    pub fn toggle_notification_center(&mut self) {
        self.toggle_component(Toggle::NotificationCentre);
    }

    /// Ask the launcher component to show or hide its panel.
    pub fn toggle_launcher(&mut self) {
        self.toggle_component(Toggle::Launcher);
    }

    /// Ask the launcher component to show or hide its application browser (the app-grid action: its shortcut, the
    /// Dock's hot corner or gesture).
    pub fn toggle_applications(&mut self) {
        self.toggle_component(Toggle::Applications);
    }

    /// Ask the control panel component to show or hide its panel.
    pub fn toggle_control_panel(&mut self) {
        self.toggle_component(Toggle::ControlPanel);
    }

    /// Ask a component to toggle what `toggle` names, on the connection that owns `org.cupertino.Session1`, as the
    /// component takes the call only from that one (D20); while the session interface is not served the toggle does
    /// nothing.
    fn toggle_component(&mut self, toggle: Toggle) {
        let (interface, method) = toggle.call();
        match self.quit.session_connection() {
            Some(conn) => send_to_component(&conn, interface, method, Body::Empty, toggle),
            None => tracing::warn!(
                ?toggle,
                "the session interface is not served: the toggle does not reach the shell component"
            ),
        }
    }

    /// Ask the login process to lock the session. The server's private connection to the lock screen carries the request;
    /// the session bus is the way only while no lock screen is connected, so a process that took `org.cupertino.Login1`
    /// before the login process did cannot make the lock go missing.
    pub fn lock_session(&mut self) {
        self.revoke_controls(cupertino_control_model::Revocation::Locked);
        if !self.sessions.lock_requested(false) {
            self.shell_call(Request::Lock);
        }
    }

    /// Ask the login process to lock the session because the person asked for it.
    pub fn lock_session_now(&mut self) {
        self.revoke_controls(cupertino_control_model::Revocation::Locked);
        if !self.sessions.lock_requested(true) {
            self.shell_call(Request::Lock);
        }
    }

    /// Ask the session agent whether to log out (its confirmation panel). The session's `log_out_requested` carries the
    /// request while an agent follows the session over the server's own Wayland connection; else it goes to the agent
    /// on the session bus ([`crate::session_agent`]), and a session without an agent component ends the server, as the
    /// shortcut did before there was a panel.
    pub fn log_out_session(&mut self) {
        if self.sessions.log_out_requested() {
            return;
        }
        self.ask_session_agent(crate::session_agent::AgentRequest::LogOut);
    }

    /// Log out without the confirmation panel (the log out now shortcut): `Session1`'s own log out, which quits the
    /// applications first.
    pub fn log_out_session_now(&mut self) {
        if let Err(refusal) = self.request_session_action(crate::quit::SessionAction::LogOut) {
            tracing::warn!(?refusal, "the log out now shortcut is refused");
        }
    }

    /// Sleep at once (the menu bar's Sleep): `Session1`'s own sleep.
    pub fn sleep_session(&mut self) {
        if let Err(refusal) = self.request_session_action(crate::quit::SessionAction::Sleep) {
            tracing::warn!(?refusal, "sleep is refused");
        }
    }

    /// Open the session agent's power panel.
    pub fn power_dialog(&mut self) {
        self.ask_session_agent(crate::session_agent::AgentRequest::PowerDialog);
    }

    /// Tell the session agent the power key went down or up.
    pub fn power_key(&mut self, pressed: bool) {
        self.ask_session_agent(crate::session_agent::AgentRequest::PowerKey { pressed });
    }

    /// Forward a media key to the on-screen displays, which change the level and show it. The call goes out on the
    /// connection that owns `org.cupertino.Session1`, as the displays take a step only from that one (D20); while the
    /// session interface is not served the key does nothing.
    pub fn osd_key(&mut self, key: OsdKey, fine: bool) {
        match self.quit.session_connection() {
            Some(conn) => {
                let (method, body) = osd_call(key, fine);
                send_to_component(&conn, OSD, method, body, key);
            }
            None => tracing::warn!(
                ?key,
                "the session interface is not served: the media key does not reach the on-screen displays"
            ),
        }
    }

    /// Send `what` to the login process from the connection that owns `org.cupertino.Session1`, as the lock screen
    /// takes the call only from that one (D20); while the session interface is not served it is not sent.
    fn shell_call(&mut self, what: Request) {
        let Some(dbus) = &self.dbus else {
            tracing::warn!(?what, "no session bus: cannot reach the shell process");
            return;
        };
        match self.quit.session_connection() {
            Some(via) => {
                dbus.send(what, via);
            }
            None => tracing::warn!(
                ?what,
                "the session interface is not served: the lock request does not reach the lock screen"
            ),
        }
    }
}
