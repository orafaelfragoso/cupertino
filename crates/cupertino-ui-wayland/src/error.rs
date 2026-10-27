//! Errors of drawing and placing surfaces.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// The transport failed.
    Transport(cupertino_wayland_client::Error),
    /// The surface size is empty or too large for a pixmap.
    BufferSize { width: u32, height: u32 },
    /// A popup rectangle with an empty extent or a non-finite value.
    InvalidShape,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => e.fmt(f),
            Self::BufferSize { width, height } => {
                write!(f, "no pixmap of {width}x{height} pixels")
            }
            Self::InvalidShape => write!(f, "popup rectangle is empty or not finite"),
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
