//! Error types.

use std::fmt;
use std::path::PathBuf;

/// Every failure of this crate.
#[derive(Debug)]
pub enum Error {
    /// A system call failed; `op` names it (for example `"SNDRV_CTL_IOCTL_ELEM_LIST"`).
    Io {
        /// The ioctl or system call that failed.
        op: &'static str,
        /// The errno it returned.
        errno: rustix::io::Errno,
    },
    /// The device directory could not be listed.
    ReadDir {
        /// The directory.
        dir: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// A fixed-size text field of a kernel structure is not valid UTF-8.
    NotUtf8 {
        /// The field, for example `"name"`.
        field: &'static str,
    },
    /// An element name does not fit the 44-byte field with its terminating NUL.
    NameTooLong {
        /// Length of the name in bytes.
        len: usize,
    },
    /// A TLV block that is cut short or whose length is not a whole number of words.
    MalformedTlv(&'static str),
    /// A volume control whose minimum is not below its maximum.
    EmptyRange {
        /// The control's minimum.
        min: i64,
        /// The control's maximum.
        max: i64,
    },
    /// A control holds no value to read.
    NotAnIntegerControl,
    /// A number of values the kernel structure cannot carry, or that is not the control's channel count.
    ValueCount {
        /// The number of values.
        count: u32,
    },
    /// No card has a playback volume control.
    NoPlaybackVolume,
    /// The master has no mute switch.
    NoMuteSwitch,
    /// The card has no control device node.
    NoSuchCard {
        /// The card number asked for.
        card: u32,
    },
    /// A `read` returned a byte count that is not a whole number of events.
    TruncatedEvent {
        /// The byte count returned.
        len: usize,
    },
    /// An event of a type other than `SNDRV_CTL_EVENT_ELEM`.
    UnknownEventType {
        /// The type received.
        kind: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { op, errno } => write!(f, "{op}: {errno}"),
            Self::ReadDir { dir, source } => write!(f, "cannot list {}: {source}", dir.display()),
            Self::NotUtf8 { field } => write!(f, "the {field} field is not valid UTF-8"),
            Self::NameTooLong { len } => write!(f, "an element name of {len} bytes does not fit 44 bytes with its NUL"),
            Self::MalformedTlv(why) => write!(f, "malformed TLV data: {why}"),
            Self::EmptyRange { min, max } => write!(f, "the control range {min}..{max} is empty"),
            Self::NotAnIntegerControl => write!(f, "the control returned no value"),
            Self::ValueCount { count } => write!(f, "{count} values do not fit the control"),
            Self::NoPlaybackVolume => write!(f, "no card has a playback volume control"),
            Self::NoMuteSwitch => write!(f, "the master volume has no mute switch"),
            Self::NoSuchCard { card } => write!(f, "sound card {card} has no control device"),
            Self::TruncatedEvent { len } => write!(f, "a read of {len} bytes is not a whole number of control events"),
            Self::UnknownEventType { kind } => write!(f, "unknown control event type {kind}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadDir { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result of this crate's operations.
pub type Result<T> = std::result::Result<T, Error>;
