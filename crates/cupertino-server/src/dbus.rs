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
    pub reply: Box<dyn FnOnce(Result<String, OwnerRefusal>) + Send>,
}

/// Why the owner of a bus name is not taken for the Wayland client.
#[derive(Debug)]
pub enum OwnerRefusal {
    /// No session bus to ask.
    NoBus,
    /// The client's connection named no process of its own (a connection the server made, or a kernel without
    /// `SO_PEERPIDFD`).
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
    /// The client's checks timed out or were busy `crate::owner_check::STRIKE_LIMIT` times: its later checks are
    /// refused at once.
    Stalled,
}

/// What the worker is asked to do.
enum Job {
    Call(Request),
    Owner(OwnerQuery),
}

/// Handle on the worker thread.
pub struct DbusHandle {
    tx: async_channel::Sender<Job>,
    login: std::sync::Arc<std::sync::Mutex<LoginTrust>>,
}

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
        let (tx, rx) = async_channel::unbounded::<Job>();
        let login_missing: LoginMissing = std::sync::Arc::new(login_missing);
        let login = std::sync::Arc::new(std::sync::Mutex::new(LoginTrust::Nobody));
        let worker_login = login.clone();
        let identifying = std::sync::Arc::new(AtomicUsize::new(0));
        let spawned = std::thread::Builder::new()
            .name("cupertino-dbus".into())
            .spawn(move || zbus::block_on(worker(rx, login_missing, worker_login, identifying)));
        match spawned {
            Ok(_) => Some(DbusHandle { tx, login }),
            Err(e) => {
                tracing::warn!(error = %e, "cannot start the D-Bus worker");
                None
            }
        }
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
        let queued = self.tx.try_send(Job::Call(what)).is_ok();
        if !queued {
            tracing::warn!(?what, "no session bus: cannot reach the shell process");
        }
        queued
    }

    /// Queue a check of who owns a bus name; false when the worker is gone (no session bus), and the reply is then
    /// never made.
    pub fn check_owner(&self, query: OwnerQuery) -> bool {
        self.tx.try_send(Job::Owner(query)).is_ok()
    }
}

type SharedLoginTrust = std::sync::Arc<std::sync::Mutex<LoginTrust>>;

async fn worker(
    rx: async_channel::Receiver<Job>,
    login_missing: LoginMissing,
    login: SharedLoginTrust,
    identifying: Identifying,
) {
    let conn = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            tracing::info!(error = %e, "no session bus: the shell toggles and media keys are unavailable");
            return;
        }
    };
    while let Ok(job) = rx.recv().await {
        let conn = conn.clone();
        let what = match job {
            Job::Call(what) => what,
            Job::Owner(OwnerQuery {
                name,
                client,
                reply,
            }) => {
                let identifying = identifying.clone();
                conn.executor()
                    .clone()
                    .spawn(
                        async move { reply(owner_of(&conn, &name, client, &identifying).await) },
                        "cupertino-owner-check",
                    )
                    .detach();
                continue;
            }
        };
        // a key's press and release must arrive in the order they happened, so those calls are awaited in turn
        if matches!(what, Request::PowerKey { .. }) {
            call(&conn, what, &login_missing, &login).await;
        } else {
            let login_missing = login_missing.clone();
            let login = login.clone();
            conn.executor()
                .clone()
                .spawn(
                    async move { call(&conn, what, &login_missing, &login).await },
                    "cupertino-shell-call",
                )
                .detach();
        }
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

/// How many identification threads are alive.
type Identifying = std::sync::Arc<AtomicUsize>;

/// How many identification threads may be alive at once, counting those still reading after their check timed out.
pub const IDENTIFY_LIMIT: usize = 16;

/// One alive identification thread, counted while it lives.
struct Alive(Identifying);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The answer of an identification thread.
type Identified = async_channel::Receiver<Result<(), OwnerRefusal>>;

/// Identify the owner the daemon named in `credentials` against `client` on a thread of its own, reading `proc_root`.
/// Reading `/proc` of a process the client chose can stall for good (a FUSE root), so each identification has a thread
/// that may be left behind, never the worker that makes the shell calls; with [`IDENTIFY_LIMIT`] threads alive the
/// check is refused as busy.
fn identify_owner(
    identifying: &Identifying,
    proc_root: PathBuf,
    credentials: ConnectionCredentials,
    client: ProcessHandle,
) -> Result<Identified, OwnerRefusal> {
    if identifying.fetch_add(1, Ordering::SeqCst) >= IDENTIFY_LIMIT {
        identifying.fetch_sub(1, Ordering::SeqCst);
        return Err(OwnerRefusal::Busy);
    }
    let alive = Alive(identifying.clone());
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
    identifying: &Identifying,
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
    identify_owner(identifying, PathBuf::from("/proc"), credentials, client)?
        .recv()
        .await
        .map_err(|_| OwnerRefusal::Unidentified)??;
    Ok(unique.to_string())
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
            } else {
                tracing::warn!("no login process on the bus: the log out shortcut ends the server");
                login_missing();
            }
            return;
        }
        tracing::warn!(?what, name, error = %e, "cannot reach the shell process");
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

    /// Ask the control panel process to show or hide its panel.
    pub fn toggle_control_panel(&mut self) {
        self.shell_call(Request::Toggle(Toggle::ControlPanel));
    }

    /// Ask the login process to lock the session. The server's private connection to the lock screen carries the request;
    /// the session bus is the way only while no lock screen is connected, so a process that took `org.cupertino.Login1`
    /// before the login process did cannot make the lock go missing.
    pub fn lock_session(&mut self) {
        if !self.sessions.lock_requested(false) {
            self.shell_call(Request::Lock);
        }
    }

    /// Ask the login process to lock the session because the person asked for it.
    pub fn lock_session_now(&mut self) {
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

    fn identify_self(
        identifying: &Identifying,
        proc_root: &Path,
    ) -> Result<Identified, OwnerRefusal> {
        identify_owner(
            identifying,
            proc_root.to_path_buf(),
            own_credentials(),
            ProcessHandle::open(std::process::id()).unwrap(),
        )
    }

    #[test]
    fn an_identification_that_stalls_leaves_another_to_be_answered() {
        let identifying = Identifying::default();
        let stalled = scratch_proc(true);
        let fine = scratch_proc(false);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_identifying = identifying.clone();
        let (stalled_root, fine_root) = (stalled.path().to_path_buf(), fine.path().to_path_buf());
        std::thread::spawn(move || {
            let stuck = identify_self(&worker_identifying, &stalled_root).unwrap();
            let answered = identify_self(&worker_identifying, &fine_root).unwrap();
            let _ = tx.send(answered.recv_blocking());
            drop(stuck);
        });
        let answer = rx.recv_timeout(std::time::Duration::from_secs(5));
        release(stalled.path(), &identifying);
        assert!(matches!(answer, Ok(Ok(Ok(())))), "{answer:?}");
    }

    #[test]
    fn the_identification_threads_alive_are_capped() {
        let identifying = Identifying::default();
        let stalled = scratch_proc(true);
        let fine = scratch_proc(false);
        let stuck: Vec<_> = (0..IDENTIFY_LIMIT)
            .map(|_| identify_self(&identifying, stalled.path()).expect("under the limit"))
            .collect();
        let refused = identify_self(&identifying, fine.path());
        release(stalled.path(), &identifying);
        assert!(matches!(refused, Err(OwnerRefusal::Busy)), "{refused:?}");
        assert!(stuck.iter().all(|s| s.recv_blocking().is_ok()));
        let again = identify_self(&identifying, fine.path()).expect("the threads are gone");
        assert!(matches!(again.recv_blocking(), Ok(Ok(()))));
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
