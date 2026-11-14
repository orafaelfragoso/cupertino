//! The services' change signals: one thread a bus waits for the signals of the services the panes read and names what
//! changed, so a reading that has a change signal is read again when one comes, never on a timer. The bus daemon lets
//! through only the signals of these services ([`rules`]); on an idle desktop none come and the threads sleep.

use std::future::poll_fn;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use zbus::export::futures_core::Stream;
use zbus::MessageStream;

use crate::services::{
    BLUETOOTH_NAME, CENTRE_NAME, CENTRE_PATH, NETWORK_NAME, POWER_NAME, PROFILES_NAME,
    SESSION_NAME, SESSION_PATH, UPOWER_NAME,
};

/// The bus daemon's own name, which sends `NameOwnerChanged`.
const BUS_DAEMON: &str = "org.freedesktop.DBus";
/// How many signals a watch holds before the oldest go: a burst needs only one reading again.
const QUEUED: usize = 64;

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
    /// A service of the system bus the panes read (network, Bluetooth, power) sent a signal.
    System,
    /// The watch of this bus ended with its connection; the next use of the bus watches again.
    Ended(Which),
}

/// The names of the system bus's services the panes read.
fn system_services() -> [&'static str; 7] {
    [
        NETWORK_NAME,
        crate::nm::NAME,
        BLUETOOTH_NAME,
        crate::bluez::NAME,
        UPOWER_NAME,
        POWER_NAME,
        PROFILES_NAME,
    ]
}

/// The match rules of `bus`'s watch: a service of the panes starting or stopping, and its signals (on the session bus,
/// only its property changes).
pub fn rules(bus: Which) -> Vec<String> {
    let owner = |name: &str| {
        format!(
            "type='signal',sender='{BUS_DAEMON}',interface='{BUS_DAEMON}',member='NameOwnerChanged',arg0='{name}'"
        )
    };
    match bus {
        Which::Session => {
            let properties = |name: &str, path: &str| {
                format!(
                    "type='signal',sender='{name}',path='{path}',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"
                )
            };
            vec![
                owner(SESSION_NAME),
                owner(CENTRE_NAME),
                properties(SESSION_NAME, SESSION_PATH),
                properties(CENTRE_NAME, CENTRE_PATH),
            ]
        }
        Which::System => system_services()
            .into_iter()
            .flat_map(|name| [owner(name), format!("type='signal',sender='{name}'")])
            .collect(),
    }
}

/// What a signal that came through `bus`'s rules changed, by its sender, path and member. A message's sender is a
/// unique name, so the services are told apart by their paths: the rules let only theirs through.
pub fn classify(
    bus: Which,
    sender: Option<&str>,
    path: Option<&str>,
    member: Option<&str>,
) -> Option<Changed> {
    if sender == Some(BUS_DAEMON) {
        return (member == Some("NameOwnerChanged")).then_some(Changed::Owner(bus));
    }
    match bus {
        Which::Session => match path {
            Some(SESSION_PATH) => Some(Changed::Session),
            Some(CENTRE_PATH) => Some(Changed::Centre),
            _ => None,
        },
        Which::System => Some(Changed::System),
    }
}

/// Watch `bus` on `connection` from a thread of its own; `on` hears what each signal changed, and
/// [`Changed::Ended`] once the connection closes.
pub fn start(
    bus: Which,
    connection: &zbus::blocking::Connection,
    on: Arc<dyn Fn(Changed) + Send + Sync>,
) -> zbus::Result<()> {
    let streams = rules(bus)
        .into_iter()
        .map(|rule| {
            let rule = zbus::MatchRule::try_from(rule.as_str())?.to_owned();
            zbus::block_on(MessageStream::for_match_rule(
                rule,
                connection.inner(),
                Some(QUEUED),
            ))
            .map(Box::pin)
        })
        .collect::<zbus::Result<Vec<_>>>()?;
    std::thread::Builder::new()
        .name("settings-watch".into())
        .spawn(move || zbus::block_on(serve(bus, streams, on)))
        .map_err(|e| zbus::Error::Failure(format!("cannot start the watch: {e}")))?;
    Ok(())
}

async fn serve(
    bus: Which,
    mut streams: Vec<Pin<Box<MessageStream>>>,
    on: Arc<dyn Fn(Changed) + Send + Sync>,
) {
    loop {
        let (messages, ended) = poll_fn(|cx| {
            let mut messages = Vec::new();
            let mut ended = false;
            for stream in &mut streams {
                loop {
                    match stream.as_mut().poll_next(cx) {
                        Poll::Ready(Some(Ok(message))) => messages.push(message),
                        Poll::Ready(Some(Err(_))) => {}
                        Poll::Ready(None) => {
                            ended = true;
                            break;
                        }
                        Poll::Pending => break,
                    }
                }
            }
            if messages.is_empty() && !ended {
                Poll::Pending
            } else {
                Poll::Ready((messages, ended))
            }
        })
        .await;
        // a signal several rules let through comes once a rule: what it changed is told once
        let mut told = Vec::new();
        for message in &messages {
            let header = message.header();
            let changed = classify(
                bus,
                header.sender().map(|s| s.as_str()),
                header.path().map(|p| p.as_str()),
                header.member().map(|m| m.as_str()),
            );
            if let Some(changed) = changed.filter(|c| !told.contains(c)) {
                told.push(changed);
                on(changed);
            }
        }
        if ended {
            on(Changed::Ended(bus));
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_parses_and_each_service_the_panes_read_has_its_owner_rule_and_its_signals() {
        for bus in [Which::Session, Which::System] {
            for rule in rules(bus) {
                zbus::MatchRule::try_from(rule.as_str()).unwrap_or_else(|e| panic!("{rule}: {e}"));
                assert!(rule.starts_with("type='signal'"), "signals only: {rule}");
            }
        }
        let session = rules(Which::Session);
        for name in [SESSION_NAME, CENTRE_NAME] {
            assert!(session
                .iter()
                .any(|r| r.contains("NameOwnerChanged") && r.ends_with(&format!("arg0='{name}'"))));
            assert!(session.iter().any(
                |r| r.contains(&format!("sender='{name}'")) && r.contains("PropertiesChanged")
            ));
        }
        let system = rules(Which::System);
        for name in system_services() {
            assert!(
                system
                    .iter()
                    .any(|r| r.ends_with(&format!("arg0='{name}'"))),
                "{name} starting"
            );
            assert!(
                system.contains(&format!("type='signal',sender='{name}'")),
                "{name}'s signals"
            );
        }
        assert!(
            system.iter().all(|r| r.contains("sender='")),
            "no rule takes every signal of the bus"
        );
    }

    #[test]
    fn a_signal_names_the_service_it_came_from() {
        let owner = Some("NameOwnerChanged");
        assert_eq!(
            classify(
                Which::Session,
                Some(BUS_DAEMON),
                Some("/org/freedesktop/DBus"),
                owner
            ),
            Some(Changed::Owner(Which::Session))
        );
        assert_eq!(
            classify(
                Which::System,
                Some(BUS_DAEMON),
                Some("/org/freedesktop/DBus"),
                owner
            ),
            Some(Changed::Owner(Which::System))
        );
        assert_eq!(
            classify(Which::System, Some(BUS_DAEMON), None, Some("NameAcquired")),
            None,
            "the daemon's other signals change nothing"
        );
        let changed = Some("PropertiesChanged");
        assert_eq!(
            classify(Which::Session, Some(":1.4"), Some(SESSION_PATH), changed),
            Some(Changed::Session)
        );
        assert_eq!(
            classify(Which::Session, Some(":1.5"), Some(CENTRE_PATH), changed),
            Some(Changed::Centre)
        );
        assert_eq!(
            classify(Which::Session, Some(":1.6"), Some("/elsewhere"), changed),
            None
        );
        assert_eq!(
            classify(
                Which::System,
                Some(":1.9"),
                Some("/org/bluez/hci0"),
                changed
            ),
            Some(Changed::System)
        );
    }
}
