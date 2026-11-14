//! Session bus worker: relays the shell toggles (notification centre, launcher, control panel), the media keys
//! (overlay process), the lock request and the power key (login process) to the processes that own them.
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

/// What the worker asks a shell process to toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// The notification centre list of `cupertino-notifications`.
    NotificationCentre,
    /// The panel of `cupertino-launcher`.
    Launcher,
    /// The application browser of `cupertino-launcher`.
    Applications,
    /// The panel of `cupertino-controlcenter`.
    ControlPanel,
}

/// One call the worker makes on a shell process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Toggle(Toggle),
    /// A media key for `cupertino-osd`; `fine` is the Shift+Alt step.
    Osd {
        key: OsdKey,
        fine: bool,
    },
    /// Lock the session through `cupertino-login`: the display slept for the password delay (lock request type 5 of the design).
    Lock,
    /// Lock the session on the person's request (the lock shortcut; lock request type 8, whose lock screen hides after 5 s).
    LockNow,
    /// Ask `cupertino-login` whether to log out (its confirmation panel).
    LogOut,
    /// Open `cupertino-login`'s power panel.
    PowerDialog,
    /// The menu bar's entries, forwarded by the server: sleep without a panel, the restart and shut down panels, the
    /// switch to the login window, and reading what the machine allows again.
    Sleep,
    Restart,
    ShutDown,
    SwitchUser,
    RefreshCapabilities,
    /// The power key went down (`true`) or up; `cupertino-login` times the hold.
    PowerKey {
        pressed: bool,
    },
}

fn login_call(
    method: &'static str,
) -> (&'static str, &'static str, &'static str, &'static str, Body) {
    (
        "org.cupertino.Login1",
        "/org/cupertino/Login1",
        "org.cupertino.Login1",
        method,
        Body::Empty,
    )
}

/// Arguments of a control method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    Empty,
    /// Direction (+1 or -1) and the fine flag of the overlay's step methods.
    Step(i32, bool),
    /// Whether the power key went down (`PowerKey`).
    Pressed(bool),
}

/// Bus name, object path and interface of the overlay process (`cupertino_osd::control`).
const OSD: (&str, &str, &str) = (
    "org.cupertino.Osd1",
    "/org/cupertino/Osd1",
    "org.cupertino.Osd1",
);

impl Request {
    /// Bus name, object path, interface, method and arguments of the control interface of the process
    /// (`cupertino_notifications::dbus`, `cupertino_launcher::control`, `cupertino_controlcenter::control`, `cupertino_osd::control`; the lock request goes to the login process).
    fn call(self) -> (&'static str, &'static str, &'static str, &'static str, Body) {
        let (name, path, interface) = OSD;
        match self {
            Request::Toggle(Toggle::NotificationCentre) => (
                "org.cupertino.NotificationCenter1",
                "/org/cupertino/NotificationCenter1",
                "org.cupertino.NotificationCenter1",
                "ToggleCentre",
                Body::Empty,
            ),
            Request::Toggle(Toggle::Launcher) => (
                "org.cupertino.Launcher1",
                "/org/cupertino/Launcher1",
                "org.cupertino.Launcher1",
                "Toggle",
                Body::Empty,
            ),
            Request::Toggle(Toggle::Applications) => (
                "org.cupertino.Launcher1",
                "/org/cupertino/Launcher1",
                "org.cupertino.Launcher1",
                "ToggleApplications",
                Body::Empty,
            ),
            Request::Toggle(Toggle::ControlPanel) => (
                "org.cupertino.ControlPanel1",
                "/org/cupertino/ControlPanel1",
                "org.cupertino.ControlPanel1",
                "Toggle",
                Body::Empty,
            ),
            Request::Lock => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "Lock",
                Body::Empty,
            ),
            Request::LockNow => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "LockNow",
                Body::Empty,
            ),
            Request::LogOut => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "RequestLogOut",
                Body::Empty,
            ),
            Request::PowerDialog => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "RequestPowerDialog",
                Body::Empty,
            ),
            Request::Sleep => login_call("Sleep"),
            Request::Restart => login_call("RequestRestart"),
            Request::ShutDown => login_call("RequestShutDown"),
            Request::SwitchUser => login_call("SwitchToLoginWindow"),
            Request::RefreshCapabilities => login_call("RefreshCapabilities"),
            Request::PowerKey { pressed } => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "PowerKey",
                Body::Pressed(pressed),
            ),
            Request::Osd { key, fine } => {
                let (method, body) = match key {
                    OsdKey::VolumeUp => ("VolumeStep", Body::Step(1, fine)),
                    OsdKey::VolumeDown => ("VolumeStep", Body::Step(-1, fine)),
                    OsdKey::Mute => ("ToggleMute", Body::Empty),
                    OsdKey::MicMute => ("ToggleMicrophoneMute", Body::Empty),
                    OsdKey::BrightnessUp => ("BrightnessStep", Body::Step(1, fine)),
                    OsdKey::BrightnessDown => ("BrightnessStep", Body::Step(-1, fine)),
                    OsdKey::KeyboardBrightnessUp => ("KeyboardBrightnessStep", Body::Step(1, fine)),
                    OsdKey::KeyboardBrightnessDown => {
                        ("KeyboardBrightnessStep", Body::Step(-1, fine))
                    }
                };
                (name, path, interface, method, body)
            }
        }
    }
}

/// Who may own `org.cupertino.Login1`: the lock, log out and power key requests go to nobody else.
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
    Call(Request),
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

/// Runs on the worker thread when the login process, the one that shows the log out panel, is not on the bus.
pub type LoginMissing = std::sync::Arc<dyn Fn() + Send + Sync>;

impl DbusHandle {
    /// Start the worker.
    pub fn start() -> Option<DbusHandle> {
        Self::start_with(|| {})
    }

    /// Start the worker; `login_missing` runs on it when a log out request finds nobody to ask (no `org.cupertino.Login1`
    /// on the bus), so the caller can do what a session without a login process does.
    pub fn start_with(login_missing: impl Fn() + Send + Sync + 'static) -> Option<DbusHandle> {
        Self::start_on(None, login_missing)
    }

    /// Start the worker on the bus at `address`, the session bus when `None`; `login_missing` as for
    /// [`start_with`](Self::start_with).
    pub fn start_on(
        address: Option<String>,
        login_missing: impl Fn() + Send + Sync + 'static,
    ) -> Option<DbusHandle> {
        let (tx, rx) = async_channel::unbounded::<Job>();
        let login_missing: LoginMissing = std::sync::Arc::new(login_missing);
        let login = std::sync::Arc::new(std::sync::Mutex::new(LoginTrust::Nobody));
        let worker_login = login.clone();
        let identifying = std::sync::Arc::new(AtomicUsize::new(0));
        let calls = ShellCalls::default();
        let worker_calls = calls.clone();
        let spawned = std::thread::Builder::new()
            .name("cupertino-dbus".into())
            .spawn(move || {
                zbus::block_on(worker(
                    address,
                    rx,
                    login_missing,
                    worker_login,
                    identifying,
                    worker_calls,
                ))
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

    /// Queue a call on a shell process; false when the worker is gone (no session bus).
    pub fn send(&self, what: Request) -> bool {
        // counted before the worker can be done with it, so the done count never runs ahead
        self.calls.0 .0.fetch_add(1, Ordering::SeqCst);
        let queued = self.tx.try_send(Job::Call(what)).is_ok();
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
    login_missing: LoginMissing,
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
                match serve(
                    &conn,
                    &rx,
                    &login_missing,
                    &login,
                    &identifying,
                    &calls,
                    &mut keep,
                )
                .await
                {
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
        Job::Call(_) => {
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
    login_missing: &LoginMissing,
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
        let (login_missing, login, identifying, calls) = (
            login_missing.clone(),
            login.clone(),
            identifying.clone(),
            calls.clone(),
        );
        let what = match job {
            Job::Call(what) => what,
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
        // a key's press and release must arrive in the order they happened, so those calls are awaited in turn
        if matches!(what, Request::PowerKey { .. }) {
            call(&conn, what, &login_missing, &login).await;
            calls.0 .1.fetch_add(1, Ordering::SeqCst);
        } else {
            conn.executor()
                .clone()
                .spawn(
                    async move {
                        call(&conn, what, &login_missing, &login).await;
                        calls.0 .1.fetch_add(1, Ordering::SeqCst);
                    },
                    "cupertino-shell-call",
                )
                .detach();
        }
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

/// Make the call of `what` and log a failure.
async fn call(
    conn: &zbus::Connection,
    what: Request,
    login_missing: &LoginMissing,
    login: &SharedLoginTrust,
) {
    let (name, path, interface, method, body) = what.call();
    // an owner that is not the login process gets nothing, and a log out is not the end of the session then: the name
    // being taken is not the same as nobody to ask
    let Some(destination) = destination(conn, name, login).await else {
        if what == Request::LogOut {
            post_notice(conn, LogOutDropped::OwnerNotTheLoginProcess).await;
        }
        return;
    };
    let destination = Some(destination.as_str());
    let sent = match body {
        Body::Empty => {
            conn.call_method(destination, path, Some(interface), method, &())
                .await
        }
        Body::Step(direction, fine) => {
            conn.call_method(
                destination,
                path,
                Some(interface),
                method,
                &(direction, fine),
            )
            .await
        }
        Body::Pressed(pressed) => {
            conn.call_method(destination, path, Some(interface), method, &pressed)
                .await
        }
    };
    if let Err(e) = sent {
        if what == Request::LogOut && is_nobody_there(&e) {
            let trust = *login
                .lock()
                .expect("no thread panics while holding the login trust");
            // a supervised login process that has not taken its name yet (it is starting or restarting) is still the
            // one to ask: only a session without a login process ends
            if let LoginTrust::Agent(pid) = trust {
                tracing::warn!(pid, "the login process has not taken its bus name yet: the log out request is dropped");
                post_notice(conn, LogOutDropped::LoginProcessNotReady).await;
            } else {
                tracing::warn!("no login process on the bus: the log out shortcut ends the server");
                login_missing();
            }
            return;
        }
        tracing::warn!(?what, name, error = %e, "cannot reach the shell process");
    }
}

/// Why a log out request reached nobody and the session goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogOutDropped {
    /// A process other than the login process owns `org.cupertino.Login1`.
    OwnerNotTheLoginProcess,
    /// The login process is supervised but has not taken its name yet (it is starting or restarting).
    LoginProcessNotReady,
}

impl LogOutDropped {
    /// The summary and the body of the notice that tells the person.
    pub fn text(self) -> (&'static str, &'static str) {
        match self {
            LogOutDropped::OwnerNotTheLoginProcess => (
                "Log out did not start",
                "Another program has taken the name of the login service, so the request was not sent. The session goes on.",
            ),
            LogOutDropped::LoginProcessNotReady => (
                "Log out did not start",
                "The login service is not ready yet. Try again in a moment.",
            ),
        }
    }
}

/// Tell the person a log out request was dropped: a banner from the notification service. Nothing else shows it, and a
/// shortcut that does nothing looks like a broken one. A session without a notification service only has the log.
pub async fn post_notice(conn: &zbus::Connection, dropped: LogOutDropped) {
    let (summary, body) = dropped.text();
    // urgency 1 is normal; the banner is not kept in the centre
    let hints = std::collections::HashMap::from([
        ("urgency", zbus::zvariant::Value::U8(1)),
        ("transient", zbus::zvariant::Value::Bool(true)),
    ]);
    let actions: Vec<&str> = Vec::new();
    // sent without waiting for a reply: whoever owns the name answers it, and a squatter that never does must not hold
    // anything up
    let sent = async {
        let message =
            zbus::message::Message::method_call("/org/freedesktop/Notifications", "Notify")?
                .destination("org.freedesktop.Notifications")?
                .interface("org.freedesktop.Notifications")?
                .with_flags(zbus::message::Flags::NoReplyExpected)?
                .build(&("Cupertino", 0u32, "", summary, body, actions, hints, -1i32))?;
        conn.send(&message).await
    }
    .await;
    if let Err(e) = sent {
        tracing::warn!(error = %e, "cannot show that the log out request was dropped");
    }
}

/// Whether a call failed because nobody owns the name it went to.
fn is_nobody_there(e: &zbus::Error) -> bool {
    matches!(
        e,
        zbus::Error::MethodError(name, ..)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"
            )
    )
}

impl crate::state::Cupertino {
    /// Ask the notification process to open or close its centre list.
    pub fn toggle_notification_center(&mut self) {
        self.shell_call(Request::Toggle(Toggle::NotificationCentre));
    }

    /// Ask the launcher process to show or hide its panel.
    pub fn toggle_launcher(&mut self) {
        self.shell_call(Request::Toggle(Toggle::Launcher));
    }

    /// Ask the launcher process to show or hide its application browser (the app-grid action: its shortcut, the
    /// Dock's hot corner or gesture).
    pub fn toggle_applications(&mut self) {
        self.shell_call(Request::Toggle(Toggle::Applications));
    }

    /// Ask the control panel process to show or hide its panel.
    pub fn toggle_control_panel(&mut self) {
        self.shell_call(Request::Toggle(Toggle::ControlPanel));
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
            self.shell_call(Request::LockNow);
        }
    }

    /// Ask the login process whether to log out. The private connection of the lock screen carries the request while one
    /// is connected, so a process that took `org.cupertino.Login1` first cannot turn it into the end of the session. A
    /// session without a login process (no session bus, or nobody owns the name: a headless or capture run) ends the
    /// server instead, as the shortcut did before there was a panel.
    pub fn log_out_session(&mut self) {
        if self.sessions.log_out_requested() {
            return;
        }
        if !self.dbus.as_ref().is_some_and(|d| d.send(Request::LogOut)) {
            tracing::warn!("no login process to ask: the log out shortcut ends the server");
            self.quit_compositor();
        }
    }

    /// A menu bar entry of the session menu: the server asks the login process on the menu bar's behalf, because the login
    /// process answers only the server.
    pub fn menu_bar_session_action(&mut self, action: Request) {
        if action == Request::LogOut {
            return self.log_out_session();
        }
        self.shell_call(action);
    }

    /// Open the login process's power panel.
    pub fn power_dialog(&mut self) {
        self.shell_call(Request::PowerDialog);
    }

    /// Tell the login process the power key went down or up.
    pub fn power_key(&mut self, pressed: bool) {
        self.shell_call(Request::PowerKey { pressed });
    }

    /// Forward a media key to the overlay process, which changes the level and shows it.
    pub fn osd_key(&mut self, key: OsdKey, fine: bool) {
        self.shell_call(Request::Osd { key, fine });
    }

    fn shell_call(&mut self, what: Request) {
        match &self.dbus {
            Some(dbus) => {
                dbus.send(what);
            }
            None => tracing::warn!(?what, "no session bus: cannot reach the shell process"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_peer::ProcessHandle;

    fn unclassified_self() -> Caller {
        Caller::Unclassified(ProcessHandle::open(std::process::id()).unwrap())
    }

    /// A scratch `/proc` for this process. With `stall`, its AppArmor label is a FIFO nobody writes: reading it stalls
    /// until [`release`].
    fn scratch_proc(stall: bool) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let pid = root.path().join(std::process::id().to_string());
        std::fs::create_dir_all(pid.join("root")).unwrap();
        std::fs::create_dir_all(pid.join("attr").join("apparmor")).unwrap();
        let label = label_of(root.path());
        if stall {
            let path = std::ffi::CString::new(label.into_os_string().into_encoded_bytes()).unwrap();
            // Safety: mkfifo on a path that lives through the call.
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        } else {
            std::fs::write(label, "unconfined\n").unwrap();
        }
        root
    }

    fn label_of(proc_root: &Path) -> PathBuf {
        proc_root
            .join(std::process::id().to_string())
            .join("attr")
            .join("apparmor")
            .join("current")
    }

    /// Let the threads stalled on the FIFO of `proc_root` go on, until none of `identifying` is alive.
    fn release(proc_root: &Path, identifying: &Identifying) {
        use std::os::unix::fs::OpenOptionsExt;
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while identifying.load(Ordering::SeqCst) > 0 {
            assert!(std::time::Instant::now() < end, "the stalled threads go on");
            // a writer that opens and closes gives every reader that opened meanwhile its end of file
            let _ = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(label_of(proc_root));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn own_credentials() -> ConnectionCredentials {
        // Safety: pidfd_open returns a new descriptor for our own pid, which nothing else owns.
        let fd: std::os::fd::OwnedFd = unsafe {
            std::os::fd::FromRawFd::from_raw_fd(libc::syscall(
                libc::SYS_pidfd_open,
                std::process::id() as libc::pid_t,
                0,
            ) as i32)
        };
        ConnectionCredentials::default().set_process_fd(fd.into())
    }

    fn identify_as(
        all: &Identifying,
        client: &Identifying,
        proc_root: &Path,
    ) -> Result<Identified, OwnerRefusal> {
        identify_owner(
            (all, client),
            proc_root.to_path_buf(),
            own_credentials(),
            ProcessHandle::open(std::process::id()).unwrap(),
        )
    }

    fn wait_for(count: &Identifying, alive: usize) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while count.load(Ordering::SeqCst) != alive {
            assert!(std::time::Instant::now() < end, "{alive} threads alive");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_check_past_its_deadline_starts_no_identification() {
        let (all, client) = (Identifying::default(), Identifying::default());
        let fine = scratch_proc(false);
        let identify = |expires| {
            identify_before(
                (&all, &client),
                fine.path().to_path_buf(),
                own_credentials(),
                ProcessHandle::open(std::process::id()).unwrap(),
                expires,
            )
        };
        let past = Instant::now() - std::time::Duration::from_millis(1);
        assert!(matches!(identify(past), Err(OwnerRefusal::TimedOut)));
        assert_eq!(all.load(Ordering::SeqCst), 0, "no thread started");
        assert_eq!(
            client.load(Ordering::SeqCst),
            0,
            "nothing counted for the client"
        );

        let ahead = Instant::now() + std::time::Duration::from_secs(10);
        let answer = identify(ahead).unwrap().recv_blocking();
        assert!(matches!(answer, Ok(Ok(()))), "{answer:?}");
    }

    #[test]
    fn an_identification_that_stalls_leaves_another_to_be_answered() {
        let all = Identifying::default();
        let stalled = scratch_proc(true);
        let fine = scratch_proc(false);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_all = all.clone();
        let (stalled_root, fine_root) = (stalled.path().to_path_buf(), fine.path().to_path_buf());
        std::thread::spawn(move || {
            let stuck = identify_as(&worker_all, &Identifying::default(), &stalled_root).unwrap();
            let answered = identify_as(&worker_all, &Identifying::default(), &fine_root).unwrap();
            let _ = tx.send(answered.recv_blocking());
            drop(stuck);
        });
        let answer = rx.recv_timeout(std::time::Duration::from_secs(5));
        release(stalled.path(), &all);
        assert!(matches!(answer, Ok(Ok(Ok(())))), "{answer:?}");
    }

    #[test]
    fn the_identification_threads_alive_are_capped() {
        let all = Identifying::default();
        let stalled = scratch_proc(true);
        let fine = scratch_proc(false);
        let stuck: Vec<_> = (0..IDENTIFY_LIMIT)
            .map(|_| {
                identify_as(&all, &Identifying::default(), stalled.path()).expect("under the limit")
            })
            .collect();
        let client = Identifying::default();
        let refused = identify_as(&all, &client, fine.path());
        let client_after_refusal = client.load(Ordering::SeqCst);
        release(stalled.path(), &all);
        assert!(matches!(refused, Err(OwnerRefusal::Busy)), "{refused:?}");
        assert_eq!(client_after_refusal, 0, "a refused thread is not counted");
        assert!(stuck.iter().all(|s| s.recv_blocking().is_ok()));
        let again = identify_as(&all, &client, fine.path()).expect("the threads are gone");
        assert!(matches!(again.recv_blocking(), Ok(Ok(()))));
    }

    #[test]
    fn a_client_mixing_verified_and_stalled_identifications_holds_at_most_its_limit() {
        let (all, client, other) = (
            Identifying::default(),
            Identifying::default(),
            Identifying::default(),
        );
        let stalled = scratch_proc(true);
        let fine = scratch_proc(false);
        let mut stuck = Vec::new();
        let mut most = 0;
        for _ in 0..CLIENT_IDENTIFY_LIMIT {
            let verified = identify_as(&all, &client, fine.path()).expect("under its limit");
            assert!(matches!(verified.recv_blocking(), Ok(Ok(()))));
            wait_for(&client, stuck.len());
            stuck.push(identify_as(&all, &client, stalled.path()).expect("under its limit"));
            most = most.max(client.load(Ordering::SeqCst));
        }
        let refused = [
            identify_as(&all, &client, fine.path()),
            identify_as(&all, &client, stalled.path()),
        ];
        let at_limit = client.load(Ordering::SeqCst);
        let another = identify_as(&all, &other, fine.path()).map(|a| a.recv_blocking());

        // the stalled threads end: the client is identified again
        release(stalled.path(), &all);
        let after = client.load(Ordering::SeqCst);
        let again = identify_as(&all, &client, fine.path()).map(|a| a.recv_blocking());

        assert!(
            most <= CLIENT_IDENTIFY_LIMIT,
            "{most} threads of one client"
        );
        assert_eq!(at_limit, CLIENT_IDENTIFY_LIMIT);
        assert!(
            refused
                .iter()
                .all(|r| matches!(r, Err(OwnerRefusal::Stalled))),
            "{refused:?}"
        );
        assert!(matches!(another, Ok(Ok(Ok(())))), "{another:?}");
        assert_eq!(after, 0);
        assert!(matches!(again, Ok(Ok(Ok(())))), "{again:?}");
    }

    #[test]
    fn a_thread_that_never_starts_is_counted_no_longer() {
        let (all, client) = (Identifying::default(), Identifying::default());
        let alive = Alive::enter(&all, &client).unwrap();
        assert_eq!(
            (all.load(Ordering::SeqCst), client.load(Ordering::SeqCst)),
            (1, 1)
        );
        // a spawn that fails drops the closure that holds the count
        let never_run = move || {
            let _alive = alive;
        };
        drop(never_run);
        assert_eq!(
            (all.load(Ordering::SeqCst), client.load(Ordering::SeqCst)),
            (0, 0)
        );
    }

    #[test]
    fn the_login_name_is_trusted_to_the_supervised_process_even_when_it_is_not_dumpable() {
        let me = std::process::id();
        assert!(trusts(LoginTrust::Agent(me), &unclassified_self()));
        assert!(!trusts(LoginTrust::Agent(me + 1), &unclassified_self()));
        assert!(!trusts(LoginTrust::Nobody, &unclassified_self()));
        assert!(!trusts(LoginTrust::Agent(me), &Caller::Anonymous));
        assert!(trusts(LoginTrust::Any, &Caller::Anonymous));
    }

    #[test]
    fn the_lock_request_calls_the_login_process() {
        assert_eq!(
            Request::Lock.call(),
            (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "Lock",
                Body::Empty
            )
        );
    }

    #[test]
    fn the_log_out_and_power_dialog_requests_call_the_panels_of_the_login_process() {
        for (request, method) in [
            (Request::LogOut, "RequestLogOut"),
            (Request::PowerDialog, "RequestPowerDialog"),
        ] {
            assert_eq!(
                request.call(),
                (
                    "org.cupertino.Login1",
                    "/org/cupertino/Login1",
                    "org.cupertino.Login1",
                    method,
                    Body::Empty
                )
            );
        }
    }

    #[test]
    fn the_explicit_lock_request_calls_lock_now() {
        assert_eq!(
            Request::LockNow.call(),
            (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "LockNow",
                Body::Empty
            )
        );
    }

    #[test]
    fn the_power_key_request_calls_the_power_key_method_of_the_login_process_with_the_key_state() {
        assert_eq!(
            Request::PowerKey { pressed: true }.call(),
            (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "PowerKey",
                Body::Pressed(true)
            )
        );
        assert_eq!(
            Request::PowerKey { pressed: false }.call().4,
            Body::Pressed(false)
        );
    }

    #[test]
    fn media_keys_map_to_the_overlay_methods() {
        let call = |key, fine| {
            let (name, _, _, method, body) = Request::Osd { key, fine }.call();
            (name, method, body)
        };
        assert_eq!(
            call(OsdKey::VolumeUp, false),
            ("org.cupertino.Osd1", "VolumeStep", Body::Step(1, false))
        );
        assert_eq!(
            call(OsdKey::VolumeDown, true),
            ("org.cupertino.Osd1", "VolumeStep", Body::Step(-1, true))
        );
        assert_eq!(
            call(OsdKey::Mute, false),
            ("org.cupertino.Osd1", "ToggleMute", Body::Empty)
        );
        assert_eq!(
            call(OsdKey::MicMute, false),
            ("org.cupertino.Osd1", "ToggleMicrophoneMute", Body::Empty)
        );
        assert_eq!(
            call(OsdKey::BrightnessDown, false),
            (
                "org.cupertino.Osd1",
                "BrightnessStep",
                Body::Step(-1, false)
            )
        );
        assert_eq!(
            call(OsdKey::KeyboardBrightnessUp, true),
            (
                "org.cupertino.Osd1",
                "KeyboardBrightnessStep",
                Body::Step(1, true)
            )
        );
    }
}
