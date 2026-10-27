//! Errors of the app effects.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A backdrop shape with a negative extent or radius, or a non-finite value.
    InvalidShape,
    /// A backdrop alpha outside 0 to 1.
    InvalidAlpha,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidShape => write!(
                f,
                "backdrop shape has a negative extent or radius, or is not finite"
            ),
            Self::InvalidAlpha => write!(f, "backdrop alpha is outside 0 to 1"),
        }
    }
}

impl std::error::Error for Error {}
