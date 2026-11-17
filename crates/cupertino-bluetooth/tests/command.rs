//! The bytes of every management command.

use cupertino_bluetooth::{
    Address, AddressType, AddressTypes, Command, ControllerIndex, DeviceId, Discoverable, Error,
    IoCapability, KeyValue, LinkKey, LinkKeyType, LongTermKey, LongTermKeyType,
};

fn device() -> DeviceId {
    DeviceId {
        address: "00:1A:7D:DA:71:13".parse::<Address>().unwrap(),
        kind: AddressType::LePublic,
    }
}

/// The wire form of `device()`: address least significant byte first, then the type.
const DEVICE_BYTES: [u8; 7] = [0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00, 1];

fn encode(command: &Command) -> Vec<u8> {
    command.encode(ControllerIndex(2)).unwrap()
}

fn header(opcode: u16, index: u16, length: u16) -> Vec<u8> {
    [
        opcode.to_le_bytes(),
        index.to_le_bytes(),
        length.to_le_bytes(),
    ]
    .concat()
}

fn link_key(first: u8) -> LinkKey {
    LinkKey {
        device: device(),
        key_type: LinkKeyType::AuthenticatedP256,
        value: KeyValue([first; 16]),
        pin_length: 4,
    }
}

fn long_term_key(first: u8) -> LongTermKey {
    LongTermKey {
        device: device(),
        key_type: LongTermKeyType::AuthenticatedP256,
        central: true,
        encryption_size: 16,
        diversifier: 0x0A0B,
        random: [1, 2, 3, 4, 5, 6, 7, 8],
        value: KeyValue([first; 16]),
    }
}

#[test]
fn a_command_is_its_opcode_controller_and_length_then_its_parameters() {
    let with_device = |extra: &[u8]| [DEVICE_BYTES.as_slice(), extra].concat();
    let mut rows = vec![
        ("read version", Command::ReadVersion, 0x0001, vec![]),
        ("read info", Command::ReadControllerInfo, 0x0004, vec![]),
        ("connections", Command::GetConnections, 0x0015, vec![]),
        ("powered", Command::SetPowered(true), 0x0005, vec![1]),
        (
            "connectable",
            Command::SetConnectable(false),
            0x0007,
            vec![0],
        ),
        ("bondable", Command::SetBondable(true), 0x0009, vec![1]),
        (
            "not discoverable",
            Command::SetDiscoverable(Discoverable::Off),
            0x0006,
            vec![0, 0, 0],
        ),
        (
            "discoverable with a little-endian timeout",
            Command::SetDiscoverable(Discoverable::General { timeout: 0x0102 }),
            0x0006,
            vec![1, 0x02, 0x01],
        ),
        (
            "limited discoverable",
            Command::SetDiscoverable(Discoverable::Limited { timeout: 30 }),
            0x0006,
            vec![2, 30, 0],
        ),
        (
            "start discovery",
            Command::StartDiscovery(AddressTypes::ALL),
            0x0023,
            vec![7],
        ),
        (
            "stop discovery",
            Command::StopDiscovery(AddressTypes::LE),
            0x0024,
            vec![6],
        ),
        (
            "confirm name",
            Command::ConfirmName {
                device: device(),
                name_known: true,
            },
            0x0025,
            with_device(&[1]),
        ),
        (
            "pair",
            Command::PairDevice {
                device: device(),
                io_capability: IoCapability::KeyboardOnly,
            },
            0x0019,
            with_device(&[2]),
        ),
        (
            "unpair",
            Command::UnpairDevice {
                device: device(),
                disconnect: true,
            },
            0x001B,
            with_device(&[1]),
        ),
        (
            "cancel pairing",
            Command::CancelPairDevice(device()),
            0x001A,
            with_device(&[]),
        ),
        (
            "disconnect",
            Command::Disconnect(device()),
            0x0014,
            with_device(&[]),
        ),
        (
            "refuse a PIN",
            Command::PinCodeNegativeReply(device()),
            0x0017,
            with_device(&[]),
        ),
        (
            "confirm",
            Command::UserConfirmationReply {
                device: device(),
                accept: true,
            },
            0x001C,
            with_device(&[]),
        ),
        (
            "do not confirm",
            Command::UserConfirmationReply {
                device: device(),
                accept: false,
            },
            0x001D,
            with_device(&[]),
        ),
        (
            "a passkey",
            Command::UserPasskeyReply {
                device: device(),
                passkey: Some(123_456),
            },
            0x001E,
            with_device(&123_456u32.to_le_bytes()),
        ),
        (
            "no passkey",
            Command::UserPasskeyReply {
                device: device(),
                passkey: None,
            },
            0x001F,
            with_device(&[]),
        ),
        (
            "no link keys",
            Command::LoadLinkKeys(Vec::new()),
            0x0012,
            vec![0, 0, 0],
        ),
    ];
    for (capability, wire) in [
        (IoCapability::DisplayOnly, 0),
        (IoCapability::DisplayYesNo, 1),
        (IoCapability::KeyboardOnly, 2),
        (IoCapability::NoInputNoOutput, 3),
        (IoCapability::KeyboardDisplay, 4),
    ] {
        assert_eq!(capability.to_wire(), wire, "{capability:?}");
        rows.push((
            "io capability",
            Command::SetIoCapability(capability),
            0x0018,
            vec![wire],
        ));
    }
    for (name, command, opcode, parameters) in rows {
        assert_eq!(command.opcode().0, opcode, "{name}");
        assert_eq!(
            encode(&command),
            [header(opcode, 2, parameters.len() as u16), parameters].concat(),
            "{name}"
        );
    }
    assert_eq!(
        Command::ReadIndexList
            .encode(ControllerIndex::NONE)
            .unwrap(),
        header(0x0003, 0xFFFF, 0)
    );
}

#[test]
fn only_commands_that_name_a_device_report_one() {
    for (command, expected) in [
        (Command::Disconnect(device()), Some(device())),
        (
            Command::UserPasskeyReply {
                device: device(),
                passkey: None,
            },
            Some(device()),
        ),
        (Command::ReadVersion, None),
        (Command::SetPowered(true), None),
        (Command::LoadLinkKeys(Vec::new()), None),
    ] {
        assert_eq!(command.device(), expected, "{command:?}");
    }
}

#[test]
fn keys_load_with_their_count_and_link_keys_with_debug_keys_refused() {
    let packet = encode(&Command::LoadLinkKeys(vec![link_key(0xAA), link_key(0xBB)]));
    assert_eq!(packet[..6], header(0x0012, 2, 1 + 2 + 2 * 25)[..]);
    let body = &packet[6..];
    assert_eq!(body[0], 0, "Debug_Keys");
    assert_eq!(body[1..3], [2, 0]);
    let first = &body[3..28];
    assert_eq!(first[..7], DEVICE_BYTES);
    assert_eq!(first[7], 0x08);
    assert_eq!(first[8..24], [0xAA; 16]);
    assert_eq!(first[24], 4);
    assert_eq!(body[28 + 8..28 + 24], [0xBB; 16]);

    let packet = encode(&Command::LoadLongTermKeys(vec![long_term_key(0xCC)]));
    assert_eq!(packet[..6], header(0x0013, 2, 2 + 36)[..]);
    let body = &packet[6..];
    assert_eq!(body[..2], [1, 0]);
    let key = &body[2..];
    assert_eq!(key[..7], DEVICE_BYTES);
    assert_eq!(key[7], 0x03);
    assert_eq!(key[8], 1);
    assert_eq!(key[9], 16);
    assert_eq!(key[10..12], [0x0B, 0x0A]);
    assert_eq!(key[12..20], [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(key[20..36], [0xCC; 16]);
}

#[test]
fn the_most_link_keys_that_fit_the_length_field_are_encoded_and_one_more_is_refused() {
    // 2621 keys: 3 + 2621 * 25 = 65 528 parameter bytes
    let packet = Command::LoadLinkKeys(vec![link_key(1); 2621])
        .encode(ControllerIndex(0))
        .unwrap();
    assert_eq!(packet.len(), 6 + 65_528);
    assert_eq!(packet[4..6], 65_528u16.to_le_bytes());

    // 2622 link keys are 65 553 parameter bytes
    let error = Command::LoadLinkKeys(vec![link_key(1); 2622])
        .encode(ControllerIndex(0))
        .unwrap_err();
    assert!(
        matches!(
            error,
            Error::CommandTooLong {
                opcode: 0x0012,
                length: 65_553
            }
        ),
        "{error:?}"
    );
}

#[test]
fn key_values_never_reach_debug_output() {
    for text in [
        format!("{:?}", link_key(0xAB)),
        format!("{:?}", long_term_key(0xAB)),
        format!("{:?}", Command::LoadLinkKeys(vec![link_key(0xAB)])),
    ] {
        assert!(text.contains("redacted"), "{text}");
        assert!(!text.contains("171"), "{text}");
        assert!(!text.to_lowercase().contains("ab, ab"), "{text}");
    }
}

#[test]
fn key_types_keep_values_the_document_does_not_define() {
    for wire in 0..=0x08 {
        assert_eq!(LinkKeyType::from_wire(wire).to_wire(), wire);
    }
    assert_eq!(LinkKeyType::from_wire(0x20), LinkKeyType::Other(0x20));
    assert_eq!(LinkKeyType::Other(0x20).to_wire(), 0x20);
    for wire in 0..=0x04 {
        assert_eq!(LongTermKeyType::from_wire(wire).to_wire(), wire);
    }
    assert_eq!(LongTermKeyType::from_wire(9), LongTermKeyType::Other(9));
}
