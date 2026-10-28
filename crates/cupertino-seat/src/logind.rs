//! The D-Bus side: a thin layer over the `org.freedesktop.login1` interfaces.
//!
//! Protocol reference: `man org.freedesktop.login1` (the same text is on
//! <https://www.freedesktop.org/software/systemd/man/latest/org.freedesktop.login1.html>).
//! Every call goes to logind's unique bus name, resolved from the reply to the first lookup, and
//! the signal subscription is restricted to that sender: any peer can emit a signal on any
//! object path, and a forged `PauseDevice` or `ResumeDevice` would otherwise pause or activate
//! the session. Because of that binding a logind restart silences the subscription instead of
//! ending it, so [`watch_owner`] reports the unique name going away.

use std::collections::HashMap;
use std::os::fd::OwnedFd;

use cupertino_devices::DevT;
use zbus::blocking::proxy::Builder;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::names::OwnedUniqueName;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{self, ObjectPath, OwnedObjectPath, OwnedValue};
use zbus::{MatchRule, Message};

use crate::error::Error;
use crate::state::{Input, PauseKind};

const LOGIND: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER_INTERFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";
const SEAT_INTERFACE: &str = "org.freedesktop.login1.Seat";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
const BUS: &str = "org.freedesktop.DBus";

/// How to find the session this process belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLookup {
    /// `Manager.GetSession(id)`: the id the login manager exported in `XDG_SESSION_ID`.
    ById(String),
    /// `Manager.GetSessionByPID(pid)`: the session whose cgroup contains the process.
    ByPid(u32),
}

impl SessionLookup {
    /// `XDG_SESSION_ID` when it is set, because the login manager put it there on purpose and a
    /// compositor started from a user service is not inside the session's cgroup; otherwise the
    /// process id. A failing lookup by id is an error, never a retry by pid.
    pub fn choose(xdg_session_id: Option<String>, pid: u32) -> Self {
        match xdg_session_id {
            Some(id) => Self::ById(id),
            None => Self::ByPid(pid),
        }
    }

    /// [`SessionLookup::choose`] on this process's environment and pid.
    pub fn from_environment() -> Result<Self, Error> {
        let id = match std::env::var("XDG_SESSION_ID") {
            Ok(id) => Some(id),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => return Err(Error::SessionIdNotUnicode),
        };
        Ok(Self::choose(id, std::process::id()))
    }
}

/// A session found on the system bus.
#[derive(Debug, Clone)]
pub struct SessionRef {
    path: OwnedObjectPath,
    /// logind's unique bus name, the sender of the lookup reply.
    owner: OwnedUniqueName,
}

impl SessionRef {
    /// The session's object path, for example `/org/freedesktop/login1/session/_32`.
    pub fn path(&self) -> &str {
        self.path.as_str()
    }
}

/// What logind reports about a session. Reading it takes no control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub path: String,
    /// The `Id` property.
    pub id: String,
    /// The `Type` property: `tty`, `x11`, `wayland`, ...
    pub session_type: String,
    /// The `Active` property.
    pub active: bool,
    /// The `VTNr` property; 0 when the session has no virtual terminal.
    pub vt: u32,
    /// The name part of the `Seat` property.
    pub seat: String,
}

/// Connects to the system bus.
pub fn connect() -> Result<Connection, Error> {
    Connection::system().map_err(Error::bus("connecting to the system bus"))
}

/// Asks logind for the session behind `lookup`.
pub fn locate(conn: &Connection, lookup: &SessionLookup) -> Result<SessionRef, Error> {
    let (method, what) = match lookup {
        SessionLookup::ById(_) => ("GetSession", "looking up the session by id"),
        SessionLookup::ByPid(_) => ("GetSessionByPID", "looking up the session by pid"),
    };
    let reply = match lookup {
        SessionLookup::ById(id) => conn.call_method(
            Some(LOGIND),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            method,
            &(id.as_str(),),
        ),
        SessionLookup::ByPid(pid) => conn.call_method(
            Some(LOGIND),
            MANAGER_PATH,
            Some(MANAGER_INTERFACE),
            method,
            &(*pid,),
        ),
    }
    .map_err(Error::bus(what))?;
    let path: OwnedObjectPath = reply.body().deserialize().map_err(Error::bus(what))?;
    // A reply from the bus daemon itself would carry no sender; the method call reached logind.
    let owner = reply
        .header()
        .sender()
        .map(|sender| OwnedUniqueName::from(sender.to_owned()))
        .ok_or_else(|| {
            Error::bus(what)(zbus::Error::Failure("the reply has no sender".to_owned()))
        })?;
    Ok(SessionRef { path, owner })
}

/// Reads the session's properties without taking control of it.
pub fn inspect(conn: &Connection, session: &SessionRef) -> Result<SessionInfo, Error> {
    let proxy = session_proxy(conn, session)?;
    let (seat, _): (String, OwnedObjectPath) = property(&proxy, "Seat")?;
    Ok(SessionInfo {
        path: session.path().to_owned(),
        id: property(&proxy, "Id")?,
        session_type: property(&proxy, "Type")?,
        active: property(&proxy, "Active")?,
        vt: property(&proxy, "VTNr")?,
        seat,
    })
}

fn proxy_for(
    conn: &Connection,
    owner: &OwnedUniqueName,
    path: ObjectPath<'static>,
    interface: &'static str,
) -> Result<Proxy<'static>, Error> {
    // Uncached: `Active` is read once at start and tracked through signals afterwards.
    Builder::<Proxy>::new(conn)
        .destination(owner.clone())
        .and_then(|builder| builder.path(path))
        .and_then(|builder| builder.interface(interface))
        .and_then(|builder| builder.cache_properties(CacheProperties::No).build())
        .map_err(Error::bus("creating a logind proxy"))
}

fn session_proxy(conn: &Connection, session: &SessionRef) -> Result<Proxy<'static>, Error> {
    proxy_for(
        conn,
        &session.owner,
        session.path.clone().into_inner(),
        SESSION_INTERFACE,
    )
}

fn property<T>(proxy: &Proxy<'_>, name: &'static str) -> Result<T, Error>
where
    T: TryFrom<OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    proxy.get_property(name).map_err(Error::bus(name))
}

/// The signals of one session: `PauseDevice`, `ResumeDevice` and `PropertiesChanged`, from
/// logind only. Subscribe before `TakeControl` so that no signal is missed.
pub fn subscribe(conn: &Connection, session: &SessionRef) -> Result<MessageIterator, Error> {
    let what = "subscribing to the session signals";
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(session.owner.clone())
        .and_then(|builder| builder.path(session.path.clone()))
        .map_err(Error::bus(what))?
        .build();
    MessageIterator::for_match_rule(rule, conn, None).map_err(Error::bus(what))
}

/// The bus's `NameOwnerChanged` for logind's well-known name leaving the unique name the session
/// was found on: logind exited or restarted, and every signal and call of the session is bound to
/// a name that no longer exists. Any message from this iterator means the session is lost.
/// Subscribe before `TakeControl`, as with [`subscribe`].
pub fn watch_owner(conn: &Connection, session: &SessionRef) -> Result<MessageIterator, Error> {
    let what = "watching logind's bus name";
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(BUS)
        .and_then(|builder| builder.interface(BUS))
        .and_then(|builder| builder.member("NameOwnerChanged"))
        .and_then(|builder| builder.arg(0, LOGIND))
        .and_then(|builder| builder.arg(1, session.owner.as_str()))
        .map_err(Error::bus(what))?
        .build();
    MessageIterator::for_match_rule(rule, conn, None).map_err(Error::bus(what))
}

/// Turns a signal into a state machine input. Signals the session does not use (`Lock`,
/// `Unlock`, other properties) are not an error and give `None`.
pub fn decode(message: &Message) -> Result<Option<Input>, Error> {
    let what = "decoding a session signal";
    let header = message.header();
    let (Some(interface), Some(member)) = (header.interface(), header.member()) else {
        return Ok(None);
    };
    let bus = Error::bus(what);
    match (interface.as_str(), member.as_str()) {
        (SESSION_INTERFACE, "PauseDevice") => {
            let (major, minor, kind): (u32, u32, String) =
                message.body().deserialize().map_err(bus)?;
            Ok(Some(Input::PauseDevice {
                dev: DevT::new(major, minor),
                kind: PauseKind::parse(&kind)?,
            }))
        }
        (SESSION_INTERFACE, "ResumeDevice") => {
            // The descriptor is closed here; see `Input::ResumeDevice`.
            let (major, minor, _fd): (u32, u32, zvariant::OwnedFd) =
                message.body().deserialize().map_err(bus)?;
            Ok(Some(Input::ResumeDevice {
                dev: DevT::new(major, minor),
            }))
        }
        (PROPERTIES_INTERFACE, "PropertiesChanged") => {
            let (interface, changed, invalidated): (
                String,
                HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize().map_err(bus)?;
            if interface != SESSION_INTERFACE {
                return Ok(None);
            }
            if invalidated.iter().any(|name| name == "Active") {
                return Err(Error::ActiveInvalidated);
            }
            match changed.get("Active") {
                Some(value) => Ok(Some(Input::ActiveChanged(
                    bool::try_from(value).map_err(|e| bus(e.into()))?,
                ))),
                None => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

/// Control of one session: `TakeControl` succeeded, and `ReleaseControl` runs on drop.
pub struct Logind {
    session: Proxy<'static>,
    seat: Proxy<'static>,
}

impl Logind {
    /// Calls `Session.TakeControl(false)` ("force" is root only and would evict another
    /// controller) and reads `Active` and the seat. Returns the control, the initial `Active`
    /// value and the seat name.
    pub fn take_control(
        conn: &Connection,
        session: &SessionRef,
    ) -> Result<(Self, bool, String), Error> {
        let proxy = session_proxy(conn, session)?;
        proxy
            .call::<_, _, ()>("TakeControl", &(false,))
            .map_err(Error::bus("TakeControl"))?;
        // From here on the control is ours and released by `Drop`, also when a later step fails.
        let (seat_name, seat_path): (String, OwnedObjectPath) = property(&proxy, "Seat")?;
        let active = property(&proxy, "Active")?;
        let seat = proxy_for(conn, &session.owner, seat_path.into_inner(), SEAT_INTERFACE)?;
        Ok((
            Self {
                session: proxy,
                seat,
            },
            active,
            seat_name,
        ))
    }

    /// `Session.TakeDevice(major, minor)`: the descriptor and logind's `inactive` flag (the
    /// session is not active, so the descriptor is paused until `ResumeDevice`).
    pub fn take_device(&self, dev: DevT) -> Result<(OwnedFd, bool), Error> {
        let (fd, inactive): (zvariant::OwnedFd, bool) = self
            .session
            .call("TakeDevice", &(dev.major(), dev.minor()))
            .map_err(Error::bus("TakeDevice"))?;
        Ok((fd.into(), inactive))
    }

    /// `Session.ReleaseDevice(major, minor)`.
    pub fn release_device(&self, dev: DevT) -> Result<(), Error> {
        self.session
            .call("ReleaseDevice", &(dev.major(), dev.minor()))
            .map_err(Error::bus("ReleaseDevice"))
    }

    /// `Session.PauseDeviceComplete(major, minor)`: the answer to `PauseDevice("pause")`.
    pub fn pause_device_complete(&self, dev: DevT) -> Result<(), Error> {
        self.session
            .call("PauseDeviceComplete", &(dev.major(), dev.minor()))
            .map_err(Error::bus("PauseDeviceComplete"))
    }

    /// `Seat.SwitchTo(vtnr)`.
    pub fn switch_to(&self, vt: u32) -> Result<(), Error> {
        self.seat
            .call("SwitchTo", &(vt,))
            .map_err(Error::bus("SwitchTo"))
    }
}

impl Drop for Logind {
    /// `Session.ReleaseControl()` also releases every device taken. Closing the bus connection
    /// would do the same, but the connection may outlive this value.
    fn drop(&mut self) {
        if let Err(source) = self.session.call::<_, _, ()>("ReleaseControl", &()) {
            tracing::error!(error = %source, "ReleaseControl failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_test_bus::PrivateBus;

    use std::sync::mpsc;
    use std::time::Duration;
    use zbus::fdo::{ReleaseNameReply, RequestNameFlags, RequestNameReply};

    #[test]
    fn losing_the_name_is_reported_and_another_owner_is_not() {
        let Some(bus) = PrivateBus::start() else {
            eprintln!("skipping: dbus-daemon is not available");
            return;
        };
        let request = |conn: &Connection| {
            zbus::blocking::fdo::DBusProxy::new(conn)
                .unwrap()
                .request_name(
                    LOGIND.try_into().unwrap(),
                    RequestNameFlags::DoNotQueue.into(),
                )
                .unwrap()
        };
        let logind = bus.connect();
        assert_eq!(request(&logind), RequestNameReply::PrimaryOwner);
        let session = SessionRef {
            path: OwnedObjectPath::try_from("/org/freedesktop/login1/session/_31").unwrap(),
            owner: logind.unique_name().unwrap().to_owned(),
        };
        let compositor = bus.connect();
        let mut changes = watch_owner(&compositor, &session).unwrap();
        let (sender, received) = mpsc::channel();
        std::thread::spawn(move || {
            for message in changes.by_ref() {
                sender
                    .send(
                        message
                            .unwrap()
                            .body()
                            .deserialize::<(String, String, String)>()
                            .unwrap(),
                    )
                    .unwrap();
            }
        });

        let released = zbus::blocking::fdo::DBusProxy::new(&logind)
            .unwrap()
            .release_name(LOGIND.try_into().unwrap())
            .unwrap();
        assert_eq!(released, ReleaseNameReply::Released);
        let (name, old, new) = received
            .recv_timeout(Duration::from_secs(5))
            .expect("the name change is reported");
        assert_eq!(
            (name.as_str(), old.as_str(), new.as_str()),
            (LOGIND, session.owner.as_str(), "")
        );

        // A restarted logind taking the name moves it away from no one the session knows.
        let restarted = bus.connect();
        assert_eq!(request(&restarted), RequestNameReply::PrimaryOwner);
        assert!(
            received.recv_timeout(Duration::from_millis(300)).is_err(),
            "reported once"
        );
    }

    #[test]
    fn session_id_wins_over_pid() {
        assert_eq!(
            SessionLookup::choose(Some("c2".to_owned()), 4242),
            SessionLookup::ById("c2".to_owned())
        );
        assert_eq!(
            SessionLookup::choose(None, 4242),
            SessionLookup::ByPid(4242)
        );
    }
}
