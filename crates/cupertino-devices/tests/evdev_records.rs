//! evdev records: bitmaps, the event layout, reading a descriptor, axis geometry and error text.

use std::os::fd::{AsFd, OwnedFd};

use cupertino_devices::evdev::codes::{
    is_button, BTN_GEAR_UP, BTN_LEFT, BTN_MISC, BTN_TOOL_FINGER, KEY_MAX, MT_TOOL_FINGER,
    MT_TOOL_PALM, MT_TOOL_PEN,
};
use cupertino_devices::evdev::{
    read_events, AbsInfo, BitSet, DeviceInfo, Error, FrameError, InputEvent, Resolution, Tool,
    EVENT_SIZE,
};
use rustix::io::write;
use rustix::pipe::{pipe_with, PipeFlags};

#[test]
fn bits_are_numbered_from_the_low_end_of_each_byte() {
    let set = BitSet::from_bytes(vec![0b0000_0101, 0b1000_0000]);
    assert!(set.contains(0) && !set.contains(1) && set.contains(2));
    assert!(set.contains(15) && !set.contains(14) && !set.contains(8));
    assert_eq!(set.iter().collect::<Vec<_>>(), [0, 2, 15]);
}

#[test]
fn bits_past_the_end_are_clear() {
    let set = BitSet::from_bytes(vec![0xff]);
    assert!(set.contains(7) && !set.contains(8) && !set.contains(u16::MAX));
    assert!(!BitSet::default().contains(0));
    assert_eq!(BitSet::default().iter().count(), 0);
}

#[test]
fn a_set_built_from_codes_holds_exactly_them() {
    let set = BitSet::from_codes(&[3, 300, 9, 3]);
    assert_eq!(set.iter().collect::<Vec<_>>(), [3, 9, 300]);
    assert_eq!(BitSet::from_codes(&[]), BitSet::default());
}

#[test]
fn inserting_grows_the_set_and_clearing_shrinks_nothing_else() {
    let mut set = BitSet::default();
    set.insert(700, true);
    assert!(set.contains(700));
    set.insert(5, true);
    set.insert(700, false);
    set.insert(701, false);
    assert_eq!(set.iter().collect::<Vec<_>>(), [5]);
    set.insert(5, false);
    assert_eq!(set.iter().count(), 0);
}

#[test]
fn an_event_is_24_bytes_in_the_kernel_layout() {
    assert_eq!(EVENT_SIZE, 24);
    let event = InputEvent {
        sec: 0x0102_0304_0506_0708,
        usec: 999_999,
        kind: 3,
        code: 0x35,
        value: -7,
    };
    let bytes = event.to_bytes();
    assert_eq!(bytes[0..8], 0x0102_0304_0506_0708i64.to_ne_bytes());
    assert_eq!(bytes[8..16], 999_999i64.to_ne_bytes());
    assert_eq!(bytes[16..18], 3u16.to_ne_bytes());
    assert_eq!(bytes[18..20], 0x35u16.to_ne_bytes());
    assert_eq!(bytes[20..24], (-7i32).to_ne_bytes());
    assert_eq!(InputEvent::from_bytes(&bytes), event);
}

#[test]
fn events_round_trip_at_the_extremes() {
    for event in [
        InputEvent {
            sec: 0,
            usec: 0,
            kind: 0,
            code: 0,
            value: 0,
        },
        InputEvent {
            sec: i64::MAX,
            usec: 999_999,
            kind: u16::MAX,
            code: u16::MAX,
            value: i32::MAX,
        },
        InputEvent {
            sec: -1,
            usec: 0,
            kind: 1,
            code: 2,
            value: i32::MIN,
        },
    ] {
        assert_eq!(InputEvent::from_bytes(&event.to_bytes()), event);
    }
}

#[test]
fn an_events_time_is_in_microseconds() {
    let event = InputEvent {
        sec: 12,
        usec: 345_678,
        kind: 0,
        code: 0,
        value: 0,
    };
    assert_eq!(event.time_us(), 12_345_678);
}

#[test]
fn buttons_are_the_range_from_btn_misc_to_btn_gear_up() {
    assert!(!is_button(BTN_MISC - 1));
    assert!(
        is_button(BTN_MISC)
            && is_button(BTN_LEFT)
            && is_button(BTN_TOOL_FINGER)
            && is_button(BTN_GEAR_UP)
    );
    assert!(!is_button(BTN_GEAR_UP + 1));
    assert!(!is_button(30) && !is_button(KEY_MAX));
}

#[test]
fn raw_tool_types_map_to_tools() {
    assert_eq!(Tool::from_raw(MT_TOOL_FINGER), Tool::Finger);
    assert_eq!(Tool::from_raw(MT_TOOL_PEN), Tool::Pen);
    assert_eq!(Tool::from_raw(MT_TOOL_PALM), Tool::Palm);
    assert_eq!(Tool::from_raw(9), Tool::Other(9));
    assert_eq!(Tool::from_raw(-1), Tool::Other(-1));
}

fn pipe() -> (OwnedFd, OwnedFd) {
    pipe_with(PipeFlags::NONBLOCK | PipeFlags::CLOEXEC).unwrap()
}

fn event(n: i32) -> InputEvent {
    InputEvent {
        sec: 1,
        usec: i64::from(n),
        kind: 1,
        code: 30,
        value: n,
    }
}

fn send(fd: &OwnedFd, events: &[InputEvent]) {
    let bytes: Vec<u8> = events.iter().flat_map(|e| e.to_bytes()).collect();
    assert_eq!(write(fd, &bytes).unwrap(), bytes.len());
}

#[test]
fn reading_an_empty_descriptor_yields_nothing() {
    let (read, _write) = pipe();
    let mut out = Vec::new();
    read_events(read.as_fd(), &mut out).unwrap();
    assert!(out.is_empty());
}

#[test]
fn reading_decodes_every_queued_record_in_order() {
    let (read, writer) = pipe();
    let sent: Vec<_> = (0..5).map(event).collect();
    send(&writer, &sent);
    let mut out = Vec::new();
    read_events(read.as_fd(), &mut out).unwrap();
    assert_eq!(out, sent);
    // appended, and nothing is left
    send(&writer, &[event(9)]);
    read_events(read.as_fd(), &mut out).unwrap();
    assert_eq!(out.len(), 6);
    assert_eq!(out[5], event(9));
}

#[test]
fn reading_goes_on_past_one_batch() {
    let (read, writer) = pipe();
    // more than a read's 64 records and than one pipe write's worth of one call
    let sent: Vec<_> = (0..150).map(event).collect();
    send(&writer, &sent);
    let mut out = Vec::new();
    read_events(read.as_fd(), &mut out).unwrap();
    assert_eq!(out, sent);
}

#[test]
fn a_read_that_ends_inside_a_record_is_an_error() {
    let (read, writer) = pipe();
    let bytes = event(1).to_bytes();
    assert_eq!(write(&writer, &bytes[..10]).unwrap(), 10);
    let mut out = Vec::new();
    match read_events(read.as_fd(), &mut out) {
        Err(Error::TruncatedRecord { len }) => assert_eq!(len, 10),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_closed_descriptor_is_an_error_not_an_empty_read() {
    let (read, writer) = pipe();
    drop(writer);
    let mut out = Vec::new();
    assert!(matches!(
        read_events(read.as_fd(), &mut out),
        Err(Error::TruncatedRecord { len: 0 })
    ));
}

#[test]
fn a_descriptor_that_cannot_be_read_names_the_call() {
    let (_read, writer) = pipe();
    // the write end of a pipe is not readable
    let mut out = Vec::new();
    match read_events(writer.as_fd(), &mut out) {
        Err(Error::Io { op, .. }) => assert_eq!(op, "read"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_non_positive_resolution_is_unknown() {
    assert_eq!(Resolution::from_raw(0), Resolution::Unknown);
    assert_eq!(Resolution::from_raw(-12), Resolution::Unknown);
    assert_eq!(Resolution::from_raw(i32::MIN), Resolution::Unknown);
    match Resolution::from_raw(12) {
        Resolution::UnitsPerMm(n) => assert_eq!(n.get(), 12),
        other => panic!("{other:?}"),
    }
}

fn axis(resolution: i32) -> AbsInfo {
    AbsInfo {
        value: 0,
        min: 0,
        max: 1000,
        fuzz: 0,
        flat: 0,
        resolution: Resolution::from_raw(resolution),
    }
}

#[test]
fn units_convert_to_millimetres_by_the_resolution() {
    assert_eq!(axis(10).units_to_mm(25), Some(2.5));
    assert_eq!(axis(10).units_to_mm(-10), Some(-1.0));
    assert_eq!(axis(10).units_to_mm(0), Some(0.0));
    assert_eq!(axis(0).units_to_mm(25), None);
    assert_eq!(axis(-3).units_to_mm(25), None);
}

#[test]
fn device_info_finds_axes_and_properties_by_code() {
    use cupertino_devices::evdev::{AbsAxis, DeviceId};
    let info = DeviceInfo {
        name: "pad".into(),
        id: DeviceId {
            bustype: 3,
            vendor: 1,
            product: 2,
            version: 3,
        },
        properties: BitSet::from_codes(&[2]),
        event_types: BitSet::from_codes(&[1, 3]),
        keys: BitSet::default(),
        rel_axes: BitSet::default(),
        switches: BitSet::default(),
        abs_axes: vec![
            AbsAxis {
                code: 0,
                info: axis(20),
            },
            AbsAxis {
                code: 1,
                info: axis(0),
            },
        ],
    };
    assert_eq!(info.abs(0).unwrap().resolution, axis(20).resolution);
    assert_eq!(info.abs(1).unwrap().max, 1000);
    assert!(info.abs(2).is_none());
    assert!(info.has_property(2) && !info.has_property(1));
}

#[test]
fn errors_name_what_failed() {
    let io = Error::Io {
        op: "EVIOCGNAME",
        errno: rustix::io::Errno::NOTTY,
    };
    assert!(io.to_string().starts_with("EVIOCGNAME failed: "));
    assert!(std::error::Error::source(&io).is_some());
    assert!(Error::TruncatedRecord { len: 10 }
        .to_string()
        .contains("10 bytes"));
    assert!(Error::NameNotUtf8.to_string().contains("UTF-8"));
    assert!(std::error::Error::source(&Error::NameNotUtf8).is_none());
    let frame: Error = FrameError::ProtocolA.into();
    assert!(frame.to_string().contains("protocol A"));
    assert!(std::error::Error::source(&frame).is_some());
}

#[test]
fn every_frame_error_has_its_own_message() {
    let all = [
        FrameError::MultitouchWithoutSlots,
        FrameError::MultitouchWithoutPosition,
        FrameError::EmptySlotRange,
        FrameError::ProtocolA,
        FrameError::UnknownEventType { kind: 0x40 },
        FrameError::UnexpectedEvent {
            kind: 3,
            code: 0x35,
        },
        FrameError::InvalidKeyValue { code: 30, value: 7 },
        FrameError::InvalidSwitchValue { code: 0, value: 2 },
        FrameError::SlotOutOfRange {
            slot: 9,
            slot_count: 5,
        },
        FrameError::InvalidTrackingId { value: -4 },
        FrameError::MissingSnapshotAxis { code: 0x39 },
        FrameError::SnapshotSlotCount {
            code: 0x35,
            actual: 3,
            expected: 5,
        },
    ];
    let messages: std::collections::HashSet<_> = all.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), all.len());
    assert!(all[4].to_string().contains("0x40"));
    assert!(all[7].to_string().contains('2'));
    assert!(all[8].to_string().contains('9') && all[8].to_string().contains('5'));
    assert!(all[11].to_string().contains('3') && all[11].to_string().contains('5'));
}
