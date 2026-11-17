//! The rfkill event layout and the Wi-Fi switch state folded from it.

use cupertino_netlink::rfkill::{
    encode_soft_block_all, parse_event, Event, Kind, Operation, WlanSwitches,
};
use cupertino_netlink::ParseError;

fn raw(index: u32, kind: u8, operation: u8, soft: u8, hard: u8) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&index.to_ne_bytes());
    bytes[4] = kind;
    bytes[5] = operation;
    bytes[6] = soft;
    bytes[7] = hard;
    bytes
}

fn event(index: u32, kind: Kind, operation: Operation, soft: bool, hard: bool) -> Event {
    Event {
        index,
        kind,
        operation,
        soft_blocked: soft,
        hard_blocked: hard,
    }
}

#[test]
fn an_event_decodes_its_index_kind_operation_and_blocks() {
    assert_eq!(
        parse_event(&raw(3, 1, 0, 1, 0)).unwrap(),
        event(3, Kind::Wlan, Operation::Added, true, false)
    );
    assert_eq!(
        parse_event(&raw(9, 2, 2, 0, 1)).unwrap(),
        event(9, Kind::Bluetooth, Operation::Changed, false, true)
    );
    assert_eq!(
        parse_event(&raw(0, 0, 1, 0, 0)).unwrap(),
        event(0, Kind::All, Operation::Deleted, false, false)
    );
}

#[test]
fn any_nonzero_byte_is_a_block() {
    let parsed = parse_event(&raw(0, 1, 2, 0x7F, 2)).unwrap();
    assert!(parsed.soft_blocked && parsed.hard_blocked);
}

#[test]
fn kinds_and_operations_this_crate_does_not_name_are_kept() {
    let parsed = parse_event(&raw(0, 7, 9, 0, 0)).unwrap();
    assert_eq!(parsed.kind, Kind::Other(7));
    assert_eq!(parsed.operation, Operation::Other(9));
    assert_eq!(
        parse_event(&raw(0, 1, 3, 0, 0)).unwrap().operation,
        Operation::ChangedAll
    );
}

#[test]
fn a_longer_event_of_a_newer_kernel_decodes_by_its_first_eight_bytes() {
    let mut bytes = raw(4, 1, 2, 1, 0).to_vec();
    bytes.extend([0xFF; 4]);
    assert_eq!(
        parse_event(&bytes).unwrap(),
        event(4, Kind::Wlan, Operation::Changed, true, false)
    );
}

#[test]
fn an_event_shorter_than_eight_bytes_is_refused() {
    assert_eq!(
        parse_event(&[0; 7]),
        Err(ParseError::RfkillSize { actual: 7 })
    );
    assert_eq!(parse_event(&[]), Err(ParseError::RfkillSize { actual: 0 }));
}

#[test]
fn the_block_all_request_is_a_change_all_event() {
    let bytes = encode_soft_block_all(Kind::Wlan, true);
    assert_eq!(bytes, raw(0, 1, 3, 1, 0));
    let bytes = encode_soft_block_all(Kind::Bluetooth, false);
    assert_eq!(bytes, raw(0, 2, 3, 0, 0));
    assert_eq!(encode_soft_block_all(Kind::All, true), raw(0, 0, 3, 1, 0));
    assert_eq!(
        encode_soft_block_all(Kind::Other(5), true),
        raw(0, 5, 3, 1, 0)
    );
}

#[test]
fn no_events_mean_no_wifi_switch() {
    let switches = WlanSwitches::new();
    assert!(!switches.is_present());
    assert!(!switches.is_soft_blocked());
    assert!(!switches.is_hard_blocked());
}

#[test]
fn an_added_switch_is_present_and_carries_its_blocks() {
    let mut switches = WlanSwitches::new();
    switches.apply(&event(0, Kind::Wlan, Operation::Added, true, false));
    assert!(switches.is_present());
    assert!(switches.is_soft_blocked());
    assert!(!switches.is_hard_blocked());
}

#[test]
fn a_change_replaces_the_blocks_of_that_switch() {
    let mut switches = WlanSwitches::new();
    switches.apply(&event(0, Kind::Wlan, Operation::Added, true, false));
    switches.apply(&event(0, Kind::Wlan, Operation::Changed, false, true));
    assert!(!switches.is_soft_blocked());
    assert!(switches.is_hard_blocked());
}

#[test]
fn a_block_on_any_switch_counts_and_a_deleted_switch_stops_counting() {
    let mut switches = WlanSwitches::new();
    switches.apply(&event(0, Kind::Wlan, Operation::Added, false, false));
    switches.apply(&event(1, Kind::Wlan, Operation::Added, true, false));
    assert!(switches.is_soft_blocked());
    switches.apply(&event(1, Kind::Wlan, Operation::Deleted, false, false));
    assert!(!switches.is_soft_blocked());
    assert!(switches.is_present());
    switches.apply(&event(0, Kind::Wlan, Operation::Deleted, false, false));
    assert!(!switches.is_present());
}

#[test]
fn other_kinds_and_other_operations_change_nothing() {
    let mut switches = WlanSwitches::new();
    switches.apply(&event(0, Kind::Bluetooth, Operation::Added, true, true));
    assert!(!switches.is_present());
    switches.apply(&event(0, Kind::Wlan, Operation::Added, false, false));
    let before = switches.clone();
    switches.apply(&event(0, Kind::Wlan, Operation::ChangedAll, true, true));
    switches.apply(&event(0, Kind::Wlan, Operation::Other(8), true, true));
    switches.apply(&event(5, Kind::Other(9), Operation::Added, true, true));
    assert_eq!(switches, before);
}
