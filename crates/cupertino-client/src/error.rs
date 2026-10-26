//! Errors of the client toolkit.

use std::fmt;
use wayland_client::globals::GlobalError;
use wayland_client::{ConnectError, DispatchError};

#[derive(Debug)]
pub enum ClientError {
    /// No connection to the server (`WAYLAND_DISPLAY` unset or unreachable).
    Connect(ConnectError),
    /// The registry could not be read.
    Registry(GlobalError),
    /// A required global is not advertised by the server.
    MissingGlobal(&'static str),
    /// A global is advertised in a version below what the toolkit needs.
    GlobalVersion {
        interface: &'static str,
        minimum: u32,
    },
    /// The connection failed or the server raised a protocol error.
    Dispatch(DispatchError),
    /// Reading from or writing to the connection failed.
    Wayland(wayland_client::backend::WaylandError),
    /// Creating, sizing or mapping shared memory failed.
    Shm(std::io::Error),
    /// The wake channel or the poll on the connection failed.
    Io(std::io::Error),
    /// Mapping the keymap the server sent failed.
    Keymap(std::io::Error),
    /// The server sent a value the protocol forbids.
    Protocol(&'static str),
    /// No surface with this id.
    UnknownSurface,
    /// No output with this id (it may have been unplugged).
    UnknownOutput,
    /// The request applies to layer surfaces only.
    NotLayerSurface,
    /// The request applies to popups only.
    NotPopup,
    /// The request applies to application windows only.
    NotToplevel,
    /// The surface still has popups; destroy them first.
    HasPopups,
    /// The surface has not received its first configure.
    NotConfigured,
    /// The surface size is empty or too large for a shm buffer.
    BufferSize { width: u32, height: u32 },
    /// Both buffers of the surface are still held by the server.
    NoFreeBuffer,
    /// An activation token needs the serial of an input event and none arrived yet.
    NoInputSerial,
    /// A backdrop shape, popup rectangle or window size with a negative or empty extent, or a non-finite value.
    InvalidShape,
    /// A backdrop alpha outside 0 to 1.
    InvalidAlpha,
    /// A session lock that has not finished already exists.
    SessionLockActive,
    /// No session lock was requested, or the server finished it.
    NoSessionLock,
    /// The session is not locked: the server has not confirmed the lock, or there is none.
    NotLocked,
    /// An idle timeout beyond what the protocol's milliseconds hold.
    InvalidTimeout,
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "cannot connect to the display server: {e}"),
            Self::Registry(e) => write!(f, "cannot read the registry: {e}"),
            Self::MissingGlobal(name) => write!(
                f,
                "the server does not advertise the required global {name}"
            ),
            Self::GlobalVersion { interface, minimum } => {
                write!(f, "the server offers {interface} below version {minimum}")
            }
            Self::Dispatch(e) => write!(f, "connection error: {e}"),
            Self::Wayland(e) => write!(f, "connection error: {e}"),
            Self::Shm(e) => write!(f, "shared memory: {e}"),
            Self::Io(e) => write!(f, "event wait: {e}"),
            Self::Keymap(e) => write!(f, "cannot read the keymap: {e}"),
            Self::Protocol(what) => write!(f, "protocol violation by the server: {what}"),
            Self::UnknownSurface => write!(f, "unknown surface"),
            Self::UnknownOutput => write!(f, "unknown output"),
            Self::NotLayerSurface => write!(f, "the surface is not a layer surface"),
            Self::NotPopup => write!(f, "the surface is not a popup"),
            Self::NotToplevel => write!(f, "the surface is not an application window"),
            Self::HasPopups => write!(f, "the surface still has popups"),
            Self::NotConfigured => write!(f, "the surface has not been configured yet"),
            Self::BufferSize { width, height } => {
                write!(f, "no shm buffer for a {width}x{height} surface")
            }
            Self::NoFreeBuffer => write!(f, "both buffers are still in use by the server"),
            Self::NoInputSerial => {
                write!(f, "no input event yet to derive an activation token from")
            }
            Self::InvalidShape => write!(
                f,
                "backdrop shape has a negative size or radius or a non-finite value"
            ),
            Self::InvalidAlpha => write!(f, "backdrop alpha is outside 0 to 1"),
            Self::SessionLockActive => write!(f, "a session lock already exists"),
            Self::NoSessionLock => write!(
                f,
                "no session lock is requested or it was finished by the server"
            ),
            Self::NotLocked => write!(f, "the session is not locked"),
            Self::InvalidTimeout => write!(f, "idle timeout does not fit in milliseconds"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<DispatchError> for ClientError {
    fn from(e: DispatchError) -> Self {
        Self::Dispatch(e)
    }
}

impl From<wayland_client::backend::WaylandError> for ClientError {
    fn from(e: wayland_client::backend::WaylandError) -> Self {
        Self::Wayland(e)
    }
}
