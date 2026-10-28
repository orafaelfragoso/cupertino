//! The kernel's Bluetooth management interface, without BlueZ.
//!
//! - [`address`]: [`Address`], [`AddressType`], [`DeviceId`].
//! - [`command`]: [`Command`] and its encoding.
//! - [`event`]: [`Event`], [`Packet`] and [`event::decode_packet`].
//! - [`reply`]: the return parameters of Command Complete.
//! - [`eir`]: extended inquiry response and advertising data.
//! - [`key`]: link keys and long term keys.
//! - [`socket`]: [`ControlSocket`], the control channel.
//! - [`link`]: [`Link`], an L2CAP socket that opens a baseband link.
//!
//! The codec is pure: it turns values into bytes and bytes into values, and the tests run it on
//! literal packets. Wire layouts follow BlueZ `doc/mgmt-protocol.rst`, the former
//! `doc/mgmt-api.txt`.
#![deny(unsafe_code)]

pub mod address;
pub mod command;
pub mod eir;
pub mod error;
pub mod event;
pub mod key;
pub mod link;
pub mod reply;
pub mod socket;
pub mod status;

mod reader;
mod sockaddr;

pub use address::{Address, AddressType, AddressTypes, DeviceId};
pub use command::{Command, ControllerIndex, Discoverable, IoCapability, Opcode};
pub use eir::{Eir, EirName, Uuid};
pub use error::{Error, ParseError, Result};
pub use event::{decode_packet, DisconnectReason, Event, FoundFlags, Packet};
pub use key::{KeyValue, LinkKey, LinkKeyType, LongTermKey, LongTermKeyType};
pub use link::Link;
pub use socket::{ControlSocket, Drained, OPEN_CALL};
pub use status::{ClassOfDevice, MajorClass, Settings, Status};
