//! Extended inquiry response (EIR) and advertising data, as carried by Device Found and Device
//! Connected.
//!
//! The data is a run of structures `[length][type][payload]` where `length` counts the type byte
//! and the payload. A length of zero ends the significant part (Core Specification, Vol 3 Part C
//! section 8; the rest is padding). Field types are from the Core Specification Supplement,
//! "Common Data Types". Types this parser does not know are skipped, as the specification
//! requires.

use std::fmt;

use crate::error::ParseError;
use crate::status::ClassOfDevice;

const FLAGS: u8 = 0x01;
const UUID16_INCOMPLETE: u8 = 0x02;
const UUID16_COMPLETE: u8 = 0x03;
const UUID32_INCOMPLETE: u8 = 0x04;
const UUID32_COMPLETE: u8 = 0x05;
const UUID128_INCOMPLETE: u8 = 0x06;
const UUID128_COMPLETE: u8 = 0x07;
const NAME_SHORTENED: u8 = 0x08;
const NAME_COMPLETE: u8 = 0x09;
const TX_POWER_LEVEL: u8 = 0x0A;
const CLASS_OF_DEVICE: u8 = 0x0D;
const APPEARANCE: u8 = 0x19;

/// The Bluetooth Base UUID, `00000000-0000-1000-8000-00805F9B34FB`, which 16-bit and 32-bit UUIDs
/// extend (Core Specification, Vol 3 Part B section 2.5.1).
const BASE_UUID: u128 = 0x0000_0000_0000_1000_8000_0080_5F9B_34FB;

/// A 128-bit UUID. Short UUIDs are stored extended onto the Bluetooth Base UUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid(pub u128);

impl Uuid {
    pub const fn from_u16(short: u16) -> Self {
        Self(BASE_UUID | (short as u128) << 96)
    }

    pub const fn from_u32(short: u32) -> Self {
        Self(BASE_UUID | (short as u128) << 96)
    }

    /// The 16-bit form, when this UUID extends the base UUID with a value that fits.
    pub const fn as_u16(self) -> Option<u16> {
        if self.0 & ((1 << 96) - 1) == BASE_UUID && (self.0 >> 96) <= u16::MAX as u128 {
            Some((self.0 >> 96) as u16)
        } else {
            None
        }
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = self.0;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (v >> 96) as u32,
            (v >> 80) as u16,
            (v >> 64) as u16,
            (v >> 48) as u16,
            v & 0xFFFF_FFFF_FFFF
        )
    }
}

/// A local name from the data, and whether it is the whole name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EirName {
    pub text: String,
    pub complete: bool,
}

/// The fields of the data that the daemon uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Eir {
    pub flags: Option<u8>,
    pub name: Option<EirName>,
    pub tx_power: Option<i8>,
    pub class: Option<ClassOfDevice>,
    pub appearance: Option<u16>,
    /// Every service UUID listed, complete or not, in order of appearance.
    pub uuids: Vec<Uuid>,
}

impl Eir {
    /// Parses the data. A structure that overruns the data or has a length its type forbids is an
    /// error. Names come from the remote device and may not be UTF-8: invalid sequences become
    /// U+FFFD so that a hostile name cannot hide a device.
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut eir = Self::default();
        let mut offset = 0;
        while let Some(&length) = data.get(offset) {
            let length = length as usize;
            if length == 0 {
                break;
            }
            let end = offset + 1 + length;
            if end > data.len() {
                return Err(ParseError::EirOverrun { offset });
            }
            let kind = data[offset + 1];
            let payload = &data[offset + 2..end];
            eir.field(kind, payload)?;
            offset = end;
        }
        Ok(eir)
    }

    fn field(&mut self, kind: u8, payload: &[u8]) -> Result<(), ParseError> {
        let wrong_length = || ParseError::EirFieldLength {
            kind,
            length: payload.len(),
        };
        match kind {
            FLAGS => self.flags = Some(*payload.first().ok_or_else(wrong_length)?),
            UUID16_INCOMPLETE | UUID16_COMPLETE => self.uuids_of(payload, 2, |b| {
                Uuid::from_u16(u16::from_le_bytes([b[0], b[1]]))
            })?,
            UUID32_INCOMPLETE | UUID32_COMPLETE => self.uuids_of(payload, 4, |b| {
                Uuid::from_u32(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            })?,
            UUID128_INCOMPLETE | UUID128_COMPLETE => self.uuids_of(payload, 16, |b| {
                let mut bytes = [0u8; 16];
                bytes.copy_from_slice(b);
                Uuid(u128::from_le_bytes(bytes))
            })?,
            NAME_SHORTENED | NAME_COMPLETE => {
                self.name = Some(EirName {
                    text: String::from_utf8_lossy(payload).into_owned(),
                    complete: kind == NAME_COMPLETE,
                })
            }
            TX_POWER_LEVEL => match payload {
                [level] => self.tx_power = Some(*level as i8),
                _ => return Err(wrong_length()),
            },
            CLASS_OF_DEVICE => match payload {
                [a, b, c] => self.class = Some(ClassOfDevice::from_wire([*a, *b, *c])),
                _ => return Err(wrong_length()),
            },
            APPEARANCE => match payload {
                [a, b] => self.appearance = Some(u16::from_le_bytes([*a, *b])),
                _ => return Err(wrong_length()),
            },
            _ => {}
        }
        Ok(())
    }

    fn uuids_of(
        &mut self,
        payload: &[u8],
        size: usize,
        make: impl Fn(&[u8]) -> Uuid,
    ) -> Result<(), ParseError> {
        if !payload.len().is_multiple_of(size) {
            return Err(ParseError::EirUuidLength {
                size,
                length: payload.len(),
            });
        }
        self.uuids.extend(payload.chunks_exact(size).map(make));
        Ok(())
    }
}
