use std::fmt;

use cupertino_diagnostic_events::{self as diag, Severity, Value};

use crate::keys::KeyStoreError;

/// Every failure of the daemon that is not a refused request (those are
/// [`crate::request::RequestError`]).
#[derive(Debug)]
pub enum Error {
    /// The management socket failed.
    Bluetooth(cupertino_bluetooth::Error),
    /// The stored keys could not be read or written.
    Keys(KeyStoreError),
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
    /// The kind of failure, as a diagnostic event names it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bluetooth(_) => "management_socket",
            Self::Keys(_) => "keys",
            #[cfg(feature = "dbus")]
            Self::Bus { .. } => "bus",
            #[cfg(feature = "dbus")]
            Self::NameTaken(_) => "name_taken",
            #[cfg(feature = "dbus")]
            Self::Seat(_) => "logind",
        }
    }
}

/// Records `bluetooth.backend.start_failed`: the step of the start that failed (`logind`, `management_socket`, `bus`),
/// the error's kind and its text. The caller still returns the error.
pub fn start_failed(step: &'static str, error: &Error) {
    diag::event(
        Severity::Error,
        "bluetooth.backend.start_failed",
        "the Bluetooth service could not start",
    )
    .field("step", Value::Public(step))
    .field("kind", Value::Public(error.kind()))
    .field("error", Value::Public(&error.to_string()))
    .emit();
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

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bluetooth(source) => write!(f, "bluetooth: {source}"),
            Self::Keys(source) => write!(f, "keys: {source}"),
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
            Self::Bluetooth(source) => Some(source),
            Self::Keys(source) => Some(source),
            #[cfg(feature = "dbus")]
            Self::Bus { source, .. } => Some(source.as_ref()),
            #[cfg(feature = "dbus")]
            Self::NameTaken(_) => None,
            #[cfg(feature = "dbus")]
            Self::Seat(source) => Some(source),
        }
    }
}

impl From<cupertino_bluetooth::Error> for Error {
    fn from(source: cupertino_bluetooth::Error) -> Self {
        Self::Bluetooth(source)
    }
}

impl From<KeyStoreError> for Error {
    fn from(source: KeyStoreError) -> Self {
        Self::Keys(source)
    }
}
