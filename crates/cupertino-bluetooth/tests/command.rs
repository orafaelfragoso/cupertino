//! The bytes of every management command.

use cupertino_bluetooth::{
    Address, AddressType, AddressTypes, Command, ControllerIndex, DeviceId, Discoverable, Error,
    IoCapability, KeyValue, LinkKey, LinkKeyType, LongTermKey, LongTermKeyType, Opcode,
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

#[test]
fn a_command_without_parameters_is_a_header() {
    assert_eq!(encode(&Command::ReadVersion), header(0x0001, 2, 0));
    assert_eq!(
        Command::ReadIndexList
            .encode(ControllerIndex::NONE)
            .unwrap(),
        header(0x0003, 0xFFFF, 0)
    );
    assert_eq!(encode(&Command::ReadControllerInfo), header(0x0004, 2, 0));
    assert_eq!(encode(&Command::GetConnections), header(0x0015, 2, 0));
}

#[test]
fn switches_carry_one_byte() {
    assert_eq!(
        encode(&Command::SetPowered(true)),
        [header(0x0005, 2, 1), vec![1]].concat()
    );
    assert_eq!(
        encode(&Command::SetConnectable(false)),
        [header(0x0007, 2, 1), vec![0]].concat()
    );
    assert_eq!(
        encode(&Command::SetBondable(true)),
        [header(0x0009, 2, 1), vec![1]].concat()
    );
}

#[test]
fn discoverable_carries_the_mode_and_a_little_endian_timeout() {
    let parameters = |command| encode(&command)[6..].to_vec();
    assert_eq!(
        parameters(Command::SetDiscoverable(Discoverable::Off)),
        [0, 0, 0]
    );
    assert_eq!(
        parameters(Command::SetDiscoverable(Discoverable::General {
            timeout: 0x0102
        })),
        [1, 0x02, 0x01]
    );
    assert_eq!(
        parameters(Command::SetDiscoverable(Discoverable::Limited {
            timeout: 30
        })),
        [2, 30, 0]
    );
    assert_eq!(
        encode(&Command::SetDiscoverable(Discoverable::Off)),
        [header(0x0006, 2, 3), vec![0, 0, 0]].concat()
    );
}

#[test]
fn io_capabilities_use_the_documents_numbers() {
    for (capability, wire) in [
        (IoCapability::DisplayOnly, 0),
        (IoCapability::DisplayYesNo, 1),
        (IoCapability::KeyboardOnly, 2),
        (IoCapability::NoInputNoOutput, 3),
        (IoCapability::KeyboardDisplay, 4),
    ] {
        assert_eq!(capability.to_wire(), wire);
        assert_eq!(
            encode(&Command::SetIoCapability(capability)),
            [header(0x0018, 2, 1), vec![wire]].concat()
        );
    }
}

#[test]
fn discovery_carries_the_address_type_mask() {
    assert_eq!(
        encode(&Command::StartDiscovery(AddressTypes::ALL)),
        [header(0x0023, 2, 1), vec![7]].concat()
    );
    assert_eq!(
        encode(&Command::StopDiscovery(AddressTypes::LE)),
        [header(0x0024, 2, 1), vec![6]].concat()
    );
}

#[test]
fn commands_about_a_device_start_with_its_address_and_type() {
    let with = |extra: &[u8]| [DEVICE_BYTES.as_slice(), extra].concat();
    let packet = |opcode, extra: &[u8]| {
        let body = with(extra);
        [header(opcode, 2, body.len() as u16), body].concat()
    };
    assert_eq!(
        encode(&Command::ConfirmName {
            device: device(),
            name_known: true
        }),
        packet(0x0025, &[1])
    );
    assert_eq!(
        encode(&Command::PairDevice {
            device: device(),
            io_capability: IoCapability::KeyboardOnly
        }),
        packet(0x0019, &[2])
    );
    assert_eq!(
        encode(&Command::UnpairDevice {
            device: device(),
            disconnect: true
        }),
        packet(0x001B, &[1])
    );
    assert_eq!(
        encode(&Command::CancelPairDevice(device())),
        packet(0x001A, &[])
    );
    assert_eq!(encode(&Command::Disconnect(device())), packet(0x0014, &[]));
    assert_eq!(
        encode(&Command::PinCodeNegativeReply(device())),
        packet(0x0017, &[])
    );
}

#[test]
fn a_confirmation_answer_picks_the_positive_or_negative_opcode() {
    let yes = Command::UserConfirmationReply {
        device: device(),
        accept: true,
    };
    let no = Command::UserConfirmationReply {
        device: device(),
        accept: false,
    };
    assert_eq!(yes.opcode(), Opcode::USER_CONFIRMATION_REPLY);
    assert_eq!(no.opcode(), Opcode::USER_CONFIRMATION_NEGATIVE_REPLY);
    assert_eq!(
        encode(&yes),
        [header(0x001C, 2, 7), DEVICE_BYTES.to_vec()].concat()
    );
    assert_eq!(
        encode(&no),
        [header(0x001D, 2, 7), DEVICE_BYTES.to_vec()].concat()
    );
}

#[test]
fn a_passkey_answer_carries_the_passkey_and_a_refusal_does_not() {
    let reply = Command::UserPasskeyReply {
        device: device(),
        passkey: Some(123_456),
    };
    assert_eq!(reply.opcode(), Opcode::USER_PASSKEY_REPLY);
    assert_eq!(
        encode(&reply),
        [
            header(0x001E, 2, 11),
            DEVICE_BYTES.to_vec(),
            123_456u32.to_le_bytes().to_vec()
        ]
        .concat()
    );
    let refusal = Command::UserPasskeyReply {
        device: device(),
        passkey: None,
    };
    assert_eq!(refusal.opcode(), Opcode::USER_PASSKEY_NEGATIVE_REPLY);
    assert_eq!(
        encode(&refusal),
        [header(0x001F, 2, 7), DEVICE_BYTES.to_vec()].concat()
    );
}

#[test]
fn only_commands_that_name_a_device_report_one() {
    assert_eq!(Command::Disconnect(device()).device(), Some(device()));
    assert_eq!(
        Command::UserPasskeyReply {
            device: device(),
            passkey: None
        }
        .device(),
        Some(device())
    );
    assert_eq!(Command::ReadVersion.device(), None);
    assert_eq!(Command::SetPowered(true).device(), None);
    assert_eq!(Command::LoadLinkKeys(Vec::new()).device(), None);
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
fn link_keys_load_with_debug_keys_refused() {
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
}

#[test]
fn no_link_keys_load_as_a_count_of_zero() {
    assert_eq!(
        encode(&Command::LoadLinkKeys(Vec::new())),
        [header(0x0012, 2, 3), vec![0, 0, 0]].concat()
    );
}

#[test]
fn long_term_keys_load_with_their_count() {
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
fn a_command_that_does_not_fit_the_length_field_is_refused() {
    // 2622 link keys are 65 550 parameter bytes
    let keys = vec![link_key(1); 2622];
    let error = Command::LoadLinkKeys(keys)
        .encode(ControllerIndex(0))
        .unwrap_err();
    assert!(matches!(
        error,
        Error::CommandTooLong {
            opcode: 0x0012,
            length: 65_553
        }
    ));
    assert!(error.to_string().contains("0x0012"));
}

#[test]
fn the_most_link_keys_that_fit_are_encoded() {
    // 2621 keys: 3 + 2621 * 25 = 65 528 parameter bytes
    let packet = Command::LoadLinkKeys(vec![link_key(1); 2621])
        .encode(ControllerIndex(0))
        .unwrap();
    assert_eq!(packet.len(), 6 + 65_528);
    assert_eq!(packet[4..6], 65_528u16.to_le_bytes());
}

#[test]
fn key_values_never_reach_debug_output() {
    let text = format!("{:?}", link_key(0xAB));
    assert!(text.contains("redacted"));
    assert!(!text.contains("171"));
    assert!(!text.to_lowercase().contains("ab, ab"));
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
