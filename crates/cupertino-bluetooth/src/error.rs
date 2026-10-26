use std::fmt;

/// Why bytes from the kernel did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer bytes than the field needs.
    Truncated { what: &'static str, needed: usize, found: usize },
    /// More bytes than the layout has.
    TrailingBytes { what: &'static str, extra: usize },
    /// The packet header's parameter length disagrees with the bytes received.
    LengthMismatch { header: usize, actual: usize },
    /// An address type outside 0 to 2.
    UnknownAddressType(u8),
    /// A flag byte that is neither 0 nor 1.
    InvalidBool { what: &'static str, value: u8 },
    /// Text that is not six hex pairs separated by colons.
    InvalidAddress(String),
    /// Text that names no address type.
    InvalidAddressTypeName(String),
    /// An EIR structure that runs past the end of the data.
    EirOverrun { offset: usize },
    /// An EIR UUID list whose length is not a multiple of the UUID size.
    EirUuidLength { size: usize, length: usize },
    /// An EIR field with the wrong length for its type.
    EirFieldLength { kind: u8, length: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { what, needed, found } => write!(f, "{what}: needs {needed} bytes, got {found}"),
            Self::TrailingBytes { what, extra } => write!(f, "{what}: {extra} bytes too many"),
            Self::LengthMismatch { header, actual } => write!(f, "header says {header} parameter bytes, packet has {actual}"),
            Self::UnknownAddressType(value) => write!(f, "unknown address type {value}"),
            Self::InvalidBool { what, value } => write!(f, "{what}: {value} is not 0 or 1"),
            Self::InvalidAddress(text) => write!(f, "not a device address: {text:?}"),
            Self::InvalidAddressTypeName(text) => write!(f, "not an address type: {text:?}"),
            Self::EirOverrun { offset } => write!(f, "EIR structure at offset {offset} runs past the data"),
            Self::EirUuidLength { size, length } => write!(f, "EIR UUID list of {length} bytes is not a multiple of {size}"),
            Self::EirFieldLength { kind, length } => write!(f, "EIR field {kind:#04x} has {length} bytes"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Every failure of this crate.
#[derive(Debug)]
pub enum Error {
    /// A system call failed.
    Syscall { call: &'static str, source: rustix::io::Errno },
    /// The kernel sent a packet that does not decode.
    Parse(ParseError),
    /// A command does not fit the 16-bit parameter length of the packet header.
    CommandTooLong { opcode: u16, length: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syscall { call, source } => write!(f, "{call}: {source}"),
            Self::Parse(source) => write!(f, "malformed management packet: {source}"),
            Self::CommandTooLong { opcode, length } => write!(f, "command {opcode:#06x} has {length} parameter bytes, more than a packet holds"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Syscall { source, .. } => Some(source),
            Self::Parse(source) => Some(source),
            Self::CommandTooLong { .. } => None,
        }
    }
}

impl From<ParseError> for Error {
    fn from(source: ParseError) -> Self {
        Self::Parse(source)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
