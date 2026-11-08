//! Errors of the app effects.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// A backdrop shape with a negative extent or radius, or a non-finite value.
    InvalidShape,
    /// A backdrop alpha outside 0 to 1.
    InvalidAlpha,
    /// A glass spacing that is negative or not finite.
    InvalidGlass,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidShape => write!(
                f,
                "backdrop shape has a negative extent or radius, or is not finite"
            ),
            Self::InvalidAlpha => write!(f, "backdrop alpha is outside 0 to 1"),
            Self::InvalidGlass => write!(f, "glass spacing is negative or not finite"),
        }
    }
}

impl std::error::Error for Error {}
