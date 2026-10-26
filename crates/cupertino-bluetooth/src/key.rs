//! Pairing keys, as the Load Link Keys / Load Long Term Keys commands and the New Link Key / New
//! Long Term Key events carry them (`doc/mgmt-protocol.rst`).

use std::fmt;

use crate::address::DeviceId;
use crate::error::ParseError;
use crate::reader::Reader;

/// 16 secret bytes. `Debug` never prints them, so a key cannot reach a log by accident.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct KeyValue(pub [u8; 16]);

impl fmt::Debug for KeyValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("KeyValue(<redacted>)")
    }
}

/// Link key types ("Load Link Keys"). A value the document does not define is kept as `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKeyType {
    Combination,
    LocalUnit,
    RemoteUnit,
    DebugCombination,
    UnauthenticatedP192,
    AuthenticatedP192,
    ChangedCombination,
    UnauthenticatedP256,
    AuthenticatedP256,
    Other(u8),
}

impl LinkKeyType {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0x00 => Self::Combination,
            0x01 => Self::LocalUnit,
            0x02 => Self::RemoteUnit,
            0x03 => Self::DebugCombination,
            0x04 => Self::UnauthenticatedP192,
            0x05 => Self::AuthenticatedP192,
            0x06 => Self::ChangedCombination,
            0x07 => Self::UnauthenticatedP256,
            0x08 => Self::AuthenticatedP256,
            other => Self::Other(other),
        }
    }

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::Combination => 0x00,
            Self::LocalUnit => 0x01,
            Self::RemoteUnit => 0x02,
            Self::DebugCombination => 0x03,
            Self::UnauthenticatedP192 => 0x04,
            Self::AuthenticatedP192 => 0x05,
            Self::ChangedCombination => 0x06,
            Self::UnauthenticatedP256 => 0x07,
            Self::AuthenticatedP256 => 0x08,
            Self::Other(value) => value,
        }
    }
}

/// A BR/EDR link key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkKey {
    pub device: DeviceId,
    pub key_type: LinkKeyType,
    pub value: KeyValue,
    pub pin_length: u8,
}

/// Size of one link key in Load Link Keys and New Link Key: address 6, address type 1, key type 1,
/// value 16, PIN length 1.
pub(crate) const LINK_KEY_SIZE: usize = 25;

impl LinkKey {
    pub(crate) fn to_wire(self) -> [u8; LINK_KEY_SIZE] {
        let mut out = [0u8; LINK_KEY_SIZE];
        out[..7].copy_from_slice(&self.device.to_wire());
        out[7] = self.key_type.to_wire();
        out[8..24].copy_from_slice(&self.value.0);
        out[24] = self.pin_length;
        out
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, ParseError> {
        Ok(Self {
            device: reader.device()?,
            key_type: LinkKeyType::from_wire(reader.u8()?),
            value: KeyValue(reader.array()?),
            pin_length: reader.u8()?,
        })
    }
}

/// Long term key types ("Load Long Term Keys"). A value the document does not define is kept as
/// `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LongTermKeyType {
    UnauthenticatedLegacy,
    AuthenticatedLegacy,
    UnauthenticatedP256,
    AuthenticatedP256,
    DebugP256,
    Other(u8),
}

impl LongTermKeyType {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0x00 => Self::UnauthenticatedLegacy,
            0x01 => Self::AuthenticatedLegacy,
            0x02 => Self::UnauthenticatedP256,
            0x03 => Self::AuthenticatedP256,
            0x04 => Self::DebugP256,
            other => Self::Other(other),
        }
    }

    pub const fn to_wire(self) -> u8 {
        match self {
            Self::UnauthenticatedLegacy => 0x00,
            Self::AuthenticatedLegacy => 0x01,
            Self::UnauthenticatedP256 => 0x02,
            Self::AuthenticatedP256 => 0x03,
            Self::DebugP256 => 0x04,
            Self::Other(value) => value,
        }
    }
}

/// A low-energy long term key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongTermKey {
    pub device: DeviceId,
    pub key_type: LongTermKeyType,
    /// True when the key was generated with the local side as central (the document's "Central").
    pub central: bool,
    pub encryption_size: u8,
    pub diversifier: u16,
    /// The 8-byte random number, in wire order.
    pub random: [u8; 8],
    pub value: KeyValue,
}

/// Size of one long term key in Load Long Term Keys and New Long Term Key: address 6, address type
/// 1, key type 1, central 1, encryption size 1, diversifier 2, random number 8, value 16.
pub(crate) const LONG_TERM_KEY_SIZE: usize = 36;

impl LongTermKey {
    pub(crate) fn to_wire(self) -> [u8; LONG_TERM_KEY_SIZE] {
        let mut out = [0u8; LONG_TERM_KEY_SIZE];
        out[..7].copy_from_slice(&self.device.to_wire());
        out[7] = self.key_type.to_wire();
        out[8] = u8::from(self.central);
        out[9] = self.encryption_size;
        out[10..12].copy_from_slice(&self.diversifier.to_le_bytes());
        out[12..20].copy_from_slice(&self.random);
        out[20..36].copy_from_slice(&self.value.0);
        out
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, ParseError> {
        Ok(Self {
            device: reader.device()?,
            key_type: LongTermKeyType::from_wire(reader.u8()?),
            central: reader.bool("long term key central")?,
            encryption_size: reader.u8()?,
            diversifier: reader.u16()?,
            random: reader.array()?,
            value: KeyValue(reader.array()?),
        })
    }
}
