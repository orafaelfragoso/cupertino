//! Result codes, settings bits and the class of device.

use std::fmt;

/// The status of a Command Complete or Command Status event (`doc/mgmt-protocol.rst`, "Error
/// Codes"). A value the document does not define is kept as [`Status::Other`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Success,
    UnknownCommand,
    NotConnected,
    Failed,
    ConnectFailed,
    AuthenticationFailed,
    NotPaired,
    NoResources,
    Timeout,
    AlreadyConnected,
    Busy,
    Rejected,
    NotSupported,
    InvalidParameters,
    Disconnected,
    NotPowered,
    Cancelled,
    InvalidIndex,
    RfKilled,
    AlreadyPaired,
    PermissionDenied,
    Other(u8),
}

impl Status {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0x00 => Self::Success,
            0x01 => Self::UnknownCommand,
            0x02 => Self::NotConnected,
            0x03 => Self::Failed,
            0x04 => Self::ConnectFailed,
            0x05 => Self::AuthenticationFailed,
            0x06 => Self::NotPaired,
            0x07 => Self::NoResources,
            0x08 => Self::Timeout,
            0x09 => Self::AlreadyConnected,
            0x0A => Self::Busy,
            0x0B => Self::Rejected,
            0x0C => Self::NotSupported,
            0x0D => Self::InvalidParameters,
            0x0E => Self::Disconnected,
            0x0F => Self::NotPowered,
            0x10 => Self::Cancelled,
            0x11 => Self::InvalidIndex,
            0x12 => Self::RfKilled,
            0x13 => Self::AlreadyPaired,
            0x14 => Self::PermissionDenied,
            other => Self::Other(other),
        }
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Success => "success",
            Self::UnknownCommand => "unknown command",
            Self::NotConnected => "not connected",
            Self::Failed => "failed",
            Self::ConnectFailed => "connect failed",
            Self::AuthenticationFailed => "authentication failed",
            Self::NotPaired => "not paired",
            Self::NoResources => "no resources",
            Self::Timeout => "timeout",
            Self::AlreadyConnected => "already connected",
            Self::Busy => "busy",
            Self::Rejected => "rejected",
            Self::NotSupported => "not supported",
            Self::InvalidParameters => "invalid parameters",
            Self::Disconnected => "disconnected",
            Self::NotPowered => "not powered",
            Self::Cancelled => "cancelled",
            Self::InvalidIndex => "invalid index",
            Self::RfKilled => "blocked by rfkill",
            Self::AlreadyPaired => "already paired",
            Self::PermissionDenied => "permission denied",
            Self::Other(value) => return write!(f, "status {value:#04x}"),
        };
        f.write_str(text)
    }
}

/// The settings bit mask of a controller (`doc/mgmt-protocol.rst`, "Read Controller
/// Information").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings(pub u32);

impl Settings {
    pub const POWERED: u32 = 1 << 0;
    pub const CONNECTABLE: u32 = 1 << 1;
    pub const FAST_CONNECTABLE: u32 = 1 << 2;
    pub const DISCOVERABLE: u32 = 1 << 3;
    pub const BONDABLE: u32 = 1 << 4;
    pub const LINK_SECURITY: u32 = 1 << 5;
    pub const SECURE_SIMPLE_PAIRING: u32 = 1 << 6;
    pub const BR_EDR: u32 = 1 << 7;
    pub const HIGH_SPEED: u32 = 1 << 8;
    pub const LOW_ENERGY: u32 = 1 << 9;
    pub const ADVERTISING: u32 = 1 << 10;
    pub const SECURE_CONNECTIONS: u32 = 1 << 11;

    pub const fn contains(self, bit: u32) -> bool {
        self.0 & bit == bit
    }

    pub const fn powered(self) -> bool {
        self.contains(Self::POWERED)
    }

    pub const fn connectable(self) -> bool {
        self.contains(Self::CONNECTABLE)
    }

    pub const fn discoverable(self) -> bool {
        self.contains(Self::DISCOVERABLE)
    }

    pub const fn bondable(self) -> bool {
        self.contains(Self::BONDABLE)
    }
}

/// The 24-bit class of device (Bluetooth Assigned Numbers, "Class of Device": major service
/// classes in bits 13 to 23, major device class in bits 8 to 12, minor device class in bits 2 to
/// 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClassOfDevice(u32);

/// The major device class, bits 8 to 12 of the class of device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MajorClass {
    Miscellaneous,
    Computer,
    Phone,
    NetworkAccessPoint,
    AudioVideo,
    Peripheral,
    Imaging,
    Wearable,
    Toy,
    Health,
    Uncategorized,
    /// A value the Assigned Numbers do not list.
    Other(u8),
}

impl ClassOfDevice {
    /// From the three wire bytes, least significant first.
    pub const fn from_wire(bytes: [u8; 3]) -> Self {
        Self(bytes[0] as u32 | (bytes[1] as u32) << 8 | (bytes[2] as u32) << 16)
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    pub const fn major(self) -> MajorClass {
        match (self.0 >> 8) & 0x1F {
            0 => MajorClass::Miscellaneous,
            1 => MajorClass::Computer,
            2 => MajorClass::Phone,
            3 => MajorClass::NetworkAccessPoint,
            4 => MajorClass::AudioVideo,
            5 => MajorClass::Peripheral,
            6 => MajorClass::Imaging,
            7 => MajorClass::Wearable,
            8 => MajorClass::Toy,
            9 => MajorClass::Health,
            31 => MajorClass::Uncategorized,
            other => MajorClass::Other(other as u8),
        }
    }

    /// The minor device class, bits 2 to 7 (6 bits; its meaning depends on the major class).
    pub const fn minor(self) -> u8 {
        ((self.0 >> 2) & 0x3F) as u8
    }
}
