//! The lock screen's calls to the session manager's bus interface `org.cupertino.LoginManager1` (system bus): the
//! announcement that the session is up, and the manager's `Unlock` after a fast user switch and its restarts. The
//! manager authorises every call by the caller's process.

use std::fmt;

use cupertino_peer::signal::{from_owner, sent_by_bus};
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::MatchRule;

const BUS_NAME: &str = "org.cupertino.LoginManager1";
const OBJECT_PATH: &str = "/org/cupertino/LoginManager1";

/// Why the manager did not do what it was asked: it refused, or the call failed.
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

/// The connection to the manager.
#[derive(Clone)]
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

    /// Call `notify` on a reader thread for every `Unlock` the manager sends this process, and once with the error when the
    /// stream breaks or ends (no signal arrives after that). Only the owner of the manager's bus name is accepted as the
    /// sender, and only root may own it. A match rule cannot hold a well-known name as its sender: the bus delivers a signal
    /// sent to this process by its unique name whatever the rule says, so an `Unlock` from any other peer arrives too and is
    /// ignored here.
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
        let signals = from_owner(&self.connection, BUS_NAME, rule, |_| {
            eprintln!("cupertino-login: an Unlock not sent by the session manager was ignored")
        })
        .map_err(fail)?;
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
    /// Only the bus daemon's own `NameOwnerChanged` counts.
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
                    if message.as_ref().is_ok_and(|m| !sent_by_bus(m)) {
                        eprintln!(
                            "cupertino-login: a NameOwnerChanged not sent by the bus was ignored"
                        );
                        continue;
                    }
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
