use std::fmt;
use std::io;
use std::path::PathBuf;

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use rustix::io::Errno;

use crate::dhcp::message::DecodeError;

/// Every failure of this crate.
#[derive(Debug)]
pub enum Error {
    /// Netlink, rfkill or the packet socket failed.
    Netlink(cupertino_netlink::Error),
    /// Reading or writing a file failed.
    Io { path: PathBuf, source: io::Error },
    /// A system call failed; `call` names it.
    Syscall { call: &'static str, errno: Errno },
    /// A packet on the DHCP socket of `interface` is not a valid DHCP message.
    Dhcp {
        interface: String,
        source: DecodeError,
    },
    /// The supplicant is not on the bus.
    SupplicantMissing,
    /// An effect names an interface index the model no longer knows.
    UnknownDevice(u32),
    /// The supplicant does not manage the device `interface` (it has no Wi-Fi station there).
    SupplicantDevice(String),
    /// The supplicant has no network that matches the request.
    SupplicantNetwork(String),
    /// A D-Bus call or the bus connection failed; `what` names the step.
    #[cfg(feature = "dbus")]
    Bus {
        what: &'static str,
        source: Box<zbus::Error>,
    },
    /// Another process owns the bus name.
    #[cfg(feature = "dbus")]
    NameTaken(&'static str),
    /// Asking logind about a caller failed.
    #[cfg(feature = "dbus")]
    Seat(cupertino_seat::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// The adapter for `map_err` on a system call: names the call that failed.
    pub(crate) fn syscall(call: &'static str) -> impl Fn(Errno) -> Self + Copy {
        move |errno| Self::Syscall { call, errno }
    }

    /// The adapter for `map_err` on file access.
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}

#[cfg(feature = "dbus")]
impl Error {
    /// The adapter for `map_err` on a D-Bus call: names the step that failed. The `zbus` error is
    /// boxed because it is large.
    pub(crate) fn bus(what: &'static str) -> impl Fn(zbus::Error) -> Self + Copy {
        move |source| Self::Bus {
            what,
            source: Box::new(source),
        }
    }
}

impl Error {
    /// The kind of failure, as a diagnostic event names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Netlink(_) => "netlink",
            Self::Io { .. } => "io",
            Self::Syscall { .. } => "syscall",
            Self::Dhcp { .. } => "dhcp",
            Self::SupplicantMissing => "supplicant_missing",
            Self::UnknownDevice(_) => "unknown_device",
            Self::SupplicantDevice(_) => "supplicant_device",
            Self::SupplicantNetwork(_) => "supplicant_network",
            #[cfg(feature = "dbus")]
            Self::Bus { .. } => "bus",
            #[cfg(feature = "dbus")]
            Self::NameTaken(_) => "name_taken",
            #[cfg(feature = "dbus")]
            Self::Seat(_) => "logind",
        }
    }
}

/// Records `network.backend.start_failed`: the step of the start that failed (`logind`, `rtnetlink`, `nl80211`,
/// `host`, `bus`, `supplicant`, `rfkill`, `bus_name`), the error's kind and its text. The caller still returns the error.
pub fn start_failed(step: &'static str, error: &Error) {
    diag::event(
        Severity::Error,
        "network.backend.start_failed",
        "the network service could not start",
    )
    .field("step", Value::Public(step))
    .field("kind", Value::Public(error.kind()))
    .field("error", Value::Public(&error.to_string()))
    .emit();
}

impl From<cupertino_netlink::Error> for Error {
    fn from(source: cupertino_netlink::Error) -> Self {
        Self::Netlink(source)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Netlink(source) => write!(f, "{source}"),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Syscall { call, errno } => write!(f, "{call}: {errno}"),
            Self::Dhcp { interface, source } => write!(f, "{interface}: {source}"),
            Self::SupplicantMissing => {
                write!(f, "the supplicant (net.connman.iwd) is not on the bus")
            }
            Self::UnknownDevice(ifindex) => write!(f, "no device with interface index {ifindex}"),
            Self::SupplicantDevice(interface) => {
                write!(f, "the supplicant does not manage {interface}")
            }
            Self::SupplicantNetwork(what) => {
                write!(f, "the supplicant has no such network: {what}")
            }
            #[cfg(feature = "dbus")]
            Self::Bus { what, source } => write!(f, "{what}: {source}"),
            #[cfg(feature = "dbus")]
            Self::NameTaken(name) => write!(f, "the bus name {name} is owned by another process"),
            #[cfg(feature = "dbus")]
            Self::Seat(source) => write!(f, "asking logind about the caller: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Netlink(source) => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Syscall { errno, .. } => Some(errno),
            Self::Dhcp { source, .. } => Some(source),
            Self::SupplicantMissing
            | Self::UnknownDevice(_)
            | Self::SupplicantDevice(_)
            | Self::SupplicantNetwork(_) => None,
            #[cfg(feature = "dbus")]
            Self::Bus { source, .. } => Some(source.as_ref()),
            #[cfg(feature = "dbus")]
            Self::NameTaken(_) => None,
            #[cfg(feature = "dbus")]
            Self::Seat(source) => Some(source),
        }
    }
}
