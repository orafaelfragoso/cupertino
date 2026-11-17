//! Decoding of management event packets.

use cupertino_bluetooth::{
    decode_packet, Address, AddressType, AddressTypes, ClassOfDevice, ControllerIndex, DeviceId,
    DisconnectReason, Eir, Event, FoundFlags, KeyValue, LinkKeyType, LongTermKeyType, Opcode,
    ParseError, Settings, Status,
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

fn decode(code: u16, parameters: &[u8]) -> Event {
    decode_packet(&packet(code, 0, parameters)).unwrap().event
}

fn decode_error(code: u16, parameters: &[u8]) -> ParseError {
    decode_packet(&packet(code, 0, parameters)).unwrap_err()
}

#[test]
fn the_controller_index_comes_from_the_header() {
    let decoded = decode_packet(&packet(0x0004, 7, &[])).unwrap();
    assert_eq!(decoded.index, ControllerIndex(7));
    assert_eq!(decoded.event, Event::IndexAdded);
    assert_eq!(
        decode_packet(&packet(0x0005, 0xFFFF, &[])).unwrap().index,
        ControllerIndex::NONE
    );
}

#[test]
fn a_packet_shorter_than_its_header_is_truncated() {
    assert!(matches!(
        decode_packet(&[0x01, 0x00, 0x00]),
        Err(ParseError::Truncated {
            what: "packet header",
            ..
        })
    ));
    assert!(matches!(
        decode_packet(&[]),
        Err(ParseError::Truncated { .. })
    ));
}

#[test]
fn a_header_length_that_disagrees_with_the_bytes_is_refused() {
    let mut bytes = packet(0x0003, 0, &[9]);
    bytes[4] = 2;
    assert_eq!(
        decode_packet(&bytes),
        Err(ParseError::LengthMismatch {
            header: 2,
            actual: 1
        })
    );
    bytes[4] = 0;
    assert_eq!(
        decode_packet(&bytes),
        Err(ParseError::LengthMismatch {
            header: 0,
            actual: 1
        })
    );
}

#[test]
fn command_complete_keeps_the_return_parameters() {
    assert_eq!(
        decode(0x0001, &[0x05, 0x00, 0x00, 0xAA, 0xBB]),
        Event::CommandComplete {
            opcode: Opcode::SET_POWERED,
            status: Status::Success,
            data: vec![0xAA, 0xBB]
        }
    );
    assert_eq!(
        decode(0x0001, &[0x23, 0x00, 0x0A]),
        Event::CommandComplete {
            opcode: Opcode::START_DISCOVERY,
            status: Status::Busy,
            data: Vec::new()
        }
    );
}

#[test]
fn command_status_has_exactly_an_opcode_and_a_status() {
    assert_eq!(
        decode(0x0002, &[0x19, 0x00, 0x0C]),
        Event::CommandStatus {
            opcode: Opcode::PAIR_DEVICE,
            status: Status::NotSupported
        }
    );
    assert_eq!(
        decode_error(0x0002, &[0x19, 0x00, 0x0C, 0]),
        ParseError::TrailingBytes {
            what: "Command Status",
            extra: 1
        }
    );
    assert_eq!(
        decode_error(0x0002, &[0x19, 0x00]),
        ParseError::Truncated {
            what: "Command Status",
            needed: 1,
            found: 0
        }
    );
}

#[test]
fn simple_controller_events_decode() {
    assert_eq!(
        decode(0x0003, &[0x42]),
        Event::ControllerError { code: 0x42 }
    );
    assert_eq!(decode(0x0005, &[]), Event::IndexRemoved);
    assert_eq!(
        decode(0x0006, &1u32.to_le_bytes()),
        Event::NewSettings(Settings(1))
    );
    assert_eq!(
        decode(0x0007, &[0x0C, 0x01, 0x1A]),
        Event::ClassOfDeviceChanged(ClassOfDevice::from_wire([0x0C, 0x01, 0x1A]))
    );
}

#[test]
fn a_local_name_stops_at_its_nul_in_each_field() {
    let mut parameters = vec![0u8; 249 + 11];
    parameters[..6].copy_from_slice(b"Laptop");
    parameters[249..252].copy_from_slice(b"Lap");
    assert_eq!(
        decode(0x0008, &parameters),
        Event::LocalNameChanged {
            name: "Laptop".to_owned(),
            short_name: "Lap".to_owned()
        }
    );
    assert!(matches!(
        decode_error(0x0008, &parameters[..259]),
        ParseError::Truncated { .. }
    ));
}

#[test]
fn a_new_link_key_carries_the_store_hint_and_the_key() {
    let mut parameters = vec![1u8];
    parameters.extend(DEVICE_BYTES);
    parameters.push(0x07);
    parameters.extend([0x5A; 16]);
    parameters.push(0);
    let Event::NewLinkKey { store, key } = decode(0x0009, &parameters) else {
        panic!("a new link key");
    };
    assert!(store);
    assert_eq!(key.device, device());
    assert_eq!(key.key_type, LinkKeyType::UnauthenticatedP256);
    assert_eq!(key.value, KeyValue([0x5A; 16]));
    assert_eq!(key.pin_length, 0);
}

#[test]
fn a_store_hint_that_is_not_zero_or_one_is_refused() {
    let mut parameters = vec![2u8];
    parameters.extend(DEVICE_BYTES);
    parameters.push(0);
    parameters.extend([0; 17]);
    assert_eq!(
        decode_error(0x0009, &parameters),
        ParseError::InvalidBool {
            what: "store hint",
            value: 2
        }
    );
}

#[test]
fn a_new_long_term_key_decodes_every_field() {
    let mut parameters = vec![0u8];
    parameters.extend([0x13, 0x71, 0xDA, 0x7D, 0x1A, 0x00, 2]);
    parameters.extend([0x03, 1, 16, 0x0B, 0x0A]);
    parameters.extend([1, 2, 3, 4, 5, 6, 7, 8]);
    parameters.extend([0x77; 16]);
    let Event::NewLongTermKey { store, key } = decode(0x000A, &parameters) else {
        panic!("a new long term key");
    };
    assert!(!store);
    assert_eq!(key.device.kind, AddressType::LeRandom);
    assert_eq!(key.key_type, LongTermKeyType::AuthenticatedP256);
    assert!(key.central);
    assert_eq!(key.encryption_size, 16);
    assert_eq!(key.diversifier, 0x0A0B);
    assert_eq!(key.random, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(key.value, KeyValue([0x77; 16]));
}

#[test]
fn a_connected_device_carries_its_flags_and_advertising_data() {
    let eir = [3, 0x19, 0xC1, 0x03];
    let mut parameters = DEVICE_BYTES.to_vec();
    parameters.extend(8u32.to_le_bytes());
    parameters.extend((eir.len() as u16).to_le_bytes());
    parameters.extend(eir);
    let Event::DeviceConnected {
        device: found,
        flags,
        eir,
    } = decode(0x000B, &parameters)
    else {
        panic!("a connected device");
    };
    assert_eq!(found, device());
    assert_eq!(flags, 8);
    assert_eq!(eir.appearance, Some(0x03C1));
}

#[test]
fn advertising_data_longer_than_the_packet_is_truncated() {
    let mut parameters = DEVICE_BYTES.to_vec();
    parameters.extend(0u32.to_le_bytes());
    parameters.extend(10u16.to_le_bytes());
    parameters.extend([2, 0x01, 0x06]);
    assert!(matches!(
        decode_error(0x000B, &parameters),
        ParseError::Truncated {
            needed: 10,
            found: 3,
            ..
        }
    ));
}

#[test]
fn bad_advertising_data_fails_the_whole_event() {
    let mut parameters = DEVICE_BYTES.to_vec();
    parameters.extend(0u32.to_le_bytes());
    parameters.extend(3u16.to_le_bytes());
    parameters.extend([9, 0x09, b'a']);
    assert_eq!(
        decode_error(0x000B, &parameters),
        ParseError::EirOverrun { offset: 0 }
    );
}

#[test]
fn disconnect_reasons_keep_values_the_document_does_not_define() {
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
        assert_eq!(
            decode(0x000C, &[&DEVICE_BYTES[..], &[wire]].concat()),
            Event::DeviceDisconnected {
                device: device(),
                reason
            }
        );
    }
}

#[test]
fn pairing_events_decode() {
    assert_eq!(
        decode(0x000D, &[&DEVICE_BYTES[..], &[0x04]].concat()),
        Event::ConnectFailed {
            device: device(),
            status: Status::ConnectFailed
        }
    );
    assert_eq!(
        decode(0x000E, &[&DEVICE_BYTES[..], &[1]].concat()),
        Event::PinCodeRequest {
            device: device(),
            secure: true
        }
    );
    assert_eq!(
        decode(
            0x000F,
            &[&DEVICE_BYTES[..], &[1], &482_913u32.to_le_bytes()].concat()
        ),
        Event::UserConfirmationRequest {
            device: device(),
            just_works: true,
            value: 482_913
        }
    );
    assert_eq!(
        decode(0x0010, &DEVICE_BYTES),
        Event::UserPasskeyRequest { device: device() }
    );
    assert_eq!(
        decode(0x0011, &[&DEVICE_BYTES[..], &[0x05]].concat()),
        Event::AuthenticationFailed {
            device: device(),
            status: Status::AuthenticationFailed
        }
    );
    assert_eq!(
        decode(0x0016, &DEVICE_BYTES),
        Event::DeviceUnpaired(device())
    );
    assert_eq!(
        decode(
            0x0017,
            &[&DEVICE_BYTES[..], &654_321u32.to_le_bytes(), &[3]].concat()
        ),
        Event::PasskeyNotify {
            device: device(),
            passkey: 654_321,
            entered: 3
        }
    );
}

fn device_found(rssi: i8, flags: u32, eir: &[u8]) -> Vec<u8> {
    let mut parameters = DEVICE_BYTES.to_vec();
    parameters.push(rssi as u8);
    parameters.extend(flags.to_le_bytes());
    parameters.extend((eir.len() as u16).to_le_bytes());
    parameters.extend(eir);
    parameters
}

#[test]
fn a_found_device_has_a_signed_signal_level_and_flags() {
    let eir = [5, 0x09, b'T', b'V', b'!', b'?'];
    let Event::DeviceFound {
        device: found,
        rssi,
        flags,
        eir,
    } = decode(0x0012, &device_found(-60, 0b10_0101, &eir))
    else {
        panic!("a found device");
    };
    assert_eq!(found, device());
    assert_eq!(rssi, Some(-60));
    assert!(flags.confirm_name());
    assert!(!flags.legacy_pairing());
    assert!(flags.not_connectable());
    assert!(!flags.name_request_failed());
    assert!(flags.scan_response());
    assert_eq!(eir.name.unwrap().text, "TV!?");
}

#[test]
fn a_signal_level_of_127_is_not_available() {
    let Event::DeviceFound { rssi, eir, .. } = decode(0x0012, &device_found(127, 0, &[])) else {
        panic!("a found device");
    };
    assert_eq!(rssi, None);
    assert_eq!(eir, Eir::default());
}

#[test]
fn found_flags_are_independent_bits() {
    let flags = FoundFlags(1 << 1 | 1 << 4);
    assert!(flags.legacy_pairing());
    assert!(flags.name_request_failed());
    assert!(!flags.confirm_name());
    assert!(!flags.not_connectable());
    assert!(!flags.scan_response());
}

#[test]
fn discovery_state_carries_the_address_types() {
    assert_eq!(
        decode(0x0013, &[0x07, 1]),
        Event::Discovering {
            types: AddressTypes::ALL,
            active: true
        }
    );
    assert_eq!(
        decode_error(0x0013, &[0x07, 5]),
        ParseError::InvalidBool {
            what: "discovering",
            value: 5
        }
    );
}

#[test]
fn an_event_the_crate_does_not_know_is_kept_whole() {
    assert_eq!(
        decode(0x0030, &[1, 2, 3]),
        Event::Other {
            code: 0x0030,
            parameters: vec![1, 2, 3]
        }
    );
}

#[test]
fn an_address_type_outside_the_document_fails_the_event() {
    let mut parameters = DEVICE_BYTES.to_vec();
    parameters[6] = 9;
    assert_eq!(
        decode_error(0x0010, &parameters),
        ParseError::UnknownAddressType(9)
    );
}

#[test]
fn parameters_beyond_the_layout_are_refused() {
    assert_eq!(
        decode_error(0x0003, &[1, 2]),
        ParseError::TrailingBytes {
            what: "Controller Error",
            extra: 1
        }
    );
    assert_eq!(
        decode_error(0x0004, &[1]),
        ParseError::TrailingBytes {
            what: "Index Added",
            extra: 1
        }
    );
}

#[test]
fn truncation_names_the_event_being_read() {
    let error = decode_error(0x000C, &DEVICE_BYTES);
    assert_eq!(
        error,
        ParseError::Truncated {
            what: "Device Disconnected",
            needed: 1,
            found: 0
        }
    );
    assert_eq!(
        error.to_string(),
        "Device Disconnected: needs 1 bytes, got 0"
    );
}
