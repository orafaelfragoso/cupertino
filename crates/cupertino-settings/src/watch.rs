//! The services' change signals: one thread a bus waits for the signals of the services the panes read and names what
//! changed, so a reading that has a change signal is read again when one comes, never on a timer. The bus daemon lets
//! through only the signals of these services on the objects the panes read ([`rules`]); the thread drops what the
//! shown pane does not read ([`Interest`]) before it reaches the window. On an idle desktop none come and the threads
//! sleep.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::task::Poll;

use zbus::export::futures_core::Stream;
use zbus::message::Type;
use zbus::MessageStream;

use cupertino_radios::watch::Radio;

use crate::services::{
    CENTRE_NAME, CENTRE_PATH, KEYBOARD_BACKLIGHT_PATH, POWER_NAME, POWER_PATH, PROFILES_NAME,
    PROFILES_PATH, SESSION_NAME, SESSION_PATH, UPOWER_DISPLAY_DEVICE, UPOWER_NAME,
};

/// The bus daemon's own name, which sends `NameOwnerChanged`.
const BUS_DAEMON: &str = "org.freedesktop.DBus";
const BUS_DAEMON_PATH: &str = "/org/freedesktop/DBus";
/// The power service's root object, which tells devices coming and going.
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

/// One of the two buses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Which {
    Session,
    System,
}

/// What a signal changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Changed {
    /// A service of this bus the panes read started or stopped.
    Owner(Which),
    /// The session interface's properties: the appearance, the mice.
    Session,
    /// The notification centre's properties: do not disturb.
    Centre,
    /// The network service: its radio, devices, networks and connections.
    Network,
    /// The Bluetooth service: its adapter and devices.
    Bluetooth,
    /// The battery, the power devices and the power profile.
    Battery,
    /// The display's brightness.
    Brightness,
    /// The keyboard backlight.
    KeyboardBacklight,
    /// A device was plugged in or removed ([`crate::hotplug`]).
    Hardware,
    /// The watch of this bus ended with its connection; the window watches the bus again.
    Ended(Which),
}

impl Changed {
    /// The bit of a change only a pane that reads it wants; `None` for those every pane wants (a service starting or
    /// stopping names the sidebar's rows, the session's properties hold the appearance, a hot-plug may add a row).
    fn bit(self) -> Option<u32> {
        match self {
            Changed::Owner(_) | Changed::Session | Changed::Hardware | Changed::Ended(_) => None,
            Changed::Centre => Some(1),
            Changed::Network => Some(1 << 1),
            Changed::Bluetooth => Some(1 << 2),
            Changed::Battery => Some(1 << 3),
            Changed::Brightness => Some(1 << 4),
            Changed::KeyboardBacklight => Some(1 << 5),
        }
    }
}

/// Which changes the shown pane reads, shared with the watch threads: a change no one reads is dropped there and
/// never wakes the window.
#[derive(Clone, Debug, Default)]
pub struct Interest(Arc<AtomicU32>);

impl Interest {
    /// The shown pane reads `wanted` (and what every pane reads), nothing else.
    pub fn set(&self, wanted: &[Changed]) {
        let bits = wanted.iter().filter_map(|c| c.bit()).fold(0, |a, b| a | b);
        self.0.store(bits, Ordering::Relaxed);
    }

    /// Whether `changed` is read now.
    pub fn wants(&self, changed: Changed) -> bool {
        changed
            .bit()
            .is_none_or(|bit| self.0.load(Ordering::Relaxed) & bit != 0)
    }
}

/// The match rules of `bus`'s watch: a service of the panes starting or stopping, and its signals on the objects the
/// panes read; the network's and the Bluetooth adapter's are the ones the control panel and the menu bar follow
/// (`cupertino_radios::watch`). An access point's strength, a device's statistics or any other object of these
/// services sends nothing here.
pub fn rules(bus: Which) -> Vec<String> {
    let owner = |name: &str| {
        format!(
            "type='signal',sender='{BUS_DAEMON}',interface='{BUS_DAEMON}',member='NameOwnerChanged',arg0='{name}'"
        )
    };
    let properties = |name: &str, path: &str| {
        format!(
            "type='signal',sender='{name}',path='{path}',interface='{PROPERTIES}',member='PropertiesChanged'"
        )
    };
    // every signal of one object: the services' own interfaces tell their changes in signals of their own
    let object = |name: &str, path: &str| format!("type='signal',sender='{name}',path='{path}'");
    match bus {
        Which::Session => vec![
            owner(SESSION_NAME),
            owner(CENTRE_NAME),
            properties(SESSION_NAME, SESSION_PATH),
            properties(CENTRE_NAME, CENTRE_PATH),
        ],
        Which::System => {
            let mut rules = cupertino_radios::watch::rules();
            rules.extend([
                owner(UPOWER_NAME),
                object(UPOWER_NAME, UPOWER_PATH),
                properties(UPOWER_NAME, UPOWER_DISPLAY_DEVICE),
                object(UPOWER_NAME, KEYBOARD_BACKLIGHT_PATH),
                owner(POWER_NAME),
                object(POWER_NAME, POWER_PATH),
                owner(PROFILES_NAME),
                properties(PROFILES_NAME, PROFILES_PATH),
            ]);
            rules
        }
    }
}

/// What a signal that came through `bus`'s rules changed, by its sender, path, interface and member. A message's
/// sender is a unique name, so the services are told apart by their objects: the rules let only theirs through.
pub fn classify(
    bus: Which,
    sender: Option<&str>,
    path: Option<&str>,
    interface: Option<&str>,
    member: Option<&str>,
) -> Option<Changed> {
    if sender == Some(BUS_DAEMON) {
        return (member == Some("NameOwnerChanged")).then_some(Changed::Owner(bus));
    }
    let path = path?;
    match bus {
        Which::Session => match path {
            SESSION_PATH => Some(Changed::Session),
            CENTRE_PATH => Some(Changed::Centre),
            _ => None,
        },
        Which::System => match path {
            _ if cupertino_radios::watch::classify(path, interface) == Some(Radio::Network) => {
                Some(Changed::Network)
            }
            _ if cupertino_radios::watch::classify(path, interface) == Some(Radio::Bluetooth) => {
                Some(Changed::Bluetooth)
            }
            UPOWER_PATH | UPOWER_DISPLAY_DEVICE | PROFILES_PATH => Some(Changed::Battery),
            POWER_PATH => Some(Changed::Brightness),
            KEYBOARD_BACKLIGHT_PATH => Some(Changed::KeyboardBacklight),
            _ => None,
        },
    }
}

/// What the signal `message` that came through `bus`'s rules changed ([`classify`]). A change of the Bluetooth stack's
/// objects that the pane does not show (a device's signal strength and advertisement while the adapter discovers)
/// changed nothing.
fn changed_by(bus: Which, message: &zbus::Message) -> Option<Changed> {
    let header = message.header();
    let changed = classify(
        bus,
        header.sender().map(|s| s.as_str()),
        header.path().map(|p| p.as_str()),
        header.interface().map(|i| i.as_str()),
        header.member().map(|m| m.as_str()),
    )?;
    if changed == Changed::Bluetooth && cupertino_radios::watch::concern(message).is_none() {
        return None;
    }
    Some(changed)
}

/// Watch `bus` on `connection` from a thread of its own; `on` hears what each signal the shown pane reads changed
/// (`interest`), and [`Changed::Ended`] once the connection closes. Returns once the bus daemon has the rules.
///
/// The thread reads every message of the connection from one queue, which it drains while it asks the daemon for the
/// rules: a full queue would stop the connection reading its socket, replies included, so nothing else waits on it.
pub fn start(
    bus: Which,
    connection: &zbus::blocking::Connection,
    interest: Interest,
    on: Arc<dyn Fn(Changed) + Send + Sync>,
) -> zbus::Result<()> {
    let connection = connection.inner().clone();
    let (subscribed, result) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("settings-watch".into())
        .spawn(move || {
            zbus::block_on(serve(bus, connection, interest, on, move |r| {
                let _ = subscribed.send(r);
            }))
        })
        .map_err(|e| zbus::Error::Failure(format!("cannot start the watch: {e}")))?;
    result
        .recv()
        .map_err(|_| zbus::Error::Failure("the watch ended before it subscribed".into()))?
}

/// Ask the daemon to send `bus`'s rules to `connection`, one after the other.
async fn subscribe(bus: Which, connection: zbus::Connection) -> zbus::Result<()> {
    for rule in rules(bus) {
        connection
            .call_method(
                Some(BUS_DAEMON),
                BUS_DAEMON_PATH,
                Some(BUS_DAEMON),
                "AddMatch",
                &(rule.as_str(),),
            )
            .await?;
    }
    Ok(())
}

async fn serve(
    bus: Which,
    connection: zbus::Connection,
    interest: Interest,
    on: Arc<dyn Fn(Changed) + Send + Sync>,
    subscribed: impl FnOnce(zbus::Result<()>),
) {
    // the queue exists before the first rule: no signal of the rules comes before it
    let mut stream = MessageStream::from(&connection);
    let mut subscribing: Option<Pin<Box<dyn Future<Output = zbus::Result<()>> + Send>>> =
        Some(Box::pin(subscribe(bus, connection.clone())));
    let mut subscribed = Some(subscribed);
    loop {
        let (messages, ended, done) = poll_fn(|cx| {
            let done = match subscribing.as_mut().map(|s| s.as_mut().poll(cx)) {
                Some(Poll::Ready(result)) => {
                    subscribing = None;
                    Some(result)
                }
                _ => None,
            };
            let mut messages = Vec::new();
            let mut ended = false;
            loop {
                match Pin::new(&mut stream).poll_next(cx) {
                    Poll::Ready(Some(Ok(message))) => messages.push(message),
                    Poll::Ready(Some(Err(_))) => {}
                    Poll::Ready(None) => {
                        ended = true;
                        break;
                    }
                    Poll::Pending => break,
                }
            }
            if messages.is_empty() && !ended && done.is_none() {
                Poll::Pending
            } else {
                Poll::Ready((messages, ended, done))
            }
        })
        .await;
        if let Some(result) = done {
            let failed = result.is_err();
            if let Some(tell) = subscribed.take() {
                tell(result);
            }
            if failed {
                return;
            }
        }
        // a signal several rules let through, or a burst, is told once
        let mut told = Vec::new();
        for message in &messages {
            if message.message_type() != Type::Signal {
                continue;
            }
            let changed = changed_by(bus, message);
            if let Some(changed) = changed.filter(|c| !told.contains(c) && interest.wants(*c)) {
                told.push(changed);
                on(changed);
            }
        }
        if ended {
            if let Some(tell) = subscribed.take() {
                tell(Err(zbus::Error::Failure(
                    "the connection closed before the watch subscribed".into(),
                )));
                return;
            }
            on(Changed::Ended(bus));
            return;
        }
    }
}
