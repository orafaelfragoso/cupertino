//! Who sent a signal. The bus daemon hands a connection every signal addressed to its unique name, whatever its match
//! rules say, and zbus compares a rule's `sender` with a message only when the rule names a unique name (zbus counts the
//! daemon's own `org.freedesktop.DBus` as one): a rule with `sender='org.freedesktop.login1'` lets the same signal through
//! from any peer on the bus. A handler that acts on a signal checks the sender of each message it receives with one of
//! these, also where the rule filters already, so the check does not rest on how a rule's name is parsed.

use zbus::blocking::Connection;
use zbus::Message;

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

/// Whether `message` comes from the connection that owns the well-known `name` now, asked of the bus daemon
/// (`GetNameOwner`) for each message. False when the message has no sender, nobody owns `name` or the daemon does not
/// answer: a signal sent just before its sender left the name is dropped with the forged ones.
pub fn sent_by_owner(connection: &Connection, message: &Message, name: &str) -> bool {
    let header = message.header();
    let Some(sender) = header.sender() else {
        return false;
    };
    connection
        .call_method(
            Some(BUS_DAEMON),
            BUS_DAEMON_PATH,
            Some(BUS_DAEMON),
            "GetNameOwner",
            &(name,),
        )
        .ok()
        .and_then(|reply| reply.body().deserialize::<String>().ok())
        .is_some_and(|owner| owner == sender.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `NameOwnerChanged` as it arrives with `sender` written on it.
    fn owner_change(sender: Option<&str>) -> Message {
        let mut builder =
            Message::signal(BUS_DAEMON_PATH, BUS_DAEMON, "NameOwnerChanged").expect("a signal");
        if let Some(sender) = sender {
            builder = builder.sender(sender).expect("a sender");
        }
        builder
            .build(&("org.example.SignalTest", "", ":1.4242"))
            .expect("built")
    }

    #[test]
    fn only_the_daemon_passes_for_the_daemon() {
        assert!(sent_by_bus(&owner_change(Some(BUS_DAEMON))));
        // what the daemon writes on a peer's forgery, the forgery's own unique name
        assert!(!sent_by_bus(&owner_change(Some(":1.4242"))));
        assert!(!sent_by_bus(&owner_change(None)));
    }
}
