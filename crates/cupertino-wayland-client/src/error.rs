//! Errors of the transport.

use std::fmt;
use wayland_client::backend::WaylandError;
use wayland_client::globals::GlobalError;
use wayland_client::{ConnectError, DispatchError};

#[derive(Debug)]
pub enum Error {
    /// The compositor does not advertise a global the transport needs, or only below `minimum_version`.
    MissingRequiredProtocol {
        interface: &'static str,
        minimum_version: u32,
    },
    /// The connection is gone or unusable; nothing more can be sent on it.
    Disconnected(Disconnect),
    /// The compositor sent configure `serial` after `latest`: configure serials must increase.
    InvalidSerial { serial: u32, latest: u32 },
    /// The compositor does not support this shm format, or the selection is not offered in this MIME type.
    UnsupportedFormat(String),
    /// The compositor sent a scale of zero, or a size times the scale does not fit a buffer.
    InvalidScale,
    /// The buffer device went away; shm buffers never report it.
    DeviceLost,
    /// The compositor only grants the request in answer to user input, and there was none yet.
    PermissionDenied(&'static str),
    /// The compositor or another client ended the operation first (a replaced selection, a dismissed popup, a
    /// transfer that did not finish in time).
    Cancelled,
    /// No surface with this handle; it was destroyed.
    UnknownSurface,
    /// The surface has not received its first configure.
    NotConfigured,
    /// The compositor still holds every buffer of the surface; draw again after the next frame callback.
    NoFreeBuffer,
    /// The compositor could not import a dmabuf ([`crate::Connection::create_dmabuf_buffer`]); the surface draws into
    /// shared memory instead.
    BufferRejected,
    /// The optional protocol the request needs is not advertised; check [`crate::CapabilitySet`] first.
    Unsupported(crate::Capability),
    /// The compositor sent a value the protocol forbids.
    Protocol(&'static str),
    /// Creating shared memory, a pipe or the wake channel failed.
    Io(std::io::Error),
}

/// Why the connection ended.
#[derive(Debug)]
pub enum Disconnect {
    /// `WAYLAND_DISPLAY` is unset or unreachable.
    Connect(ConnectError),
    /// The registry could not be read.
    Registry(GlobalError),
    /// The compositor raised a protocol error or closed the socket.
    Dispatch(DispatchError),
    /// Reading from or writing to the socket failed.
    Backend(WaylandError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingRequiredProtocol {
                interface,
                minimum_version,
            } => write!(
                f,
                "the compositor does not offer {interface} version {minimum_version} or newer"
            ),
            Self::Disconnected(d) => write!(f, "disconnected from the compositor: {d}"),
            Self::InvalidSerial { serial, latest } => {
                write!(f, "configure {serial} arrived after configure {latest}")
            }
            Self::UnsupportedFormat(format) => write!(f, "format {format} is not offered"),
            Self::InvalidScale => write!(f, "invalid scale"),
            Self::DeviceLost => write!(f, "the buffer device was lost"),
            Self::PermissionDenied(what) => write!(f, "{what} needs a user input event first"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::UnknownSurface => write!(f, "unknown surface"),
            Self::NotConfigured => write!(f, "the surface has not been configured yet"),
            Self::NoFreeBuffer => write!(f, "the compositor holds every buffer of the surface"),
            Self::BufferRejected => write!(f, "the compositor could not import the dmabuf"),
            Self::Unsupported(capability) => {
                write!(f, "the compositor does not offer {capability:?}")
            }
            Self::Protocol(what) => write!(f, "protocol violation by the compositor: {what}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl fmt::Display for Disconnect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "{e}"),
            Self::Registry(e) => write!(f, "{e}"),
            Self::Dispatch(e) => write!(f, "{e}"),
            Self::Backend(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<DispatchError> for Error {
    fn from(e: DispatchError) -> Self {
        Self::Disconnected(Disconnect::Dispatch(e))
    }
}

impl From<WaylandError> for Error {
    fn from(e: WaylandError) -> Self {
        Self::Disconnected(Disconnect::Backend(e))
    }
}
