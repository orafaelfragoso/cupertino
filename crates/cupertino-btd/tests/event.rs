//! Decoding of management event packets.

use cupertino_btd::bluetooth::{
    decode_packet, Address, AddressType, AddressTypes, ClassOfDevice, ControllerIndex, DeviceId,
    DisconnectReason, Eir, EirName, Event, FoundFlags, KeyValue, LinkKey, LinkKeyType, LongTermKey,
    LongTermKeyType, Opcode, ParseError, Settings, Status,
};

const DEVICE_BYTES: [u8; 7] = [0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00, 0];

fn device() -> DeviceId {
    DeviceId {
        address: "00:1A:7D:DA:71:13".parse::<Address>().unwrap(),
        kind: AddressType::BrEdr,
    }
}

/// A packet of event `code` from controller `index` whose header length is right.
fn packet(code: u16, index: u16, parameters: &[u8]) -> Vec<u8> {
    [
        code.to_le_bytes().as_slice(),
        index.to_le_bytes().as_slice(),
        (parameters.len() as u16).to_le_bytes().as_slice(),
        parameters,
    ]
    .concat()
}

fn decode(code: u16, parameters: &[u8]) -> Result<Event, ParseError> {
    decode_packet(&packet(code, 0, parameters)).map(|decoded| decoded.event)
}

fn with_device(extra: &[&[u8]]) -> Vec<u8> {
    [&[DEVICE_BYTES.as_slice()], extra].concat().concat()
}

fn device_found(rssi: i8, flags: u32, eir: &[u8]) -> Vec<u8> {
    with_device(&[
        &[rssi as u8],
        &flags.to_le_bytes(),
        &(eir.len() as u16).to_le_bytes(),
        eir,
    ])
}

#[test]
fn the_header_gives_the_controller_index_and_must_agree_with_the_bytes() {
    let decoded = decode_packet(&packet(0x0004, 7, &[])).unwrap();
    assert_eq!(
        (decoded.index, decoded.event),
        (ControllerIndex(7), Event::IndexAdded)
    );
    assert_eq!(
        decode_packet(&packet(0x0005, 0xFFFF, &[])).unwrap().index,
        ControllerIndex::NONE
    );

    for bytes in [&[0x01, 0x00, 0x00][..], &[]] {
        assert!(
            matches!(
                decode_packet(bytes),
                Err(ParseError::Truncated {
                    what: "packet header",
                    ..
                })
            ),
            "{bytes:?}"
        );
    }
    let mut bytes = packet(0x0003, 0, &[9]);
    for header in [2, 0] {
        bytes[4] = header as u8;
        assert_eq!(
            decode_packet(&bytes),
            Err(ParseError::LengthMismatch { header, actual: 1 }),
            "{header}"
        );
    }
}

#[test]
fn each_event_decodes_to_its_fields() {
    let mut local_name = vec![0u8; 249 + 11];
    local_name[..6].copy_from_slice(b"Laptop");
    local_name[249..252].copy_from_slice(b"Lap");
    let mut long_term_key = vec![0u8];
    long_term_key.extend([0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00, 2]);
    long_term_key.extend([0x03, 1, 16, 0x0B, 0x0A]);
    long_term_key.extend([1, 2, 3, 4, 5, 6, 7, 8]);
    long_term_key.extend([0x77; 16]);
    let mut le_random = device();
    le_random.kind = AddressType::LeRandom;
    let appearance = [3, 0x19, 0xC1, 0x03];

    let mut rows = vec![
        (
            "command complete",
            0x0001,
            vec![0x05, 0x00, 0x00, 0xAA, 0xBB],
            Event::CommandComplete {
                opcode: Opcode::SET_POWERED,
                status: Status::Success,
                data: vec![0xAA, 0xBB],
            },
        ),
        (
            "command complete without data",
            0x0001,
            vec![0x23, 0x00, 0x0A],
            Event::CommandComplete {
                opcode: Opcode::START_DISCOVERY,
                status: Status::Busy,
                data: Vec::new(),
            },
        ),
        (
            "command status",
            0x0002,
            vec![0x19, 0x00, 0x0C],
            Event::CommandStatus {
                opcode: Opcode::PAIR_DEVICE,
                status: Status::NotSupported,
            },
        ),
        (
            "controller error",
            0x0003,
            vec![0x42],
            Event::ControllerError { code: 0x42 },
        ),
        ("index removed", 0x0005, vec![], Event::IndexRemoved),
        (
            "new settings",
            0x0006,
            1u32.to_le_bytes().to_vec(),
            Event::NewSettings(Settings(1)),
        ),
        (
            "class changed",
            0x0007,
            vec![0x0C, 0x01, 0x1A],
            Event::ClassOfDeviceChanged(ClassOfDevice::from_wire([0x0C, 0x01, 0x1A])),
        ),
        (
            "local name, each field to its nul",
            0x0008,
            local_name,
            Event::LocalNameChanged {
                name: "Laptop".to_owned(),
                short_name: "Lap".to_owned(),
            },
        ),
        (
            "new link key",
            0x0009,
            [vec![1u8], with_device(&[&[0x07], &[0x5A; 16], &[0]])].concat(),
            Event::NewLinkKey {
                store: true,
                key: LinkKey {
                    device: device(),
                    key_type: LinkKeyType::UnauthenticatedP256,
                    value: KeyValue([0x5A; 16]),
                    pin_length: 0,
                },
            },
        ),
        (
            "new long term key",
            0x000A,
            long_term_key,
            Event::NewLongTermKey {
                store: false,
                key: LongTermKey {
                    device: le_random,
                    key_type: LongTermKeyType::AuthenticatedP256,
                    central: true,
                    encryption_size: 16,
                    diversifier: 0x0A0B,
                    random: [1, 2, 3, 4, 5, 6, 7, 8],
                    value: KeyValue([0x77; 16]),
                },
            },
        ),
        (
            "device connected with its advertising data",
            0x000B,
            with_device(&[&8u32.to_le_bytes(), &4u16.to_le_bytes(), &appearance]),
            Event::DeviceConnected {
                device: device(),
                flags: 8,
                eir: Eir {
                    appearance: Some(0x03C1),
                    ..Eir::default()
                },
            },
        ),
        (
            "connect failed",
            0x000D,
            with_device(&[&[0x04]]),
            Event::ConnectFailed {
                device: device(),
                status: Status::ConnectFailed,
            },
        ),
        (
            "PIN request",
            0x000E,
            with_device(&[&[1]]),
            Event::PinCodeRequest {
                device: device(),
                secure: true,
            },
        ),
        (
            "confirmation request",
            0x000F,
            with_device(&[&[1], &482_913u32.to_le_bytes()]),
            Event::UserConfirmationRequest {
                device: device(),
                just_works: true,
                value: 482_913,
            },
        ),
        (
            "passkey request",
            0x0010,
            DEVICE_BYTES.to_vec(),
            Event::UserPasskeyRequest { device: device() },
        ),
        (
            "authentication failed",
            0x0011,
            with_device(&[&[0x05]]),
            Event::AuthenticationFailed {
                device: device(),
                status: Status::AuthenticationFailed,
            },
        ),
        (
            "device found with a signed signal level",
            0x0012,
            device_found(-60, 0b10_0101, &[5, 0x09, b'T', b'V', b'!', b'?']),
            Event::DeviceFound {
                device: device(),
                rssi: Some(-60),
                flags: FoundFlags(0b10_0101),
                eir: Eir {
                    name: Some(EirName {
                        text: "TV!?".to_owned(),
                        complete: true,
                    }),
                    ..Eir::default()
                },
            },
        ),
        (
            "a signal level of 127 is not available",
            0x0012,
            device_found(127, 0, &[]),
            Event::DeviceFound {
                device: device(),
                rssi: None,
                flags: FoundFlags(0),
                eir: Eir::default(),
            },
        ),
        (
            "discovering",
            0x0013,
            vec![0x07, 1],
            Event::Discovering {
                types: AddressTypes::ALL,
                active: true,
            },
        ),
        (
            "unpaired",
            0x0016,
            DEVICE_BYTES.to_vec(),
            Event::DeviceUnpaired(device()),
        ),
        (
            "passkey notify",
            0x0017,
            with_device(&[&654_321u32.to_le_bytes(), &[3]]),
            Event::PasskeyNotify {
                device: device(),
                passkey: 654_321,
                entered: 3,
            },
        ),
        (
            "an unknown event kept whole",
            0x0030,
            vec![1, 2, 3],
            Event::Other {
                code: 0x0030,
                parameters: vec![1, 2, 3],
            },
        ),
    ];
    for (wire, reason) in [
        (0, DisconnectReason::Unspecified),
        (1, DisconnectReason::Timeout),
        (2, DisconnectReason::LocalHost),
        (3, DisconnectReason::RemoteHost),
        (4, DisconnectReason::AuthenticationFailure),
        (5, DisconnectReason::LocalHostSuspend),
        (6, DisconnectReason::Other(6)),
    ] {
        assert_eq!(DisconnectReason::from_wire(wire), reason);
        rows.push((
            "disconnected",
            0x000C,
            with_device(&[&[wire]]),
            Event::DeviceDisconnected {
                device: device(),
                reason,
            },
        ));
    }
    for (name, code, parameters, expected) in rows {
        assert_eq!(decode(code, &parameters), Ok(expected), "{name}");
    }
}

#[test]
fn found_flags_are_independent_bits() {
    for (bits, expected) in [
        (0b10_0101, [true, false, true, false, true]),
        (1 << 1 | 1 << 4, [false, true, false, true, false]),
    ] {
        let flags = FoundFlags(bits);
        assert_eq!(
            [
                flags.confirm_name(),
                flags.legacy_pairing(),
                flags.not_connectable(),
                flags.name_request_failed(),
                flags.scan_response(),
            ],
            expected,
            "{bits:#b}"
        );
    }
}

#[test]
fn parameters_that_do_not_fit_the_events_layout_fail_the_event_naming_it() {
    let mut bad_store = vec![2u8];
    bad_store.extend(with_device(&[&[0], &[0; 17]]));
    let mut bad_address_type = DEVICE_BYTES.to_vec();
    bad_address_type[6] = 9;
    let mut short_name = vec![0u8; 259];
    short_name[..6].copy_from_slice(b"Laptop");
    for (name, code, parameters, expected) in [
        (
            "command status with a byte more",
            0x0002,
            vec![0x19, 0x00, 0x0C, 0],
            Some(ParseError::TrailingBytes {
                what: "Command Status",
                extra: 1,
            }),
        ),
        (
            "command status without its status",
            0x0002,
            vec![0x19, 0x00],
            Some(ParseError::Truncated {
                what: "Command Status",
                needed: 1,
                found: 0,
            }),
        ),
        (
            "controller error with a byte more",
            0x0003,
            vec![1, 2],
            Some(ParseError::TrailingBytes {
                what: "Controller Error",
                extra: 1,
            }),
        ),
        (
            "index added with a byte",
            0x0004,
            vec![1],
            Some(ParseError::TrailingBytes {
                what: "Index Added",
                extra: 1,
            }),
        ),
        ("a short local name", 0x0008, short_name, None),
        (
            "a store hint that is not a boolean",
            0x0009,
            bad_store,
            Some(ParseError::InvalidBool {
                what: "store hint",
                value: 2,
            }),
        ),
        (
            "advertising data longer than the packet",
            0x000B,
            with_device(&[&0u32.to_le_bytes(), &10u16.to_le_bytes(), &[2, 0x01, 0x06]]),
            None,
        ),
        (
            "bad advertising data",
            0x000B,
            with_device(&[&0u32.to_le_bytes(), &3u16.to_le_bytes(), &[9, 0x09, b'a']]),
            Some(ParseError::EirOverrun { offset: 0 }),
        ),
        (
            "a disconnection without its reason",
            0x000C,
            DEVICE_BYTES.to_vec(),
            Some(ParseError::Truncated {
                what: "Device Disconnected",
                needed: 1,
                found: 0,
            }),
        ),
        (
            "an address type outside the document",
            0x0010,
            bad_address_type,
            Some(ParseError::UnknownAddressType(9)),
        ),
        (
            "discovering that is not a boolean",
            0x0013,
            vec![0x07, 5],
            Some(ParseError::InvalidBool {
                what: "discovering",
                value: 5,
            }),
        ),
    ] {
        let error = decode(code, &parameters).unwrap_err();
        match expected {
            Some(expected) => assert_eq!(error, expected, "{name}"),
            None => assert!(
                matches!(error, ParseError::Truncated { .. }),
                "{name}: {error:?}"
            ),
        }
    }
    assert!(matches!(
        decode(
            0x000B,
            &with_device(&[&0u32.to_le_bytes(), &10u16.to_le_bytes(), &[2, 0x01, 0x06]])
        ),
        Err(ParseError::Truncated {
            needed: 10,
            found: 3,
            ..
        })
    ));
}
