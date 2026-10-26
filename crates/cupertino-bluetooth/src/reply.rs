//! The return parameters of Command Complete, per command (`doc/mgmt-protocol.rst`, "Commands").

use crate::address::{Address, AddressTypes, DeviceId};
use crate::command::ControllerIndex;
use crate::error::ParseError;
use crate::reader::{fixed_string, Reader};
use crate::status::{ClassOfDevice, Settings};

/// Read Management Version Information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub version: u8,
    pub revision: u16,
}

impl Version {
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut r = Reader::new(data, "Read Management Version Information reply");
        let version = Self { version: r.u8()?, revision: r.u16()? };
        r.finish()?;
        Ok(version)
    }
}

/// Read Controller Index List.
pub fn parse_index_list(data: &[u8]) -> Result<Vec<ControllerIndex>, ParseError> {
    let mut r = Reader::new(data, "Read Controller Index List reply");
    let count = r.u16()?;
    let indexes = (0..count).map(|_| r.u16().map(ControllerIndex)).collect::<Result<Vec<_>, _>>()?;
    r.finish()?;
    Ok(indexes)
}

/// Read Controller Information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerInfo {
    pub address: Address,
    pub bluetooth_version: u8,
    pub manufacturer: u16,
    pub supported_settings: Settings,
    pub current_settings: Settings,
    pub class: ClassOfDevice,
    pub name: String,
    pub short_name: String,
}

impl ControllerInfo {
    pub fn parse(data: &[u8]) -> Result<Self, ParseError> {
        let mut r = Reader::new(data, "Read Controller Information reply");
        let info = Self {
            address: Address::from_wire(r.array()?),
            bluetooth_version: r.u8()?,
            manufacturer: r.u16()?,
            supported_settings: Settings(r.u32()?),
            current_settings: Settings(r.u32()?),
            class: ClassOfDevice::from_wire(r.array()?),
            name: fixed_string(&r.array::<249>()?),
            short_name: fixed_string(&r.array::<11>()?),
        };
        r.finish()?;
        Ok(info)
    }
}

/// The reply of the Set commands: the controller's settings afterwards.
pub fn parse_settings(data: &[u8]) -> Result<Settings, ParseError> {
    let mut r = Reader::new(data, "Current_Settings reply");
    let settings = Settings(r.u32()?);
    r.finish()?;
    Ok(settings)
}

/// The reply of the commands that act on one device: the address and its type.
pub fn parse_device(data: &[u8]) -> Result<DeviceId, ParseError> {
    let mut r = Reader::new(data, "device reply");
    let device = r.device()?;
    r.finish()?;
    Ok(device)
}

/// The reply of Start Discovery and Stop Discovery: the address types.
pub fn parse_address_types(data: &[u8]) -> Result<AddressTypes, ParseError> {
    let mut r = Reader::new(data, "discovery reply");
    let types = AddressTypes::from_wire(r.u8()?);
    r.finish()?;
    Ok(types)
}

/// Get Connections.
pub fn parse_connections(data: &[u8]) -> Result<Vec<DeviceId>, ParseError> {
    let mut r = Reader::new(data, "Get Connections reply");
    let count = r.u16()?;
    let devices = (0..count).map(|_| r.device()).collect::<Result<Vec<_>, _>>()?;
    r.finish()?;
    Ok(devices)
}
