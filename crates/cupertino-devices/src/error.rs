use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why a uevent, a sysfs attribute or a capability mask could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A `KEY=VALUE` entry without `=`.
    MalformedProperty(String),
    /// A property entry that is empty (two NULs in a row, or a blank line).
    EmptyProperty,
    /// A datagram or file that is not valid UTF-8.
    NotUtf8,
    /// A datagram without the `action@devpath` header.
    MissingHeader,
    /// A required property is absent.
    MissingProperty(&'static str),
    /// A property or attribute whose value is not the expected number.
    InvalidNumber { key: &'static str, value: String },
    /// Only one of `MAJOR` and `MINOR` is present.
    IncompleteDevnum,
    /// A capability or flag attribute with no words.
    EmptyMask,
    /// The datagram was larger than the receive buffer.
    Truncated,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedProperty(entry) => write!(f, "property without '=': {entry:?}"),
            Self::EmptyProperty => write!(f, "empty property entry"),
            Self::NotUtf8 => write!(f, "not valid UTF-8"),
            Self::MissingHeader => write!(f, "missing action@devpath header"),
            Self::MissingProperty(key) => write!(f, "missing property {key}"),
            Self::InvalidNumber { key, value } => write!(f, "invalid number for {key}: {value:?}"),
            Self::IncompleteDevnum => write!(f, "MAJOR and MINOR must come together"),
            Self::EmptyMask => write!(f, "capability mask has no words"),
            Self::Truncated => write!(f, "datagram larger than the receive buffer"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Every failure of this crate.
#[derive(Debug)]
pub enum Error {
    /// A sysfs file or directory could not be read.
    Io { path: PathBuf, source: io::Error },
    /// A socket system call failed.
    Syscall { call: &'static str, source: rustix::io::Errno },
    /// Data was read but is malformed; `origin` names the file or datagram.
    Parse { origin: String, source: ParseError },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Syscall { call, source } => write!(f, "{call}: {source}"),
            Self::Parse { origin, source } => write!(f, "{origin}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Syscall { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
        }
    }
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;
