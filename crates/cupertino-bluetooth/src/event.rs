//! Management events and their decoding.
//!
//! Source: BlueZ `doc/mgmt-protocol.rst` (the former `mgmt-api.txt`), section "Events". All
//! fields are little-endian.

use crate::address::{AddressTypes, DeviceId};
use crate::command::{ControllerIndex, Opcode};
use crate::eir::Eir;
use crate::error::ParseError;
use crate::key::{LinkKey, LongTermKey};
use crate::reader::{fixed_string, Reader};
use crate::status::{ClassOfDevice, Settings, Status};

const HEADER_SIZE: usize = 6;

/// The "Reason" of Device Disconnected. A value the document does not define is kept as `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectReason {
    Unspecified,
    Timeout,
    LocalHost,
    RemoteHost,
    AuthenticationFailure,
    LocalHostSuspend,
    Other(u8),
}

impl DisconnectReason {
    pub const fn from_wire(value: u8) -> Self {
        match value {
            0 => Self::Unspecified,
            1 => Self::Timeout,
            2 => Self::LocalHost,
            3 => Self::RemoteHost,
            4 => Self::AuthenticationFailure,
            5 => Self::LocalHostSuspend,
            other => Self::Other(other),
        }
    }
}

/// The Flags of Device Found: bit 0 confirm name, 1 legacy pairing, 2 not connectable, 4 name
/// request failed, 5 scan response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FoundFlags(pub u32);

impl FoundFlags {
    pub const fn confirm_name(self) -> bool {
        self.0 & (1 << 0) != 0
    }

    pub const fn legacy_pairing(self) -> bool {
        self.0 & (1 << 1) != 0
    }

    pub const fn not_connectable(self) -> bool {
        self.0 & (1 << 2) != 0
    }

    pub const fn name_request_failed(self) -> bool {
        self.0 & (1 << 4) != 0
    }

    pub const fn scan_response(self) -> bool {
        self.0 & (1 << 5) != 0
    }
}

/// A management event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `data` is the command's return parameters; the `reply` module decodes them.
    CommandComplete {
        opcode: Opcode,
        status: Status,
        data: Vec<u8>,
    },
    CommandStatus {
        opcode: Opcode,
        status: Status,
    },
    ControllerError {
        code: u8,
    },
    IndexAdded,
    IndexRemoved,
    NewSettings(Settings),
    ClassOfDeviceChanged(ClassOfDevice),
    LocalNameChanged {
        name: String,
        short_name: String,
    },
    NewLinkKey {
        store: bool,
        key: LinkKey,
    },
    NewLongTermKey {
        store: bool,
        key: LongTermKey,
    },
    DeviceConnected {
        device: DeviceId,
        flags: u32,
        eir: Eir,
    },
    DeviceDisconnected {
        device: DeviceId,
        reason: DisconnectReason,
    },
    ConnectFailed {
        device: DeviceId,
        status: Status,
    },
    PinCodeRequest {
        device: DeviceId,
        secure: bool,
    },
    /// `just_works` is the document's Confirm_Hint: ask a plain yes or no instead of showing
    /// `value` for comparison.
    UserConfirmationRequest {
        device: DeviceId,
        just_works: bool,
        value: u32,
    },
    UserPasskeyRequest {
        device: DeviceId,
    },
    AuthenticationFailed {
        device: DeviceId,
        status: Status,
    },
    /// `rssi` is `None` when the controller reports 127, "not available".
    DeviceFound {
        device: DeviceId,
        rssi: Option<i8>,
        flags: FoundFlags,
        eir: Eir,
    },
    Discovering {
        types: AddressTypes,
        active: bool,
    },
    DeviceUnpaired(DeviceId),
    PasskeyNotify {
        device: DeviceId,
        passkey: u32,
        entered: u8,
    },
    /// An event this crate does not decode (the kernel adds events over time); the daemon ignores
    /// it.
    Other {
        code: u16,
        parameters: Vec<u8>,
    },
}

/// An event and the controller it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub index: ControllerIndex,
    pub event: Event,
}

/// Decodes one packet read from the socket. The header's parameter length must match the bytes
/// received, and the parameters must have exactly the layout of the event.
pub fn decode_packet(bytes: &[u8]) -> Result<Packet, ParseError> {
    let mut header = Reader::new(bytes, "packet header");
    let code = header.u16()?;
    let index = ControllerIndex(header.u16()?);
    let length = usize::from(header.u16()?);
    let parameters = header.rest();
    if parameters.len() != length {
        return Err(ParseError::LengthMismatch {
            header: length,
            actual: parameters.len(),
        });
    }
    debug_assert_eq!(bytes.len(), HEADER_SIZE + length);
    Ok(Packet {
        index,
        event: decode_event(code, parameters)?,
    })
}

fn decode_event(code: u16, parameters: &[u8]) -> Result<Event, ParseError> {
    let mut r = Reader::new(parameters, event_name(code));
    let event = match code {
        0x0001 => {
            let opcode = Opcode(r.u16()?);
            let status = Status::from_wire(r.u8()?);
            Event::CommandComplete {
                opcode,
                status,
                data: r.rest().to_vec(),
            }
        }
        0x0002 => Event::CommandStatus {
            opcode: Opcode(r.u16()?),
            status: Status::from_wire(r.u8()?),
        },
        0x0003 => Event::ControllerError { code: r.u8()? },
        0x0004 => Event::IndexAdded,
        0x0005 => Event::IndexRemoved,
        0x0006 => Event::NewSettings(Settings(r.u32()?)),
        0x0007 => Event::ClassOfDeviceChanged(ClassOfDevice::from_wire(r.array()?)),
        0x0008 => Event::LocalNameChanged {
            name: fixed_string(&r.array::<249>()?),
            short_name: fixed_string(&r.array::<11>()?),
        },
        0x0009 => Event::NewLinkKey {
            store: r.bool("store hint")?,
            key: LinkKey::read(&mut r)?,
        },
        0x000A => Event::NewLongTermKey {
            store: r.bool("store hint")?,
            key: LongTermKey::read(&mut r)?,
        },
        0x000B => {
            let device = r.device()?;
            let flags = r.u32()?;
            let length = usize::from(r.u16()?);
            Event::DeviceConnected {
                device,
                flags,
                eir: Eir::parse(r.take(length)?)?,
            }
        }
        0x000C => Event::DeviceDisconnected {
            device: r.device()?,
            reason: DisconnectReason::from_wire(r.u8()?),
        },
        0x000D => Event::ConnectFailed {
            device: r.device()?,
            status: Status::from_wire(r.u8()?),
        },
        0x000E => Event::PinCodeRequest {
            device: r.device()?,
            secure: r.bool("secure")?,
        },
        0x000F => Event::UserConfirmationRequest {
            device: r.device()?,
            just_works: r.bool("confirm hint")?,
            value: r.u32()?,
        },
        0x0010 => Event::UserPasskeyRequest {
            device: r.device()?,
        },
        0x0011 => Event::AuthenticationFailed {
            device: r.device()?,
            status: Status::from_wire(r.u8()?),
        },
        0x0012 => {
            let device = r.device()?;
            let rssi = match r.u8()? as i8 {
                127 => None,
                level => Some(level),
            };
            let flags = FoundFlags(r.u32()?);
            let length = usize::from(r.u16()?);
            Event::DeviceFound {
                device,
                rssi,
                flags,
                eir: Eir::parse(r.take(length)?)?,
            }
        }
        0x0013 => Event::Discovering {
            types: AddressTypes::from_wire(r.u8()?),
            active: r.bool("discovering")?,
        },
        0x0016 => Event::DeviceUnpaired(r.device()?),
        0x0017 => Event::PasskeyNotify {
            device: r.device()?,
            passkey: r.u32()?,
            entered: r.u8()?,
        },
        other => {
            return Ok(Event::Other {
                code: other,
                parameters: parameters.to_vec(),
            })
        }
    };
    r.finish()?;
    Ok(event)
}

/// The document's name of an event code, for error messages.
fn event_name(code: u16) -> &'static str {
    match code {
        0x0001 => "Command Complete",
        0x0002 => "Command Status",
        0x0003 => "Controller Error",
        0x0004 => "Index Added",
        0x0005 => "Index Removed",
        0x0006 => "New Settings",
        0x0007 => "Class Of Device Changed",
        0x0008 => "Local Name Changed",
        0x0009 => "New Link Key",
        0x000A => "New Long Term Key",
        0x000B => "Device Connected",
        0x000C => "Device Disconnected",
        0x000D => "Connect Failed",
        0x000E => "PIN Code Request",
        0x000F => "User Confirmation Request",
        0x0010 => "User Passkey Request",
        0x0011 => "Authentication Failed",
        0x0012 => "Device Found",
        0x0013 => "Discovering",
        0x0016 => "Device Unpaired",
        0x0017 => "Passkey Notify",
        _ => "event",
    }
}
