//! Who sent a signal. The bus daemon hands a connection every signal addressed to its unique name, whatever its match
//! rules say, and zbus compares a rule's `sender` with a message only when the rule names a unique name (zbus counts the
//! daemon's own `org.freedesktop.DBus` as one): a rule with `sender='org.freedesktop.login1'` lets the same signal through
//! from any peer on the bus. A handler that acts on a signal takes it from [`from_owner`] for a service's signals, or checks
//! it with [`sent_by_bus`] for the daemon's own, also where the rule filters already, so the check does not rest on how a
//! rule's name is parsed.
//!
//! [`from_owner`] makes no call per signal. A zbus stream holds a fixed number of messages, and while one is full the
//! connection reads nothing more, the replies to its own calls included: a reader that asked the bus who owns a name for
//! each signal would wait for an answer that never comes once another peer had sent enough forged signals to fill the
//! stream. Instead the owner is asked once, followed through the daemon's `NameOwnerChanged` in the order the connection
//! received them along with the signals, and a task on the connection's own executor empties both streams as they fill.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use ordered_stream::{OrderedStream, OrderedStreamExt};
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Sequence;
use zbus::names::{OwnedUniqueName, UniqueName};
use zbus::{MatchRule, Message};

/// The bus daemon's name. It is the sender the daemon writes on its own signals (`NameOwnerChanged`, `NameLost`), and the
/// daemon writes the sender of every message, so no peer can send as it.
pub const BUS_DAEMON: &str = "org.freedesktop.DBus";
const BUS_DAEMON_PATH: &str = "/org/freedesktop/DBus";

/// Whether the bus daemon itself sent `message`: the only sender of a genuine `NameOwnerChanged`.
pub fn sent_by_bus(message: &Message) -> bool {
    message
        .header()
        .sender()
        .is_some_and(|sender| sender.as_str() == BUS_DAEMON)
}

/// The connection that owns a well-known name, as [`from_owner`] follows it: `None` while nobody does.
#[derive(Clone, Debug, Default)]
pub struct Owner(Arc<Mutex<Option<OwnedUniqueName>>>);

impl Owner {
    /// Whether `sender` owns the name now. False for a message without a sender and while nobody owns the name.
    pub fn is(&self, sender: Option<&UniqueName<'_>>) -> bool {
        let owner = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        matches!((sender, owner.as_ref()), (Some(sender), Some(owner)) if sender.as_str() == owner.as_str())
    }

    fn set(&self, owner: Option<OwnedUniqueName>) {
        *self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = owner;
    }

    /// Takes the new owner from the daemon's `NameOwnerChanged` for `name`; any other message changes nothing.
    fn follow(&self, name: &str, change: &Message) {
        if !sent_by_bus(change) {
            return;
        }
        if let Ok((changed, _, new_owner)) = change.body().deserialize::<(String, String, String)>()
        {
            if changed == name {
                self.set(UniqueName::try_from(new_owner).ok().map(Into::into));
            }
        }
    }
}

/// The signals matching a rule that the owner of a name sent, in the order they arrived; made by [`from_owner`]. It ends
/// after an error, or when the connection closes. Dropping it stops the task that reads them and removes both rules.
pub struct FromOwner {
    signals: mpsc::Receiver<zbus::Result<Message>>,
    owner: Owner,
    _reader: zbus::Task<()>,
}

impl FromOwner {
    /// The owner this stream follows, for a check outside it (a method call from the same service).
    pub fn owner(&self) -> Owner {
        self.owner.clone()
    }
}

impl Iterator for FromOwner {
    type Item = zbus::Result<Message>;

    fn next(&mut self) -> Option<Self::Item> {
        self.signals.recv().ok()
    }
}

#[derive(Clone, Copy)]
enum Source {
    /// The daemon's `NameOwnerChanged` for the name.
    Owners,
    /// What the caller's rule matches.
    Signals,
}

type Tagged = (Source, zbus::Result<Message>);

fn from_owners(_: &Sequence, message: zbus::Result<Message>) -> Tagged {
    (Source::Owners, message)
}

fn from_signals(_: &Sequence, message: zbus::Result<Message>) -> Tagged {
    (Source::Signals, message)
}

/// The signals matching `rule` that the connection owning the well-known `name` sent, checked against the owner at the
/// moment each one was received. `ignored` is called, on the connection's executor, for each one another connection
/// sent; it must not block. A signal sent just before its sender left the name is ignored with the forged ones.
pub fn from_owner(
    connection: &Connection,
    name: &str,
    rule: MatchRule<'_>,
    ignored: impl Fn(&Message) + Send + 'static,
) -> zbus::Result<FromOwner> {
    let owners_rule = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender(BUS_DAEMON)?
        .path(BUS_DAEMON_PATH)?
        .interface(BUS_DAEMON)?
        .member("NameOwnerChanged")?
        .arg(0, name)?
        .build();
    let owner = Owner::default();
    // every change after the answer arrives on this stream, which is up first
    let owners = MessageIterator::for_match_rule(owners_rule, connection, None)?.into_inner();
    match connection.call_method(
        Some(BUS_DAEMON),
        BUS_DAEMON_PATH,
        Some(BUS_DAEMON),
        "GetNameOwner",
        &(name,),
    ) {
        Ok(reply) => owner.set(Some(reply.body().deserialize::<UniqueName<'_>>()?.into())),
        // nobody owns the name yet
        Err(zbus::Error::MethodError(..)) => {}
        Err(error) => return Err(error),
    }
    // every signal is received after the answer, so the changes before it are applied before any signal
    let signals = MessageIterator::for_match_rule(rule, connection, None)?.into_inner();
    let (sender, received) = mpsc::channel();
    let stream = ordered_stream::join(
        owners.map_item(from_owners as fn(&Sequence, zbus::Result<Message>) -> Tagged),
        signals.map_item(from_signals as fn(&Sequence, zbus::Result<Message>) -> Tagged),
    );
    let reader = connection.inner().executor().spawn(
        read(stream, name.to_owned(), owner.clone(), sender, ignored),
        "signals from a name's owner",
    );
    Ok(FromOwner {
        signals: received,
        owner,
        _reader: reader,
    })
}

/// Empties both streams in the order their messages arrived: a change of owner is applied before any signal received
/// after it, and only the owner's signals are passed on.
async fn read(
    stream: impl OrderedStream<Data = Tagged>,
    name: String,
    owner: Owner,
    sender: mpsc::Sender<zbus::Result<Message>>,
    ignored: impl Fn(&Message),
) {
    let mut stream = std::pin::pin!(stream);
    while let Some((source, message)) = stream.next().await {
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        match source {
            Source::Owners => owner.follow(&name, &message),
            Source::Signals if owner.is(message.header().sender()) => {
                if sender.send(Ok(message)).is_err() {
                    return;
                }
            }
            Source::Signals => ignored(&message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const SERVICE: &str = "org.example.SignalTest";
    const PATH: &str = "/org/example/SignalTest";
    const INTERFACE: &str = "org.example.SignalTest";

    /// A `NameOwnerChanged` as it arrives with `sender` written on it.
    fn owner_change(sender: Option<&str>) -> Message {
        let mut builder =
            Message::signal(BUS_DAEMON_PATH, BUS_DAEMON, "NameOwnerChanged").expect("a signal");
        if let Some(sender) = sender {
            builder = builder.sender(sender).expect("a sender");
        }
        builder.build(&(SERVICE, "", ":1.4242")).expect("built")
    }

    #[test]
    fn only_the_daemon_passes_for_the_daemon() {
        assert!(sent_by_bus(&owner_change(Some(BUS_DAEMON))));
        // what the daemon writes on a peer's forgery, the forgery's own unique name
        assert!(!sent_by_bus(&owner_change(Some(":1.4242"))));
        assert!(!sent_by_bus(&owner_change(None)));
    }

    fn rule() -> MatchRule<'static> {
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(SERVICE)
            .and_then(|b| b.path(PATH))
            .and_then(|b| b.interface(INTERFACE))
            .and_then(|b| b.member("Said"))
            .expect("a rule")
            .build()
    }

    fn say(from: &Connection, to: Option<&str>, what: &str) {
        from.emit_signal(to, PATH, INTERFACE, "Said", &what)
            .expect("sent");
    }

    /// What the stream passes on within `wait`: the text of each signal.
    fn heard(signals: &mut FromOwner, wait: Duration) -> Option<String> {
        let said = signals.signals.recv_timeout(wait).ok()?;
        Some(said.and_then(|m| m.body().deserialize()).expect("a text"))
    }

    #[test]
    fn only_the_owner_is_heard_also_across_a_change_of_owner() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let listener = bus.connect();
        let to = listener.unique_name().unwrap().to_string();
        let ignored = Arc::new(Mutex::new(0));
        let count = ignored.clone();
        let mut signals = from_owner(&listener, SERVICE, rule(), move |_| {
            *count.lock().unwrap() += 1;
        })
        .expect("subscribed");
        let peer = bus.connect();

        // nobody owns the name yet: nobody is heard
        say(&peer, Some(&to), "before an owner");
        say(&peer, None, "before an owner");
        assert_eq!(heard(&mut signals, Duration::from_millis(300)), None);

        // the first owner, heard at once after it took the name, to everyone and directly
        let first = bus.connect();
        first.request_name(SERVICE).unwrap();
        say(&first, None, "first");
        say(&peer, Some(&to), "forged");
        say(&first, Some(&to), "first, directly");
        assert_eq!(
            heard(&mut signals, Duration::from_secs(5)).as_deref(),
            Some("first")
        );
        assert_eq!(
            heard(&mut signals, Duration::from_secs(5)).as_deref(),
            Some("first, directly")
        );
        assert!(!signals.owner().is(peer.unique_name().map(|n| &**n)));
        assert!(signals.owner().is(first.unique_name().map(|n| &**n)));

        // the name passes to another connection: the old owner is no longer heard
        first.release_name(SERVICE).unwrap();
        let second = bus.connect();
        second.request_name(SERVICE).unwrap();
        say(&second, None, "second");
        say(&first, Some(&to), "old owner");
        say(&second, Some(&to), "second, directly");
        assert_eq!(
            heard(&mut signals, Duration::from_secs(5)).as_deref(),
            Some("second")
        );
        assert_eq!(
            heard(&mut signals, Duration::from_secs(5)).as_deref(),
            Some("second, directly")
        );
        assert_eq!(heard(&mut signals, Duration::from_millis(300)), None);
        // the three sent directly by another connection; the daemon does not route the others
        assert_eq!(*ignored.lock().unwrap(), 3);
    }

    #[test]
    fn a_flood_of_forged_signals_stops_neither_the_owner_nor_the_connection() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let service = bus.connect();
        service.request_name(SERVICE).unwrap();
        let listener = bus.connect();
        let to = listener.unique_name().unwrap().to_string();
        let mut signals = from_owner(&listener, SERVICE, rule(), |_| {}).expect("subscribed");
        // many times what a stream holds, sent while the reader below is not reading
        let peer = bus.connect();
        for _ in 0..1000 {
            say(&peer, Some(&to), "forged");
        }
        say(&service, None, "genuine");
        // a call on the same connection is answered: the forged signals did not stop it reading
        let owner = listener
            .call_method(
                Some(BUS_DAEMON),
                BUS_DAEMON_PATH,
                Some(BUS_DAEMON),
                "GetNameOwner",
                &(SERVICE,),
            )
            .expect("answered")
            .body()
            .deserialize::<String>()
            .unwrap();
        assert_eq!(owner, service.unique_name().unwrap().as_str());
        assert_eq!(
            heard(&mut signals, Duration::from_secs(10)).as_deref(),
            Some("genuine")
        );
    }
}
