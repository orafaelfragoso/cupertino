//! Device addresses as the management interface carries them.

use std::fmt;
use std::str::FromStr;

use crate::error::ParseError;

/// A 48-bit device address. The bytes are kept in wire order (least significant first), which is
/// the reverse of how an address is written.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address([u8; 6]);

impl Address {
    /// The all-zero address, which a controller without a public address reports.
    pub const ZERO: Self = Self([0; 6]);

    pub const fn from_wire(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub const fn to_wire(self) -> [u8; 6] {
        self.0
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(f, "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", b[5], b[4], b[3], b[2], b[1], b[0])
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl FromStr for Address {
    type Err = ParseError;

    /// Six hex pairs separated by colons, most significant first, in either case.
    fn from_str(text: &str) -> Result<Self, ParseError> {
        let invalid = || ParseError::InvalidAddress(text.to_owned());
        let mut bytes = [0u8; 6];
        let mut parts = text.split(':');
        for slot in bytes.iter_mut().rev() {
            let part = parts.next().ok_or_else(invalid)?;
            if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid());
            }
            *slot = u8::from_str_radix(part, 16).map_err(|_| invalid())?;
        }
        if parts.next().is_some() {
            return Err(invalid());
        }
        Ok(Self(bytes))
    }
}

/// The address type of a device (BlueZ `doc/mgmt-protocol.rst`, "Address_Type").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AddressType {
    BrEdr,
    LePublic,
    LeRandom,
}

impl AddressType {
    pub fn from_wire(value: u8) -> Result<Self, ParseError> {
        match value {
            0 => Ok(Self::BrEdr),
            1 => Ok(Self::LePublic),
            2 => Ok(Self::LeRandom),
            other => Err(ParseError::UnknownAddressType(other)),
        }
    }

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::BrEdr => 0,
            Self::LePublic => 1,
            Self::LeRandom => 2,
        }
    }

    /// True for the two low-energy types.
    pub const fn is_le(self) -> bool {
        !matches!(self, Self::BrEdr)
    }
}

impl fmt::Display for AddressType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BrEdr => "bredr",
            Self::LePublic => "le-public",
            Self::LeRandom => "le-random",
        })
    }
}

impl FromStr for AddressType {
    type Err = ParseError;

    fn from_str(text: &str) -> Result<Self, ParseError> {
        match text {
            "bredr" => Ok(Self::BrEdr),
            "le-public" => Ok(Self::LePublic),
            "le-random" => Ok(Self::LeRandom),
            other => Err(ParseError::InvalidAddressTypeName(other.to_owned())),
        }
    }
}

/// A remote device: its address and the type of that address. The same six bytes can name a
/// BR/EDR device and a low-energy one, so both parts are the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId {
    pub address: Address,
    pub kind: AddressType,
}

impl DeviceId {
    /// The 7 bytes that name a device in most commands: address, then type.
    pub(crate) fn to_wire(self) -> [u8; 7] {
        let a = self.address.to_wire();
        [a[0], a[1], a[2], a[3], a[4], a[5], self.kind.to_wire()]
    }
}

/// The set of address types a discovery covers: a bit mask (`doc/mgmt-protocol.rst`, "Start
/// Discovery": bit 0 BR/EDR, bit 1 LE public, bit 2 LE random).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddressTypes(u8);

impl AddressTypes {
    pub const BR_EDR: Self = Self(0x01);
    /// LE public and random (the `0x06` of the document).
    pub const LE: Self = Self(0x06);
    /// BR/EDR and LE, interleaved (the `0x07` of the document).
    pub const ALL: Self = Self(0x07);

    pub const fn to_wire(self) -> u8 {
        self.0
    }

    pub const fn from_wire(bits: u8) -> Self {
        Self(bits)
    }
}
