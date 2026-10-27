//! Traffic recorded from a real controller with `examples/record-bluetooth.rs` (read-only; the controller's
//! address and name were replaced by fixed fake values). The codec must reproduce the commands the
//! example sent byte for byte and decode what the kernel answered.

use cupertino_bluetooth::reply::{parse_index_list, ControllerInfo, Version};
use cupertino_bluetooth::{
    decode_packet, ClassOfDevice, Command, ControllerIndex, Event, MajorClass, Opcode, Settings,
    Status,
};

const RECORDING: &str = include_str!("fixtures/recorded-mgmt.txt");

fn from_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// The `>` and `<` lines of the recording, in order, as bytes.
fn recorded(direction: char) -> Vec<Vec<u8>> {
    RECORDING
        .lines()
        .filter_map(|line| line.strip_prefix(direction))
        .map(|hex| from_hex(hex.trim()))
        .collect()
}

#[test]
fn the_commands_encode_to_what_the_recording_sent() {
    let sent = recorded('>');
    assert_eq!(sent.len(), 3);
    assert_eq!(
        Command::ReadVersion.encode(ControllerIndex::NONE).unwrap(),
        sent[0]
    );
    assert_eq!(
        Command::ReadIndexList
            .encode(ControllerIndex::NONE)
            .unwrap(),
        sent[1]
    );
    assert_eq!(
        Command::ReadControllerInfo
            .encode(ControllerIndex(0))
            .unwrap(),
        sent[2]
    );
}

#[test]
fn the_replies_decode_to_the_recorded_values() {
    let received = recorded('<');
    assert_eq!(received.len(), 3);

    let Event::CommandComplete {
        opcode,
        status,
        data,
    } = decode_packet(&received[0]).unwrap().event
    else {
        panic!("not a Command Complete")
    };
    assert_eq!((opcode, status), (Opcode::READ_VERSION, Status::Success));
    assert_eq!(
        Version::parse(&data).unwrap(),
        Version {
            version: 1,
            revision: 23
        }
    );

    let Event::CommandComplete {
        opcode,
        status,
        data,
    } = decode_packet(&received[1]).unwrap().event
    else {
        panic!("not a Command Complete")
    };
    assert_eq!((opcode, status), (Opcode::READ_INDEX_LIST, Status::Success));
    assert_eq!(parse_index_list(&data).unwrap(), [ControllerIndex(0)]);

    let packet = decode_packet(&received[2]).unwrap();
    assert_eq!(packet.index, ControllerIndex(0));
    let Event::CommandComplete {
        opcode,
        status,
        data,
    } = packet.event
    else {
        panic!("not a Command Complete")
    };
    assert_eq!(
        (opcode, status),
        (Opcode::READ_CONTROLLER_INFO, Status::Success)
    );
    let info = ControllerInfo::parse(&data).unwrap();
    assert_eq!(info.address.to_string(), "AA:BB:CC:DD:EE:01");
    assert_eq!(info.bluetooth_version, 12);
    assert_eq!(info.manufacturer, 0x5D);
    assert_eq!(info.supported_settings, Settings(0x01BF_BEFF));
    assert_eq!(info.current_settings, Settings(0x01BE_0AD1));
    assert!(
        info.current_settings.powered()
            && info.current_settings.bondable()
            && !info.current_settings.connectable()
    );
    assert_eq!(info.class, ClassOfDevice::from_wire([0x04, 0x01, 0x7C]));
    assert_eq!(info.class.major(), MajorClass::Computer);
    assert_eq!(info.class.minor(), 1);
    assert_eq!(info.name, "cupertino-test");
    assert_eq!(info.short_name, "");
}
