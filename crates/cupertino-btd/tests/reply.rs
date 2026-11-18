//! The return parameters of Command Complete, and the status, settings and class types.

use cupertino_btd::bluetooth::reply::{
    parse_address_types, parse_connections, parse_device, parse_index_list, parse_settings,
    ControllerInfo, Version,
};
use cupertino_btd::bluetooth::{
    Address, AddressType, AddressTypes, ClassOfDevice, ControllerIndex, DeviceId, MajorClass,
    ParseError, Settings, Status,
};

const DEVICE_BYTES: [u8; 7] = [0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00, 1];

fn device() -> DeviceId {
    DeviceId {
        address: "00:1A:7D:DA:71:13".parse::<Address>().unwrap(),
        kind: AddressType::LePublic,
    }
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

fn connections() -> Vec<u8> {
    let mut data = vec![2, 0];
    data.extend(DEVICE_BYTES);
    data.extend([1, 2, 3, 4, 5, 6, 0]);
    data
}

#[test]
fn each_reply_decodes_to_its_value() {
    assert_eq!(
        Version::parse(&[1, 0x15, 0x01]),
        Ok(Version {
            version: 1,
            revision: 0x0115
        })
    );
    assert_eq!(
        parse_index_list(&[2, 0, 0, 0, 5, 0]),
        Ok(vec![ControllerIndex(0), ControllerIndex(5)])
    );
    assert_eq!(parse_index_list(&[0, 0]), Ok(Vec::new()));
    assert_eq!(parse_settings(&0x0Bu32.to_le_bytes()), Ok(Settings(0x0B)));
    assert_eq!(parse_device(&DEVICE_BYTES), Ok(device()));
    assert_eq!(parse_address_types(&[7]), Ok(AddressTypes::ALL));
    let devices = parse_connections(&connections()).unwrap();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0], device());
    assert_eq!(devices[1].kind, AddressType::BrEdr);
    assert_eq!(parse_connections(&[0, 0]), Ok(Vec::new()));

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
fn a_reply_shorter_or_longer_than_its_layout_is_refused() {
    let info = controller_info();
    let mut long_info = info.clone();
    long_info.push(0);
    let connections = connections();
    for (name, result, trailing) in [
        ("short version", Version::parse(&[1, 2]).map(drop), None),
        (
            "long version",
            Version::parse(&[1, 2, 3, 4]).map(drop),
            Some(1),
        ),
        (
            "fewer indexes than counted",
            parse_index_list(&[2, 0, 0, 0]).map(drop),
            None,
        ),
        (
            "more indexes than counted",
            parse_index_list(&[1, 0, 0, 0, 9, 9]).map(drop),
            Some(2),
        ),
        ("no index count", parse_index_list(&[]).map(drop), None),
        (
            "short controller info",
            ControllerInfo::parse(&info[..info.len() - 1]).map(drop),
            None,
        ),
        (
            "long controller info",
            ControllerInfo::parse(&long_info).map(drop),
            Some(1),
        ),
        ("short settings", parse_settings(&[1, 0, 0]).map(drop), None),
        (
            "long settings",
            parse_settings(&[1, 0, 0, 0, 0]).map(drop),
            Some(1),
        ),
        (
            "short device",
            parse_device(&DEVICE_BYTES[..6]).map(drop),
            None,
        ),
        ("no address types", parse_address_types(&[]).map(drop), None),
        (
            "long address types",
            parse_address_types(&[7, 7]).map(drop),
            Some(1),
        ),
        (
            "fewer connections than counted",
            parse_connections(&connections[..connections.len() - 1]).map(drop),
            None,
        ),
    ] {
        let refused = match (trailing, &result) {
            (None, Err(ParseError::Truncated { .. })) => true,
            (Some(n), Err(ParseError::TrailingBytes { extra, .. })) => *extra == n,
            _ => false,
        };
        assert!(refused, "{name}: {result:?}");
    }
}

#[test]
fn a_status_decodes_to_its_name_and_only_success_succeeds() {
    for (wire, status, success) in [
        (0, Status::Success, true),
        (0x0A, Status::Busy, false),
        (0x12, Status::RfKilled, false),
        (0x14, Status::PermissionDenied, false),
        (0x55, Status::Other(0x55), false),
    ] {
        assert_eq!(Status::from_wire(wire), status, "{wire:#x}");
        assert_eq!(status.is_success(), success, "{wire:#x}");
    }
}

#[test]
fn settings_bits_are_tested_as_a_set() {
    let settings = Settings(Settings::POWERED | Settings::BONDABLE);
    assert_eq!(
        [
            settings.powered(),
            settings.bondable(),
            settings.connectable(),
            settings.discoverable()
        ],
        [true, true, false, false]
    );
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
        assert_eq!(
            ClassOfDevice::from_wire([0, major, 0]).major(),
            expected,
            "{major}"
        );
    }
}
