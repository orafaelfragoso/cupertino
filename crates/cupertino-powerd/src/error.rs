use std::fmt;
use std::io;
use std::path::PathBuf;

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use rustix::io::Errno;

use crate::{backlight, supply};

/// Every failure of this crate.
#[derive(Debug)]
pub enum Error {
    /// Reading, listing or writing a sysfs path failed.
    Io { path: PathBuf, source: io::Error },
    /// A power supply attribute does not parse.
    Supply {
        name: String,
        source: supply::ParseError,
    },
    /// A backlight attribute does not parse.
    Backlight {
        name: String,
        source: backlight::ParseError,
    },
    /// Enumerating or classifying input devices failed.
    Devices(cupertino_devices::Error),
    /// Opening or reading the lid switch device failed.
    Evdev {
        path: PathBuf,
        source: cupertino_evdev::Error,
    },
    /// Opening a device node failed.
    Open { path: PathBuf, errno: Errno },
    /// Waiting for events failed.
    Poll(Errno),
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
            Self::Io { .. } => "io",
            Self::Supply { .. } => "supply",
            Self::Backlight { .. } => "backlight",
            Self::Devices(_) => "devices",
            Self::Evdev { .. } => "evdev",
            Self::Open { .. } => "open",
            Self::Poll(_) => "poll",
            #[cfg(feature = "dbus")]
            Self::Bus { .. } => "bus",
            #[cfg(feature = "dbus")]
            Self::NameTaken(_) => "name_taken",
            #[cfg(feature = "dbus")]
            Self::Seat(_) => "logind",
        }
    }
}

/// Records `power.backend.start_failed`: the step of the start that failed (`lid`, `logind`, `uevent`, `bus`), the
/// error's kind and its text. The caller still returns the error.
pub fn start_failed(step: &'static str, error: &Error) {
    diag::event(
        Severity::Error,
        "power.backend.start_failed",
        "the power service could not start",
    )
    .field("step", Value::Public(step))
    .field("kind", Value::Public(error.kind()))
    .field("error", Value::Public(&error.to_string()))
    .emit();
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Supply { name, source } => write!(f, "power supply {name}: {source}"),
            Self::Backlight { name, source } => write!(f, "backlight {name}: {source}"),
            Self::Devices(source) => write!(f, "listing input devices: {source}"),
            Self::Evdev { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Open { path, errno } => write!(f, "opening {}: {errno}", path.display()),
            Self::Poll(errno) => write!(f, "poll: {errno}"),
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
            Self::Io { source, .. } => Some(source),
            Self::Supply { source, .. } => Some(source),
            Self::Backlight { source, .. } => Some(source),
            Self::Devices(source) => Some(source),
            Self::Evdev { source, .. } => Some(source),
            Self::Open { errno, .. } | Self::Poll(errno) => Some(errno),
            #[cfg(feature = "dbus")]
            Self::Bus { source, .. } => Some(source.as_ref()),
            #[cfg(feature = "dbus")]
            Self::NameTaken(_) => None,
            #[cfg(feature = "dbus")]
            Self::Seat(source) => Some(source),
        }
    }
}
