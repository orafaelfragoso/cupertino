//! The daemon: owns the bus names, keeps the latest [`Snapshot`] for the bus objects to read,
//! hands authorised requests to the engine, and turns the difference between two snapshots into
//! D-Bus signals.
//!
//! The host loop calls [`Daemon::publish`] after every change and answers the [`Request`]s that
//! [`Requests::drain`] returns. Objects hold no state of their own: each call reads the current
//! snapshot, so a getter never disagrees with the last signal.
//!
//! Who a caller is, the bus daemon is asked on a second connection ([`Lookup`]), never on the one
//! that reads the calls: there each answer, which carries the caller's process descriptor, would
//! queue behind the calls that came first, and a flood of calls would fill the daemon's queue for
//! the service with them, so that the daemon refused every other caller's call.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

use cupertino_peer::lookup::Lookup;
use rustix::event::{eventfd, EventfdFlags};
use rustix::io::{read, write, Errno};
use zbus::blocking::connection::Builder;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::Address;

use crate::auth::Authorizer;
use crate::engine::{Command, CommandError};
use crate::error::{start_failed, Error, Result};
use crate::interfaces::{
    Manager, NetworkObject, MANAGER_NAME, MANAGER_PATH, NETWORK_NAME, NETWORK_PATH,
};
use crate::model::Failure;
use crate::nm;
use crate::snapshot::Snapshot;

/// Which bus to serve on.
pub enum Bus {
    /// The system bus, where the daemon runs as root.
    System,
    /// The session bus, to run unprivileged.
    Session,
    /// A bus at an explicit address (tests).
    Address(String),
}

/// Everything the daemon needs to start.
pub struct Config {
    pub bus: Bus,
    /// Decides who may change settings.
    pub authorizer: Box<dyn Authorizer>,
}

/// A request that passed authorisation, waiting for the engine. Answer it with [`Request::reply`].
pub struct Request {
    pub command: Command,
    reply: async_channel::Sender<std::result::Result<(), CommandError>>,
}

impl Request {
    /// Sends the outcome to the caller on the bus. A caller that has gone away is not an error.
    pub fn reply(self, result: std::result::Result<(), CommandError>) {
        // the only failure is a closed channel: the caller disconnected before the answer
        let _ = self.reply.try_send(result);
    }
}

/// The requests from the bus, for the host loop to poll.
pub struct Requests {
    receiver: Receiver<Request>,
    waker: Arc<OwnedFd>,
}

impl Requests {
    /// A descriptor that becomes readable when [`Requests::drain`] has something.
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.waker.as_fd()
    }

    /// The requests since the last call.
    pub fn drain(&self) -> Result<Vec<Request>> {
        let mut counter = [0u8; 8];
        match read(&*self.waker, &mut counter) {
            // the counter was zero: another call took the wake already
            Ok(_) | Err(Errno::AGAIN) => {}
            Err(errno) => {
                return Err(Error::Syscall {
                    call: "read of the request eventfd",
                    errno,
                })
            }
        }
        Ok(self.receiver.try_iter().collect())
    }
}

/// State shared between the host thread and the bus objects.
pub struct Shared {
    snapshot: Mutex<Snapshot>,
    authorizer: Box<dyn Authorizer>,
    /// Asks the bus daemon who callers are, on a connection of its own.
    lookup: Lookup,
    requests: Sender<Request>,
    waker: Arc<OwnedFd>,
}

impl Shared {
    /// The current snapshot, locked. Keep the guard short: bus calls wait on it.
    pub fn snapshot(&self) -> MutexGuard<'_, Snapshot> {
        self.snapshot
            .lock()
            .expect("no thread panics while holding the snapshot")
    }

    pub fn authorizer(&self) -> &dyn Authorizer {
        self.authorizer.as_ref()
    }

    /// Where the callers' credentials are asked.
    pub fn lookup(&self) -> &Lookup {
        &self.lookup
    }

    /// Hands `command` to the engine and waits for its answer.
    pub async fn submit(&self, command: Command) -> std::result::Result<(), CommandError> {
        let (reply, answer) = async_channel::bounded(1);
        self.requests
            .send(Request { command, reply })
            .map_err(|_| CommandError::Failed("the daemon is shutting down".to_owned()))?;
        write(&*self.waker, &1u64.to_ne_bytes())
            .expect("the counter of an eventfd that is read does not overflow");
        answer
            .recv()
            .await
            .map_err(|_| CommandError::Failed("the daemon is shutting down".to_owned()))?
    }
}

/// The running daemon.
pub struct Daemon {
    connection: zbus::blocking::Connection,
    shared: Arc<Shared>,
}

impl Daemon {
    /// Connects to the bus and serves every object, without taking the names yet, so the host can
    /// publish its first snapshot before anyone asks. A failure is recorded as
    /// `network.backend.start_failed` (step `bus`).
    pub fn connect(config: Config) -> Result<(Self, Requests)> {
        Self::serve(config).inspect_err(|error| start_failed("bus", error))
    }

    fn serve(config: Config) -> Result<(Self, Requests)> {
        let (sender, receiver) = channel();
        let waker = Arc::new(
            eventfd(0, EventfdFlags::NONBLOCK | EventfdFlags::CLOEXEC)
                .map_err(Error::syscall("eventfd"))?,
        );
        let address = match &config.bus {
            Bus::System => Address::system(),
            Bus::Session => Address::session(),
            Bus::Address(address) => address.parse(),
        }
        .map_err(Error::bus("finding the bus"))?;
        let lookup =
            Lookup::connect(address.clone()).map_err(Error::bus("connecting to the bus"))?;
        let shared = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot::default()),
            authorizer: config.authorizer,
            lookup,
            requests: sender,
            waker: waker.clone(),
        });
        let builder = Builder::address(address).map_err(Error::bus("connecting to the bus"))?;
        let serve = Error::bus("exporting an object");
        let connection = builder
            .serve_at(NETWORK_PATH, NetworkObject(shared.clone()))
            .and_then(|builder| builder.serve_at(MANAGER_PATH, Manager(shared.clone())))
            .map_err(serve)?
            .build()
            .map_err(Error::bus("building the connection"))?;
        Ok((Self { connection, shared }, Requests { receiver, waker }))
    }

    /// Takes both bus names. Fails with [`Error::NameTaken`] when another daemon (NetworkManager,
    /// for instance) owns one of them, recorded as `network.backend.start_failed` (step `bus_name`).
    pub fn claim_names(&self) -> Result<()> {
        self.take_names()
            .inspect_err(|error| start_failed("bus_name", error))
    }

    fn take_names(&self) -> Result<()> {
        for name in [MANAGER_NAME, NETWORK_NAME] {
            let reply = match self
                .connection
                .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
            {
                Ok(reply) => reply,
                // with DoNotQueue the bus refuses a name that has an owner
                Err(zbus::Error::NameTaken) => return Err(Error::NameTaken(name)),
                Err(source) => return Err(Error::bus("requesting a bus name")(source)),
            };
            match reply {
                RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
                RequestNameReply::Exists | RequestNameReply::InQueue => {
                    return Err(Error::NameTaken(name))
                }
            }
        }
        Ok(())
    }

    pub fn connection(&self) -> &zbus::blocking::Connection {
        &self.connection
    }

    /// Records a new snapshot and signals what changed: properties of both interfaces,
    /// `StateChanged`, `NetworksChanged` and `ConnectionFailed`.
    pub fn publish(&self, next: Snapshot) -> Result<()> {
        let previous = std::mem::replace(&mut *self.shared.snapshot(), next.clone());
        zbus::block_on(announce(self.connection.inner(), &previous, &next))
    }
}

/// The failures that appear in `new` and were not in `old`, as (device name, reason).
fn new_failures<'a>(old: &Snapshot, new: &'a Snapshot) -> Vec<(&'a str, Failure)> {
    new.devices
        .iter()
        .filter_map(|device| {
            let failure = device.failure?;
            let before = old
                .devices
                .iter()
                .find(|before| before.ifindex == device.ifindex)
                .and_then(|before| before.failure);
            (before != Some(failure)).then_some((device.name.as_str(), failure))
        })
        .collect()
}

async fn announce(connection: &zbus::Connection, old: &Snapshot, new: &Snapshot) -> Result<()> {
    let signal = Error::bus("emitting a signal");
    let objects = connection.object_server();

    let network = objects
        .interface::<_, NetworkObject>(NETWORK_PATH)
        .await
        .map_err(signal)?;
    let emitter = network.signal_emitter();
    let object = network.get().await;
    if old.wireless_enabled != new.wireless_enabled {
        object
            .wireless_enabled_changed(emitter)
            .await
            .map_err(signal)?;
    }
    if old.devices != new.devices {
        object.devices_changed(emitter).await.map_err(signal)?;
    }
    if nm::state(old) != nm::state(new) {
        object.state_changed(emitter).await.map_err(signal)?;
    }
    if old.networks != new.networks {
        NetworkObject::networks_changed(emitter)
            .await
            .map_err(signal)?;
    }
    for (device, failure) in new_failures(old, new) {
        NetworkObject::connection_failed(emitter, device, failure.name())
            .await
            .map_err(signal)?;
    }

    let manager = objects
        .interface::<_, Manager>(MANAGER_PATH)
        .await
        .map_err(signal)?;
    let emitter = manager.signal_emitter();
    let object = manager.get().await;
    let (before, now) = (nm::state(old), nm::state(new));
    if before != now {
        object.state_changed(emitter).await.map_err(signal)?;
        Manager::state_signal(emitter, now as u32)
            .await
            .map_err(signal)?;
    }
    if nm::connectivity(old) != nm::connectivity(new) {
        object.connectivity_changed(emitter).await.map_err(signal)?;
    }
    if old.wireless_enabled != new.wireless_enabled {
        object
            .wireless_enabled_changed(emitter)
            .await
            .map_err(signal)?;
    }
    if nm::primary_connection_type(old) != nm::primary_connection_type(new) {
        object
            .primary_connection_type_changed(emitter)
            .await
            .map_err(signal)?;
    }
    Ok(())
}
