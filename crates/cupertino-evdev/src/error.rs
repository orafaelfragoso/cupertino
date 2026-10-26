//! Error types.

use std::fmt;

/// A failure while setting up, reading or interpreting a device.
#[derive(Debug)]
pub enum Error {
    /// A system call failed; `op` names it (for example `"EVIOCGNAME"`).
    Io {
        /// The ioctl or system call that failed.
        op: &'static str,
        /// The errno it returned.
        errno: rustix::io::Errno,
    },
    /// The device name is not valid UTF-8.
    NameNotUtf8,
    /// A `read` returned a byte count that is not a whole number of records.
    TruncatedRecord {
        /// The byte count returned.
        len: usize,
    },
    /// The event stream or the device layout is not one this crate models.
    Frame(FrameError),
}

/// An event stream or capability layout the framer refuses to interpret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameError {
    /// The device has multitouch position axes but no `ABS_MT_SLOT` (protocol A).
    MultitouchWithoutSlots,
    /// The device has `ABS_MT_SLOT` but lacks `ABS_MT_POSITION_X/Y` or `ABS_MT_TRACKING_ID`.
    MultitouchWithoutPosition,
    /// `ABS_MT_SLOT` reports a range with no slots.
    EmptySlotRange,
    /// The stream contains `SYN_MT_REPORT`, which only protocol A devices emit.
    ProtocolA,
    /// An event type above `EV_MAX`.
    UnknownEventType {
        /// The type received.
        kind: u16,
    },
    /// An event of a modelled type that the device never declared, or a code out of the type's range.
    UnexpectedEvent {
        /// The event type.
        kind: u16,
        /// The event code.
        code: u16,
    },
    /// A key value other than 0, 1 or 2.
    InvalidKeyValue {
        /// The key code.
        code: u16,
        /// The value received.
        value: i32,
    },
    /// A switch value other than 0 or 1.
    InvalidSwitchValue {
        /// The switch code.
        code: u16,
        /// The value received.
        value: i32,
    },
    /// `ABS_MT_SLOT` selects a slot the device does not have.
    SlotOutOfRange {
        /// The slot selected.
        slot: i32,
        /// The number of slots the device declared.
        slot_count: u32,
    },
    /// A tracking id below -1.
    InvalidTrackingId {
        /// The value received.
        value: i32,
    },
    /// A resync snapshot lacks an axis the device needs.
    MissingSnapshotAxis {
        /// The absolute axis code.
        code: u16,
    },
    /// A resync snapshot holds the wrong number of slot values for an axis.
    SnapshotSlotCount {
        /// The multitouch axis code.
        code: u16,
        /// The values received.
        actual: usize,
        /// The number of slots the device declared.
        expected: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { op, errno } => write!(f, "{op} failed: {errno}"),
            Self::NameNotUtf8 => f.write_str("device name is not valid UTF-8"),
            Self::TruncatedRecord { len } => {
                write!(
                    f,
                    "read returned {len} bytes, not a whole number of input_event records"
                )
            }
            Self::Frame(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultitouchWithoutSlots => {
                f.write_str("multitouch axes without ABS_MT_SLOT (protocol A) are not supported")
            }
            Self::MultitouchWithoutPosition => {
                f.write_str("multitouch device lacks ABS_MT_POSITION_X/Y or ABS_MT_TRACKING_ID")
            }
            Self::EmptySlotRange => f.write_str("ABS_MT_SLOT declares no slots"),
            Self::ProtocolA => f.write_str("SYN_MT_REPORT: multitouch protocol A is not supported"),
            Self::UnknownEventType { kind } => write!(f, "unknown event type {kind:#x}"),
            Self::UnexpectedEvent { kind, code } => {
                write!(
                    f,
                    "event type {kind:#x} code {code:#x} is not valid for this device"
                )
            }
            Self::InvalidKeyValue { code, value } => {
                write!(f, "key {code:#x} has invalid value {value}")
            }
            Self::InvalidSwitchValue { code, value } => {
                write!(f, "switch {code:#x} has invalid value {value}")
            }
            Self::SlotOutOfRange { slot, slot_count } => {
                write!(f, "slot {slot} out of range for {slot_count} slots")
            }
            Self::InvalidTrackingId { value } => write!(f, "invalid tracking id {value}"),
            Self::MissingSnapshotAxis { code } => write!(f, "resync snapshot lacks axis {code:#x}"),
            Self::SnapshotSlotCount {
                code,
                actual,
                expected,
            } => {
                write!(
                    f,
                    "resync snapshot has {actual} values for axis {code:#x}, expected {expected}"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { errno, .. } => Some(errno),
            Self::Frame(error) => Some(error),
            Self::NameNotUtf8 | Self::TruncatedRecord { .. } => None,
        }
    }
}

impl std::error::Error for FrameError {}

impl From<FrameError> for Error {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}
