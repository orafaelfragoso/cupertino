//! The rfkill event layout and the Wi-Fi switch state folded from it.

use cupertino_netd::netlink::rfkill::{
    encode_soft_block_all, parse_event, Event, Kind, Operation, WlanSwitches,
};
use cupertino_netd::netlink::ParseError;

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
fn an_event_decodes_by_its_first_eight_bytes_keeping_kinds_and_operations_not_named() {
    let longer = [&raw(4, 1, 2, 1, 0)[..], &[0xFF; 4]].concat();
    for (name, bytes, expected) in [
        (
            "added",
            raw(3, 1, 0, 1, 0).to_vec(),
            event(3, Kind::Wlan, Operation::Added, true, false),
        ),
        (
            "changed",
            raw(9, 2, 2, 0, 1).to_vec(),
            event(9, Kind::Bluetooth, Operation::Changed, false, true),
        ),
        (
            "deleted",
            raw(0, 0, 1, 0, 0).to_vec(),
            event(0, Kind::All, Operation::Deleted, false, false),
        ),
        (
            "changed all",
            raw(0, 1, 3, 0, 0).to_vec(),
            event(0, Kind::Wlan, Operation::ChangedAll, false, false),
        ),
        (
            "any nonzero byte is a block",
            raw(0, 1, 2, 0x7F, 2).to_vec(),
            event(0, Kind::Wlan, Operation::Changed, true, true),
        ),
        (
            "not named",
            raw(0, 7, 9, 0, 0).to_vec(),
            event(0, Kind::Other(7), Operation::Other(9), false, false),
        ),
        (
            "a newer kernel's longer event",
            longer,
            event(4, Kind::Wlan, Operation::Changed, true, false),
        ),
    ] {
        assert_eq!(parse_event(&bytes), Ok(expected), "{name}");
    }
    for length in [7, 0] {
        assert_eq!(
            parse_event(&vec![0; length]),
            Err(ParseError::RfkillSize { actual: length }),
            "{length}"
        );
    }
}

#[test]
fn the_block_all_request_is_a_change_all_event_of_its_kind() {
    for (kind, block, expected) in [
        (Kind::Wlan, true, raw(0, 1, 3, 1, 0)),
        (Kind::Bluetooth, false, raw(0, 2, 3, 0, 0)),
        (Kind::All, true, raw(0, 0, 3, 1, 0)),
        (Kind::Other(5), true, raw(0, 5, 3, 1, 0)),
    ] {
        assert_eq!(encode_soft_block_all(kind, block), expected, "{kind:?}");
    }
}

#[test]
fn the_wifi_switch_is_present_and_blocked_while_any_added_wlan_switch_is() {
    let mut switches = WlanSwitches::new();
    let state = |s: &WlanSwitches| (s.is_present(), s.is_soft_blocked(), s.is_hard_blocked());
    for (name, applied, expected) in [
        ("nothing yet", None, (false, false, false)),
        (
            "another kind",
            Some(event(0, Kind::Bluetooth, Operation::Added, true, true)),
            (false, false, false),
        ),
        (
            "added soft blocked",
            Some(event(0, Kind::Wlan, Operation::Added, true, false)),
            (true, true, false),
        ),
        (
            "a change replaces the blocks",
            Some(event(0, Kind::Wlan, Operation::Changed, false, true)),
            (true, false, true),
        ),
        (
            "back to unblocked",
            Some(event(0, Kind::Wlan, Operation::Changed, false, false)),
            (true, false, false),
        ),
        (
            "a block on another switch counts",
            Some(event(1, Kind::Wlan, Operation::Added, true, false)),
            (true, true, false),
        ),
        (
            "a deleted switch stops counting",
            Some(event(1, Kind::Wlan, Operation::Deleted, false, false)),
            (true, false, false),
        ),
        (
            "the last switch deleted",
            Some(event(0, Kind::Wlan, Operation::Deleted, false, false)),
            (false, false, false),
        ),
    ] {
        if let Some(event) = applied {
            switches.apply(&event);
        }
        assert_eq!(state(&switches), expected, "{name}");
    }

    switches.apply(&event(0, Kind::Wlan, Operation::Added, false, false));
    let before = switches.clone();
    for ignored in [
        event(0, Kind::Wlan, Operation::ChangedAll, true, true),
        event(0, Kind::Wlan, Operation::Other(8), true, true),
        event(5, Kind::Other(9), Operation::Added, true, true),
    ] {
        switches.apply(&ignored);
        assert_eq!(switches, before, "{ignored:?}");
    }
}
