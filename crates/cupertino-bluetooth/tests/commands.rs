//! Every command encodes to the bytes `doc/mgmt-protocol.rst` (the former `mgmt-api.txt`)
//! describes. Each expected packet is written field by field: header (opcode, controller index,
//! parameter length, all little-endian) then the parameters in the order the document lists them.

use cupertino_bluetooth::reply::{parse_address_types, parse_connections, parse_device, parse_index_list, parse_settings, ControllerInfo, Version};
use cupertino_bluetooth::{
    Address, AddressType, AddressTypes, ClassOfDevice, Command, ControllerIndex, Discoverable, DeviceId, IoCapability, KeyValue, LinkKey, LinkKeyType, LongTermKey, LongTermKeyType,
    ParseError, Settings,
};

/// Parses `"01 02 ab"` into bytes.
fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace().map(|byte| u8::from_str_radix(byte, 16).unwrap()).collect()
}

const CONTROLLER: ControllerIndex = ControllerIndex(0);

/// 11:22:33:44:55:66, written least significant byte first on the wire.
const WIRE_ADDRESS: &str = "66 55 44 33 22 11";

fn device(kind: AddressType) -> DeviceId {
    DeviceId { address: "11:22:33:44:55:66".parse().unwrap(), kind }
}

fn assert_encodes(command: Command, index: ControllerIndex, expected: &str) {
    assert_eq!(command.encode(index).unwrap(), hex(expected), "{command:?}");
}

#[test]
fn the_read_commands_have_no_parameters() {
    // Read Management Version Information: code 0x0001, <non-controller> 0xFFFF.
    assert_encodes(Command::ReadVersion, ControllerIndex::NONE, "01 00 ff ff 00 00");
    // Read Controller Index List: code 0x0003.
    assert_encodes(Command::ReadIndexList, ControllerIndex::NONE, "03 00 ff ff 00 00");
    // Read Controller Information: code 0x0004.
    assert_encodes(Command::ReadControllerInfo, ControllerIndex(1), "04 00 01 00 00 00");
    // Get Connections: code 0x0015.
    assert_encodes(Command::GetConnections, CONTROLLER, "15 00 00 00 00 00");
}

#[test]
fn the_set_commands_carry_one_flag() {
    // Set Powered 0x0005, Set Connectable 0x0007, Set Bondable 0x0009: Powered (1 octet).
    assert_encodes(Command::SetPowered(true), CONTROLLER, "05 00 00 00 01 00 01");
    assert_encodes(Command::SetPowered(false), CONTROLLER, "05 00 00 00 01 00 00");
    assert_encodes(Command::SetConnectable(true), CONTROLLER, "07 00 00 00 01 00 01");
    assert_encodes(Command::SetBondable(true), CONTROLLER, "09 00 00 00 01 00 01");
}

#[test]
fn set_discoverable_carries_the_mode_and_a_timeout_in_seconds() {
    // Set Discoverable 0x0006: Discoverable (1 octet), Timeout (2 octets). 180 s = 0x00B4.
    assert_encodes(Command::SetDiscoverable(Discoverable::General { timeout: 180 }), CONTROLLER, "06 00 00 00 03 00 01 b4 00");
    assert_encodes(Command::SetDiscoverable(Discoverable::Limited { timeout: 30 }), CONTROLLER, "06 00 00 00 03 00 02 1e 00");
    assert_encodes(Command::SetDiscoverable(Discoverable::Off), CONTROLLER, "06 00 00 00 03 00 00 00 00");
}

#[test]
fn io_capability_and_discovery() {
    // Set IO Capability 0x0018: 0x04 is KeyboardDisplay.
    assert_encodes(Command::SetIoCapability(IoCapability::KeyboardDisplay), CONTROLLER, "18 00 00 00 01 00 04");
    assert_encodes(Command::SetIoCapability(IoCapability::DisplayYesNo), CONTROLLER, "18 00 00 00 01 00 01");
    // Start Discovery 0x0023 and Stop Discovery 0x0024: Address_Type is a bit mask; 0x07 is
    // BR/EDR and LE interleaved, 0x01 BR/EDR, 0x06 LE.
    assert_encodes(Command::StartDiscovery(AddressTypes::ALL), CONTROLLER, "23 00 00 00 01 00 07");
    assert_encodes(Command::StartDiscovery(AddressTypes::LE), CONTROLLER, "23 00 00 00 01 00 06");
    assert_encodes(Command::StopDiscovery(AddressTypes::BR_EDR), CONTROLLER, "24 00 00 00 01 00 01");
}

#[test]
fn pairing_commands_name_the_device() {
    let bredr = device(AddressType::BrEdr);
    let le = device(AddressType::LePublic);
    // Pair Device 0x0019: Address (6), Address_Type (1), IO_Capability (1).
    assert_encodes(Command::PairDevice { device: bredr, io_capability: IoCapability::KeyboardDisplay }, CONTROLLER, &format!("19 00 00 00 08 00 {WIRE_ADDRESS} 00 04"));
    // Cancel Pair Device 0x001A: Address, Address_Type.
    assert_encodes(Command::CancelPairDevice(le), CONTROLLER, &format!("1a 00 00 00 07 00 {WIRE_ADDRESS} 01"));
    // Unpair Device 0x001B: Address, Address_Type, Disconnect (1).
    assert_encodes(Command::UnpairDevice { device: le, disconnect: true }, CONTROLLER, &format!("1b 00 00 00 08 00 {WIRE_ADDRESS} 01 01"));
    // Disconnect 0x0014: Address, Address_Type (LE random is 0x02).
    assert_encodes(Command::Disconnect(device(AddressType::LeRandom)), CONTROLLER, &format!("14 00 00 00 07 00 {WIRE_ADDRESS} 02"));
    // Confirm Name 0x0025: Address, Address_Type, Name_Known (1).
    assert_encodes(Command::ConfirmName { device: bredr, name_known: false }, CONTROLLER, &format!("25 00 00 00 08 00 {WIRE_ADDRESS} 00 00"));
    // PIN Code Negative Reply 0x0017.
    assert_encodes(Command::PinCodeNegativeReply(bredr), CONTROLLER, &format!("17 00 00 00 07 00 {WIRE_ADDRESS} 00"));
}

#[test]
fn confirmation_and_passkey_replies_pick_their_opcode_from_the_answer() {
    let bredr = device(AddressType::BrEdr);
    // User Confirmation Reply 0x001C and Negative Reply 0x001D: Address, Address_Type.
    assert_encodes(Command::UserConfirmationReply { device: bredr, accept: true }, CONTROLLER, &format!("1c 00 00 00 07 00 {WIRE_ADDRESS} 00"));
    assert_encodes(Command::UserConfirmationReply { device: bredr, accept: false }, CONTROLLER, &format!("1d 00 00 00 07 00 {WIRE_ADDRESS} 00"));
    // User Passkey Reply 0x001E: Address, Address_Type, Passkey (4 octets). 123456 = 0x0001E240.
    assert_encodes(Command::UserPasskeyReply { device: bredr, passkey: Some(123_456) }, CONTROLLER, &format!("1e 00 00 00 0b 00 {WIRE_ADDRESS} 00 40 e2 01 00"));
    // User Passkey Negative Reply 0x001F.
    assert_encodes(Command::UserPasskeyReply { device: bredr, passkey: None }, CONTROLLER, &format!("1f 00 00 00 07 00 {WIRE_ADDRESS} 00"));
}

#[test]
fn load_link_keys_refuses_debug_keys_and_lists_each_key() {
    let key = LinkKey { device: device(AddressType::BrEdr), key_type: LinkKeyType::AuthenticatedP192, value: KeyValue([0xA0; 16]), pin_length: 4 };
    // Load Link Keys 0x0012: Debug_Keys (1), Key_Count (2), then per key Address (6),
    // Address_Type (1), Key_Type (1), Value (16), PIN_Length (1). 3 + 25 = 28 = 0x1C bytes.
    let value = "a0 ".repeat(16);
    assert_encodes(Command::LoadLinkKeys(vec![key]), CONTROLLER, &format!("12 00 00 00 1c 00 00 01 00 {WIRE_ADDRESS} 00 05 {value}04"));
    assert_encodes(Command::LoadLinkKeys(Vec::new()), CONTROLLER, "12 00 00 00 03 00 00 00 00");
}

#[test]
fn load_long_term_keys_lists_each_key() {
    let key = LongTermKey {
        device: device(AddressType::LeRandom),
        key_type: LongTermKeyType::AuthenticatedP256,
        central: true,
        encryption_size: 16,
        diversifier: 0x1234,
        random: [1, 2, 3, 4, 5, 6, 7, 8],
        value: KeyValue([0xB1; 16]),
    };
    // Load Long Term Keys 0x0013: Key_Count (2), then per key Address (6), Address_Type (1),
    // Key_Type (1), Central (1), Encryption_Size (1), Encryption_Diversifier (2), Random_Number
    // (8), Value (16). 2 + 36 = 38 = 0x26 bytes.
    let value = "b1 ".repeat(15);
    assert_encodes(Command::LoadLongTermKeys(vec![key]), CONTROLLER, &format!("13 00 00 00 26 00 01 00 {WIRE_ADDRESS} 02 03 01 10 34 12 01 02 03 04 05 06 07 08 {value}b1"));
}

#[test]
fn too_many_keys_for_one_packet_is_an_error() {
    let key = LinkKey { device: device(AddressType::BrEdr), key_type: LinkKeyType::Combination, value: KeyValue([0; 16]), pin_length: 0 };
    // 65535 / 25 = 2621 keys fit; one more does not.
    let error = Command::LoadLinkKeys(vec![key; 2622]).encode(CONTROLLER).unwrap_err();
    assert_eq!(error.to_string(), "command 0x0012 has 65553 parameter bytes, more than a packet holds");
}

#[test]
fn an_address_is_written_most_significant_byte_first() {
    let address: Address = "aa:bb:cc:dd:ee:01".parse().unwrap();
    assert_eq!(address.to_wire(), [0x01, 0xEE, 0xDD, 0xCC, 0xBB, 0xAA]);
    assert_eq!(address.to_string(), "AA:BB:CC:DD:EE:01");
    assert_eq!("AA:BB:CC:DD:EE".parse::<Address>(), Err(ParseError::InvalidAddress("AA:BB:CC:DD:EE".to_owned())));
    assert_eq!("AA:BB:CC:DD:EE:01:02".parse::<Address>(), Err(ParseError::InvalidAddress("AA:BB:CC:DD:EE:01:02".to_owned())));
    assert_eq!("AA:BB:CC:DD:EE:0G".parse::<Address>(), Err(ParseError::InvalidAddress("AA:BB:CC:DD:EE:0G".to_owned())));
}

#[test]
fn key_values_never_print() {
    let key = LinkKey { device: device(AddressType::BrEdr), key_type: LinkKeyType::Combination, value: KeyValue([0xEE; 16]), pin_length: 0 };
    let text = format!("{key:?}");
    assert!(text.contains("KeyValue(<redacted>)"), "{text}");
    assert!(!text.to_lowercase().contains("ee, ee"), "{text}");
}

#[test]
fn return_parameters_decode() {
    // Read Management Version Information: Version (1), Revision (2). 1.23.
    assert_eq!(Version::parse(&hex("01 17 00")).unwrap(), Version { version: 1, revision: 23 });
    // Read Controller Index List: Num_Controllers (2), Controller_Index[] (2 each).
    assert_eq!(parse_index_list(&hex("02 00 00 00 03 00")).unwrap(), [ControllerIndex(0), ControllerIndex(3)]);
    assert_eq!(
        parse_index_list(&hex("02 00 00 00")),
        Err(ParseError::Truncated { what: "Read Controller Index List reply", needed: 2, found: 0 })
    );
    // Set Powered: Current_Settings (4). Bit 0 powered, bit 7 BR/EDR, bit 9 LE.
    let settings = parse_settings(&hex("81 02 00 00")).unwrap();
    assert_eq!(settings, Settings(0x0281));
    assert!(settings.powered() && !settings.connectable());
    // Pair Device: Address, Address_Type.
    assert_eq!(parse_device(&hex(&format!("{WIRE_ADDRESS} 01"))).unwrap(), device(AddressType::LePublic));
    assert_eq!(parse_address_types(&hex("07")).unwrap(), AddressTypes::ALL);
    // Get Connections: Connection_Count (2), then Address, Address_Type per connection.
    assert_eq!(parse_connections(&hex(&format!("01 00 {WIRE_ADDRESS} 00"))).unwrap(), [device(AddressType::BrEdr)]);
}

#[test]
fn controller_information_decodes_every_field() {
    // Address 6, Bluetooth_Version 1 (0x0A is 5.1), Manufacturer 2, Supported_Settings 4,
    // Current_Settings 4, Class_Of_Device 3, Name 249, Short_Name 11.
    let mut reply = hex("01 ee dd cc bb aa 0a 02 00 ff 03 00 00 81 02 00 00 0c 01 7a");
    let mut name = b"test-pc".to_vec();
    name.resize(249, 0);
    reply.extend_from_slice(&name);
    let mut short = b"pc".to_vec();
    short.resize(11, 0);
    reply.extend_from_slice(&short);
    let info = ControllerInfo::parse(&reply).unwrap();
    assert_eq!(info.address.to_string(), "AA:BB:CC:DD:EE:01");
    assert_eq!(info.bluetooth_version, 10);
    assert_eq!(info.manufacturer, 2);
    assert_eq!(info.supported_settings, Settings(0x03FF));
    assert_eq!(info.current_settings, Settings(0x0281));
    assert_eq!(info.class, ClassOfDevice::from_wire([0x0c, 0x01, 0x7a]));
    assert_eq!(info.name, "test-pc");
    assert_eq!(info.short_name, "pc");
    assert!(ControllerInfo::parse(&reply[..reply.len() - 1]).is_err());
}
