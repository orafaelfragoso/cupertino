//! The kernel's sound control interface without alsa-lib: `/dev/snd/controlC*` over ioctls.
//!
//! - [`abi`]: byte layouts and ioctl numbers of the control structures, parsed from and encoded to byte buffers.
//! - [`tlv`]: gain curves (`DB_SCALE`, `DB_MINMAX`, `DB_RANGE`) from a control's TLV block.
//! - [`volume`]: the mapping between raw values and a level from 0 to 1 (`VolumeRange`).
//! - [`card`]: an open control device: card info, listing, reading and writing controls, TLV, change events.
//! - [`master`]: which control is the playback master volume and its mute switch (`Master`).
//!
//! Every function that opens a device takes the device directory (`/dev/snd` in production, see
//! [`card::DEVICE_DIR`]); every parser takes bytes. 64-bit kernels only.

pub mod abi;
pub mod card;
pub mod error;
pub mod master;
pub mod tlv;
pub mod volume;

pub use abi::{Access, CardInfo, ElementId, ElementInfo, ElementType, Event, IntegerRange, Interface};
pub use card::{card_indexes, Card, OpenMode, DEVICE_DIR};
pub use error::{Error, Result};
pub use master::{find_switch, select_master, Master, MasterRank};
pub use tlv::{DbCurve, DbSegment, DB_GAIN_MUTE};
pub use volume::VolumeRange;
