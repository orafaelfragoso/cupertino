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
    /// A layer value outside its property's range.
    InvalidLayerValue,
    /// A request the server's version of the protocol does not have.
    Unsupported(&'static str),
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
            Self::InvalidLayerValue => write!(f, "layer value is outside its property's range"),
            Self::Unsupported(request) => write!(f, "the server does not take {request}"),
        }
    }
}

impl std::error::Error for Error {}
