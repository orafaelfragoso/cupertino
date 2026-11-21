//! Byte layouts and ioctl numbers of the sound control interface (`include/uapi/sound/asound.h`,
//! protocol 2.0.10, 64-bit kernels).
//!
//! The kernel structures are handled as byte buffers with explicit offsets, so the parsers run on literal
//! bytes in tests and no pointer is ever cast to a struct. Every `snd_ctl_*` request is
//! `_IOC(dir, 'U', nr, sizeof(struct))`; the sizes below are asserted against the header in the tests.

use rustix::ioctl::{opcode, Direction, Opcode};

use super::error::{Error, Result};

#[cfg(not(target_pointer_width = "64"))]
compile_error!("the control structures are laid out for a 64-bit kernel (`long` is 8 bytes)");

/// The ioctl "type" byte of every control request (`'U'`).
const CTL_TYPE: u8 = b'U';

/// `sizeof(struct snd_ctl_elem_id)`.
pub const ELEM_ID_SIZE: usize = 64;
/// `sizeof(struct snd_ctl_elem_list)`.
pub const ELEM_LIST_SIZE: usize = 80;
/// `sizeof(struct snd_ctl_elem_info)`.
pub const ELEM_INFO_SIZE: usize = 272;
/// `sizeof(struct snd_ctl_elem_value)`.
pub const ELEM_VALUE_SIZE: usize = 1224;
/// `sizeof(struct snd_ctl_tlv)`: `numid` and `length`; the TLV words follow the header.
pub const TLV_HEADER_SIZE: usize = 8;

/// `SNDRV_CTL_ELEM_ID_NAME_MAXLEN`.
const NAME_FIELD: usize = 44;
/// Values a control can hold (`long value[128]`).
pub const MAX_VALUES: usize = 128;

/// `SNDRV_CTL_IOCTL_ELEM_LIST`.
pub(crate) const ELEM_LIST: Opcode = opcode::read_write::<[u8; ELEM_LIST_SIZE]>(CTL_TYPE, 0x10);
/// `SNDRV_CTL_IOCTL_ELEM_INFO`.
pub(crate) const ELEM_INFO: Opcode = opcode::read_write::<[u8; ELEM_INFO_SIZE]>(CTL_TYPE, 0x11);
/// `SNDRV_CTL_IOCTL_ELEM_READ`.
pub(crate) const ELEM_READ: Opcode = opcode::read_write::<[u8; ELEM_VALUE_SIZE]>(CTL_TYPE, 0x12);
/// `SNDRV_CTL_IOCTL_ELEM_WRITE`.
pub(crate) const ELEM_WRITE: Opcode = opcode::read_write::<[u8; ELEM_VALUE_SIZE]>(CTL_TYPE, 0x13);
/// `SNDRV_CTL_IOCTL_TLV_READ`: the encoded size is the header only; the kernel reads `length` from it and fills the words after it.
pub(crate) const TLV_READ: Opcode =
    opcode::from_components(Direction::ReadWrite, CTL_TYPE, 0x1a, TLV_HEADER_SIZE);

fn u32_at(buf: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(buf[offset..offset + 4].try_into().expect("4-byte slice"))
}

fn i64_at(buf: &[u8], offset: usize) -> i64 {
    i64::from_ne_bytes(buf[offset..offset + 8].try_into().expect("8-byte slice"))
}

/// A NUL-terminated (or full-width) text field as UTF-8.
fn text_at(buf: &[u8], offset: usize, len: usize, field: &'static str) -> Result<String> {
    let bytes = &buf[offset..offset + len];
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8(bytes[..end].to_vec()).map_err(|_| Error::NotUtf8 { field })
}

/// `SNDRV_CTL_ELEM_IFACE_*`: which device a control belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interface {
    /// A global control.
    Card,
    /// A hardware-dependent device.
    Hwdep,
    /// The virtual mixer device: volumes and switches.
    Mixer,
    /// A PCM device.
    Pcm,
    /// A raw MIDI device.
    Rawmidi,
    /// A timer.
    Timer,
    /// A sequencer client.
    Sequencer,
    /// A value this crate does not know.
    Other(u32),
}

impl Interface {
    /// The kernel's number.
    pub fn raw(self) -> u32 {
        match self {
            Self::Card => 0,
            Self::Hwdep => 1,
            Self::Mixer => 2,
            Self::Pcm => 3,
            Self::Rawmidi => 4,
            Self::Timer => 5,
            Self::Sequencer => 6,
            Self::Other(raw) => raw,
        }
    }

    /// The interface of a kernel number.
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            0 => Self::Card,
            1 => Self::Hwdep,
            2 => Self::Mixer,
            3 => Self::Pcm,
            4 => Self::Rawmidi,
            5 => Self::Timer,
            6 => Self::Sequencer,
            other => Self::Other(other),
        }
    }
}

/// `SNDRV_CTL_ELEM_TYPE_*`: the kind of value a control holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementType {
    /// Invalid.
    None,
    /// On or off.
    Boolean,
    /// A 32-bit range.
    Integer,
    /// One of a list of names.
    Enumerated,
    /// A byte array.
    Bytes,
    /// IEC958 (S/PDIF) setup.
    Iec958,
    /// A 64-bit range.
    Integer64,
    /// A value this crate does not know.
    Other(u32),
}

impl ElementType {
    /// The type of a kernel number.
    pub fn from_raw(raw: u32) -> Self {
        match raw {
            0 => Self::None,
            1 => Self::Boolean,
            2 => Self::Integer,
            3 => Self::Enumerated,
            4 => Self::Bytes,
            5 => Self::Iec958,
            6 => Self::Integer64,
            other => Self::Other(other),
        }
    }
}

/// `SNDRV_CTL_ELEM_ACCESS_*`: what can be done with a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access(pub u32);

impl Access {
    /// `SNDRV_CTL_ELEM_ACCESS_READ`.
    pub const READ: u32 = 1 << 0;
    /// `SNDRV_CTL_ELEM_ACCESS_WRITE`.
    pub const WRITE: u32 = 1 << 1;
    /// `SNDRV_CTL_ELEM_ACCESS_TLV_READ`.
    pub const TLV_READ: u32 = 1 << 4;
    /// `SNDRV_CTL_ELEM_ACCESS_INACTIVE`: the control does nothing now but may be updated.
    pub const INACTIVE: u32 = 1 << 8;

    /// The value can be read.
    pub fn readable(self) -> bool {
        self.0 & Self::READ != 0
    }

    /// The value can be written.
    pub fn writable(self) -> bool {
        self.0 & Self::WRITE != 0
    }

    /// The control currently has no effect.
    pub fn inactive(self) -> bool {
        self.0 & Self::INACTIVE != 0
    }

    /// The TLV data can be read.
    pub fn tlv_readable(self) -> bool {
        self.0 & Self::TLV_READ != 0
    }
}

/// `struct snd_ctl_elem_id`: names one control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementId {
    /// Numeric identifier on this card; zero is invalid.
    pub numid: u32,
    /// The device the control belongs to.
    pub interface: Interface,
    /// Device or client number.
    pub device: u32,
    /// Subdevice (substream) number.
    pub subdevice: u32,
    /// ASCII name, for example `"Master Playback Volume"`.
    pub name: String,
    /// Index among controls of the same name.
    pub index: u32,
}

impl ElementId {
    /// Parse the 64 bytes at the start of `buf`.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        Ok(Self {
            numid: u32_at(buf, 0),
            interface: Interface::from_raw(u32_at(buf, 4)),
            device: u32_at(buf, 8),
            subdevice: u32_at(buf, 12),
            name: text_at(buf, 16, NAME_FIELD, "name")?,
            index: u32_at(buf, 60),
        })
    }

    /// Write the 64 bytes at the start of `buf`.
    pub fn encode(&self, buf: &mut [u8]) -> Result<()> {
        if self.name.len() >= NAME_FIELD {
            return Err(Error::NameTooLong {
                len: self.name.len(),
            });
        }
        buf[0..4].copy_from_slice(&self.numid.to_ne_bytes());
        buf[4..8].copy_from_slice(&self.interface.raw().to_ne_bytes());
        buf[8..12].copy_from_slice(&self.device.to_ne_bytes());
        buf[12..16].copy_from_slice(&self.subdevice.to_ne_bytes());
        buf[16..16 + NAME_FIELD].fill(0);
        buf[16..16 + self.name.len()].copy_from_slice(self.name.as_bytes());
        buf[60..64].copy_from_slice(&self.index.to_ne_bytes());
        Ok(())
    }
}

/// The range of an integer control, in the control's own units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntegerRange {
    /// Smallest value.
    pub min: i64,
    /// Largest value.
    pub max: i64,
    /// Step between values; 0 when it varies.
    pub step: i64,
}

/// `struct snd_ctl_elem_info`: what a control is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementInfo {
    /// The control.
    pub id: ElementId,
    /// The kind of value.
    pub kind: ElementType,
    /// What can be done with it.
    pub access: Access,
    /// Number of values (channels).
    pub count: u32,
    /// The range of an integer or 64-bit integer control.
    pub range: Option<IntegerRange>,
}

impl ElementInfo {
    /// Parse the 272 bytes the kernel fills.
    pub fn parse(buf: &[u8; ELEM_INFO_SIZE]) -> Result<Self> {
        let kind = ElementType::from_raw(u32_at(buf, 64));
        let range = match kind {
            ElementType::Integer | ElementType::Integer64 => Some(IntegerRange {
                min: i64_at(buf, 80),
                max: i64_at(buf, 88),
                step: i64_at(buf, 96),
            }),
            _ => None,
        };
        Ok(Self {
            id: ElementId::parse(buf)?,
            kind,
            access: Access(u32_at(buf, 68)),
            count: u32_at(buf, 72),
            range,
        })
    }
}

/// The request buffer of `SNDRV_CTL_IOCTL_ELEM_INFO` for `id`.
pub(crate) fn info_request(id: &ElementId) -> Result<[u8; ELEM_INFO_SIZE]> {
    let mut buf = [0u8; ELEM_INFO_SIZE];
    id.encode(&mut buf)?;
    Ok(buf)
}

/// The request buffer of `SNDRV_CTL_IOCTL_ELEM_LIST`: `space` ids starting at `offset`, written to `ids`.
pub(crate) fn list_request(offset: u32, space: u32, ids: u64) -> [u8; ELEM_LIST_SIZE] {
    let mut buf = [0u8; ELEM_LIST_SIZE];
    buf[0..4].copy_from_slice(&offset.to_ne_bytes());
    buf[4..8].copy_from_slice(&space.to_ne_bytes());
    buf[16..24].copy_from_slice(&ids.to_ne_bytes());
    buf
}

/// `(used, count)` of an element list reply: the ids written and the controls the card has.
pub(crate) fn list_reply(buf: &[u8; ELEM_LIST_SIZE]) -> (u32, u32) {
    (u32_at(buf, 8), u32_at(buf, 12))
}

/// The request buffer of an element read or write: `id` and `values` (`long` each, booleans included).
pub(crate) fn value_request(id: &ElementId, values: &[i64]) -> Result<[u8; ELEM_VALUE_SIZE]> {
    if values.len() > MAX_VALUES {
        return Err(Error::ValueCount {
            count: values.len() as u32,
        });
    }
    let mut buf = [0u8; ELEM_VALUE_SIZE];
    id.encode(&mut buf)?;
    for (i, value) in values.iter().enumerate() {
        buf[72 + i * 8..80 + i * 8].copy_from_slice(&value.to_ne_bytes());
    }
    Ok(buf)
}

/// The first `count` values of an element read reply.
pub(crate) fn value_reply(buf: &[u8; ELEM_VALUE_SIZE], count: u32) -> Result<Vec<i64>> {
    if count as usize > MAX_VALUES {
        return Err(Error::ValueCount { count });
    }
    Ok((0..count as usize)
        .map(|i| i64_at(buf, 72 + i * 8))
        .collect())
}
