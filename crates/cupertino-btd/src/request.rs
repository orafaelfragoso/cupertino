//! The vocabulary between the model and the code that runs it: what callers ask, what the model
//! answers, and what it wants done.

use std::fmt;

use cupertino_bluetooth::{Address, Command, ControllerIndex, DeviceId, Status};

use crate::device::{DeviceInfo, DeviceRef};
use crate::keys::KeySet;
use crate::pairing::{PairError, PairingId, Prompt};

/// Names one request, so that its answer reaches the caller that made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket(pub u64);

/// What a caller on the bus can ask. Each acts on the default adapter, the lowest controller
/// index that is ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    SetPowered(bool),
    StartDiscovery,
    StopDiscovery,
    Pair(DeviceRef),
    Connect(DeviceRef),
    Disconnect(DeviceRef),
    /// Unpair, and forget a device that was only discovered.
    Remove(DeviceRef),
}

/// Why a request failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// The daemon runs read-only.
    ReadOnly,
    /// There is no adapter that is ready.
    NoAdapter,
    NotPowered,
    UnknownDevice(String),
    /// Two devices share the address; name the address type too.
    AmbiguousDevice(String),
    AlreadyPaired,
    NotPaired,
    AlreadyConnected,
    NotConnected,
    /// The controller or the daemon is busy with something else (another pairing, a discovery
    /// started elsewhere).
    Busy,
    ConnectFailed,
    /// The user refused the pairing.
    Rejected,
    AuthenticationFailed,
    Timeout,
    Cancelled,
    /// The agent could not be asked or failed.
    Agent(String),
    /// The remote device wants a legacy PIN, which is not supported.
    PinUnsupported,
    /// Bluetooth is blocked by a switch (rfkill).
    Blocked,
    NotSupported,
    /// The kernel refused the daemon (it lacks the privilege).
    PermissionDenied,
    /// Any other status the controller answered with.
    Controller(Status),
    /// Sending to the controller failed.
    Transport(String),
}

impl RequestError {
    /// Maps the status of a failed command.
    pub fn from_status(status: Status) -> Self {
        match status {
            Status::NotPowered => Self::NotPowered,
            Status::Busy => Self::Busy,
            Status::AlreadyPaired => Self::AlreadyPaired,
            Status::NotPaired => Self::NotPaired,
            Status::AlreadyConnected => Self::AlreadyConnected,
            Status::NotConnected => Self::NotConnected,
            Status::ConnectFailed => Self::ConnectFailed,
            Status::AuthenticationFailed => Self::AuthenticationFailed,
            Status::Timeout => Self::Timeout,
            Status::Cancelled => Self::Cancelled,
            Status::RfKilled => Self::Blocked,
            Status::NotSupported => Self::NotSupported,
            Status::PermissionDenied => Self::PermissionDenied,
            other => Self::Controller(other),
        }
    }
}

impl From<PairError> for RequestError {
    fn from(error: PairError) -> Self {
        match error {
            PairError::Rejected => Self::Rejected,
            PairError::AuthenticationFailed(_) => Self::AuthenticationFailed,
            PairError::Timeout => Self::Timeout,
            PairError::Cancelled => Self::Cancelled,
            PairError::Agent(message) => Self::Agent(message),
            PairError::PinUnsupported => Self::PinUnsupported,
            PairError::Controller(status) => Self::from_status(status),
        }
    }
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "the daemon runs read-only"),
            Self::NoAdapter => write!(f, "there is no Bluetooth adapter"),
            Self::NotPowered => write!(f, "the adapter is powered off"),
            Self::UnknownDevice(device) => write!(f, "no device {device}"),
            Self::AmbiguousDevice(address) => write!(f, "{address} names a BR/EDR and a low-energy device; add /bredr, /le-public or /le-random"),
            Self::AlreadyPaired => write!(f, "the device is already paired"),
            Self::NotPaired => write!(f, "the device is not paired"),
            Self::AlreadyConnected => write!(f, "the device is already connected"),
            Self::NotConnected => write!(f, "the device is not connected"),
            Self::Busy => write!(f, "the controller is busy"),
            Self::ConnectFailed => write!(f, "the connection failed"),
            Self::Rejected => write!(f, "the pairing was refused"),
            Self::AuthenticationFailed => write!(f, "authentication failed"),
            Self::Timeout => write!(f, "timed out"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Agent(message) => write!(f, "the pairing agent failed: {message}"),
            Self::PinUnsupported => write!(f, "the device asks for a PIN, which is not supported"),
            Self::Blocked => write!(f, "Bluetooth is blocked by a switch"),
            Self::NotSupported => write!(f, "the controller does not support this"),
            Self::PermissionDenied => write!(f, "the kernel refused the daemon: it needs CAP_NET_ADMIN"),
            Self::Controller(status) => write!(f, "the controller answered: {status}"),
            Self::Transport(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for RequestError {}

/// The properties of the default adapter that the bus shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdapterProps {
    /// An adapter is ready.
    pub available: bool,
    pub powered: bool,
    pub discovering: bool,
}

/// A change clients should hear about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Adapter {
        old: AdapterProps,
        new: AdapterProps,
    },
    DeviceAdded(DeviceInfo),
    DeviceChanged(DeviceInfo),
    /// The key of the device that went away (`DeviceInfo::key`).
    DeviceRemoved(String),
}

/// The agent is asked to do `prompt` for a pairing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentCall {
    pub pairing: PairingId,
    pub device: DeviceInfo,
    pub prompt: Prompt,
}

/// The agent's answer to a call, or the news that it could not answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentAnswer {
    pub pairing: PairingId,
    pub reply: crate::pairing::AgentReply,
}

/// What the model wants done. The model does none of it: the caller sends, writes, opens, signals
/// and answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Send {
        index: ControllerIndex,
        command: Command,
    },
    /// Start a baseband connection and hold it open. `ticket` is the request that waits for it.
    OpenLink {
        index: ControllerIndex,
        adapter: Address,
        device: DeviceId,
        ticket: Ticket,
    },
    CloseLink {
        adapter: Address,
        device: DeviceId,
    },
    /// Read the stored keys of the adapter and pass them to `Model::keys_loaded`.
    LoadKeys {
        index: ControllerIndex,
        address: Address,
    },
    SaveKeys {
        address: Address,
        keys: KeySet,
    },
    /// A request is answered.
    Complete {
        ticket: Ticket,
        result: Result<(), RequestError>,
    },
    Changed(Change),
    Agent(AgentCall),
    /// Something to print: a command that failed without a caller waiting for it.
    Log(String),
}
