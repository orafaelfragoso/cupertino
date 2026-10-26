//! Framer behaviour over recorded event sequences (`tests/fixtures/*.events`) with literal expected frames.

use cupertino_evdev::codes::*;
use cupertino_evdev::{
    AbsAxis, AbsInfo, AbsPoint, BitSet, Contact, DeviceId, DeviceInfo, Frame, FrameError, Framer,
    FramerOutput, InputEvent, KeyEvent, KeyState, Layout, Motion, MtSlotValues, Resolution,
    Snapshot, Switch, SwitchEvent, Tool, Wheel,
};

/// Parses `<sec>.<usec> <type> <code> <value>` lines; `#` starts a comment.
fn load(fixture: &str) -> Vec<InputEvent> {
    let path = format!(
        "{}/tests/fixtures/{fixture}.events",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    text.lines()
        .map(|line| line.split('#').next().unwrap().trim())
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (sec, usec) = fields[0].split_once('.').unwrap();
            InputEvent {
                sec: sec.parse().unwrap(),
                usec: usec.parse().unwrap(),
                kind: fields[1].parse().unwrap(),
                code: fields[2].parse().unwrap(),
                value: fields[3].parse().unwrap(),
            }
        })
        .collect()
}

fn event(kind: u16, code: u16, value: i32) -> InputEvent {
    InputEvent {
        sec: 1,
        usec: 0,
        kind,
        code,
        value,
    }
}

fn axis(code: u16, max: i32, resolution: i32) -> AbsAxis {
    AbsAxis {
        code,
        info: AbsInfo {
            value: 0,
            min: 0,
            max,
            fuzz: 0,
            flat: 0,
            resolution: Resolution::from_raw(resolution),
        },
    }
}

fn info(keys: &[u16], rel: &[u16], switches: &[u16], abs_axes: Vec<AbsAxis>) -> DeviceInfo {
    DeviceInfo {
        name: "fixture".to_string(),
        id: DeviceId {
            bustype: 0,
            vendor: 0,
            product: 0,
            version: 0,
        },
        properties: BitSet::default(),
        event_types: BitSet::default(),
        keys: BitSet::from_codes(keys),
        rel_axes: BitSet::from_codes(rel),
        switches: BitSet::from_codes(switches),
        abs_axes,
    }
}

fn keyboard() -> DeviceInfo {
    info(&[30], &[], &[], vec![])
}

fn mouse() -> DeviceInfo {
    info(
        &[BTN_LEFT],
        &[
            REL_X,
            REL_Y,
            REL_HWHEEL,
            REL_WHEEL,
            REL_WHEEL_HI_RES,
            REL_HWHEEL_HI_RES,
        ],
        &[],
        vec![],
    )
}

/// Clickpad with 5 slots; 30 units/mm on both position axes.
fn touchpad() -> DeviceInfo {
    info(
        &[
            BTN_LEFT,
            330,
            BTN_TOOL_FINGER,
            BTN_TOOL_DOUBLETAP,
            BTN_TOOL_TRIPLETAP,
            BTN_TOOL_QUADTAP,
            BTN_TOOL_QUINTTAP,
        ],
        &[],
        &[],
        vec![
            axis(ABS_X, 3000, 30),
            axis(ABS_Y, 2000, 30),
            axis(ABS_MT_SLOT, 4, 0),
            axis(ABS_MT_TOUCH_MAJOR, 1020, 0),
            axis(ABS_MT_TOUCH_MINOR, 1020, 0),
            axis(ABS_MT_POSITION_X, 3000, 30),
            axis(ABS_MT_POSITION_Y, 2000, 30),
            axis(ABS_MT_TOOL_TYPE, 2, 0),
            axis(ABS_MT_TRACKING_ID, 65535, 0),
            axis(ABS_MT_PRESSURE, 255, 0),
        ],
    )
}

/// QEMU virtio tablet: absolute pointer with no resolution, legacy wheel only.
fn tablet() -> DeviceInfo {
    info(
        &[BTN_LEFT],
        &[REL_WHEEL],
        &[],
        vec![axis(ABS_X, 32767, 0), axis(ABS_Y, 32767, 0)],
    )
}

/// Feeds `events` and collects frames; a resync is answered with `snapshot`.
fn drive(info: &DeviceInfo, events: &[InputEvent], snapshot: Option<&Snapshot>) -> Vec<Frame> {
    let mut framer = Framer::new(Layout::from_info(info).unwrap());
    let mut frames = Vec::new();
    for event in events {
        match framer.push(event).unwrap() {
            None => {}
            Some(FramerOutput::Frame(frame)) => frames.push(frame),
            Some(FramerOutput::ResyncNeeded { time_us }) => {
                frames.push(
                    framer
                        .apply_snapshot(snapshot.expect("test expected no resync"), time_us)
                        .unwrap(),
                );
            }
        }
    }
    frames
}

/// A frame with nothing in it, for struct-update in expectations.
fn blank(time_us: i64) -> Frame {
    Frame {
        time_us,
        resynced: false,
        keys: vec![],
        held_buttons: vec![],
        motion: Motion { dx: 0, dy: 0 },
        wheel: Wheel {
            vertical: 0,
            horizontal: 0,
        },
        abs_pointer: None,
        contacts: vec![],
        finger_count: None,
        switches: vec![],
    }
}

fn key(code: u16, state: KeyState) -> KeyEvent {
    KeyEvent { code, state }
}

fn finger(
    slot: u32,
    tracking_id: u32,
    x: i32,
    y: i32,
    pressure: i32,
    major: i32,
    minor: i32,
) -> Contact {
    Contact {
        slot,
        tracking_id,
        x,
        y,
        pressure: Some(pressure),
        touch_major: Some(major),
        touch_minor: Some(minor),
        tool: Tool::Finger,
    }
}

#[test]
fn keyboard_press_repeat_release() {
    let frames = drive(&keyboard(), &load("keyboard"), None);
    assert_eq!(
        frames,
        vec![
            Frame {
                keys: vec![key(30, KeyState::Pressed)],
                ..blank(1_000_000_100)
            },
            Frame {
                keys: vec![key(30, KeyState::Repeat)],
                ..blank(1_000_500_200)
            },
            Frame {
                keys: vec![key(30, KeyState::Repeat)],
                ..blank(1_000_583_200)
            },
            Frame {
                keys: vec![key(30, KeyState::Released)],
                ..blank(1_000_700_300)
            },
        ]
    );
}

#[test]
fn mouse_hi_res_wheel_is_not_double_counted() {
    let frames = drive(&mouse(), &load("mouse"), None);
    assert_eq!(
        frames,
        vec![
            Frame {
                motion: Motion { dx: 3, dy: -2 },
                ..blank(2_000_001_000)
            },
            Frame {
                wheel: Wheel {
                    vertical: 40,
                    horizontal: 0
                },
                ..blank(2_000_009_000)
            },
            Frame {
                wheel: Wheel {
                    vertical: 40,
                    horizontal: 0
                },
                ..blank(2_000_017_000)
            },
            // The legacy REL_WHEEL of this frame is the same detent: 40 + 40 + 40 = 120 in total.
            Frame {
                wheel: Wheel {
                    vertical: 40,
                    horizontal: 0
                },
                ..blank(2_000_025_000)
            },
            Frame {
                keys: vec![key(BTN_LEFT, KeyState::Pressed)],
                held_buttons: vec![BTN_LEFT],
                ..blank(2_000_100_000)
            },
            Frame {
                keys: vec![key(BTN_LEFT, KeyState::Released)],
                ..blank(2_000_150_000)
            },
            Frame {
                wheel: Wheel {
                    vertical: 0,
                    horizontal: -120
                },
                ..blank(2_000_200_000)
            },
        ]
    );
}

#[test]
fn legacy_wheel_counts_120_per_step_without_hi_res() {
    let frames = drive(&tablet(), &load("tablet"), None);
    let pointer = |x| Some(AbsPoint { x, y: 8192 });
    assert_eq!(
        frames,
        vec![
            Frame {
                abs_pointer: pointer(16384),
                ..blank(5_000_000_100)
            },
            Frame {
                keys: vec![key(BTN_LEFT, KeyState::Pressed)],
                held_buttons: vec![BTN_LEFT],
                abs_pointer: pointer(16384),
                ..blank(5_000_020_000)
            },
            Frame {
                wheel: Wheel {
                    vertical: -120,
                    horizontal: 0
                },
                held_buttons: vec![BTN_LEFT],
                abs_pointer: pointer(16384),
                ..blank(5_000_030_000)
            },
            Frame {
                keys: vec![key(BTN_LEFT, KeyState::Released)],
                abs_pointer: pointer(16400),
                ..blank(5_000_040_000)
            },
        ]
    );
}

#[test]
fn two_finger_touch_move_lift_and_slot_reuse() {
    let frames = drive(&touchpad(), &load("touchpad"), None);
    let pointer = |x, y| Some(AbsPoint { x, y });
    let base = |time_us| Frame {
        held_buttons: vec![330],
        ..blank(time_us)
    };
    assert_eq!(
        frames,
        vec![
            Frame {
                keys: vec![key(330, KeyState::Pressed)],
                abs_pointer: pointer(1200, 800),
                contacts: vec![finger(0, 100, 1200, 800, 40, 300, 250)],
                finger_count: Some(1),
                ..base(3_000_000_000)
            },
            Frame {
                abs_pointer: pointer(1200, 800),
                contacts: vec![
                    finger(0, 100, 1200, 800, 40, 300, 250),
                    finger(1, 101, 2500, 820, 38, 280, 240)
                ],
                finger_count: Some(2),
                ..base(3_000_012_000)
            },
            Frame {
                abs_pointer: pointer(1230, 800),
                contacts: vec![
                    finger(0, 100, 1230, 800, 40, 300, 250),
                    finger(1, 101, 2500, 830, 38, 280, 240)
                ],
                finger_count: Some(2),
                ..base(3_000_024_000)
            },
            Frame {
                abs_pointer: pointer(1230, 800),
                contacts: vec![finger(1, 101, 2500, 830, 38, 280, 240)],
                finger_count: Some(1),
                ..base(3_000_036_000)
            },
            Frame {
                keys: vec![key(330, KeyState::Released)],
                held_buttons: vec![],
                abs_pointer: pointer(1230, 800),
                finger_count: Some(0),
                ..blank(3_000_048_000)
            },
            Frame {
                keys: vec![
                    key(330, KeyState::Pressed),
                    key(BTN_LEFT, KeyState::Pressed)
                ],
                held_buttons: vec![BTN_LEFT, 330],
                abs_pointer: pointer(600, 400),
                contacts: vec![finger(0, 102, 600, 400, 35, 200, 190)],
                finger_count: Some(1),
                ..blank(3_000_500_000)
            },
            Frame {
                keys: vec![key(BTN_LEFT, KeyState::Released)],
                abs_pointer: pointer(600, 400),
                contacts: vec![finger(0, 102, 600, 400, 35, 200, 190)],
                finger_count: Some(1),
                ..base(3_000_620_000)
            },
        ]
    );
}

#[test]
fn syn_dropped_discards_to_report_then_resyncs_from_snapshot() {
    // State the device is in when the client catches up: finger 200 moved, a second finger (201) landed in slot 1
    // (the current slot), and the physical click is held.
    let per_slot = |code, values: [i32; 5]| MtSlotValues {
        code,
        values: values.to_vec(),
    };
    let snapshot = Snapshot {
        keys: BitSet::from_codes(&[330, BTN_TOOL_DOUBLETAP, BTN_LEFT]),
        switches: BitSet::default(),
        abs: vec![(ABS_X, 1800), (ABS_Y, 1900), (ABS_MT_SLOT, 1)],
        mt: vec![
            per_slot(ABS_MT_TRACKING_ID, [200, 201, -1, -1, -1]),
            per_slot(ABS_MT_POSITION_X, [1800, 2600, 0, 0, 0]),
            per_slot(ABS_MT_POSITION_Y, [1900, 1900, 0, 0, 0]),
            per_slot(ABS_MT_PRESSURE, [30, 33, 0, 0, 0]),
            per_slot(ABS_MT_TOUCH_MAJOR, [260, 250, 0, 0, 0]),
            per_slot(ABS_MT_TOUCH_MINOR, [240, 230, 0, 0, 0]),
            per_slot(ABS_MT_TOOL_TYPE, [0, 0, 0, 0, 0]),
        ],
    };
    let frames = drive(&touchpad(), &load("touchpad-dropped"), Some(&snapshot));
    assert_eq!(
        frames,
        vec![
            Frame {
                keys: vec![key(330, KeyState::Pressed)],
                held_buttons: vec![330],
                abs_pointer: Some(AbsPoint { x: 1000, y: 1000 }),
                contacts: vec![finger(0, 200, 1000, 1000, 30, 260, 240)],
                finger_count: Some(1),
                ..blank(4_000_000_000)
            },
            // The partial x = 1010 frame and the lost y = 1020 are gone; the snapshot supplies the state.
            Frame {
                resynced: true,
                keys: vec![key(BTN_LEFT, KeyState::Pressed)],
                held_buttons: vec![BTN_LEFT, 330],
                abs_pointer: Some(AbsPoint { x: 1800, y: 1900 }),
                contacts: vec![
                    finger(0, 200, 1800, 1900, 30, 260, 240),
                    finger(1, 201, 2600, 1900, 33, 250, 230)
                ],
                finger_count: Some(2),
                ..blank(4_000_020_000)
            },
            // The next event lands on the snapshot's current slot (1).
            Frame {
                held_buttons: vec![BTN_LEFT, 330],
                abs_pointer: Some(AbsPoint { x: 1800, y: 1900 }),
                contacts: vec![
                    finger(0, 200, 1800, 1900, 30, 260, 240),
                    finger(1, 201, 2610, 1900, 33, 250, 230)
                ],
                finger_count: Some(2),
                ..blank(4_000_100_000)
            },
        ]
    );
}

#[test]
fn resync_releases_a_key_released_during_the_drop() {
    let events = [
        event(EV_KEY, 30, 1),
        event(EV_SYN, SYN_REPORT, 0),
        event(EV_SYN, SYN_DROPPED, 0),
        event(EV_SYN, SYN_REPORT, 0),
    ];
    let snapshot = Snapshot {
        keys: BitSet::default(),
        switches: BitSet::default(),
        abs: vec![],
        mt: vec![],
    };
    let frames = drive(&keyboard(), &events, Some(&snapshot));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[1].keys, vec![key(30, KeyState::Released)]);
    assert!(frames[1].resynced);
}

#[test]
fn lid_and_tablet_mode_switches() {
    let device = info(&[], &[], &[SW_LID, SW_TABLET_MODE], vec![]);
    let events = [
        event(EV_SW, SW_LID, 1),
        event(EV_SYN, SYN_REPORT, 0),
        event(EV_SW, SW_TABLET_MODE, 1),
        event(EV_SW, SW_LID, 0),
        event(EV_SYN, SYN_REPORT, 0),
    ];
    let frames = drive(&device, &events, None);
    assert_eq!(
        frames[0].switches,
        vec![SwitchEvent {
            switch: Switch::Lid,
            on: true
        }]
    );
    assert_eq!(
        frames[1].switches,
        vec![
            SwitchEvent {
                switch: Switch::TabletMode,
                on: true
            },
            SwitchEvent {
                switch: Switch::Lid,
                on: false
            }
        ]
    );
}

#[test]
fn axis_geometry_converts_to_millimetres_only_with_a_resolution() {
    let pad = touchpad();
    assert_eq!(
        pad.abs(ABS_MT_POSITION_X).unwrap().units_to_mm(300),
        Some(10.0)
    );
    assert_eq!(tablet().abs(ABS_X).unwrap().resolution, Resolution::Unknown);
    assert_eq!(tablet().abs(ABS_X).unwrap().units_to_mm(300), None);
}

fn push_all(framer: &mut Framer, events: &[InputEvent]) -> Result<(), FrameError> {
    events
        .iter()
        .try_for_each(|event| framer.push(event).map(|_| ()))
}

#[test]
fn malformed_streams_are_errors() {
    let new = || Framer::new(Layout::from_info(&touchpad()).unwrap());
    assert_eq!(
        push_all(&mut new(), &[event(EV_KEY, 30, 3)]),
        Err(FrameError::InvalidKeyValue { code: 30, value: 3 })
    );
    assert_eq!(
        push_all(&mut new(), &[event(EV_ABS, ABS_MT_SLOT, 7)]),
        Err(FrameError::SlotOutOfRange {
            slot: 7,
            slot_count: 5
        })
    );
    assert_eq!(
        push_all(&mut new(), &[event(EV_SYN, SYN_MT_REPORT, 0)]),
        Err(FrameError::ProtocolA)
    );
    assert_eq!(
        push_all(&mut new(), &[event(EV_ABS, ABS_MT_TRACKING_ID, -2)]),
        Err(FrameError::InvalidTrackingId { value: -2 })
    );
    assert_eq!(
        push_all(&mut new(), &[event(0x40, 0, 0)]),
        Err(FrameError::UnknownEventType { kind: 0x40 })
    );
    // A multitouch axis on a device without multitouch.
    let mut plain = Framer::new(Layout::from_info(&mouse()).unwrap());
    assert_eq!(
        push_all(&mut plain, &[event(EV_ABS, ABS_MT_POSITION_X, 1)]),
        Err(FrameError::UnexpectedEvent {
            kind: EV_ABS,
            code: ABS_MT_POSITION_X
        })
    );
}

#[test]
fn contact_data_on_an_inactive_slot_is_ignored() {
    // Position data on a slot that never got a tracking id, then on a slot whose tracking id ends in the same frame.
    let events = [
        event(EV_ABS, ABS_MT_POSITION_X, 10),
        event(EV_SYN, SYN_REPORT, 0),
        event(EV_ABS, ABS_MT_TRACKING_ID, 7),
        event(EV_ABS, ABS_MT_POSITION_X, 20),
        event(EV_SYN, SYN_REPORT, 0),
        event(EV_ABS, ABS_MT_TRACKING_ID, -1),
        event(EV_ABS, ABS_MT_POSITION_X, 30),
        event(EV_SYN, SYN_REPORT, 0),
    ];
    let frames = drive(&touchpad(), &events, None);
    let contacts: Vec<usize> = frames.iter().map(|frame| frame.contacts.len()).collect();
    assert_eq!(contacts, vec![0, 1, 0]);
    assert_eq!(frames[1].contacts[0].x, 20);
}

fn timed(usec: i64, kind: u16, code: u16, value: i32) -> InputEvent {
    InputEvent {
        sec: 1,
        usec,
        kind,
        code,
        value,
    }
}

fn held_key_snapshot() -> Snapshot {
    Snapshot {
        keys: BitSet::from_codes(&[30]),
        switches: BitSet::default(),
        abs: vec![],
        mt: vec![],
    }
}

#[test]
fn events_queued_before_the_snapshot_are_replays_and_never_move_time_back() {
    // The snapshot (key 30 held) is stamped 1_000_500; the kernel had queued the press at 1_000_100 meanwhile.
    let mut framer = Framer::new(Layout::from_info(&keyboard()).unwrap());
    let resync = framer
        .apply_snapshot(&held_key_snapshot(), 1_000_500)
        .unwrap();
    assert_eq!(resync.keys, vec![key(30, KeyState::Pressed)]);
    assert_eq!(framer.push(&timed(100, EV_KEY, 30, 1)).unwrap(), None);
    let Some(FramerOutput::Frame(replayed)) =
        framer.push(&timed(100, EV_SYN, SYN_REPORT, 0)).unwrap()
    else {
        panic!("expected a frame");
    };
    assert_eq!(replayed.keys, vec![]);
    assert_eq!(replayed.held_buttons, Vec::<u16>::new());
    assert_eq!(replayed.time_us, 1_000_500);
    // A later release is a real transition and keeps its own time.
    framer.push(&timed(900, EV_KEY, 30, 0)).unwrap();
    let Some(FramerOutput::Frame(released)) =
        framer.push(&timed(900, EV_SYN, SYN_REPORT, 0)).unwrap()
    else {
        panic!("expected a frame");
    };
    assert_eq!(released.keys, vec![key(30, KeyState::Released)]);
    assert_eq!(released.time_us, 1_000_900);
}

#[test]
fn replaying_events_already_in_the_snapshot_changes_nothing() {
    let per_slot = |code, values: [i32; 5]| MtSlotValues {
        code,
        values: values.to_vec(),
    };
    let snapshot = Snapshot {
        keys: BitSet::from_codes(&[BTN_LEFT, BTN_TOOL_FINGER]),
        switches: BitSet::from_codes(&[SW_LID]),
        abs: vec![(ABS_X, 1800), (ABS_Y, 1900), (ABS_MT_SLOT, 0)],
        mt: vec![
            per_slot(ABS_MT_TRACKING_ID, [200, -1, -1, -1, -1]),
            per_slot(ABS_MT_POSITION_X, [1800, 0, 0, 0, 0]),
            per_slot(ABS_MT_POSITION_Y, [1900, 0, 0, 0, 0]),
            per_slot(ABS_MT_PRESSURE, [30, 0, 0, 0, 0]),
            per_slot(ABS_MT_TOUCH_MAJOR, [260, 0, 0, 0, 0]),
            per_slot(ABS_MT_TOUCH_MINOR, [240, 0, 0, 0, 0]),
            per_slot(ABS_MT_TOOL_TYPE, [0, 0, 0, 0, 0]),
        ],
    };
    let device = info(
        &[BTN_LEFT, BTN_TOOL_FINGER],
        &[],
        &[SW_LID],
        vec![
            axis(ABS_X, 4000, 40),
            axis(ABS_Y, 4000, 40),
            axis(ABS_MT_SLOT, 4, 0),
            axis(ABS_MT_TRACKING_ID, 65535, 0),
            axis(ABS_MT_POSITION_X, 4000, 40),
            axis(ABS_MT_POSITION_Y, 4000, 40),
            axis(ABS_MT_PRESSURE, 255, 0),
            axis(ABS_MT_TOUCH_MAJOR, 1000, 0),
            axis(ABS_MT_TOUCH_MINOR, 1000, 0),
            axis(ABS_MT_TOOL_TYPE, 2, 0),
        ],
    );
    let mut framer = Framer::new(Layout::from_info(&device).unwrap());
    framer.apply_snapshot(&snapshot, 5_000_000).unwrap();
    // The same click, switch, contact start and position that the snapshot already holds, then a stale lift of
    // another slot.
    let replay = [
        timed(1, EV_KEY, BTN_LEFT, 1),
        timed(1, EV_SW, SW_LID, 1),
        timed(1, EV_ABS, ABS_X, 1800),
        timed(1, EV_ABS, ABS_MT_SLOT, 0),
        timed(1, EV_ABS, ABS_MT_TRACKING_ID, 200),
        timed(1, EV_ABS, ABS_MT_POSITION_X, 1800),
        timed(1, EV_ABS, ABS_MT_SLOT, 1),
        timed(1, EV_ABS, ABS_MT_POSITION_X, 5),
        timed(1, EV_ABS, ABS_MT_TRACKING_ID, -1),
        timed(1, EV_SYN, SYN_REPORT, 0),
    ];
    let mut frames = Vec::new();
    for event in &replay {
        if let Some(FramerOutput::Frame(frame)) = framer.push(event).unwrap() {
            frames.push(frame);
        }
    }
    assert_eq!(frames.len(), 1);
    let frame = &frames[0];
    assert_eq!(frame.keys, vec![]);
    assert_eq!(frame.switches, vec![]);
    assert_eq!(frame.abs_pointer, Some(AbsPoint { x: 1800, y: 1900 }));
    assert_eq!(
        frame.contacts,
        vec![finger(0, 200, 1800, 1900, 30, 260, 240)]
    );
}

#[test]
fn protocol_a_devices_are_rejected_at_layout() {
    let protocol_a = info(
        &[],
        &[],
        &[],
        vec![
            axis(ABS_MT_POSITION_X, 100, 0),
            axis(ABS_MT_POSITION_Y, 100, 0),
        ],
    );
    assert_eq!(
        Layout::from_info(&protocol_a),
        Err(FrameError::MultitouchWithoutSlots)
    );
}
