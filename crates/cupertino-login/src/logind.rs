//! What logind asks of a lock screen: lock the session (`loginctl lock-session`), prepare for sleep, and the delay
//! inhibitor that holds sleep back until the lock is up.
//!
//! Everything here runs on the system bus connection the lock screen's loop ticks (`cupertino_kit::bus`): the calls
//! are awaited there and the signals are a future of the loop. Each signal comes from logind only: the proxies follow
//! the owner of logind's name and pass on what that connection sent alone, since a rule naming the well-known name
//! keeps out nothing that is sent to this process directly.

use std::fmt;

use futures_lite::stream::{self, StreamExt};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};
use zbus::{Connection, Proxy};

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
/// The variable a login (`pam_systemd`) sets to the session's id.
const SESSION_VARIABLE: &str = "XDG_SESSION_ID";

/// Why a logind request failed.
#[derive(Debug)]
pub struct Error {
    what: &'static str,
    source: Box<zbus::Error>,
}

impl Error {
    fn bus(what: &'static str) -> impl Fn(zbus::Error) -> Self {
        move |source| Error {
            what,
            source: Box::new(source),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.what, self.source)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
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
#[derive(Debug)]
pub struct Inhibitor(#[allow(dead_code)] OwnedFd);

/// Whether `e` says that logind knows no session of this process or is not on the bus, rather than that a request
/// failed. Only a lookup by process id can say so: with `XDG_SESSION_ID` set every failure is a fault.
fn is_absent(e: &zbus::Error) -> bool {
    matches!(e, zbus::Error::MethodError(name, ..) if matches!(
        name.as_str(),
        "org.freedesktop.login1.NoSessionForPID"
            | "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
    ))
}

async fn manager(system: &Connection) -> zbus::Result<Proxy<'static>> {
    zbus::proxy::Builder::new(system)
        .destination(LOGIND)?
        .path(MANAGER_PATH)?
        .interface(MANAGER_INTERFACE)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

/// The object of this process's session (`XDG_SESSION_ID`, else the process id). `None` when there is no session to
/// lock: `XDG_SESSION_ID` is unset and logind knows no session of this process or is not running.
pub async fn session(system: &Connection) -> Result<Option<OwnedObjectPath>, Error> {
    let what = "finding the login session";
    let manager = manager(system).await.map_err(Error::bus(what))?;
    match std::env::var(SESSION_VARIABLE) {
        Ok(id) if !id.is_empty() => manager
            .call("GetSession", &(id.as_str(),))
            .await
            .map(Some)
            .map_err(Error::bus(what)),
        _ => match manager
            .call::<_, _, OwnedObjectPath>("GetSessionByPID", &(std::process::id(),))
            .await
        {
            Ok(path) => Ok(Some(path)),
            Err(e) if is_absent(&e) => Ok(None),
            Err(e) => Err(Error::bus(what)(e)),
        },
    }
}

/// Hold sleep back until the returned [`Inhibitor`] is dropped.
pub async fn inhibit_sleep(system: &Connection) -> Result<Inhibitor, Error> {
    let what = "taking the sleep inhibitor";
    let manager = manager(system).await.map_err(Error::bus(what))?;
    let fd: OwnedFd = manager
        .call(
            "Inhibit",
            &(
                "sleep",
                "cupertino-login",
                "Lock the screen before the system sleeps",
                "delay",
            ),
        )
        .await
        .map_err(Error::bus(what))?;
    Ok(Inhibitor(fd))
}

/// Hand every [`Signal`] for `session` to `notify` until a stream ends; returns why. `following` runs once both
/// signals are subscribed to, so a request logind sends after it is heard. Only logind's own signals count: the proxies
/// follow the owner of its name.
pub async fn follow(
    system: &Connection,
    session: OwnedObjectPath,
    following: impl FnOnce(),
    notify: impl Fn(Signal),
) -> String {
    let session = match zbus::proxy::Builder::<Proxy<'_>>::new(system)
        .destination(LOGIND)
        .and_then(|b| b.path(session))
        .and_then(|b| b.interface(SESSION_INTERFACE))
    {
        Ok(builder) => builder.cache_properties(CacheProperties::No).build().await,
        Err(e) => Err(e),
    };
    let session = match session {
        Ok(session) => session,
        Err(e) => return format!("reaching the login session: {e}"),
    };
    let manager = match manager(system).await {
        Ok(manager) => manager,
        Err(e) => return format!("reaching the login manager: {e}"),
    };
    let locks = match session.receive_signal("Lock").await {
        Ok(locks) => locks.map(|_| Some(Signal::Lock)),
        Err(e) => return format!("subscribing to the session's Lock: {e}"),
    };
    let sleeps = match manager.receive_signal("PrepareForSleep").await {
        Ok(sleeps) => sleeps.map(|message| {
            message
                .body()
                .deserialize::<bool>()
                .is_ok_and(|going| going)
                .then_some(Signal::PrepareForSleep)
        }),
        Err(e) => return format!("subscribing to PrepareForSleep: {e}"),
    };
    following();
    let mut signals = stream::or(locks, sleeps);
    while let Some(signal) = signals.next().await {
        if let Some(signal) = signal {
            notify(signal);
        }
    }
    "the login manager's signal streams ended".into()
}
