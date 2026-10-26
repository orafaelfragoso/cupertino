//! What logind asks of a lock screen and a session agent: lock the session (`loginctl lock-session`), prepare for sleep, the
//! delay inhibitor that holds sleep back until the lock is up, the block inhibitor on the power key, and the call that ends the
//! session. Restarting, shutting down and sleeping go through the session manager ([`crate::login_manager`]).
//!
//! The signals arrive on reader threads (the blocking `zbus` API has no pollable descriptor) and reach the caller through
//! the callback of [`Logind::listen`].

use std::fmt;
use std::sync::Arc;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::OwnedFd;
use zbus::{MatchRule, Message};

use cupertino_seat::{SessionLookup, SessionRef};

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";

/// Why a logind request failed or its signals stopped.
#[derive(Debug)]
pub struct Error {
    what: &'static str,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl Error {
    fn bus(what: &'static str) -> impl Fn(zbus::Error) -> Self {
        move |source| Error { what, source: Box::new(source) }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.what, self.source)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// Something logind wants the lock screen to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// `Lock` on this process's session.
    Lock,
    /// `PrepareForSleep(true)`: the system goes to sleep once the delay inhibitors are released.
    PrepareForSleep,
}

/// A delay inhibitor on sleep; the system may sleep once it is dropped.
pub struct Inhibitor(#[allow(dead_code)] OwnedFd);

/// This process's session and the system bus it is on.
pub struct Logind {
    connection: Connection,
    session: SessionRef,
    /// The session's `Id`, which `TerminateSession` takes.
    session_id: String,
}

impl Logind {
    /// Connect to the system bus and find the session of this process (`XDG_SESSION_ID`, else the process id).
    pub fn connect() -> Result<Self, Error> {
        let what = "finding the login session";
        let wrap = |e: cupertino_seat::Error| Error { what, source: Box::new(e) };
        let connection = cupertino_seat::connect().map_err(wrap)?;
        let lookup = SessionLookup::from_environment().map_err(wrap)?;
        let session = cupertino_seat::locate(&connection, &lookup).map_err(wrap)?;
        let session_id = cupertino_seat::inspect(&connection, &session).map_err(wrap)?.id;
        Ok(Logind { connection, session, session_id })
    }

    /// The session's `Id`.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The system bus connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Hold sleep back until the returned [`Inhibitor`] is dropped.
    pub fn inhibit_sleep(&self) -> Result<Inhibitor, Error> {
        self.inhibit("taking the sleep inhibitor", "sleep", "Lock the screen before the system sleeps", "delay")
    }

    /// Keep logind from powering off on the power key until the returned [`Inhibitor`] is dropped; the key then reaches the
    /// compositor, whose shortcut shows the power panel.
    pub fn block_power_key(&self) -> Result<Inhibitor, Error> {
        self.inhibit("taking the power key inhibitor", "handle-power-key", "Ask before shutting down", "block")
    }

    fn inhibit(&self, what: &'static str, lock: &str, why: &str, mode: &str) -> Result<Inhibitor, Error> {
        let reply = self.connection.call_method(Some(LOGIND), MANAGER_PATH, Some(MANAGER_INTERFACE), "Inhibit", &(lock, "cupertino-login", why, mode)).map_err(Error::bus(what))?;
        Ok(Inhibitor(reply.body().deserialize().map_err(Error::bus(what))?))
    }

    /// End this process's session (`TerminateSession`).
    pub fn terminate_session(&self) -> Result<(), Error> {
        self.connection.call_method(Some(LOGIND), MANAGER_PATH, Some(MANAGER_INTERFACE), "TerminateSession", &self.session_id.as_str()).map(drop).map_err(Error::bus("terminating the session"))
    }

    /// Start the reader threads. `notify` runs on them for every [`Signal`], and once with the error when a stream
    /// breaks or ends: after that no signal arrives any more.
    pub fn listen(&self, notify: impl Fn(Result<Signal, Error>) + Send + Sync + 'static) -> Result<(), Error> {
        let what = "subscribing to the login manager's signals";
        let notify = Arc::new(notify);
        let session_signals = cupertino_seat::logind::subscribe(&self.connection, &self.session).map_err(|e| Error { what, source: Box::new(e) })?;
        let sleep_rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(LOGIND)
            .and_then(|b| b.path(MANAGER_PATH))
            .and_then(|b| b.interface(MANAGER_INTERFACE))
            .and_then(|b| b.member("PrepareForSleep"))
            .map_err(Error::bus(what))?
            .build();
        let sleep_signals = MessageIterator::for_match_rule(sleep_rule, &self.connection, None).map_err(Error::bus(what))?;
        spawn(session_signals, Arc::clone(&notify), decode_lock)?;
        spawn(sleep_signals, notify, decode_sleep)
    }
}

fn spawn(signals: MessageIterator, notify: Arc<impl Fn(Result<Signal, Error>) + Send + Sync + 'static>, decode: fn(&Message) -> Result<Option<Signal>, zbus::Error>) -> Result<(), Error> {
    std::thread::Builder::new()
        .name("login-signals".into())
        .spawn(move || {
            for message in signals {
                match message.and_then(|m| decode(&m)) {
                    Ok(Some(signal)) => notify(Ok(signal)),
                    Ok(None) => {}
                    Err(e) => return notify(Err(Error::bus("reading a login manager signal")(e))),
                }
            }
            notify(Err(Error { what: "reading login manager signals", source: "the signal stream ended".into() }));
        })
        .map(drop)
        .map_err(|e| Error { what: "starting the signal reader", source: Box::new(e) })
}

fn decode_lock(message: &Message) -> Result<Option<Signal>, zbus::Error> {
    let header = message.header();
    let is_lock = header.interface().is_some_and(|i| i.as_str() == SESSION_INTERFACE) && header.member().is_some_and(|m| m.as_str() == "Lock");
    Ok(is_lock.then_some(Signal::Lock))
}

fn decode_sleep(message: &Message) -> Result<Option<Signal>, zbus::Error> {
    let going_to_sleep: bool = message.body().deserialize()?;
    Ok(going_to_sleep.then_some(Signal::PrepareForSleep))
}
