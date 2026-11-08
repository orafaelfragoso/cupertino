//! What logind asks of a lock screen and a session agent: lock the session (`loginctl lock-session`), prepare for sleep, the
//! delay inhibitor that holds sleep back until the lock is up, the block inhibitor on the power key, and the call that ends the
//! session. Restarting, shutting down and sleeping go through the session manager ([`crate::login_manager`]); when there is none
//! (the session was started by another display manager) the process asks logind itself ([`Logind::power`]), and logind's
//! answers say which of them the menu may offer ([`Logind::can`]).
//!
//! The signals arrive on reader threads (the blocking `zbus` API has no pollable descriptor) and reach the caller through
//! the callback of [`Logind::listen`]. Each comes from logind only: the session's signals are bound to logind's unique name,
//! and `PrepareForSleep` is checked against the owner of logind's name when it arrives, since a rule naming the well-known
//! name keeps out nothing that is sent to this process directly.

use cupertino_peer::signal::sent_by_owner;
use std::fmt;
use std::sync::Arc;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::OwnedFd;
use zbus::{MatchRule, Message};

use cupertino_seat::{SessionLookup, SessionRef};

use crate::capabilities::Answer;
use crate::login_manager::Power;

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
        move |source| Error {
            what,
            source: Box::new(source),
        }
    }
}

impl Error {
    /// logind answered with an error of its own (access denied, a block inhibitor, an operation already running): the
    /// request was understood and refused, which is an outcome, not a fault of the connection.
    pub fn is_refusal(&self) -> bool {
        matches!(
            self.source.downcast_ref::<zbus::Error>(),
            Some(zbus::Error::MethodError(..))
        )
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
#[derive(Clone)]
pub struct Logind {
    connection: Connection,
    session: SessionRef,
    /// The session's `Id`, which `TerminateSession` takes.
    session_id: String,
}

/// Whether `e` says that the machine has no system bus or logind has no session for this process, rather than that a
/// request failed. Only a lookup by process id can say so: a login (`pam_systemd`) sets `XDG_SESSION_ID`, and with it
/// set every failure is a fault to retry.
fn is_absent(lookup: &SessionLookup, e: &cupertino_seat::Error) -> bool {
    let (SessionLookup::ByPid(_), cupertino_seat::Error::Bus { source, .. }) = (lookup, e) else {
        return false;
    };
    match &**source {
        zbus::Error::MethodError(name, ..) => {
            name.as_str() == "org.freedesktop.login1.NoSessionForPID"
        }
        zbus::Error::Connection(io, _) | zbus::Error::InputOutput(io) => matches!(
            io.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ),
        _ => false,
    }
}

impl Logind {
    /// Connect to the system bus and find the session of this process (`XDG_SESSION_ID`, else the process id).
    /// `None` when there is no session to lock: `XDG_SESSION_ID` is unset and there is no system bus or logind knows no
    /// session of this process.
    pub fn connect() -> Result<Option<Self>, Error> {
        let what = "finding the login session";
        let wrap = |e: cupertino_seat::Error| Error {
            what,
            source: Box::new(e),
        };
        let lookup = SessionLookup::from_environment().map_err(wrap)?;
        let connection = match cupertino_seat::connect() {
            Err(e) if is_absent(&lookup, &e) => return Ok(None),
            connection => connection.map_err(wrap)?,
        };
        let session = match cupertino_seat::locate(&connection, &lookup) {
            Err(e) if is_absent(&lookup, &e) => return Ok(None),
            session => session.map_err(wrap)?,
        };
        let session_id = cupertino_seat::inspect(&connection, &session)
            .map_err(wrap)?
            .id;
        Ok(Some(Logind {
            connection,
            session,
            session_id,
        }))
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
        self.inhibit(
            "taking the sleep inhibitor",
            "sleep",
            "Lock the screen before the system sleeps",
            "delay",
        )
    }

    /// Keep logind from powering off on the power key until the returned [`Inhibitor`] is dropped; the key then reaches the
    /// compositor, whose shortcut shows the power panel.
    pub fn block_power_key(&self) -> Result<Inhibitor, Error> {
        self.inhibit(
            "taking the power key inhibitor",
            "handle-power-key",
            "Ask before shutting down",
            "block",
        )
    }

    fn inhibit(
        &self,
        what: &'static str,
        lock: &str,
        why: &str,
        mode: &str,
    ) -> Result<Inhibitor, Error> {
        let reply = self
            .connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                "Inhibit",
                &(lock, "cupertino-login", why, mode),
            )
            .map_err(Error::bus(what))?;
        Ok(Inhibitor(
            reply.body().deserialize().map_err(Error::bus(what))?,
        ))
    }

    /// Whether the machine can `action` and whether the person may (`CanSuspend`, `CanReboot`, `CanPowerOff`).
    pub fn can(&self, action: Power) -> Result<Answer, Error> {
        let (method, what) = match action {
            Power::Sleep => ("CanSuspend", "asking logind whether the computer can sleep"),
            Power::Restart => (
                "CanReboot",
                "asking logind whether the computer can restart",
            ),
            Power::ShutDown => (
                "CanPowerOff",
                "asking logind whether the computer can shut down",
            ),
        };
        let reply = self
            .connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                method,
                &(),
            )
            .map_err(Error::bus(what))?;
        let text: String = reply.body().deserialize().map_err(Error::bus(what))?;
        Ok(Answer::from_logind(&text))
    }

    /// Suspend, restart or power off the computer through logind (`Suspend`, `Reboot`, `PowerOff`), without an
    /// authentication dialog: logind decides from the open sessions and the block inhibitors, and refuses what the person may
    /// not do ([`Error::is_refusal`]).
    pub fn power(&self, action: Power) -> Result<(), Error> {
        let (method, what) = match action {
            Power::Sleep => ("Suspend", "putting the computer to sleep through logind"),
            Power::Restart => ("Reboot", "restarting the computer through logind"),
            Power::ShutDown => ("PowerOff", "shutting the computer down through logind"),
        };
        self.connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                method,
                &(false,),
            )
            .map(drop)
            .map_err(Error::bus(what))
    }

    /// End this process's session (`TerminateSession`).
    pub fn terminate_session(&self) -> Result<(), Error> {
        self.connection
            .call_method(
                Some(LOGIND),
                MANAGER_PATH,
                Some(MANAGER_INTERFACE),
                "TerminateSession",
                &self.session_id.as_str(),
            )
            .map(drop)
            .map_err(Error::bus("terminating the session"))
    }

    /// Start the reader threads. `notify` runs on them for every [`Signal`], and once with the error when a stream
    /// breaks or ends: after that no signal arrives any more.
    pub fn listen(
        &self,
        notify: impl Fn(Result<Signal, Error>) + Send + Sync + 'static,
    ) -> Result<(), Error> {
        let what = "subscribing to the login manager's signals";
        let notify = Arc::new(notify);
        let session_signals = cupertino_seat::logind::subscribe(&self.connection, &self.session)
            .map_err(|e| Error {
                what,
                source: Box::new(e),
            })?;
        let sleep_rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(LOGIND)
            .and_then(|b| b.path(MANAGER_PATH))
            .and_then(|b| b.interface(MANAGER_INTERFACE))
            .and_then(|b| b.member("PrepareForSleep"))
            .map_err(Error::bus(what))?
            .build();
        let connection = self.connection.clone();
        let sleep_signals = MessageIterator::for_match_rule(sleep_rule, &self.connection, None)
            .map_err(Error::bus(what))?
            .filter(move |message| match message {
                Ok(message) if !sent_by_owner(&connection, message, LOGIND) => {
                    eprintln!("cupertino-login: a PrepareForSleep not sent by logind was ignored");
                    false
                }
                _ => true,
            });
        spawn(session_signals, Arc::clone(&notify), decode_lock)?;
        spawn(sleep_signals, notify, decode_sleep)
    }
}

fn spawn(
    signals: impl Iterator<Item = zbus::Result<Message>> + Send + 'static,
    notify: Arc<impl Fn(Result<Signal, Error>) + Send + Sync + 'static>,
    decode: fn(&Message) -> Result<Option<Signal>, zbus::Error>,
) -> Result<(), Error> {
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
            notify(Err(Error {
                what: "reading login manager signals",
                source: "the signal stream ended".into(),
            }));
        })
        .map(drop)
        .map_err(|e| Error {
            what: "starting the signal reader",
            source: Box::new(e),
        })
}

fn decode_lock(message: &Message) -> Result<Option<Signal>, zbus::Error> {
    let header = message.header();
    let is_lock = header
        .interface()
        .is_some_and(|i| i.as_str() == SESSION_INTERFACE)
        && header.member().is_some_and(|m| m.as_str() == "Lock");
    Ok(is_lock.then_some(Signal::Lock))
}

fn decode_sleep(message: &Message) -> Result<Option<Signal>, zbus::Error> {
    let going_to_sleep: bool = message.body().deserialize()?;
    Ok(going_to_sleep.then_some(Signal::PrepareForSleep))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus(source: zbus::Error) -> cupertino_seat::Error {
        cupertino_seat::Error::Bus {
            what: "test",
            source: Box::new(source),
        }
    }

    #[test]
    fn an_error_of_logind_is_a_refusal_and_a_broken_connection_is_not() {
        let call = Message::method_call("/", "Reboot")
            .unwrap()
            .build(&())
            .unwrap();
        let refused = Error {
            what: "test",
            source: Box::new(zbus::Error::MethodError(
                "org.freedesktop.DBus.Error.AccessDenied"
                    .try_into()
                    .unwrap(),
                Some("Interactive authentication required.".into()),
                call,
            )),
        };
        assert!(refused.is_refusal());
        let broken = Error {
            what: "test",
            source: Box::new(zbus::Error::Failure("gone".into())),
        };
        assert!(!broken.is_refusal());
    }

    #[test]
    fn only_a_lookup_by_process_id_can_find_no_session() {
        let io = |kind| bus(zbus::Error::InputOutput(std::io::Error::from(kind).into()));
        let by_pid = SessionLookup::ByPid(42);
        let by_id = SessionLookup::ById("7".into());
        assert!(is_absent(&by_pid, &io(std::io::ErrorKind::NotFound)));
        assert!(is_absent(
            &by_pid,
            &io(std::io::ErrorKind::ConnectionRefused)
        ));
        assert!(!is_absent(
            &by_pid,
            &io(std::io::ErrorKind::PermissionDenied)
        ));
        assert!(!is_absent(&by_pid, &cupertino_seat::Error::SignalsEnded));
        assert!(!is_absent(&by_id, &io(std::io::ErrorKind::NotFound)));
        assert!(!is_absent(
            &by_id,
            &io(std::io::ErrorKind::ConnectionRefused)
        ));
    }

    struct FakeManager(String);

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl FakeManager {
        fn get_session(&self, _id: String) -> zbus::zvariant::OwnedObjectPath {
            self.0.as_str().try_into().unwrap()
        }
    }

    #[test]
    fn only_logind_locks_the_session_or_puts_it_to_sleep() {
        use std::sync::mpsc;
        use std::time::Duration;
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let path = "/org/freedesktop/login1/session/c2";
        let logind = bus
            .blocking_builder()
            .name(LOGIND)
            .unwrap()
            .serve_at(MANAGER_PATH, FakeManager(path.to_owned()))
            .unwrap()
            .build()
            .unwrap();
        let connection = bus.connect();
        let session =
            cupertino_seat::locate(&connection, &SessionLookup::ById("c2".into())).unwrap();
        let screen = Logind {
            connection,
            session,
            session_id: "c2".into(),
        };
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        screen
            .listen(move |signal| {
                let _ = tx.lock().unwrap().send(signal.map_err(|e| e.to_string()));
            })
            .unwrap();
        let to = screen.connection.unique_name().unwrap().to_string();
        let lock = |from: &Connection, to: Option<&str>| {
            from.emit_signal(to, path, SESSION_INTERFACE, "Lock", &())
                .unwrap()
        };
        let sleep = |from: &Connection, to: Option<&str>| {
            from.emit_signal(
                to,
                MANAGER_PATH,
                MANAGER_INTERFACE,
                "PrepareForSleep",
                &true,
            )
            .unwrap()
        };

        // another peer, to the lock screen directly and to everyone
        let peer = bus.connect();
        for to in [Some(to.as_str()), None] {
            lock(&peer, to);
            sleep(&peer, to);
        }
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "forged signals are ignored"
        );

        lock(&logind, None);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Ok(Signal::Lock))
        );
        sleep(&logind, None);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)),
            Ok(Ok(Signal::PrepareForSleep))
        );
    }
}
