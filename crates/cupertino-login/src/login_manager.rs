//! The session agent's calls to the session manager's bus interface `org.cupertino.LoginManager1` (system bus): restart, shut
//! down and sleep, and the announcement that the session's agent is up. The manager authorises every call by the caller's
//! process; nothing here falls back to logind.

use std::fmt;

use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::MatchRule;

const BUS_NAME: &str = "org.cupertino.LoginManager1";
const OBJECT_PATH: &str = "/org/cupertino/LoginManager1";
/// The prefix of the names of the errors the interface answers with.
const ERROR_PREFIX: &str = "org.cupertino.LoginManager1.Error.";

/// What the manager is asked to do to the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Restart,
    ShutDown,
    Sleep,
}

impl Power {
    fn word(self) -> &'static str {
        match self {
            Power::Restart => "restart",
            Power::ShutDown => "shut-down",
            Power::Sleep => "sleep",
        }
    }
}

/// Why the manager did not do what it was asked: it refused, or the call failed.
#[derive(Debug)]
pub struct Error {
    what: &'static str,
    source: Box<zbus::Error>,
}

/// A refusal the manager answers with by name; each is an expected outcome, not a fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Another user's session is open and the caller may not end it.
    OtherUsersLoggedIn,
    /// A program holds a block inhibitor on the operation.
    Inhibited,
    /// The caller is not the person at the console, or the login settings forbid the operation.
    AccessDenied,
}

impl Error {
    /// The refusal this error is, or `None` for a failure (no manager on the bus, a broken connection, a failed operation).
    pub fn refusal(&self) -> Option<Refusal> {
        let zbus::Error::MethodError(name, _, _) = self.source.as_ref() else {
            return None;
        };
        match name.as_str().strip_prefix(ERROR_PREFIX)? {
            "OtherUsersLoggedIn" => Some(Refusal::OtherUsersLoggedIn),
            "Inhibited" => Some(Refusal::Inhibited),
            "AccessDenied" => Some(Refusal::AccessDenied),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.what, self.source)
    }
}

impl std::error::Error for Error {}

/// The connection to the manager.
pub struct LoginManager {
    connection: Connection,
}

impl LoginManager {
    /// Connect to the system bus. The manager may start later than the caller: each call finds it or fails.
    pub fn connect() -> Result<Self, Error> {
        let connection = cupertino_seat::connect().map_err(|e| Error {
            what: "connecting to the system bus",
            source: Box::new(zbus::Error::Failure(e.to_string())),
        })?;
        Ok(LoginManager { connection })
    }

    /// Restart, shut down or sleep the computer.
    pub fn request_power(&self, action: Power) -> Result<(), Error> {
        let what = "asking the session manager for a power action";
        self.connection
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(BUS_NAME),
                "RequestPower",
                &(action.word(),),
            )
            .map(drop)
            .map_err(|source| Error {
                what,
                source: Box::new(source),
            })
    }

    /// The login screen takes the console (`SwitchToLoginWindow`); the caller has locked its screen first.
    pub fn switch_to_login_window(&self) -> Result<(), Error> {
        self.connection
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(BUS_NAME),
                "SwitchToLoginWindow",
                &(),
            )
            .map(drop)
            .map_err(|source| Error {
                what: "switching to the login window",
                source: Box::new(source),
            })
    }

    /// Call `notify` on a reader thread for every `Unlock` the manager sends this process, and once with the error when the
    /// stream breaks or ends (no signal arrives after that). Only the owner of the manager's bus name is accepted as the
    /// sender, and only root may own it.
    pub fn watch_unlock(
        &self,
        notify: impl Fn(Result<(), Error>) + Send + 'static,
    ) -> Result<(), Error> {
        let what = "subscribing to the session manager's Unlock";
        let fail = |source: zbus::Error| Error {
            what,
            source: Box::new(source),
        };
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(BUS_NAME)
            .and_then(|b| b.path(OBJECT_PATH))
            .and_then(|b| b.interface(BUS_NAME))
            .and_then(|b| b.member("Unlock"))
            .map_err(fail)?
            .build();
        let signals =
            MessageIterator::for_match_rule(rule, &self.connection, None).map_err(fail)?;
        std::thread::Builder::new()
            .name("login-unlock".into())
            .spawn(move || {
                for message in signals {
                    match message {
                        Ok(_) => notify(Ok(())),
                        Err(source) => {
                            return notify(Err(Error {
                                what: "reading the session manager's Unlock",
                                source: Box::new(source),
                            }))
                        }
                    }
                }
                notify(Err(Error {
                    what: "reading the session manager's Unlock",
                    source: Box::new(zbus::Error::Failure("the signal stream ended".to_owned())),
                }));
            })
            .map(drop)
            .map_err(|e| Error {
                what,
                source: Box::new(zbus::Error::InputOutput(std::sync::Arc::new(e))),
            })
    }

    /// Call `notify` on a reader thread every time the manager's bus name gets an owner (the manager started or restarted; its
    /// table of agents is gone, so the session announces itself again), and once with the error when the stream breaks or ends.
    pub fn watch_owner(
        &self,
        notify: impl Fn(Result<(), Error>) + Send + 'static,
    ) -> Result<(), Error> {
        let what = "subscribing to the session manager's bus name";
        let fail = |source: zbus::Error| Error {
            what,
            source: Box::new(source),
        };
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.DBus")
            .and_then(|b| b.path("/org/freedesktop/DBus"))
            .and_then(|b| b.interface("org.freedesktop.DBus"))
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg(0, BUS_NAME))
            .map_err(fail)?
            .build();
        let signals =
            MessageIterator::for_match_rule(rule, &self.connection, None).map_err(fail)?;
        std::thread::Builder::new()
            .name("login-manager-owner".into())
            .spawn(move || {
                for message in signals {
                    match message.and_then(|m| m.body().deserialize::<(String, String, String)>()) {
                        Ok((_, _, new_owner)) if !new_owner.is_empty() => notify(Ok(())),
                        Ok(_) => {}
                        Err(source) => {
                            return notify(Err(Error {
                                what,
                                source: Box::new(source),
                            }))
                        }
                    }
                }
                notify(Err(Error {
                    what,
                    source: Box::new(zbus::Error::Failure("the signal stream ended".to_owned())),
                }));
            })
            .map(drop)
            .map_err(|e| Error {
                what,
                source: Box::new(zbus::Error::InputOutput(std::sync::Arc::new(e))),
            })
    }

    /// Tell the manager this session's agent is up (`SessionReady`).
    pub fn session_ready(&self) -> Result<(), Error> {
        self.connection
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(BUS_NAME),
                "SessionReady",
                &(),
            )
            .map(drop)
            .map_err(|source| Error {
                what: "announcing the session",
                source: Box::new(source),
            })
    }
}
