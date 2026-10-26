//! Every event decodes to literal values. Packets are built field by field from the "Events"
//! section of `doc/mgmt-protocol.rst` (the former `mgmt-api.txt`): header (event code, controller
//! index, parameter length, all little-endian), then the parameters in the order listed.

use cupertino_bluetooth::{
    decode_packet, AddressType, AddressTypes, ClassOfDevice, ControllerIndex, DeviceId, DisconnectReason, Eir, EirName, Event, FoundFlags, KeyValue, LinkKey, LinkKeyType, LongTermKey,
    LongTermKeyType, Opcode, ParseError, Settings, Status, Uuid,
};

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace().map(|byte| u8::from_str_radix(byte, 16).unwrap()).collect()
}

/// 11:22:33:44:55:66 as the wire carries it, then the address type.
const BREDR: &str = "66 55 44 33 22 11 00";
const LE_RANDOM: &str = "66 55 44 33 22 11 02";

fn bredr() -> DeviceId {
    DeviceId { address: "11:22:33:44:55:66".parse().unwrap(), kind: AddressType::BrEdr }
}

fn event(index: u16, packet: &str) -> Event {
    let packet = decode_packet(&hex(packet)).unwrap();
    assert_eq!(packet.index, ControllerIndex(index));
    packet.event
}

#[test]
fn command_complete_carries_the_opcode_status_and_return_parameters() {
    // Event 0x0001: Command_Opcode (2) 0x0005 Set Powered, Status (1) 0, Current_Settings (4).
    assert_eq!(
        event(0, "01 00 00 00 07 00 05 00 00 81 02 00 00"),
        Event::CommandComplete { opcode: Opcode::SET_POWERED, status: Status::Success, data: hex("81 02 00 00") }
    );
    // A failure status with no return parameters: 0x0F is Not Powered.
    assert_eq!(
        event(0, "01 00 00 00 03 00 23 00 0f"),
        Event::CommandComplete { opcode: Opcode::START_DISCOVERY, status: Status::NotPowered, data: Vec::new() }
    );
}

#[test]
fn command_status_and_controller_events() {
    // Event 0x0002: Command_Opcode (2), Status (1). 0x14 is Permission Denied.
    assert_eq!(event(0, "02 00 00 00 03 00 12 00 14"), Event::CommandStatus { opcode: Opcode::LOAD_LINK_KEYS, status: Status::PermissionDenied });
    // Event 0x0003 Controller Error: Error_Code (1).
    assert_eq!(event(0, "03 00 00 00 01 00 0a"), Event::ControllerError { code: 10 });
    // Events 0x0004 and 0x0005 have no parameters.
    assert_eq!(event(2, "04 00 02 00 00 00"), Event::IndexAdded);
    assert_eq!(event(2, "05 00 02 00 00 00"), Event::IndexRemoved);
    // Event 0x0006 New Settings: Current_Settings (4). Bit 0 powered, bit 4 bondable.
    assert_eq!(event(0, "06 00 00 00 04 00 11 00 00 00"), Event::NewSettings(Settings(0x11)));
    // Event 0x0007 Class Of Device Changed: Class_Of_Device (3), least significant first.
    assert_eq!(event(0, "07 00 00 00 03 00 0c 01 7a"), Event::ClassOfDeviceChanged(ClassOfDevice::from_wire([0x0c, 0x01, 0x7a])));
}

#[test]
fn local_name_changed_reads_two_fixed_size_strings() {
    // Event 0x0008: Name (249 octets), Short_Name (11 octets), each NUL padded.
    let mut packet = hex("08 00 00 00 04 01");
    let mut name = b"workstation".to_vec();
    name.resize(249, 0);
    packet.extend_from_slice(&name);
    let mut short = b"work".to_vec();
    short.resize(11, 0);
    packet.extend_from_slice(&short);
    assert_eq!(decode_packet(&packet).unwrap().event, Event::LocalNameChanged { name: "workstation".to_owned(), short_name: "work".to_owned() });
}

#[test]
fn new_keys_carry_the_store_hint_and_the_key() {
    // Event 0x0009 New Link Key: Store_Hint (1), Address (6), Address_Type (1), Key_Type (1),
    // Value (16), PIN_Length (1). 26 = 0x1A bytes. Key type 0x04 is an unauthenticated P-192 key.
    let value = "c3 ".repeat(16);
    assert_eq!(
        event(0, &format!("09 00 00 00 1a 00 01 {BREDR} 04 {value}00")),
        Event::NewLinkKey { store: true, key: LinkKey { device: bredr(), key_type: LinkKeyType::UnauthenticatedP192, value: KeyValue([0xC3; 16]), pin_length: 0 } }
    );
    // Event 0x000A New Long Term Key: Store_Hint (1), Address (6), Address_Type (1), Key_Type
    // (1), Central (1), Encryption Size (1), Enc. Diversifier (2), Random Number (8), Value (16).
    // 37 = 0x25 bytes. Key type 0x03 is an authenticated P-256 key; address type 2 is LE random.
    let value = "d4 ".repeat(16);
    assert_eq!(
        event(0, &format!("0a 00 00 00 25 00 00 {LE_RANDOM} 03 01 10 34 12 01 02 03 04 05 06 07 08 {value}")),
        Event::NewLongTermKey {
            store: false,
            key: LongTermKey {
                device: DeviceId { address: "11:22:33:44:55:66".parse().unwrap(), kind: AddressType::LeRandom },
                key_type: LongTermKeyType::AuthenticatedP256,
                central: true,
                encryption_size: 16,
                diversifier: 0x1234,
                random: [1, 2, 3, 4, 5, 6, 7, 8],
                value: KeyValue([0xD4; 16]),
            }
        }
    );
}

#[test]
fn connection_events() {
    // Event 0x000B Device Connected: Address (6), Address_Type (1), Flags (4), EIR_Data_Length
    // (2), EIR_Data. Flags bit 3 is "Initiated Connection". The data is a complete local name.
    let connected = event(0, &format!("0b 00 00 00 14 00 {BREDR} 08 00 00 00 07 00 06 09 4d 6f 75 73 65"));
    assert_eq!(
        connected,
        Event::DeviceConnected {
            device: bredr(),
            flags: 8,
            eir: Eir { name: Some(EirName { text: "Mouse".to_owned(), complete: true }), ..Eir::default() }
        }
    );
    // Event 0x000C Device Disconnected: Address, Address_Type, Reason (1). 3 is "terminated by
    // remote host".
    assert_eq!(event(0, &format!("0c 00 00 00 08 00 {BREDR} 03")), Event::DeviceDisconnected { device: bredr(), reason: DisconnectReason::RemoteHost });
    // Event 0x000D Connect Failed: Address, Address_Type, Status (1). 4 is Connect Failed.
    assert_eq!(event(0, &format!("0d 00 00 00 08 00 {BREDR} 04")), Event::ConnectFailed { device: bredr(), status: Status::ConnectFailed });
}

#[test]
fn pairing_events() {
    // Event 0x000E PIN Code Request: Address, Address_Type, Secure (1).
    assert_eq!(event(0, &format!("0e 00 00 00 08 00 {BREDR} 01")), Event::PinCodeRequest { device: bredr(), secure: true });
    // Event 0x000F User Confirmation Request: Address, Address_Type, Confirm_Hint (1), Value (4).
    // Hint 0: show the value for comparison. 123456 = 0x0001E240.
    assert_eq!(
        event(0, &format!("0f 00 00 00 0c 00 {BREDR} 00 40 e2 01 00")),
        Event::UserConfirmationRequest { device: bredr(), just_works: false, value: 123_456 }
    );
    // Hint 1: a plain yes or no ("Just Works").
    assert_eq!(
        event(0, &format!("0f 00 00 00 0c 00 {BREDR} 01 00 00 00 00")),
        Event::UserConfirmationRequest { device: bredr(), just_works: true, value: 0 }
    );
    // Event 0x0010 User Passkey Request: Address, Address_Type.
    assert_eq!(event(0, &format!("10 00 00 00 07 00 {BREDR}")), Event::UserPasskeyRequest { device: bredr() });
    // Event 0x0011 Authentication Failed: Address, Address_Type, Status (1). 5 is Authentication
    // Failed.
    assert_eq!(event(0, &format!("11 00 00 00 08 00 {BREDR} 05")), Event::AuthenticationFailed { device: bredr(), status: Status::AuthenticationFailed });
    // Event 0x0017 Passkey Notify: Address, Address_Type, Passkey (4), Entered (1).
    assert_eq!(event(0, &format!("17 00 00 00 0c 00 {BREDR} 40 e2 01 00 03")), Event::PasskeyNotify { device: bredr(), passkey: 123_456, entered: 3 });
    // Event 0x0016 Device Unpaired: Address, Address_Type.
    assert_eq!(event(0, &format!("16 00 00 00 07 00 {BREDR}")), Event::DeviceUnpaired(bredr()));
}

#[test]
fn device_found_carries_rssi_flags_and_the_advertising_data() {
    // Event 0x0012: Address, Address_Type, RSSI (1, signed), Flags (4), EIR_Data_Length (2),
    // EIR_Data. RSSI 0xC4 is -60 dBm. Flags bit 0 is "Confirm name". The data is a class of
    // device (type 0x0D) of 0x002540 (a keyboard).
    let found = event(0, &format!("12 00 00 00 13 00 {BREDR} c4 01 00 00 00 05 00 04 0d 40 25 00"));
    assert_eq!(
        found,
        Event::DeviceFound {
            device: bredr(),
            rssi: Some(-60),
            flags: FoundFlags(1),
            eir: Eir { class: Some(ClassOfDevice::from_wire([0x40, 0x25, 0x00])), ..Eir::default() }
        }
    );
    let Event::DeviceFound { flags, .. } = found else { unreachable!() };
    assert!(flags.confirm_name() && !flags.legacy_pairing() && !flags.scan_response());
    // RSSI 127 means "not available".
    let Event::DeviceFound { rssi, .. } = event(0, &format!("12 00 00 00 0e 00 {BREDR} 7f 00 00 00 00 00 00")) else { panic!("not a Device Found") };
    assert_eq!(rssi, None);
}

#[test]
fn discovering_and_unknown_events() {
    // Event 0x0013: Address_Type (1, a bit mask), Discovering (1).
    assert_eq!(event(0, "13 00 00 00 02 00 07 01"), Event::Discovering { types: AddressTypes::ALL, active: true });
    assert_eq!(event(0, "13 00 00 00 02 00 06 00"), Event::Discovering { types: AddressTypes::LE, active: false });
    // An event this crate does not decode is passed through with its code.
    assert_eq!(event(0, "30 00 00 00 02 00 aa bb"), Event::Other { code: 0x30, parameters: vec![0xAA, 0xBB] });
}

#[test]
fn malformed_packets_are_errors() {
    // Header says 4 parameter bytes, 3 follow.
    assert_eq!(decode_packet(&hex("06 00 00 00 04 00 01 00 00")), Err(ParseError::LengthMismatch { header: 4, actual: 3 }));
    // Not even a full header.
    assert_eq!(decode_packet(&hex("06 00 00")), Err(ParseError::Truncated { what: "packet header", needed: 2, found: 1 }));
    // New Settings needs 4 bytes.
    assert_eq!(decode_packet(&hex("06 00 00 00 02 00 01 00")), Err(ParseError::Truncated { what: "New Settings", needed: 4, found: 2 }));
    // Index Added takes no parameters.
    assert_eq!(decode_packet(&hex("04 00 00 00 01 00 00")), Err(ParseError::TrailingBytes { what: "Index Added", extra: 1 }));
    // Secure must be 0 or 1.
    assert_eq!(decode_packet(&hex(&format!("0e 00 00 00 08 00 {BREDR} 02"))), Err(ParseError::InvalidBool { what: "secure", value: 2 }));
    // Address types are 0 to 2.
    assert_eq!(decode_packet(&hex("16 00 00 00 07 00 66 55 44 33 22 11 09")), Err(ParseError::UnknownAddressType(9)));
}

#[test]
fn eir_decodes_name_class_appearance_uuids_and_power() {
    let data = hex(
        "02 01 06 \
         0e 09 4b 65 79 62 6f 61 72 64 20 4b 33 38 30 \
         05 03 12 18 0f 18 \
         02 0a fc \
         03 19 c1 03 \
         04 0d 40 05 00 \
         03 06 aa bb",
    );
    // 0x03 is "complete list of 16-bit UUIDs"; the last structure is type 0x06 with 2 payload
    // bytes, which is not a multiple of 16, so the data as a whole is malformed.
    assert_eq!(Eir::parse(&data), Err(ParseError::EirUuidLength { size: 16, length: 2 }));

    let data = hex(
        "02 01 06 \
         0e 09 4b 65 79 62 6f 61 72 64 20 4b 33 38 30 \
         05 03 12 18 0f 18 \
         02 0a fc \
         03 19 c1 03 \
         04 0d 40 05 00 \
         05 04 78 56 34 12 \
         11 07 00 11 22 33 44 55 66 77 88 99 aa bb cc dd ee ff \
         02 ff 00",
    );
    let eir = Eir::parse(&data).unwrap();
    assert_eq!(eir.flags, Some(0x06));
    assert_eq!(eir.name, Some(EirName { text: "Keyboard K380".to_owned(), complete: true }));
    assert_eq!(eir.tx_power, Some(-4));
    assert_eq!(eir.appearance, Some(0x03C1));
    assert_eq!(eir.class, Some(ClassOfDevice::from_wire([0x40, 0x05, 0x00])));
    assert_eq!(
        eir.uuids,
        [
            Uuid::from_u16(0x1812),
            Uuid::from_u16(0x180F),
            Uuid::from_u32(0x1234_5678),
            Uuid(0xFFEE_DDCC_BBAA_9988_7766_5544_3322_1100)
        ]
    );
    assert_eq!(Uuid::from_u16(0x1812).to_string(), "00001812-0000-1000-8000-00805f9b34fb");
    assert_eq!(Uuid::from_u16(0x1812).as_u16(), Some(0x1812));
    assert_eq!(Uuid::from_u32(0x1234_5678).as_u16(), None);
}

#[test]
fn eir_marks_a_shortened_name_and_stops_at_a_zero_length() {
    // Type 0x08 is the shortened local name. A zero length ends the data; what follows is padding.
    let eir = Eir::parse(&hex("05 08 4d 6f 75 73 00 00 05 09 58 58 58 58")).unwrap();
    assert_eq!(eir.name, Some(EirName { text: "Mous".to_owned(), complete: false }));
}

#[test]
fn eir_rejects_a_structure_that_overruns_and_wrong_field_sizes() {
    assert_eq!(Eir::parse(&hex("02 01 06 05 09 41")), Err(ParseError::EirOverrun { offset: 3 }));
    assert_eq!(Eir::parse(&hex("03 0a 01 02")), Err(ParseError::EirFieldLength { kind: 0x0A, length: 2 }));
    assert_eq!(Eir::parse(&hex("03 03 12 18 \
                                04 03 12 18 0f")), Err(ParseError::EirUuidLength { size: 2, length: 3 }));
}

#[test]
fn eir_names_from_the_air_survive_invalid_utf8_and_unknown_types_are_skipped() {
    // 0xFF is not valid UTF-8; it becomes U+FFFD instead of hiding the device. Type 0x2A is not
    // one this parser knows and is skipped.
    let eir = Eir::parse(&hex("04 09 41 ff 42 02 2a 00")).unwrap();
    assert_eq!(eir.name.unwrap().text, "A\u{FFFD}B");
}
