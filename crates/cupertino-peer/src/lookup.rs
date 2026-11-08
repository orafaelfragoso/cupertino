//! Who called, asked on a connection of its own. A service that asks the bus daemon for a caller's credentials on the
//! connection that reads its incoming calls queues the answer behind every call that arrived before it, and each answer
//! carries the caller's process descriptor: the daemon holds only a few descriptors waiting for one connection (64 on a
//! system bus) and refuses every message for the connection while more wait. A peer that sends enough calls at once fills
//! that queue with the service's own answers, and the daemon then refuses the calls of everyone else ("destination has a
//! full message queue") for as long as the flood lasts.
//!
//! [`Lookup`] asks on a second connection to the same bus, which receives nothing but these answers, with at most
//! [`MAX_IN_FLIGHT`] questions out at once and a deadline on each, so the answers never fill a queue and the connection
//! that reads the calls never waits for one.

use std::path::Path;
use std::time::Duration;

use async_lock::Semaphore;
use zbus::fdo::ConnectionCredentials;
use zbus::names::{BusName, UniqueName};

use crate::Caller;

/// The most credential questions out at once: well under the descriptors the daemon queues for one connection, so a
/// flood of calls cannot fill it.
pub const MAX_IN_FLIGHT: usize = 8;

/// How long one question may wait for the daemon's answer before the call it serves fails.
pub const ANSWER_DEADLINE: Duration = Duration::from_secs(5);

/// A connection to the bus that only asks who callers are.
pub struct Lookup {
    connection: zbus::Connection,
    in_flight: Semaphore,
}

impl Lookup {
    /// Opens the connection, to the same bus as the service (blocking).
    pub fn connect(address: zbus::Address) -> zbus::Result<Self> {
        let connection = zbus::blocking::connection::Builder::address(address)?
            .method_timeout(ANSWER_DEADLINE)
            .build()?;
        Ok(Self::on(connection.into_inner()))
    }

    /// Asks on `connection`, which must not be the one that reads the service's calls.
    pub fn on(connection: zbus::Connection) -> Self {
        Self {
            connection,
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
        }
    }

    /// The credentials the daemon gives for `sender`, the unique name of a caller on the service's bus.
    pub async fn credentials(
        &self,
        sender: &UniqueName<'_>,
    ) -> zbus::Result<ConnectionCredentials> {
        let _turn = self.in_flight.acquire().await;
        let bus = zbus::fdo::DBusProxy::new(&self.connection).await?;
        Ok(bus
            .get_connection_credentials(BusName::from(sender.clone()))
            .await?)
    }

    /// The caller behind `sender`, as [`crate::identify`] names it; anonymous when the daemon does not answer.
    pub async fn caller(&self, sender: &UniqueName<'_>) -> Caller {
        match self.credentials(sender).await {
            Ok(credentials) => Caller::from_credentials(&credentials, Path::new("/proc")),
            Err(_) => Caller::Anonymous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lookup_names_the_caller_on_another_connection() {
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let caller = bus.connect();
        let lookup = Lookup::connect(bus.address.as_str().try_into().unwrap()).unwrap();
        let sender = caller.unique_name().unwrap().to_owned();
        let credentials = zbus::block_on(lookup.credentials(&sender)).unwrap();
        assert_eq!(credentials.process_id(), Some(std::process::id()));
        assert!(matches!(
            zbus::block_on(lookup.caller(&sender)),
            Caller::Process(ref process) if process.process.pid == std::process::id()
        ));
        // a name nobody holds is no caller
        let gone = UniqueName::try_from(":1.4242").unwrap();
        assert!(zbus::block_on(lookup.credentials(&gone)).is_err());
        assert!(matches!(
            zbus::block_on(lookup.caller(&gone)),
            Caller::Anonymous
        ));
    }
}
