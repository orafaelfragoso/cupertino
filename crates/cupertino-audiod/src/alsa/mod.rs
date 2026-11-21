//! The kernel's sound control interface without alsa-lib: `/dev/snd/controlC*` over ioctls.
//!
//! - `abi`: byte layouts and ioctl numbers of the control structures, parsed from and encoded to byte buffers.
//! - `tlv`: gain curves (`DB_SCALE`, `DB_MINMAX`, `DB_RANGE`) from a control's TLV block.
//! - `volume`: the mapping between raw values and a level from 0 to 1 (`VolumeRange`).
//! - `card`: an open control device: listing, reading and writing controls, TLV.
//! - `master`: which control is the playback master volume and its mute switch (`Master`).
//!
//! Every function that opens a device takes the device directory (`/dev/snd` in production, see
//! [`DEVICE_DIR`]); every parser takes bytes. 64-bit kernels only.

mod abi;
mod card;
mod error;
mod master;
mod tlv;
mod volume;

pub use card::{OpenMode, DEVICE_DIR};
pub use error::Error;
pub use master::Master;
