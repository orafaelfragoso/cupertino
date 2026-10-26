use std::fmt;
use std::path::PathBuf;

use rustix::io::Errno;

/// Why a kernel message could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The buffer ends inside `what`.
    Truncated {
        what: &'static str,
        needed: usize,
        have: usize,
    },
    /// A netlink header whose length field is smaller than the header or larger than the buffer.
    BadMessageLength { length: u32, available: usize },
    /// A netlink attribute whose length field is smaller than its header or larger than the buffer.
    BadAttributeLength {
        attribute: u16,
        length: u16,
        available: usize,
    },
    /// An attribute whose payload size is not the one its type has.
    AttributeSize {
        attribute: u16,
        expected: usize,
        actual: usize,
    },
    /// A string attribute that is not valid UTF-8.
    AttributeNotUtf8 { attribute: u16 },
    /// A message lacks an attribute the decoder cannot do without.
    MissingAttribute {
        message: &'static str,
        attribute: &'static str,
    },
    /// An address attribute that is not 4 or 16 bytes long.
    AddressSize { attribute: u16, actual: usize },
    /// An information element whose length runs past the end of the element list.
    InformationElement {
        id: u8,
        length: usize,
        available: usize,
    },
    /// A security element that ends before a field it announces.
    SecurityElement { what: &'static str },
    /// An event read from `/dev/rfkill` that is shorter than the 8 bytes every kernel writes.
    RfkillSize { actual: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { what, needed, have } => {
                write!(f, "{what} needs {needed} bytes, {have} left")
            }
            Self::BadMessageLength { length, available } => write!(
                f,
                "message length {length} with {available} bytes available"
            ),
            Self::BadAttributeLength {
                attribute,
                length,
                available,
            } => write!(
                f,
                "attribute {attribute} length {length} with {available} bytes available"
            ),
            Self::AttributeSize {
                attribute,
                expected,
                actual,
            } => write!(
                f,
                "attribute {attribute} has {actual} bytes, expected {expected}"
            ),
            Self::AttributeNotUtf8 { attribute } => {
                write!(f, "attribute {attribute} is not valid UTF-8")
            }
            Self::MissingAttribute { message, attribute } => {
                write!(f, "{message} without {attribute}")
            }
            Self::AddressSize { attribute, actual } => write!(
                f,
                "address attribute {attribute} has {actual} bytes, expected 4 or 16"
            ),
            Self::InformationElement {
                id,
                length,
                available,
            } => write!(
                f,
                "information element {id} of {length} bytes with {available} left"
            ),
            Self::SecurityElement { what } => write!(f, "security element ends inside {what}"),
            Self::RfkillSize { actual } => write!(f, "rfkill event of {actual} bytes, expected 8"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Every failure of this crate.
#[derive(Debug)]
pub enum Error {
    /// A system call on a netlink socket failed.
    Syscall { call: &'static str, source: Errno },
    /// The kernel answered a request with an error; `request` names the request.
    Kernel { request: &'static str, errno: Errno },
    /// Opening, reading or writing a device node failed.
    Device {
        path: PathBuf,
        call: &'static str,
        source: Errno,
    },
    /// A reply or event does not decode; `origin` names the request or socket.
    Parse {
        origin: &'static str,
        source: ParseError,
    },
    /// The kernel answered with a message of another request.
    UnexpectedSequence { expected: u32, actual: u32 },
    /// A multicast group id does not fit the 32 group bits a `bind` can join.
    GroupOutOfRange { name: String, id: u32 },
    /// The family lacks a multicast group the caller needs.
    MissingGroup {
        family: &'static str,
        group: &'static str,
    },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// The adapter for `map_err` on a socket call.
    pub(crate) fn syscall(call: &'static str) -> impl Fn(Errno) -> Self + Copy {
        move |source| Self::Syscall { call, source }
    }

    /// The adapter for `map_err` on a decoder.
    pub(crate) fn parse(origin: &'static str) -> impl Fn(ParseError) -> Self + Copy {
        move |source| Self::Parse { origin, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syscall { call, source } => write!(f, "{call}: {source}"),
            Self::Kernel { request, errno } => write!(f, "the kernel refused {request}: {errno}"),
            Self::Device { path, call, source } => write!(f, "{call} {}: {source}", path.display()),
            Self::Parse { origin, source } => write!(f, "{origin}: {source}"),
            Self::UnexpectedSequence { expected, actual } => {
                write!(f, "reply with sequence {actual}, expected {expected}")
            }
            Self::GroupOutOfRange { name, id } => write!(
                f,
                "multicast group {name} has id {id}, above the 32 a bind can join"
            ),
            Self::MissingGroup { family, group } => {
                write!(f, "the {family} family has no {group} multicast group")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Syscall { source, .. } | Self::Device { source, .. } => Some(source),
            Self::Kernel { errno, .. } => Some(errno),
            Self::Parse { source, .. } => Some(source),
            Self::UnexpectedSequence { .. }
            | Self::GroupOutOfRange { .. }
            | Self::MissingGroup { .. } => None,
        }
    }
}
