//! The battery state the session agent watches on the system bus: `WarningLevel` and `Percentage` of UPower's display device
//! (served by `cupertino-powerd`), and the notification service the warnings go to on the session bus.
//!
//! The signals arrive on reader threads, as in [`crate::logind`], and reach the caller through the callback of
//! [`watch`].

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use cupertino_peer::signal::sent_by_bus;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type;
use zbus::zvariant::{OwnedValue, Value};
use zbus::MatchRule;

use crate::low_battery::{Level, Notice, APP_NAME};

const UPOWER: &str = "org.freedesktop.UPower";
const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// Why the battery watch or a notification failed.
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

/// One report of the display device.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Battery {
    pub level: Level,
    pub percentage: f64,
}

/// What the watch reports.
#[derive(Clone, Debug)]
pub enum Report {
    Battery(Battery),
    /// The power daemon is not on the bus (or left it): there is nothing to warn about until it comes back.
    NoDaemon(String),
}

/// Read the display device now and whenever it changes or the power daemon comes back. `notify` runs on reader threads,
/// and once with an error when a stream breaks; after that nothing more arrives.
pub fn watch(
    connection: &Connection,
    notify: impl Fn(Result<Report, Error>) + Send + Sync + 'static,
) -> Result<(), Error> {
    let what = "subscribing to the power daemon's signals";
    let changes_rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface(PROPERTIES)
        .and_then(|b| b.member("PropertiesChanged"))
        .and_then(|b| b.path(DISPLAY_DEVICE))
        .and_then(|b| b.arg(0, DEVICE_INTERFACE))
        .map_err(Error::bus(what))?
        .build();
    let changes = MessageIterator::for_match_rule(changes_rule, connection, None)
        .map_err(Error::bus(what))?;
    let owner_rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|b| b.member("NameOwnerChanged"))
        .and_then(|b| b.arg(0, UPOWER))
        .map_err(Error::bus(what))?
        .build();
    let owners =
        MessageIterator::for_match_rule(owner_rule, connection, None).map_err(Error::bus(what))?;
    let notify = Arc::new(notify);
    notify(Ok(read(connection)));

    let c = connection.clone();
    spawn(
        "battery-changes",
        changes,
        Arc::clone(&notify),
        move |message| {
            let (_, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
                message.body().deserialize()?;
            // a change of either is reported with both, read again so they belong together
            Ok(
                (changed.contains_key("WarningLevel") || changed.contains_key("Percentage"))
                    .then(|| read(&c)),
            )
        },
    )?;
    let c = connection.clone();
    // a peer that said the daemon left would silence the warning: only the bus daemon's own change counts (zbus filters the
    // rule's sender already; this does not rely on it)
    let owners = owners.filter(|message| match message {
        Ok(message) if !sent_by_bus(message) => {
            eprintln!("cupertino-login: a NameOwnerChanged not sent by the bus was ignored");
            false
        }
        _ => true,
    });
    spawn("battery-daemon", owners, notify, move |message| {
        let (_, _, new_owner): (String, String, String) = message.body().deserialize()?;
        Ok(Some(if new_owner.is_empty() {
            Report::NoDaemon(format!("{UPOWER} left the bus"))
        } else {
            read(&c)
        }))
    })
}

fn read(connection: &Connection) -> Report {
    let get = |property: &str| -> zbus::Result<OwnedValue> {
        connection
            .call_method(
                Some(UPOWER),
                DISPLAY_DEVICE,
                Some(PROPERTIES),
                "Get",
                &(DEVICE_INTERFACE, property),
            )?
            .body()
            .deserialize()
    };
    let level = get("WarningLevel").and_then(|v| u32::try_from(&*v).map_err(zbus::Error::from));
    let percentage = get("Percentage").and_then(|v| f64::try_from(&*v).map_err(zbus::Error::from));
    match (level, percentage) {
        (Ok(level), Ok(percentage)) => Report::Battery(Battery {
            level: Level::from_upower(level),
            percentage,
        }),
        (Err(e), _) | (_, Err(e)) => Report::NoDaemon(format!("reading the display device: {e}")),
    }
}

fn spawn(
    name: &str,
    messages: impl Iterator<Item = zbus::Result<zbus::Message>> + Send + 'static,
    notify: Arc<impl Fn(Result<Report, Error>) + Send + Sync + 'static>,
    handle: impl Fn(&zbus::Message) -> zbus::Result<Option<Report>> + Send + 'static,
) -> Result<(), Error> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            for message in messages {
                match message.and_then(|m| handle(&m)) {
                    Ok(Some(report)) => notify(Ok(report)),
                    Ok(None) => {}
                    Err(e) => return notify(Err(Error::bus("reading a power daemon signal")(e))),
                }
            }
            notify(Err(Error::bus("reading power daemon signals")(
                zbus::Error::Failure("the signal stream ended".into()),
            )));
        })
        .map(drop)
        .map_err(|e| Error::bus("starting the battery reader")(zbus::Error::Failure(e.to_string())))
}

/// `Notify` on the session bus; returns the notification's id.
pub fn post(session: &Connection, replaces: u32, notice: &Notice) -> Result<u32, Error> {
    let what = "posting the low-battery notification";
    let hints = HashMap::from([("urgency", Value::U8(notice.urgency))]);
    let actions: Vec<&str> = Vec::new();
    // -1: the notification service decides how long the banner stays
    let reply = session
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                APP_NAME,
                replaces,
                notice.icon,
                notice.summary.as_str(),
                notice.body.as_str(),
                actions,
                hints,
                -1i32,
            ),
        )
        .map_err(Error::bus(what))?;
    reply.body().deserialize().map_err(Error::bus(what))
}

/// `CloseNotification` on the session bus.
pub fn close(session: &Connection, id: u32) -> Result<(), Error> {
    session
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "CloseNotification",
            &id,
        )
        .map(drop)
        .map_err(Error::bus("withdrawing the low-battery notification"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    struct Display;

    #[zbus::interface(name = "org.freedesktop.UPower.Device")]
    impl Display {
        #[zbus(property)]
        fn warning_level(&self) -> u32 {
            3
        }

        #[zbus(property)]
        fn percentage(&self) -> f64 {
            9.0
        }
    }

    fn described(report: Result<Report, Error>) -> String {
        match report {
            Ok(Report::Battery(battery)) => format!("{:?} {}", battery.level, battery.percentage),
            Ok(Report::NoDaemon(why)) => format!("no daemon: {why}"),
            Err(e) => format!("error: {e}"),
        }
    }

    #[test]
    fn only_the_bus_says_the_power_daemon_left() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let daemon = bus
            .blocking_builder()
            .name(UPOWER)
            .unwrap()
            .serve_at(DISPLAY_DEVICE, Display)
            .unwrap()
            .build()
            .unwrap();
        let agent = bus.connect();
        let (tx, rx) = mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        watch(&agent, move |report| {
            let _ = tx.lock().unwrap().send(described(report));
        })
        .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).as_deref(),
            Ok("Low 9")
        );

        // another peer says the daemon left, which would silence the warning
        let peer = bus.connect();
        let to = agent.unique_name().unwrap().to_string();
        let left = (UPOWER, daemon.unique_name().unwrap().to_string(), "");
        peer.emit_signal(
            Some(to.as_str()),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameOwnerChanged",
            &left,
        )
        .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout),
            "a forged departure is ignored"
        );
        drop(daemon);
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).as_deref(),
            Ok("no daemon: org.freedesktop.UPower left the bus")
        );
    }
}
