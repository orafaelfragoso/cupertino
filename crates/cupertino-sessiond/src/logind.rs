//! The manager's calls to systemd-logind on the system bus. The manager is root, so logind needs no polkit decision for any of
//! them. The calls block the manager's thread for the time logind takes, which is short: they only change which session is
//! in the foreground.

use std::fmt;

use cupertino_login_ipc::PowerAction;
use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;
use zbus::MatchRule;

use crate::caller::{CallerSession, SEAT};

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const SEAT_INTERFACE: &str = "org.freedesktop.login1.Seat";
/// The error logind answers `GetSessionByPID` with for a process outside every session.
const NO_SESSION_FOR_PID: &str = "org.freedesktop.login1.NoSessionForPID";
/// The error logind answers a restart, shut down or sleep that a block inhibitor stops.
const BLOCKED_BY_INHIBITOR: &str = "org.freedesktop.login1.BlockedByInhibitorLock";
/// `SD_LOGIND_ROOT_CHECK_INHIBITORS` of the `*WithFlags` methods (`org.freedesktop.login1(5)): without it logind
/// lets root override any inhibitor, and the manager is root.
const ROOT_CHECK_INHIBITORS: u64 = 1 << 0;

/// A logind call failed.
#[derive(Debug)]
pub struct Error {
    what: &'static str,
    source: Box<zbus::Error>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.what, self.source)
    }
}

impl std::error::Error for Error {}

impl Error {
    /// Whether logind refused because a program holds a block inhibitor on the operation.
    pub fn blocked_by_inhibitor(&self) -> bool {
        matches!(self.source.as_ref(), zbus::Error::MethodError(name, _, _) if name.as_str() == BLOCKED_BY_INHIBITOR)
    }
}

/// A session on the seat as logind describes it: what a manager that restarted reads to adopt the sessions of its earlier run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatSession {
    pub id: String,
    pub uid: u32,
    /// `Class`: `user`, `greeter`, ...
    pub class: String,
    /// `Service`: the PAM service that opened the session, which says who started it.
    pub service: String,
    /// `Leader`: the session's first process.
    pub leader: u32,
    /// `VTNr`.
    pub vt: u32,
    /// `State`: `online`, `active` or `closing`.
    pub state: String,
}

/// What logind tells the manager.
#[derive(Debug)]
pub enum Signal {
    /// `SessionRemoved`: the session is gone.
    SessionRemoved(String),
    /// The signal stream broke or ended; no signal follows. logind restarted, or the bus went away.
    Ended(String),
}

#[derive(Clone)]
pub struct Logind {
    connection: Connection,
}

impl Logind {
    /// Connect to the system bus.
    pub fn connect() -> Result<Self, cupertino_seat::Error> {
        Ok(Logind {
            connection: cupertino_seat::connect()?,
        })
    }

    /// Whether the session is in the foreground (`Active` of `org.freedesktop.login1.Session`).
    pub fn is_active(&self, session_id: &str) -> Result<bool, Error> {
        let fail = |source: zbus::Error| Error {
            what: "reading whether a session is active",
            source: Box::new(source),
        };
        let path: zbus::zvariant::OwnedObjectPath = self
            .connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                "GetSession",
                &session_id,
            )
            .map_err(fail)?
            .body()
            .deserialize()
            .map_err(fail)?;
        let value: zbus::zvariant::OwnedValue = self
            .connection
            .call_method(
                Some(LOGIND),
                path.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(SESSION_INTERFACE, "Active"),
            )
            .map_err(fail)?
            .body()
            .deserialize()
            .map_err(fail)?;
        bool::try_from(value).map_err(|e| fail(e.into()))
    }

    /// `TerminateSession(id): kill every process of the session. The design's final hooks end a session's processes after
    /// the quit sequence the same way.
    pub fn terminate_session(&self, session_id: &str) -> Result<(), Error> {
        self.connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                "TerminateSession",
                &session_id,
            )
            .map(drop)
            .map_err(|source| Error {
                what: "terminating a session",
                source: Box::new(source),
            })
    }

    /// Start a thread that calls `notify` for every `SessionRemoved`, and once with [`Signal::Ended`] when the stream ends.
    pub fn watch(&self, notify: impl Fn(Signal) + Send + 'static) -> Result<(), Error> {
        let fail = |source: zbus::Error| Error {
            what: "subscribing to logind's signals",
            source: Box::new(source),
        };
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(LOGIND)
            .and_then(|b| b.path(MANAGER_PATH))
            .and_then(|b| b.interface(MANAGER_INTERFACE))
            .and_then(|b| b.member("SessionRemoved"))
            .map_err(fail)?
            .build();
        let signals =
            MessageIterator::for_match_rule(rule, &self.connection, None).map_err(fail)?;
        std::thread::Builder::new()
            .name("logind-signals".into())
            .spawn(move || {
                for message in signals {
                    match message.and_then(|m| {
                        m.body()
                            .deserialize::<(String, zbus::zvariant::OwnedObjectPath)>()
                    }) {
                        Ok((id, _)) => notify(Signal::SessionRemoved(id)),
                        Err(e) => return notify(Signal::Ended(e.to_string())),
                    }
                }
                notify(Signal::Ended("the signal stream ended".to_owned()));
            })
            .map(drop)
            .map_err(|e| fail(zbus::Error::InputOutput(std::sync::Arc::new(e))))
    }

    /// `ActivateSession(id)`: bring the session to the foreground (a switch to its virtual terminal).
    pub fn activate_session(&self, session_id: &str) -> Result<(), Error> {
        self.connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                "ActivateSession",
                &session_id,
            )
            .map(drop)
            .map_err(|source| Error {
                what: "activating a session",
                source: Box::new(source),
            })
    }

    /// Restart, shut down or put the computer to sleep. Block inhibitors are honoured although the manager is root, and the
    /// call is not interactive.
    pub fn power(&self, action: PowerAction) -> Result<(), Error> {
        let (method, what) = match action {
            PowerAction::Restart => ("RebootWithFlags", "restarting the computer"),
            PowerAction::ShutDown => ("PowerOffWithFlags", "shutting the computer down"),
            PowerAction::Sleep => ("SuspendWithFlags", "putting the computer to sleep"),
        };
        self.connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                method,
                &ROOT_CHECK_INHIBITORS,
            )
            .map(drop)
            .map_err(|source| Error {
                what,
                source: Box::new(source),
            })
    }

    /// The session the process `pid` belongs to; `None` when it belongs to none.
    pub fn session_of_pid(&self, pid: u32) -> Result<Option<CallerSession>, Error> {
        let fail = |source: zbus::Error| Error {
            what: "finding the session of a process",
            source: Box::new(source),
        };
        let path: OwnedObjectPath = match self.connection.call_method(
            Some(LOGIND),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            "GetSessionByPID",
            &pid,
        ) {
            Ok(reply) => reply.body().deserialize().map_err(fail)?,
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == NO_SESSION_FOR_PID => {
                return Ok(None)
            }
            Err(source) => return Err(fail(source)),
        };
        let session = self.session_proxy(path)?;
        let (uid, _): (u32, OwnedObjectPath) = session.get_property("User").map_err(fail)?;
        let (seat, _): (String, OwnedObjectPath) = session.get_property("Seat").map_err(fail)?;
        Ok(Some(CallerSession {
            id: session.get_property("Id").map_err(fail)?,
            uid,
            class: session.get_property("Class").map_err(fail)?,
            seat,
            active: session.get_property("Active").map_err(fail)?,
            remote: session.get_property("Remote").map_err(fail)?,
            state: session.get_property("State").map_err(fail)?,
        }))
    }

    /// The sessions of class `user` on the seat, in the foreground or not.
    pub fn user_sessions(&self) -> Result<Vec<SeatSession>, Error> {
        Ok(self
            .seat_sessions()?
            .into_iter()
            .filter(|session| session.class == "user")
            .collect())
    }

    /// Every session on the seat with the properties adoption needs.
    pub fn seat_sessions(&self) -> Result<Vec<SeatSession>, Error> {
        let fail = |source: zbus::Error| Error {
            what: "listing the sessions",
            source: Box::new(source),
        };
        let listed: Vec<(String, u32, String, String, OwnedObjectPath)> = self
            .connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                "ListSessions",
                &(),
            )
            .map_err(fail)?
            .body()
            .deserialize()
            .map_err(fail)?;
        let mut sessions = Vec::new();
        for (id, uid, _, seat, path) in listed {
            if seat != SEAT {
                continue;
            }
            let session = self.session_proxy(path)?;
            sessions.push(SeatSession {
                id,
                uid,
                class: session.get_property("Class").map_err(fail)?,
                service: session.get_property("Service").map_err(fail)?,
                leader: session.get_property("Leader").map_err(fail)?,
                vt: session.get_property("VTNr").map_err(fail)?,
                state: session.get_property("State").map_err(fail)?,
            });
        }
        Ok(sessions)
    }

    /// The session in the foreground of the seat, `None` when the seat has none (the session that was in the foreground ended).
    pub fn active_session(&self) -> Result<Option<String>, Error> {
        let fail = |source: zbus::Error| Error {
            what: "reading the seat's active session",
            source: Box::new(source),
        };
        let seat: OwnedObjectPath = self
            .connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                "GetSeat",
                &SEAT,
            )
            .map_err(fail)?
            .body()
            .deserialize()
            .map_err(fail)?;
        let proxy = Builder::<Proxy>::new(&self.connection)
            .destination(LOGIND)
            .and_then(|builder| builder.path(seat))
            .and_then(|builder| builder.interface(SEAT_INTERFACE))
            .and_then(|builder| builder.cache_properties(CacheProperties::No).build())
            .map_err(fail)?;
        let (id, _): (String, OwnedObjectPath) =
            proxy.get_property("ActiveSession").map_err(fail)?;
        Ok((!id.is_empty()).then_some(id))
    }

    fn session_proxy(&self, path: OwnedObjectPath) -> Result<Proxy<'static>, Error> {
        // uncached: `Active` changes with every switch
        Builder::<Proxy>::new(&self.connection)
            .destination(LOGIND)
            .and_then(|builder| builder.path(path))
            .and_then(|builder| builder.interface(SESSION_INTERFACE))
            .and_then(|builder| builder.cache_properties(CacheProperties::No).build())
            .map_err(|source| Error {
                what: "creating a session proxy",
                source: Box::new(source),
            })
    }
}
