//! `org.cupertino.LoginManager1` on the system bus: what a session's agent asks of the root-owned manager. Every method is
//! authorised by the caller's process (the bus daemon names it, logind names its session, [`crate::caller`] decides), and every
//! method binds to the caller's own session: none takes a session argument.
//!
//! | Method | Needs | Does |
//! |---|---|---|
//! | `RequestPower(s action)` | the active user session | restart (`restart`), shut down (`shut-down`) or sleep (`sleep`) the computer |
//! | `SwitchToLoginWindow()` | the active user session | the login screen takes the console; the caller has locked its screen first |
//! | `SessionReady()` | a user session | the session's agent is up; the manager registers the caller as the session's agent |
//! | `SetSystemAccessibility(a{ss} options)` | an administrator at the console, through the installed settings application | writes the machine's accessibility defaults ([`crate::system_accessibility`]) |
//! | `CanSetSystemAccessibility() → b` | — | whether the caller would be allowed to (a hint for the settings application; the setter checks again) |
//!
//! The two accessibility methods take the caller from the `ProcessFD` the bus daemon gives (`cupertino_peer`): a daemon that
//! gives none is refused. The program, the session and the user are looked up through that descriptor, and the process is
//! checked to be still alive after them, so a process id that was reused cannot pass for the caller.
//!
//! The manager's only signal is `Unlock`, sent to one agent (the one that called `SessionReady` for the session) when the person
//! at the login screen has proved the session's password, so the agent unlocks without asking again.
//!
//! Restarting or shutting down the machine with other users logged in needs a decision that only the manager can take, so it is
//! the manager, as root, that calls logind for the agent. The service runs on the bus daemon's thread of the connection: a
//! method that needs the manager's state sends a [`Call`] to the main loop and waits for its answer, a bounded time.
//!
//! The caller's credentials are asked on a second connection to the bus ([`cupertino_peer::lookup::Lookup`]), never on the
//! one that reads the calls: there each answer, which carries a process descriptor, would queue behind the calls that came
//! first, and a flood of calls would fill the daemon's queue for the service with them, so that the daemon refused every
//! other caller's call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_defaults::accessibility::{SystemAccessibility, SYSTEM_KEYS};
use cupertino_peer::lookup::Lookup;
use cupertino_peer::Caller;

use cupertino_login_ipc::{PowerAction, PowerRefusal};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Header;

use crate::caller::{authorise, CallerSession, Denial, Need};
use crate::system_accessibility::{Limiter, MAX_FIELD};

pub const BUS_NAME: &str = "org.cupertino.LoginManager1";
pub const OBJECT_PATH: &str = "/org/cupertino/LoginManager1";

/// How long a method waits for the main loop before it answers `Failed` (approximate, ours; the loop answers in
/// milliseconds unless it is waiting for logind).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// What the manager's main loop is asked, with where to answer.
#[derive(Debug)]
pub enum Call {
    Power {
        session: CallerSession,
        action: PowerAction,
        reply: SyncSender<Result<(), PowerRefusal>>,
    },
    Ready {
        session: CallerSession,
        agent: String,
        reply: SyncSender<Result<(), ReadyRefusal>>,
    },
    SwitchToLoginWindow {
        session: CallerSession,
        reply: SyncSender<Result<(), SwitchRefusal>>,
    },
}

/// Why `SwitchToLoginWindow` is not carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchRefusal {
    /// The login screen's session is not running.
    NoLoginScreen,
    /// logind refused the switch; the cause is in the journal.
    Failed,
}

/// Why `SessionReady` is not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyRefusal {
    /// The manager did not start the caller's session (a session of a getty, for instance).
    UnknownSession,
}

/// What the service asks logind: the session of a process. Behind a trait so the tests answer without logind.
pub trait Authorizer: Send + Sync {
    /// The logind session of process `pid`; `None` when it belongs to none. An `Err` is a failure to find out, not a refusal.
    fn session_of(&self, pid: u32) -> Result<Option<CallerSession>, String>;
    /// Whether the account `uid` is an administrator, asked at every call (`crate::users::is_administrator`).
    fn is_administrator(&self, uid: u32) -> Result<bool, String>;
}

/// Where the machine's files are and which program is the settings application.
#[derive(Clone)]
pub struct MachineFiles {
    /// `/etc/cupertino`.
    pub dir: PathBuf,
    /// Whether a program is the installed settings application, by its path as `/proc/<pid>/exe` names it and the
    /// metadata of the file the process runs (through that link, so a file put in its place later does not count).
    pub settings_program: SettingsProgram,
}

/// See [`MachineFiles::settings_program`].
pub type SettingsProgram = Arc<dyn Fn(&Path, &std::fs::Metadata) -> bool + Send + Sync>;

/// Whether a program file is one only root can change: owned by root and writable by no group or other user, so a
/// program of the user's put under a writable install prefix is not taken for the installed one.
pub fn only_root_changes(file: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    is_root_only(file.uid(), file.mode())
}

pub use cupertino_defaults::system::is_root_only;

/// What a method call answers with when it fails; the names are `org.cupertino.LoginManager1.Error.<variant>`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.LoginManager1.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The caller is not the person at the console (see [`crate::caller::Denial`]).
    AccessDenied(String),
    InvalidArgs(String),
    /// See [`PowerRefusal::OtherUsersLoggedIn`].
    OtherUsersLoggedIn(String),
    /// See [`PowerRefusal::Inhibited`].
    Inhibited(String),
    /// The manager or logind failed; the cause is in the journal.
    Failed(String),
    /// The manager did not start the caller's session.
    UnknownSession(String),
    /// The caller's user is not an administrator.
    NotAdministrator(String),
    /// The caller is not the person at the console (a session in the background, over the network, or none).
    NotAtConsole(String),
    /// The caller's user, or anyone, wrote the machine's settings a moment ago.
    TooManyWrites(String),
}

impl From<PowerRefusal> for Error {
    fn from(refusal: PowerRefusal) -> Self {
        match refusal {
            PowerRefusal::OtherUsersLoggedIn => {
                Error::OtherUsersLoggedIn("another user's session is open".to_owned())
            }
            PowerRefusal::Inhibited => {
                Error::Inhibited("a program holds an inhibitor on the operation".to_owned())
            }
            PowerRefusal::Disabled => {
                Error::AccessDenied("the login settings disable the operation".to_owned())
            }
            PowerRefusal::Failed => Error::Failed("the operation failed".to_owned()),
        }
    }
}

/// Where the service sends a [`Call`]; `false` when the main loop is gone.
pub type Sink = Arc<dyn Fn(Call) -> bool + Send + Sync>;

/// The exported object.
pub struct LoginManager {
    authorizer: Arc<dyn Authorizer>,
    sink: Sink,
    machine: MachineFiles,
    limiter: Mutex<Limiter>,
    /// Asks the bus daemon who callers are, on a connection of its own.
    lookup: Lookup,
}

impl LoginManager {
    /// The session behind the caller of `header` and the caller's unique name, if the caller meets `need`.
    async fn caller(
        &self,
        header: &Header<'_>,
        need: Need,
    ) -> Result<(CallerSession, String), Error> {
        let sender = header
            .sender()
            .ok_or_else(|| Error::Failed("the call has no sender".to_owned()))?;
        // the bus daemon names the process behind the connection: the caller cannot choose it
        let credentials = self
            .lookup
            .credentials(sender)
            .await
            .map_err(|e| Error::Failed(format!("reading the caller's credentials: {e}")))?;
        let pid = credentials.process_id().ok_or_else(|| {
            Error::Failed("the bus did not report the caller's process".to_owned())
        })?;
        let uid = credentials
            .unix_user_id()
            .ok_or_else(|| Error::Failed("the bus did not report the caller's user".to_owned()))?;
        let session = self
            .authorizer
            .session_of(pid)
            .map_err(|e| Error::Failed(format!("finding the caller's session: {e}")))?;
        let session = authorise(session, uid, need)
            .map_err(|denial| Error::AccessDenied(denial.to_string()))?;
        Ok((session, sender.to_string()))
    }

    /// The user of the caller of `header`, when the caller is the installed settings application of an administrator at
    /// the console (the account's groups asked now).
    async fn console_administrator(&self, header: &Header<'_>) -> Result<u32, Error> {
        let sender = header
            .sender()
            .ok_or_else(|| Error::Failed("the call has no sender".to_owned()))?;
        let credentials = self
            .lookup
            .credentials(sender)
            .await
            .map_err(|e| Error::Failed(format!("reading the caller's credentials: {e}")))?;
        let uid = credentials
            .unix_user_id()
            .ok_or_else(|| Error::Failed("the bus did not report the caller's user".to_owned()))?;
        let Caller::Process(caller) = Caller::from_process_fd(&credentials, Path::new("/proc"))
        else {
            return Err(Error::AccessDenied(
                "the bus gave no process descriptor for the caller, or its process cannot be checked".to_owned(),
            ));
        };
        let file = std::fs::metadata(
            Path::new("/proc")
                .join(caller.process.pid.to_string())
                .join("exe"),
        );
        if caller.sandbox.is_some()
            || !caller
                .exe
                .as_deref()
                .zip(file.as_ref().ok())
                .is_some_and(|(exe, file)| (self.machine.settings_program)(exe, file))
        {
            return Err(Error::AccessDenied(
                "only the settings application changes the machine's settings".to_owned(),
            ));
        }
        let session = self
            .authorizer
            .session_of(caller.process.pid)
            .map_err(|e| Error::Failed(format!("finding the caller's session: {e}")))?;
        authorise(session, uid, Need::Console).map_err(|denial| match denial {
            Denial::OtherUser => Error::AccessDenied(denial.to_string()),
            _ => Error::NotAtConsole(denial.to_string()),
        })?;
        let administrator = self
            .authorizer
            .is_administrator(uid)
            .map_err(|e| Error::Failed(format!("finding the caller's groups: {e}")))?;
        if !administrator {
            return Err(Error::NotAdministrator(
                "the caller's user is not an administrator".to_owned(),
            ));
        }
        // the lookups went by the process id: it must still be the process the descriptor holds
        if caller.process.has_exited() {
            return Err(Error::AccessDenied(
                "the caller ended while it was checked".to_owned(),
            ));
        }
        Ok(uid)
    }

    /// Send a command and wait for the answer.
    fn ask<T>(&self, command: impl FnOnce(SyncSender<T>) -> Call) -> Result<T, Error> {
        let (reply, receiver) = sync_channel(1);
        if !(self.sink)(command(reply)) {
            return Err(Error::Failed(
                "the session manager is shutting down".to_owned(),
            ));
        }
        receiver
            .recv_timeout(ANSWER_TIMEOUT)
            .map_err(|_| Error::Failed("the session manager did not answer".to_owned()))
    }
}

#[zbus::interface(name = "org.cupertino.LoginManager1")]
impl LoginManager {
    /// Restart (`restart`), shut down (`shut-down`) or sleep (`sleep`). Refused with `OtherUsersLoggedIn` while another user's
    /// session is open (unless the caller is an administrator), and with `Inhibited` while a program blocks the operation.
    async fn request_power(
        &self,
        #[zbus(header)] header: Header<'_>,
        action: &str,
    ) -> Result<(), Error> {
        let action = match action {
            "restart" => PowerAction::Restart,
            "shut-down" => PowerAction::ShutDown,
            "sleep" => PowerAction::Sleep,
            _ => {
                return Err(Error::InvalidArgs(
                    "the action is restart, shut-down or sleep".to_owned(),
                ))
            }
        };
        let (session, _) = self.caller(&header, Need::ActiveUserSession).await?;
        self.ask(|reply| Call::Power {
            session,
            action,
            reply,
        })?
        .map_err(Error::from)
    }

    /// The login screen takes the console. The caller is the foreground session and has locked its screen (the agent does both
    /// in that order); a request while another switch is under way is ignored.
    async fn switch_to_login_window(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> Result<(), Error> {
        let (session, _) = self.caller(&header, Need::ActiveUserSession).await?;
        self.ask(|reply| Call::SwitchToLoginWindow { session, reply })?
            .map_err(|refusal| match refusal {
                SwitchRefusal::NoLoginScreen => {
                    Error::Failed("the login screen is not running".to_owned())
                }
                SwitchRefusal::Failed => Error::Failed("the switch failed".to_owned()),
            })
    }

    /// The caller's session is up and the caller is its agent. Repeated calls (the agent restarted) register the new caller.
    async fn session_ready(&self, #[zbus(header)] header: Header<'_>) -> Result<(), Error> {
        let (session, agent) = self.caller(&header, Need::UserSession).await?;
        self.ask(|reply| Call::Ready {
            session,
            agent,
            reply,
        })?
        .map_err(|ReadyRefusal::UnknownSession| {
            Error::UnknownSession("the session manager did not start this session".to_owned())
        })
    }

    /// Set the machine's accessibility defaults: `options` are keys of the machine's file with values as the file writes
    /// them (`reduce_motion` `on`), each replacing the file's; the others keep theirs. Refused, in this order: a call over
    /// the bounds (`InvalidArgs`), a caller that is not the installed settings application of an administrator at the
    /// console (`AccessDenied`, `NotAtConsole`, `NotAdministrator`), a key or value the file does not take (`InvalidArgs`), a
    /// write too soon after the last (`TooManyWrites`): only a call that would write counts against the limit.
    async fn set_system_accessibility(
        &self,
        #[zbus(header)] header: Header<'_>,
        options: HashMap<String, String>,
    ) -> Result<(), Error> {
        if options.len() > SYSTEM_KEYS.len()
            || options
                .iter()
                .any(|(k, v)| k.len() > MAX_FIELD || v.len() > MAX_FIELD)
        {
            return Err(Error::InvalidArgs(
                "too many settings, or a setting too long".to_owned(),
            ));
        }
        let uid = self.console_administrator(&header).await?;
        let path = self
            .machine
            .dir
            .join(cupertino_defaults::accessibility::ACCESSIBILITY_FILE);
        let current = match cupertino_defaults::system::read_root_file(&path) {
            Ok(text) => SystemAccessibility::from_text(text.as_deref().unwrap_or("")).0,
            Err(e) => {
                eprintln!(
                    "cupertino-sessiond: replacing {}, which is no file of root's: {e}",
                    path.display()
                );
                SystemAccessibility::default()
            }
        };
        let settings =
            crate::system_accessibility::apply(current, &options).map_err(Error::InvalidArgs)?;
        if !self
            .limiter
            .lock()
            .map_err(|_| Error::Failed("the write limit is unusable".to_owned()))?
            .admit(uid, Instant::now())
        {
            return Err(Error::TooManyWrites(
                "the machine's settings were written a moment ago".to_owned(),
            ));
        }
        crate::system_accessibility::write(&self.machine.dir, &settings).map_err(|e| {
            eprintln!("cupertino-sessiond: cannot write the machine's accessibility settings: {e}");
            Error::Failed("the machine's settings could not be written".to_owned())
        })?;
        eprintln!(
            "cupertino-sessiond: user {uid} set the machine's accessibility settings {:?}",
            options.keys().collect::<Vec<_>>()
        );
        Ok(())
    }

    /// Whether the caller may set the machine's accessibility defaults now: a hint for the settings application, which
    /// shows its control only then; [`Self::set_system_accessibility`] checks again.
    async fn can_set_system_accessibility(&self, #[zbus(header)] header: Header<'_>) -> bool {
        self.console_administrator(&header).await.is_ok()
    }
}

/// Send `Unlock` to the agent `agent` (a unique bus name from `SessionReady`) and to no one else.
pub fn unlock(connection: &zbus::blocking::Connection, agent: &str) -> zbus::Result<()> {
    connection.emit_signal(Some(agent), OBJECT_PATH, BUS_NAME, "Unlock", &())
}

/// Export the interface on the bus at `address` and take [`BUS_NAME`], never queued behind another owner. The interface is
/// served while the returned connection lives; a second connection to the bus asks who the callers are.
pub fn serve(
    address: zbus::Address,
    authorizer: Arc<dyn Authorizer>,
    sink: Sink,
    machine: MachineFiles,
) -> zbus::Result<zbus::blocking::Connection> {
    let lookup = Lookup::connect(address.clone())?;
    let connection = zbus::blocking::connection::Builder::address(address)?
        .serve_at(
            OBJECT_PATH,
            LoginManager {
                authorizer,
                sink,
                machine,
                limiter: Mutex::new(Limiter::default()),
                lookup,
            },
        )?
        .build()?;
    match connection.request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())? {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
        RequestNameReply::Exists | RequestNameReply::InQueue => return Err(zbus::Error::NameTaken),
    }
    if let Some(notice) = own_credentials(&connection)
        .as_ref()
        .and_then(cupertino_peer::process_fd_notice)
    {
        eprintln!("cupertino-sessiond: {notice}: the accessibility settings of the login screen cannot be changed");
    }
    Ok(connection)
}

/// The credentials the bus gives for `connection` itself; `None` when it does not answer.
fn own_credentials(
    connection: &zbus::blocking::Connection,
) -> Option<zbus::fdo::ConnectionCredentials> {
    let own = connection.unique_name()?.clone();
    zbus::blocking::fdo::DBusProxy::new(connection)
        .ok()?
        .get_connection_credentials(own.into())
        .ok()
}
