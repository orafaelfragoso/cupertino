//! `org.cupertino.LoginManager1` on the system bus: what a session's agent asks of the root-owned manager. Every method is
//! authorised by the caller's process (the bus daemon names it, logind names its session, [`crate::caller`] decides), and every
//! method binds to the caller's own session: none takes a session argument.
//!
//! | Method | Needs | Does |
//! |---|---|---|
//! | `RequestPower(s action)` | the active user session | restart (`restart`), shut down (`shut-down`) or sleep (`sleep`) the computer |
//! | `SwitchToLoginWindow()` | the active user session | the login screen takes the console; the caller has locked its screen first |
//! | `SessionReady()` | a user session | the session's agent is up; the manager registers the caller as the session's agent |
//!
//! The manager's only signal is `Unlock`, sent to one agent (the one that called `SessionReady` for the session) when the person
//! at the login screen has proved the session's password, so the agent unlocks without asking again.
//!
//! Restarting or shutting down the machine with other users logged in needs a decision that only the manager can take, so it is
//! the manager, as root, that calls logind for the agent. The service runs on the bus daemon's thread of the connection: a
//! method that needs the manager's state sends a [`Call`] to the main loop and waits for its answer, a bounded time.

use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::time::Duration;

use cupertino_login_ipc::{PowerAction, PowerRefusal};
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::Connection;

use crate::caller::{authorise, CallerSession, Need};

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
}

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
}

impl LoginManager {
    /// The session behind the caller of `header` and the caller's unique name, if the caller meets `need`.
    async fn caller(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        need: Need,
    ) -> Result<(CallerSession, String), Error> {
        let sender = header
            .sender()
            .ok_or_else(|| Error::Failed("the call has no sender".to_owned()))?;
        let bus = fdo::DBusProxy::new(connection)
            .await
            .map_err(|e| Error::Failed(format!("connecting to the bus daemon: {e}")))?;
        // the bus daemon names the process behind the connection: the caller cannot choose it
        let credentials = bus
            .get_connection_credentials(BusName::from(sender.clone()))
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
        #[zbus(connection)] connection: &Connection,
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
        let (session, _) = self
            .caller(&header, connection, Need::ActiveUserSession)
            .await?;
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
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        let (session, _) = self
            .caller(&header, connection, Need::ActiveUserSession)
            .await?;
        self.ask(|reply| Call::SwitchToLoginWindow { session, reply })?
            .map_err(|refusal| match refusal {
                SwitchRefusal::NoLoginScreen => {
                    Error::Failed("the login screen is not running".to_owned())
                }
                SwitchRefusal::Failed => Error::Failed("the switch failed".to_owned()),
            })
    }

    /// The caller's session is up and the caller is its agent. Repeated calls (the agent restarted) register the new caller.
    async fn session_ready(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), Error> {
        let (session, agent) = self.caller(&header, connection, Need::UserSession).await?;
        self.ask(|reply| Call::Ready {
            session,
            agent,
            reply,
        })?
        .map_err(|ReadyRefusal::UnknownSession| {
            Error::UnknownSession("the session manager did not start this session".to_owned())
        })
    }
}

/// Send `Unlock` to the agent `agent` (a unique bus name from `SessionReady`) and to no one else.
pub fn unlock(connection: &zbus::blocking::Connection, agent: &str) -> zbus::Result<()> {
    connection.emit_signal(Some(agent), OBJECT_PATH, BUS_NAME, "Unlock", &())
}

/// Export the interface on the bus `builder` connects to and take [`BUS_NAME`], never queued behind another owner. The
/// interface is served while the returned connection lives.
pub fn serve(
    builder: zbus::blocking::connection::Builder<'_>,
    authorizer: Arc<dyn Authorizer>,
    sink: Sink,
) -> zbus::Result<zbus::blocking::Connection> {
    let connection = builder
        .serve_at(OBJECT_PATH, LoginManager { authorizer, sink })?
        .build()?;
    match connection.request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())? {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(connection),
        RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
    }
}
