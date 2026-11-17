//! The return parameters of Command Complete, and the status, settings and class types.

use cupertino_bluetooth::reply::{
    parse_address_types, parse_connections, parse_device, parse_index_list, parse_settings,
    ControllerInfo, Version,
};
use cupertino_bluetooth::{
    Address, AddressType, AddressTypes, ClassOfDevice, ControllerIndex, DeviceId, Error,
    MajorClass, ParseError, Settings, Status,
};

const DEVICE_BYTES: [u8; 7] = [0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00, 1];

fn device() -> DeviceId {
    DeviceId {
        address: "00:1A:7D:DA:71:13".parse::<Address>().unwrap(),
        kind: AddressType::LePublic,
    }
}

#[test]
fn a_version_is_one_byte_and_a_little_endian_revision() {
    assert_eq!(
        Version::parse(&[1, 0x15, 0x01]).unwrap(),
        Version {
            version: 1,
            revision: 0x0115
        }
    );
    assert!(matches!(
        Version::parse(&[1, 2]),
        Err(ParseError::Truncated { .. })
    ));
    assert!(matches!(
        Version::parse(&[1, 2, 3, 4]),
        Err(ParseError::TrailingBytes { extra: 1, .. })
    ));
}

#[test]
fn an_index_list_has_a_count_and_that_many_indexes() {
    assert_eq!(
        parse_index_list(&[2, 0, 0, 0, 5, 0]).unwrap(),
        vec![ControllerIndex(0), ControllerIndex(5)]
    );
    assert_eq!(parse_index_list(&[0, 0]).unwrap(), Vec::new());
}

#[test]
fn an_index_list_with_the_wrong_number_of_entries_is_refused() {
    assert!(matches!(
        parse_index_list(&[2, 0, 0, 0]),
        Err(ParseError::Truncated { .. })
    ));
    assert!(matches!(
        parse_index_list(&[1, 0, 0, 0, 9, 9]),
        Err(ParseError::TrailingBytes { extra: 2, .. })
    ));
    assert!(matches!(
        parse_index_list(&[]),
        Err(ParseError::Truncated { .. })
    ));
}

fn controller_info() -> Vec<u8> {
    let mut data = vec![0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00, 9];
    data.extend(0x000Fu16.to_le_bytes());
    data.extend(0x0000_0FFFu32.to_le_bytes());
    data.extend(0x0000_0083u32.to_le_bytes());
    data.extend([0x0C, 0x01, 0x1A]);
    let mut name = [0u8; 249];
    name[..9].copy_from_slice(b"my laptop");
    data.extend(name);
    let mut short = [0u8; 11];
    short[..2].copy_from_slice(b"ml");
    data.extend(short);
    data
}

#[test]
fn controller_information_decodes_every_field() {
    let info = ControllerInfo::parse(&controller_info()).unwrap();
    assert_eq!(info.address.to_string(), "00:1A:7D:DA:71:13");
    assert_eq!(info.bluetooth_version, 9);
    assert_eq!(info.manufacturer, 0x000F);
    assert_eq!(info.supported_settings, Settings(0xFFF));
    assert!(info.current_settings.powered());
    assert!(info.current_settings.contains(Settings::BR_EDR));
    assert!(!info.current_settings.discoverable());
    assert_eq!(info.class.major(), MajorClass::Computer);
    assert_eq!(info.name, "my laptop");
    assert_eq!(info.short_name, "ml");
}

#[test]
fn controller_information_of_the_wrong_size_is_refused() {
    let data = controller_info();
    assert!(matches!(
        ControllerInfo::parse(&data[..data.len() - 1]),
        Err(ParseError::Truncated { .. })
    ));
    let mut long = data;
    long.push(0);
    assert!(matches!(
        ControllerInfo::parse(&long),
        Err(ParseError::TrailingBytes { extra: 1, .. })
    ));
}

#[test]
fn small_replies_decode_and_refuse_extra_bytes() {
    assert_eq!(
        parse_settings(&0x0Bu32.to_le_bytes()).unwrap(),
        Settings(0x0B)
    );
    assert!(parse_settings(&[1, 0, 0]).is_err());
    assert!(parse_settings(&[1, 0, 0, 0, 0]).is_err());
    assert_eq!(parse_device(&DEVICE_BYTES).unwrap(), device());
    assert!(parse_device(&DEVICE_BYTES[..6]).is_err());
    assert_eq!(parse_address_types(&[7]).unwrap(), AddressTypes::ALL);
    assert!(parse_address_types(&[]).is_err());
    assert!(parse_address_types(&[7, 7]).is_err());
}

#[test]
fn a_connection_list_has_a_count_and_that_many_devices() {
    let mut data = vec![2, 0];
    data.extend(DEVICE_BYTES);
    data.extend([1, 2, 3, 4, 5, 6, 0]);
    let devices = parse_connections(&data).unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0], device());
    assert_eq!(devices[1].kind, AddressType::BrEdr);
    assert_eq!(parse_connections(&[0, 0]).unwrap(), Vec::new());
    assert!(parse_connections(&data[..data.len() - 1]).is_err());
}

#[test]
fn a_status_decodes_to_its_name_and_only_success_succeeds() {
    assert_eq!(Status::from_wire(0), Status::Success);
    assert!(Status::from_wire(0).is_success());
    assert_eq!(Status::from_wire(0x0A), Status::Busy);
    assert_eq!(Status::from_wire(0x12), Status::RfKilled);
    assert_eq!(Status::from_wire(0x14), Status::PermissionDenied);
    assert!(!Status::from_wire(0x14).is_success());
    assert_eq!(Status::from_wire(0x55), Status::Other(0x55));
    assert!(!Status::Other(0x55).is_success());
}

#[test]
fn every_status_has_text_and_an_unknown_one_shows_its_number() {
    for wire in 0..=0x14 {
        let text = Status::from_wire(wire).to_string();
        assert!(!text.is_empty());
        assert!(!text.starts_with("status"), "{wire}: {text}");
    }
    assert_eq!(Status::RfKilled.to_string(), "blocked by rfkill");
    assert_eq!(Status::Other(0x55).to_string(), "status 0x55");
}

#[test]
fn settings_bits_are_tested_as_a_set() {
    let settings = Settings(Settings::POWERED | Settings::BONDABLE);
    assert!(settings.powered());
    assert!(settings.bondable());
    assert!(!settings.connectable());
    assert!(!settings.discoverable());
    assert!(settings.contains(Settings::POWERED | Settings::BONDABLE));
    assert!(!settings.contains(Settings::POWERED | Settings::CONNECTABLE));
    assert!(Settings(0).contains(0));
}

#[test]
fn a_class_of_device_splits_into_major_and_minor() {
    let class = ClassOfDevice::from_wire([0x04, 0x05, 0x00]);
    assert_eq!(class.value(), 0x0504);
    assert_eq!(class.major(), MajorClass::Peripheral);
    assert_eq!(class.minor(), 1);
    for (major, expected) in [
        (0, MajorClass::Miscellaneous),
        (1, MajorClass::Computer),
        (2, MajorClass::Phone),
        (3, MajorClass::NetworkAccessPoint),
        (4, MajorClass::AudioVideo),
        (5, MajorClass::Peripheral),
        (6, MajorClass::Imaging),
        (7, MajorClass::Wearable),
        (8, MajorClass::Toy),
        (9, MajorClass::Health),
        (31, MajorClass::Uncategorized),
        (12, MajorClass::Other(12)),
    ] {
        assert_eq!(ClassOfDevice::from_wire([0, major, 0]).major(), expected);
    }
}

#[test]
fn errors_say_what_went_wrong_and_chain_to_their_cause() {
    let parse = Error::from(ParseError::UnknownAddressType(7));
    assert_eq!(
        parse.to_string(),
        "malformed management packet: unknown address type 7"
    );
    assert!(std::error::Error::source(&parse).is_some());
    let syscall = Error::Syscall {
        call: "send",
        source: rustix::io::Errno::PERM,
    };
    assert!(syscall.to_string().starts_with("send: "));
    assert!(std::error::Error::source(&syscall).is_some());
}
