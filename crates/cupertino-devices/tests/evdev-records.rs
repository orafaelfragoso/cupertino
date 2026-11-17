//! evdev records: bitmaps, the event layout, reading a descriptor and axis geometry.

use std::os::fd::{AsFd, OwnedFd};

use cupertino_devices::evdev::codes::{
    is_button, BTN_GEAR_UP, BTN_LEFT, BTN_MISC, BTN_TOOL_FINGER, KEY_MAX, MT_TOOL_FINGER,
    MT_TOOL_PALM, MT_TOOL_PEN,
};
use cupertino_devices::evdev::{
    read_events, AbsAxis, AbsInfo, BitSet, DeviceId, DeviceInfo, Error, InputEvent, Resolution,
    Tool,
};
use rustix::io::write;
use rustix::pipe::{pipe_with, PipeFlags};

#[test]
fn a_bit_set_numbers_bits_from_the_low_end_of_each_byte_with_bits_past_the_end_clear() {
    let set = BitSet::from_bytes(vec![0b0000_0101, 0b1000_0000]);
    for (bit, expected) in [
        (0, true),
        (1, false),
        (2, true),
        (8, false),
        (14, false),
        (15, true),
        (16, false),
        (u16::MAX, false),
    ] {
        assert_eq!(set.contains(bit), expected, "{bit}");
    }
    assert_eq!(set.iter().collect::<Vec<_>>(), [0, 2, 15]);
    assert!(!BitSet::default().contains(0));
    assert_eq!(BitSet::default().iter().count(), 0);

    assert_eq!(
        BitSet::from_codes(&[3, 300, 9, 3])
            .iter()
            .collect::<Vec<_>>(),
        [3, 9, 300]
    );
    assert_eq!(BitSet::from_codes(&[]), BitSet::default());

    // inserting grows the set; clearing removes only that bit
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
fn an_event_is_24_bytes_in_the_kernel_layout_and_round_trips_at_the_extremes() {
    let event = InputEvent {
        sec: 0x0102_0304_0506_0708,
        usec: 999_999,
        kind: 3,
        code: 0x35,
        value: -7,
    };
    let bytes = event.to_bytes();
    assert_eq!(bytes.len(), 24);
    assert_eq!(bytes[0..8], 0x0102_0304_0506_0708i64.to_ne_bytes());
    assert_eq!(bytes[8..16], 999_999i64.to_ne_bytes());
    assert_eq!(bytes[16..18], 3u16.to_ne_bytes());
    assert_eq!(bytes[18..20], 0x35u16.to_ne_bytes());
    assert_eq!(bytes[20..24], (-7i32).to_ne_bytes());
    for event in [
        event,
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
        assert_eq!(
            InputEvent::from_bytes(&event.to_bytes()),
            event,
            "{event:?}"
        );
    }
    let timed = InputEvent {
        sec: 12,
        usec: 345_678,
        ..event
    };
    assert_eq!(timed.time_us(), 12_345_678);
}

#[test]
fn buttons_are_the_range_from_btn_misc_to_btn_gear_up_and_tools_map_from_their_raw_type() {
    for (code, expected) in [
        (BTN_MISC - 1, false),
        (BTN_MISC, true),
        (BTN_LEFT, true),
        (BTN_TOOL_FINGER, true),
        (BTN_GEAR_UP, true),
        (BTN_GEAR_UP + 1, false),
        (30, false),
        (KEY_MAX, false),
    ] {
        assert_eq!(is_button(code), expected, "{code:#x}");
    }
    for (raw, expected) in [
        (MT_TOOL_FINGER, Tool::Finger),
        (MT_TOOL_PEN, Tool::Pen),
        (MT_TOOL_PALM, Tool::Palm),
        (9, Tool::Other(9)),
        (-1, Tool::Other(-1)),
    ] {
        assert_eq!(Tool::from_raw(raw), expected, "{raw}");
    }
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
fn reading_appends_every_queued_record_in_order_across_batches() {
    let (read, writer) = pipe();
    let mut out = Vec::new();
    read_events(read.as_fd(), &mut out).unwrap();
    assert!(out.is_empty(), "an empty descriptor yields nothing");

    let sent: Vec<_> = (0..5).map(event).collect();
    send(&writer, &sent);
    read_events(read.as_fd(), &mut out).unwrap();
    assert_eq!(out, sent);
    send(&writer, &[event(9)]);
    read_events(read.as_fd(), &mut out).unwrap();
    assert_eq!(out.len(), 6);
    assert_eq!(out[5], event(9));

    // more than a read's 64 records
    let sent: Vec<_> = (0..150).map(event).collect();
    send(&writer, &sent);
    let mut out = Vec::new();
    read_events(read.as_fd(), &mut out).unwrap();
    assert_eq!(out, sent);
}

#[test]
fn a_cut_record_a_closed_descriptor_or_an_unreadable_one_is_an_error() {
    let (cut, writer) = pipe();
    let bytes = event(1).to_bytes();
    assert_eq!(write(&writer, &bytes[..10]).unwrap(), 10);
    let (closed, gone) = pipe();
    drop(gone);
    let (_read, unreadable) = pipe();
    for (name, fd, check) in [
        (
            "a read ending inside a record",
            cut.as_fd(),
            &(|e: &Error| matches!(e, Error::TruncatedRecord { len: 10 }))
                as &dyn Fn(&Error) -> bool,
        ),
        ("a closed descriptor", closed.as_fd(), &|e: &Error| {
            matches!(e, Error::TruncatedRecord { len: 0 })
        }),
        (
            "the write end of a pipe",
            unreadable.as_fd(),
            &|e: &Error| matches!(e, Error::Io { op: "read", .. }),
        ),
    ] {
        let mut out = Vec::new();
        let error = read_events(fd, &mut out).unwrap_err();
        assert!(check(&error), "{name}: {error:?}");
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
fn units_convert_to_millimetres_only_by_a_positive_resolution() {
    for raw in [0, -12, i32::MIN] {
        assert_eq!(Resolution::from_raw(raw), Resolution::Unknown, "{raw}");
    }
    match Resolution::from_raw(12) {
        Resolution::UnitsPerMm(n) => assert_eq!(n.get(), 12),
        other => panic!("{other:?}"),
    }
    for (resolution, units, expected) in [
        (10, 25, Some(2.5)),
        (10, -10, Some(-1.0)),
        (10, 0, Some(0.0)),
        (0, 25, None),
        (-3, 25, None),
    ] {
        assert_eq!(
            axis(resolution).units_to_mm(units),
            expected,
            "{resolution} {units}"
        );
    }
}

#[test]
fn device_info_finds_axes_and_properties_by_code() {
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
