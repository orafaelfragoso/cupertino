//! Management commands and their encoding.
//!
//! Source: BlueZ `doc/mgmt-protocol.rst` (the former `mgmt-api.txt`), section "Commands". A
//! packet is a header of three little-endian 16-bit fields (code, controller index, parameter
//! length) followed by the parameters.

use crate::address::{AddressTypes, DeviceId};
use crate::error::{Error, Result};
use crate::key::{LinkKey, LongTermKey, LINK_KEY_SIZE, LONG_TERM_KEY_SIZE};

/// The controller a command or event belongs to. [`ControllerIndex::NONE`] is for commands that
/// concern no controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControllerIndex(pub u16);

impl ControllerIndex {
    /// The document's `<non-controller>`, 0xFFFF.
    pub const NONE: Self = Self(0xFFFF);
}

/// A command code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Opcode(pub u16);

impl Opcode {
    pub const READ_VERSION: Self = Self(0x0001);
    pub const READ_INDEX_LIST: Self = Self(0x0003);
    pub const READ_CONTROLLER_INFO: Self = Self(0x0004);
    pub const SET_POWERED: Self = Self(0x0005);
    pub const SET_DISCOVERABLE: Self = Self(0x0006);
    pub const SET_CONNECTABLE: Self = Self(0x0007);
    pub const SET_BONDABLE: Self = Self(0x0009);
    pub const LOAD_LINK_KEYS: Self = Self(0x0012);
    pub const LOAD_LONG_TERM_KEYS: Self = Self(0x0013);
    pub const DISCONNECT: Self = Self(0x0014);
    pub const GET_CONNECTIONS: Self = Self(0x0015);
    pub const PIN_CODE_NEGATIVE_REPLY: Self = Self(0x0017);
    pub const SET_IO_CAPABILITY: Self = Self(0x0018);
    pub const PAIR_DEVICE: Self = Self(0x0019);
    pub const CANCEL_PAIR_DEVICE: Self = Self(0x001A);
    pub const UNPAIR_DEVICE: Self = Self(0x001B);
    pub const USER_CONFIRMATION_REPLY: Self = Self(0x001C);
    pub const USER_CONFIRMATION_NEGATIVE_REPLY: Self = Self(0x001D);
    pub const USER_PASSKEY_REPLY: Self = Self(0x001E);
    pub const USER_PASSKEY_NEGATIVE_REPLY: Self = Self(0x001F);
    pub const START_DISCOVERY: Self = Self(0x0023);
    pub const STOP_DISCOVERY: Self = Self(0x0024);
    pub const CONFIRM_NAME: Self = Self(0x0025);
}

/// The input and output a local user has during pairing ("Set IO Capability").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoCapability {
    DisplayOnly,
    DisplayYesNo,
    KeyboardOnly,
    NoInputNoOutput,
    /// Converted by the kernel to `DisplayYesNo` on a BR/EDR link.
    KeyboardDisplay,
}

impl IoCapability {
    pub const fn to_wire(self) -> u8 {
        match self {
            Self::DisplayOnly => 0,
            Self::DisplayYesNo => 1,
            Self::KeyboardOnly => 2,
            Self::NoInputNoOutput => 3,
            Self::KeyboardDisplay => 4,
        }
    }
}

/// The discoverable mode of "Set Discoverable".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discoverable {
    Off,
    /// General discoverable; `timeout` is in seconds, 0 for no timeout.
    General {
        timeout: u16,
    },
    /// Limited discoverable; the document requires a timeout.
    Limited {
        timeout: u16,
    },
}

/// A management command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    ReadVersion,
    ReadIndexList,
    ReadControllerInfo,
    SetPowered(bool),
    SetDiscoverable(Discoverable),
    SetConnectable(bool),
    SetBondable(bool),
    SetIoCapability(IoCapability),
    StartDiscovery(AddressTypes),
    StopDiscovery(AddressTypes),
    /// Answers a Device Found event that has the Confirm Name flag; `name_known` false makes the
    /// kernel look the name up.
    ConfirmName {
        device: DeviceId,
        name_known: bool,
    },
    PairDevice {
        device: DeviceId,
        io_capability: IoCapability,
    },
    CancelPairDevice(DeviceId),
    UnpairDevice {
        device: DeviceId,
        disconnect: bool,
    },
    Disconnect(DeviceId),
    GetConnections,
    /// `accept` false sends the negative reply.
    UserConfirmationReply {
        device: DeviceId,
        accept: bool,
    },
    /// `passkey` `None` sends the negative reply.
    UserPasskeyReply {
        device: DeviceId,
        passkey: Option<u32>,
    },
    /// Refuses a legacy PIN request; this daemon does not support PIN pairing.
    PinCodeNegativeReply(DeviceId),
    /// Feeds the kernel the stored BR/EDR link keys. Debug keys are always refused.
    LoadLinkKeys(Vec<LinkKey>),
    LoadLongTermKeys(Vec<LongTermKey>),
}

impl Command {
    pub fn opcode(&self) -> Opcode {
        match self {
            Self::ReadVersion => Opcode::READ_VERSION,
            Self::ReadIndexList => Opcode::READ_INDEX_LIST,
            Self::ReadControllerInfo => Opcode::READ_CONTROLLER_INFO,
            Self::SetPowered(_) => Opcode::SET_POWERED,
            Self::SetDiscoverable(_) => Opcode::SET_DISCOVERABLE,
            Self::SetConnectable(_) => Opcode::SET_CONNECTABLE,
            Self::SetBondable(_) => Opcode::SET_BONDABLE,
            Self::SetIoCapability(_) => Opcode::SET_IO_CAPABILITY,
            Self::StartDiscovery(_) => Opcode::START_DISCOVERY,
            Self::StopDiscovery(_) => Opcode::STOP_DISCOVERY,
            Self::ConfirmName { .. } => Opcode::CONFIRM_NAME,
            Self::PairDevice { .. } => Opcode::PAIR_DEVICE,
            Self::CancelPairDevice(_) => Opcode::CANCEL_PAIR_DEVICE,
            Self::UnpairDevice { .. } => Opcode::UNPAIR_DEVICE,
            Self::Disconnect(_) => Opcode::DISCONNECT,
            Self::GetConnections => Opcode::GET_CONNECTIONS,
            Self::UserConfirmationReply { accept: true, .. } => Opcode::USER_CONFIRMATION_REPLY,
            Self::UserConfirmationReply { accept: false, .. } => {
                Opcode::USER_CONFIRMATION_NEGATIVE_REPLY
            }
            Self::UserPasskeyReply {
                passkey: Some(_), ..
            } => Opcode::USER_PASSKEY_REPLY,
            Self::UserPasskeyReply { passkey: None, .. } => Opcode::USER_PASSKEY_NEGATIVE_REPLY,
            Self::PinCodeNegativeReply(_) => Opcode::PIN_CODE_NEGATIVE_REPLY,
            Self::LoadLinkKeys(_) => Opcode::LOAD_LINK_KEYS,
            Self::LoadLongTermKeys(_) => Opcode::LOAD_LONG_TERM_KEYS,
        }
    }

    /// The device a command is about, when it names one.
    pub fn device(&self) -> Option<DeviceId> {
        match self {
            Self::ConfirmName { device, .. }
            | Self::PairDevice { device, .. }
            | Self::UnpairDevice { device, .. }
            | Self::UserConfirmationReply { device, .. }
            | Self::UserPasskeyReply { device, .. } => Some(*device),
            Self::CancelPairDevice(device)
            | Self::Disconnect(device)
            | Self::PinCodeNegativeReply(device) => Some(*device),
            _ => None,
        }
    }

    fn parameters(&self) -> Vec<u8> {
        match self {
            Self::ReadVersion
            | Self::ReadIndexList
            | Self::ReadControllerInfo
            | Self::GetConnections => Vec::new(),
            Self::SetPowered(on) | Self::SetConnectable(on) | Self::SetBondable(on) => {
                vec![u8::from(*on)]
            }
            Self::SetDiscoverable(mode) => {
                let (value, timeout) = match mode {
                    Discoverable::Off => (0u8, 0u16),
                    Discoverable::General { timeout } => (1, *timeout),
                    Discoverable::Limited { timeout } => (2, *timeout),
                };
                let mut out = vec![value];
                out.extend_from_slice(&timeout.to_le_bytes());
                out
            }
            Self::SetIoCapability(capability) => vec![capability.to_wire()],
            Self::StartDiscovery(types) | Self::StopDiscovery(types) => vec![types.to_wire()],
            Self::ConfirmName { device, name_known } => {
                [&device.to_wire()[..], &[u8::from(*name_known)]].concat()
            }
            Self::PairDevice {
                device,
                io_capability,
            } => [&device.to_wire()[..], &[io_capability.to_wire()]].concat(),
            Self::UnpairDevice { device, disconnect } => {
                [&device.to_wire()[..], &[u8::from(*disconnect)]].concat()
            }
            Self::CancelPairDevice(device)
            | Self::Disconnect(device)
            | Self::PinCodeNegativeReply(device)
            | Self::UserConfirmationReply { device, .. }
            | Self::UserPasskeyReply {
                device,
                passkey: None,
            } => device.to_wire().to_vec(),
            Self::UserPasskeyReply {
                device,
                passkey: Some(passkey),
            } => [&device.to_wire()[..], &passkey.to_le_bytes()].concat(),
            Self::LoadLinkKeys(keys) => {
                // Debug_Keys 0: debug keys are never accepted.
                let mut out = vec![0u8];
                out.extend_from_slice(&(keys.len() as u16).to_le_bytes());
                out.reserve(keys.len() * LINK_KEY_SIZE);
                keys.iter()
                    .for_each(|key| out.extend_from_slice(&key.to_wire()));
                out
            }
            Self::LoadLongTermKeys(keys) => {
                let mut out = Vec::with_capacity(2 + keys.len() * LONG_TERM_KEY_SIZE);
                out.extend_from_slice(&(keys.len() as u16).to_le_bytes());
                keys.iter()
                    .for_each(|key| out.extend_from_slice(&key.to_wire()));
                out
            }
        }
    }

    /// The whole packet for `index`. Fails when the parameters do not fit the 16-bit length.
    pub fn encode(&self, index: ControllerIndex) -> Result<Vec<u8>> {
        let parameters = self.parameters();
        let opcode = self.opcode();
        let length = u16::try_from(parameters.len()).map_err(|_| Error::CommandTooLong {
            opcode: opcode.0,
            length: parameters.len(),
        })?;
        let mut packet = Vec::with_capacity(6 + parameters.len());
        packet.extend_from_slice(&opcode.0.to_le_bytes());
        packet.extend_from_slice(&index.0.to_le_bytes());
        packet.extend_from_slice(&length.to_le_bytes());
        packet.extend_from_slice(&parameters);
        Ok(packet)
    }
}
