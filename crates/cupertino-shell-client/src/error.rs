//! Errors of the shell protocols.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The transport failed.
    Transport(cupertino_wayland_client::Error),
    /// No output with this id (it may have been unplugged).
    UnknownOutput,
    /// A rectangle with a non-finite value, or an index beyond the protocol's.
    InvalidShape,
    /// A layer surface configure left an extent at 0 that the surface did not request.
    Unsized,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => e.fmt(f),
            Self::UnknownOutput => write!(f, "unknown output"),
            Self::InvalidShape => write!(f, "rectangle is not finite or index is out of range"),
            Self::Unsized => write!(
                f,
                "configure without a size for a surface that requested none"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(e) => Some(e),
            _ => None,
        }
    }
}

impl From<cupertino_wayland_client::Error> for Error {
    fn from(e: cupertino_wayland_client::Error) -> Self {
        Self::Transport(e)
    }
}
