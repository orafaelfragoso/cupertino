//! The framer: recorded event sequences in, frames out, with no device.

use cupertino_devices::evdev::codes::*;
use cupertino_devices::evdev::{
    AbsAxis, AbsInfo, AbsPoint, BitSet, Contact, DeviceId, DeviceInfo, Frame, FrameError, Framer,
    FramerOutput, InputEvent, KeyEvent, KeyState, Layout, Motion, MtSlotValues, Resolution,
    Snapshot, Switch, SwitchEvent, Tool, Wheel,
};

const MT_SLOTS: i32 = 5;

#[derive(Default)]
struct Spec {
    keys: Vec<u16>,
    rel: Vec<u16>,
    abs: Vec<u16>,
    switches: Vec<u16>,
    slot_max: Option<i32>,
}

fn abs_axis(code: u16, max: i32) -> AbsAxis {
    AbsAxis {
        code,
        info: AbsInfo {
            value: 0,
            min: -1,
            max,
            fuzz: 0,
            flat: 0,
            resolution: Resolution::Unknown,
        },
    }
}

fn info(spec: &Spec) -> DeviceInfo {
    let mut abs: Vec<u16> = spec.abs.clone();
    abs.sort_unstable();
    DeviceInfo {
        name: "fixture".into(),
        id: DeviceId {
            bustype: 0x19,
            vendor: 1,
            product: 1,
            version: 1,
        },
        properties: BitSet::default(),
        event_types: BitSet::default(),
        keys: BitSet::from_codes(&spec.keys),
        rel_axes: BitSet::from_codes(&spec.rel),
        switches: BitSet::from_codes(&spec.switches),
        abs_axes: abs
            .into_iter()
            .map(|code| {
                let max = if code == ABS_MT_SLOT {
                    spec.slot_max.unwrap_or(MT_SLOTS - 1)
                } else {
                    4000
                };
                abs_axis(code, max)
            })
            .collect(),
    }
}

fn layout(spec: &Spec) -> Layout {
    Layout::from_info(&info(spec)).expect("a valid layout")
}

fn mouse_spec() -> Spec {
    Spec {
        keys: vec![BTN_LEFT, BTN_RIGHT],
        rel: vec![REL_X, REL_Y, REL_WHEEL, REL_HWHEEL],
        ..Spec::default()
    }
}

fn touch_spec() -> Spec {
    Spec {
        keys: vec![
            BTN_LEFT,
            BTN_TOUCH,
            BTN_TOOL_FINGER,
            BTN_TOOL_DOUBLETAP,
            BTN_TOOL_TRIPLETAP,
        ],
        abs: vec![
            ABS_X,
            ABS_Y,
            ABS_MT_SLOT,
            ABS_MT_POSITION_X,
            ABS_MT_POSITION_Y,
            ABS_MT_TRACKING_ID,
            ABS_MT_PRESSURE,
        ],
        ..Spec::default()
    }
}

/// A framer with a running clock: each `SYN_REPORT` is stamped 1000 microseconds after the last.
struct Rig {
    framer: Framer,
    clock: i64,
}

type Ev = (u16, u16, i32);

const SYN: Ev = (EV_SYN, SYN_REPORT, 0);

fn key(code: u16, value: i32) -> Ev {
    (EV_KEY, code, value)
}
fn rel(code: u16, value: i32) -> Ev {
    (EV_REL, code, value)
}
fn abs(code: u16, value: i32) -> Ev {
    (EV_ABS, code, value)
}

impl Rig {
    fn new(spec: &Spec) -> Self {
        Self {
            framer: Framer::new(layout(spec)),
            clock: 1_000_000,
        }
    }

    fn event(&mut self, (kind, code, value): Ev) -> InputEvent {
        if (kind, code) == (EV_SYN, SYN_REPORT) {
            self.clock += 1000;
        }
        InputEvent {
            sec: self.clock / 1_000_000,
            usec: self.clock % 1_000_000,
            kind,
            code,
            value,
        }
    }

    fn push(&mut self, ev: Ev) -> Result<Option<FramerOutput>, FrameError> {
        let event = self.event(ev);
        self.framer.push(&event)
    }

    /// Pushes events up to and including the next `SYN_REPORT` and returns its frame.
    fn frame(&mut self, events: &[Ev]) -> Frame {
        let mut last = None;
        for &ev in events.iter().chain(std::iter::once(&SYN)) {
            last = self.push(ev).expect("valid events");
        }
        match last {
            Some(FramerOutput::Frame(frame)) => frame,
            other => panic!("expected a frame, got {other:?}"),
        }
    }

    fn error(&mut self, events: &[Ev]) -> FrameError {
        for &ev in events {
            if let Err(error) = self.push(ev) {
                return error;
            }
        }
        panic!("no error from {events:?}");
    }
}

#[test]
fn a_synthesis_report_alone_is_an_empty_frame() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = rig.frame(&[]);
    assert_eq!(frame.time_us, 1_001_000);
    assert!(!frame.resynced);
    assert!(frame.keys.is_empty() && frame.switches.is_empty() && frame.contacts.is_empty());
    assert_eq!(frame.motion, Motion::default());
    assert_eq!(frame.wheel, Wheel::default());
    assert_eq!(frame.abs_pointer, None);
    assert_eq!(frame.finger_count, None);
    assert!(frame.held_buttons.is_empty());
}

#[test]
fn events_before_the_report_are_not_a_frame() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(rig.push(rel(REL_X, 3)), Ok(None));
    assert_eq!(rig.push(key(BTN_LEFT, 1)), Ok(None));
}

#[test]
fn relative_motion_is_summed_within_a_frame_and_reset_after() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = rig.frame(&[rel(REL_X, 3), rel(REL_Y, -2), rel(REL_X, 4), rel(REL_Y, -1)]);
    assert_eq!(frame.motion, Motion { dx: 7, dy: -3 });
    assert_eq!(rig.frame(&[]).motion, Motion::default());
}

#[test]
fn relative_motion_saturates_instead_of_wrapping() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = rig.frame(&[
        rel(REL_X, i32::MAX),
        rel(REL_X, 10),
        rel(REL_Y, i32::MIN),
        rel(REL_Y, -10),
    ]);
    assert_eq!(
        frame.motion,
        Motion {
            dx: i32::MAX,
            dy: i32::MIN
        }
    );
}

#[test]
fn a_legacy_wheel_step_is_120() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = rig.frame(&[rel(REL_WHEEL, 1), rel(REL_WHEEL, 2), rel(REL_HWHEEL, -1)]);
    assert_eq!(
        frame.wheel,
        Wheel {
            vertical: 360,
            horizontal: -120
        }
    );
    let frame = rig.frame(&[rel(REL_WHEEL, i32::MAX)]);
    assert_eq!(frame.wheel.vertical, i32::MAX);
}

#[test]
fn a_high_resolution_wheel_is_read_through_its_own_axis_only() {
    let spec = Spec {
        keys: vec![BTN_LEFT],
        rel: vec![
            REL_X,
            REL_Y,
            REL_WHEEL,
            REL_HWHEEL,
            REL_WHEEL_HI_RES,
            REL_HWHEEL_HI_RES,
        ],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    // the kernel sends both for one detent; counting both would double it
    let frame = rig.frame(&[
        rel(REL_WHEEL_HI_RES, 60),
        rel(REL_WHEEL_HI_RES, 60),
        rel(REL_WHEEL, 1),
    ]);
    assert_eq!(frame.wheel.vertical, 120);
    let frame = rig.frame(&[rel(REL_HWHEEL_HI_RES, -30), rel(REL_HWHEEL, -1)]);
    assert_eq!(
        frame.wheel,
        Wheel {
            vertical: 0,
            horizontal: -30
        }
    );
}

#[test]
fn only_the_declared_wheel_axis_goes_high_resolution() {
    let spec = Spec {
        rel: vec![REL_WHEEL, REL_HWHEEL, REL_WHEEL_HI_RES],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    let frame = rig.frame(&[
        rel(REL_WHEEL_HI_RES, 40),
        rel(REL_WHEEL, 1),
        rel(REL_HWHEEL, 1),
    ]);
    assert_eq!(
        frame.wheel,
        Wheel {
            vertical: 40,
            horizontal: 120
        }
    );
}

#[test]
fn a_high_resolution_wheel_the_device_never_declared_is_an_error() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[rel(REL_WHEEL_HI_RES, 1)]),
        FrameError::UnexpectedEvent {
            kind: EV_REL,
            code: REL_WHEEL_HI_RES
        }
    );
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[rel(REL_HWHEEL_HI_RES, 1)]),
        FrameError::UnexpectedEvent {
            kind: EV_REL,
            code: REL_HWHEEL_HI_RES
        }
    );
}

#[test]
fn relative_axes_the_framer_does_not_model_are_skipped() {
    let mut rig = Rig::new(&mouse_spec());
    // REL_DIAL, REL_MISC
    let frame = rig.frame(&[rel(0x07, 5), rel(0x09, 5), rel(REL_X, 1)]);
    assert_eq!(frame.motion, Motion { dx: 1, dy: 0 });
    assert_eq!(frame.wheel, Wheel::default());
}

#[test]
fn key_transitions_come_in_event_order() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = rig.frame(&[key(30, 1), key(BTN_LEFT, 1), key(30, 0)]);
    assert_eq!(
        frame.keys,
        [
            KeyEvent {
                code: 30,
                state: KeyState::Pressed
            },
            KeyEvent {
                code: BTN_LEFT,
                state: KeyState::Pressed
            },
            KeyEvent {
                code: 30,
                state: KeyState::Released
            },
        ]
    );
}

#[test]
fn held_buttons_are_the_state_after_the_frame_and_exclude_keyboard_keys() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = rig.frame(&[key(BTN_RIGHT, 1), key(30, 1), key(BTN_LEFT, 1)]);
    assert_eq!(frame.held_buttons, [BTN_LEFT, BTN_RIGHT]);
    let frame = rig.frame(&[key(BTN_LEFT, 0)]);
    assert_eq!(frame.held_buttons, [BTN_RIGHT]);
    assert!(frame.keys.len() == 1);
    let frame = rig.frame(&[]);
    assert_eq!(
        frame.held_buttons,
        [BTN_RIGHT],
        "a held button stays held in the frames after"
    );
    assert!(frame.keys.is_empty());
    assert!(rig.frame(&[key(BTN_RIGHT, 0)]).held_buttons.is_empty());
}

#[test]
fn auto_repeat_is_a_transition_that_changes_no_state() {
    let mut rig = Rig::new(&mouse_spec());
    rig.frame(&[key(30, 1)]);
    let frame = rig.frame(&[key(30, 2), key(30, 2)]);
    assert_eq!(
        frame.keys,
        [
            KeyEvent {
                code: 30,
                state: KeyState::Repeat
            },
            KeyEvent {
                code: 30,
                state: KeyState::Repeat
            }
        ]
    );
    let frame = rig.frame(&[key(30, 0)]);
    assert_eq!(
        frame.keys,
        [KeyEvent {
            code: 30,
            state: KeyState::Released
        }]
    );
}

#[test]
fn a_press_of_a_held_key_and_a_release_of_a_released_one_are_replays() {
    let mut rig = Rig::new(&mouse_spec());
    assert!(rig.frame(&[key(30, 0)]).keys.is_empty());
    rig.frame(&[key(30, 1)]);
    assert!(rig.frame(&[key(30, 1)]).keys.is_empty());
    assert_eq!(rig.frame(&[key(30, 0)]).keys.len(), 1);
}

#[test]
fn a_key_value_other_than_0_1_2_is_an_error() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[key(30, 3)]),
        FrameError::InvalidKeyValue { code: 30, value: 3 }
    );
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[key(30, -1)]),
        FrameError::InvalidKeyValue {
            code: 30,
            value: -1
        }
    );
}

#[test]
fn a_key_code_past_the_range_is_an_error() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[key(KEY_MAX + 1, 1)]),
        FrameError::UnexpectedEvent {
            kind: EV_KEY,
            code: KEY_MAX + 1
        }
    );
}

#[test]
fn the_highest_key_code_is_accepted() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = rig.frame(&[key(KEY_MAX, 1)]);
    assert_eq!(frame.keys.len(), 1);
}

#[test]
fn finger_tools_are_a_count_not_key_events() {
    let mut rig = Rig::new(&touch_spec());
    let frame = rig.frame(&[abs(ABS_X, 1), abs(ABS_Y, 1), key(BTN_TOOL_FINGER, 1)]);
    assert_eq!(frame.finger_count, Some(1));
    assert!(frame.keys.is_empty());
    assert!(frame.held_buttons.is_empty());

    // the driver presses the new count, then releases the old one
    let frame = rig.frame(&[key(BTN_TOOL_DOUBLETAP, 1), key(BTN_TOOL_FINGER, 0)]);
    assert_eq!(frame.finger_count, Some(2));
    let frame = rig.frame(&[key(BTN_TOOL_TRIPLETAP, 1), key(BTN_TOOL_DOUBLETAP, 0)]);
    assert_eq!(frame.finger_count, Some(3));
    let frame = rig.frame(&[key(BTN_TOOL_TRIPLETAP, 0)]);
    assert_eq!(frame.finger_count, Some(0));
    assert_eq!(rig.frame(&[]).finger_count, Some(0));
}

#[test]
fn a_finger_tool_on_a_device_without_them_is_an_error() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[key(BTN_TOOL_FINGER, 1)]),
        FrameError::UnexpectedEvent {
            kind: EV_KEY,
            code: BTN_TOOL_FINGER
        }
    );
}

#[test]
fn the_touch_button_is_an_ordinary_held_button() {
    let mut rig = Rig::new(&touch_spec());
    let frame = rig.frame(&[key(BTN_TOUCH, 1), key(BTN_LEFT, 1)]);
    assert_eq!(frame.held_buttons, [BTN_LEFT, BTN_TOUCH]);
}

#[test]
fn the_absolute_pointer_is_present_with_both_axes_and_keeps_its_value() {
    let spec = Spec {
        keys: vec![BTN_LEFT],
        abs: vec![ABS_X, ABS_Y],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    assert_eq!(rig.frame(&[]).abs_pointer, Some(AbsPoint { x: 0, y: 0 }));
    assert_eq!(
        rig.frame(&[abs(ABS_X, 120), abs(ABS_Y, -4)]).abs_pointer,
        Some(AbsPoint { x: 120, y: -4 })
    );
    assert_eq!(
        rig.frame(&[abs(ABS_X, 7)]).abs_pointer,
        Some(AbsPoint { x: 7, y: -4 })
    );
    assert_eq!(rig.frame(&[]).abs_pointer, Some(AbsPoint { x: 7, y: -4 }));
}

#[test]
fn one_absolute_axis_is_not_a_pointer() {
    let spec = Spec {
        abs: vec![ABS_X],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    assert_eq!(rig.frame(&[]).abs_pointer, None);
    assert_eq!(
        rig.error(&[abs(ABS_X, 1)]),
        FrameError::UnexpectedEvent {
            kind: EV_ABS,
            code: ABS_X
        }
    );
}

#[test]
fn absolute_axes_the_framer_does_not_model_are_skipped() {
    let spec = Spec {
        abs: vec![ABS_X, ABS_Y, ABS_PRESSURE],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    let frame = rig.frame(&[abs(ABS_PRESSURE, 400), abs(ABS_X, 5)]);
    assert_eq!(frame.abs_pointer, Some(AbsPoint { x: 5, y: 0 }));
}

#[test]
fn events_outside_the_modelled_types_are_skipped() {
    let mut rig = Rig::new(&mouse_spec());
    // EV_MSC scancode, EV_LED, EV_REP, EV_FF, SYN_CONFIG
    let frame = rig.frame(&[
        (0x04, 0x04, 458_792),
        (0x11, 0, 1),
        (0x14, 0, 250),
        (0x15, 0, 1),
        (EV_SYN, 1, 0),
        rel(REL_X, 2),
    ]);
    assert_eq!(frame.motion.dx, 2);
    assert!(frame.keys.is_empty());
}

#[test]
fn an_event_type_above_the_maximum_is_an_error() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[(0x20, 0, 0)]),
        FrameError::UnknownEventType { kind: 0x20 }
    );
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[(0xffff, 0, 0)]),
        FrameError::UnknownEventType { kind: 0xffff }
    );
    // the last valid type is not
    let mut rig = Rig::new(&mouse_spec());
    assert!(rig.push((EV_MAX, 0, 0)).is_ok());
}

#[test]
fn the_multitouch_report_of_protocol_a_is_an_error() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[(EV_SYN, SYN_MT_REPORT, 0)]),
        FrameError::ProtocolA
    );
}

#[test]
fn switches_report_their_transitions() {
    let spec = Spec {
        switches: vec![SW_LID, SW_TABLET_MODE],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    let frame = rig.frame(&[(EV_SW, SW_LID, 1)]);
    assert_eq!(
        frame.switches,
        [SwitchEvent {
            switch: Switch::Lid,
            on: true
        }]
    );
    let frame = rig.frame(&[(EV_SW, SW_TABLET_MODE, 1), (EV_SW, SW_LID, 0)]);
    assert_eq!(
        frame.switches,
        [
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
    assert!(rig.frame(&[]).switches.is_empty());
}

#[test]
fn a_switch_event_that_matches_the_known_state_is_not_a_transition() {
    let spec = Spec {
        switches: vec![SW_LID],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    assert!(rig.frame(&[(EV_SW, SW_LID, 0)]).switches.is_empty());
    rig.frame(&[(EV_SW, SW_LID, 1)]);
    assert!(rig.frame(&[(EV_SW, SW_LID, 1)]).switches.is_empty());
}

#[test]
fn a_switch_value_other_than_0_or_1_is_an_error_and_other_switches_are_skipped() {
    let spec = Spec {
        switches: vec![SW_LID],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    assert_eq!(
        rig.error(&[(EV_SW, SW_LID, 2)]),
        FrameError::InvalidSwitchValue {
            code: SW_LID,
            value: 2
        }
    );
    let mut rig = Rig::new(&spec);
    // SW_HEADPHONE_INSERT is not one the framer reports
    assert!(rig.frame(&[(EV_SW, 0x02, 1)]).switches.is_empty());
}

#[test]
fn frame_times_are_the_reports_and_never_go_back() {
    let mut rig = Rig::new(&mouse_spec());
    let first = rig.frame(&[]);
    let second = rig.frame(&[]);
    assert!(second.time_us > first.time_us);
    // a report stamped earlier than the last one is clamped up to it, keeping its changes
    let early = InputEvent {
        sec: 0,
        usec: 5,
        kind: EV_REL,
        code: REL_X,
        value: 9,
    };
    rig.framer.push(&early).unwrap();
    let report = InputEvent {
        sec: 0,
        usec: 10,
        kind: EV_SYN,
        code: SYN_REPORT,
        value: 0,
    };
    match rig.framer.push(&report).unwrap() {
        Some(FramerOutput::Frame(frame)) => {
            assert_eq!(frame.time_us, second.time_us);
            assert_eq!(frame.motion.dx, 9);
        }
        other => panic!("{other:?}"),
    }
}

fn contact(slot: u32, id: u32, x: i32, y: i32, pressure: i32) -> Contact {
    Contact {
        slot,
        tracking_id: id,
        x,
        y,
        pressure: Some(pressure),
        touch_major: None,
        touch_minor: None,
        tool: Tool::Finger,
    }
}

fn touch(slot: i32, id: i32, x: i32, y: i32, pressure: i32) -> Vec<Ev> {
    vec![
        abs(ABS_MT_SLOT, slot),
        abs(ABS_MT_TRACKING_ID, id),
        abs(ABS_MT_POSITION_X, x),
        abs(ABS_MT_POSITION_Y, y),
        abs(ABS_MT_PRESSURE, pressure),
    ]
}

#[test]
fn a_contact_starts_moves_and_ends() {
    let mut rig = Rig::new(&touch_spec());
    let frame = rig.frame(&touch(0, 17, 100, 200, 30));
    assert_eq!(frame.contacts, [contact(0, 17, 100, 200, 30)]);

    let frame = rig.frame(&[abs(ABS_MT_POSITION_X, 110)]);
    assert_eq!(
        frame.contacts,
        [contact(0, 17, 110, 200, 30)],
        "a contact stays in every frame while down"
    );

    let frame = rig.frame(&[abs(ABS_MT_TRACKING_ID, -1)]);
    assert!(frame.contacts.is_empty());
    assert!(rig.frame(&[]).contacts.is_empty());
}

#[test]
fn contacts_are_listed_by_slot_whatever_order_they_arrive_in() {
    let mut rig = Rig::new(&touch_spec());
    let mut events = touch(3, 40, 30, 30, 1);
    events.extend(touch(1, 41, 10, 10, 2));
    events.extend(touch(2, 42, 20, 20, 3));
    let frame = rig.frame(&events);
    let slots: Vec<_> = frame
        .contacts
        .iter()
        .map(|c| (c.slot, c.tracking_id))
        .collect();
    assert_eq!(slots, [(1, 41), (2, 42), (3, 40)]);
}

#[test]
fn data_applies_to_the_slot_last_selected() {
    let mut rig = Rig::new(&touch_spec());
    let mut events = touch(0, 1, 5, 5, 1);
    events.extend(touch(1, 2, 6, 6, 1));
    rig.frame(&events);
    // no slot event: the data belongs to slot 1
    let frame = rig.frame(&[abs(ABS_MT_POSITION_Y, 99)]);
    assert_eq!(frame.contacts[0].y, 5);
    assert_eq!(frame.contacts[1].y, 99);
    let frame = rig.frame(&[abs(ABS_MT_SLOT, 0), abs(ABS_MT_POSITION_Y, 77)]);
    assert_eq!(frame.contacts[0].y, 77);
}

#[test]
fn a_slot_is_reused_with_a_new_tracking_id() {
    let mut rig = Rig::new(&touch_spec());
    rig.frame(&touch(0, 5, 1, 1, 1));
    rig.frame(&[abs(ABS_MT_TRACKING_ID, -1)]);
    let frame = rig.frame(&[
        abs(ABS_MT_TRACKING_ID, 6),
        abs(ABS_MT_POSITION_X, 9),
        abs(ABS_MT_POSITION_Y, 9),
    ]);
    assert_eq!(frame.contacts.len(), 1);
    assert_eq!(frame.contacts[0].tracking_id, 6);
    assert_eq!(frame.contacts[0].slot, 0);
}

#[test]
fn contact_data_on_an_inactive_slot_makes_no_contact() {
    let mut rig = Rig::new(&touch_spec());
    let frame = rig.frame(&[
        abs(ABS_MT_POSITION_X, 5),
        abs(ABS_MT_POSITION_Y, 5),
        abs(ABS_MT_PRESSURE, 9),
    ]);
    assert!(frame.contacts.is_empty());
    // ended and updated in the same frame
    rig.frame(&touch(0, 1, 1, 1, 1));
    let frame = rig.frame(&[abs(ABS_MT_TRACKING_ID, -1), abs(ABS_MT_POSITION_X, 50)]);
    assert!(frame.contacts.is_empty());
}

#[test]
fn a_slot_past_the_declared_count_is_an_error() {
    let mut rig = Rig::new(&touch_spec());
    assert_eq!(
        rig.error(&[abs(ABS_MT_SLOT, MT_SLOTS)]),
        FrameError::SlotOutOfRange {
            slot: MT_SLOTS,
            slot_count: MT_SLOTS as u32
        }
    );
    let mut rig = Rig::new(&touch_spec());
    assert_eq!(
        rig.error(&[abs(ABS_MT_SLOT, -1)]),
        FrameError::SlotOutOfRange {
            slot: -1,
            slot_count: 5
        }
    );
    let mut rig = Rig::new(&touch_spec());
    assert!(rig.push(abs(ABS_MT_SLOT, MT_SLOTS - 1)).is_ok());
}

#[test]
fn a_tracking_id_below_minus_one_is_an_error() {
    let mut rig = Rig::new(&touch_spec());
    assert_eq!(
        rig.error(&[abs(ABS_MT_TRACKING_ID, -2)]),
        FrameError::InvalidTrackingId { value: -2 }
    );
}

#[test]
fn a_multitouch_axis_the_device_never_declared_is_an_error() {
    let mut rig = Rig::new(&touch_spec());
    assert_eq!(
        rig.error(&[abs(ABS_MT_TOUCH_MAJOR, 4)]),
        FrameError::UnexpectedEvent {
            kind: EV_ABS,
            code: ABS_MT_TOUCH_MAJOR
        }
    );
    let mut rig = Rig::new(&touch_spec());
    assert_eq!(
        rig.error(&[abs(ABS_MT_TOOL_TYPE, 0)]),
        FrameError::UnexpectedEvent {
            kind: EV_ABS,
            code: ABS_MT_TOOL_TYPE
        }
    );
    // and any multitouch event on a device with no slots
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[abs(ABS_MT_SLOT, 0)]),
        FrameError::UnexpectedEvent {
            kind: EV_ABS,
            code: ABS_MT_SLOT
        }
    );
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.error(&[abs(ABS_MT_POSITION_X, 0)]),
        FrameError::UnexpectedEvent {
            kind: EV_ABS,
            code: ABS_MT_POSITION_X
        }
    );
}

#[test]
fn optional_contact_axes_appear_only_when_the_device_has_them() {
    let spec = Spec {
        abs: vec![
            ABS_MT_SLOT,
            ABS_MT_POSITION_X,
            ABS_MT_POSITION_Y,
            ABS_MT_TRACKING_ID,
            ABS_MT_TOUCH_MAJOR,
            ABS_MT_TOUCH_MINOR,
            ABS_MT_TOOL_TYPE,
        ],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    let frame = rig.frame(&[
        abs(ABS_MT_SLOT, 0),
        abs(ABS_MT_TRACKING_ID, 3),
        abs(ABS_MT_POSITION_X, 10),
        abs(ABS_MT_POSITION_Y, 20),
        abs(ABS_MT_TOUCH_MAJOR, 8),
        abs(ABS_MT_TOUCH_MINOR, 6),
        abs(ABS_MT_TOOL_TYPE, MT_TOOL_PALM),
    ]);
    assert_eq!(
        frame.contacts,
        [Contact {
            slot: 0,
            tracking_id: 3,
            x: 10,
            y: 20,
            pressure: None,
            touch_major: Some(8),
            touch_minor: Some(6),
            tool: Tool::Palm
        }]
    );
    let frame = rig.frame(&[abs(ABS_MT_TOOL_TYPE, 7)]);
    assert_eq!(frame.contacts[0].tool, Tool::Other(7));
    let frame = rig.frame(&[abs(ABS_MT_TOOL_TYPE, MT_TOOL_PEN)]);
    assert_eq!(frame.contacts[0].tool, Tool::Pen);
}

#[test]
fn a_contact_defaults_to_a_finger_without_a_tool_axis() {
    let mut rig = Rig::new(&touch_spec());
    assert_eq!(
        rig.frame(&touch(0, 1, 1, 1, 1)).contacts[0].tool,
        Tool::Finger
    );
}

#[test]
fn more_fingers_than_slots_show_in_the_count() {
    let spec = Spec {
        slot_max: Some(1),
        ..touch_spec()
    };
    let mut rig = Rig::new(&spec);
    let mut events = touch(0, 1, 1, 1, 1);
    events.extend(touch(1, 2, 2, 2, 1));
    events.push(key(BTN_TOOL_TRIPLETAP, 1));
    let frame = rig.frame(&events);
    assert_eq!(frame.contacts.len(), 2);
    assert_eq!(frame.finger_count, Some(3));
}

#[test]
fn layout_is_derived_from_the_declared_capabilities() {
    let plain = layout(&mouse_spec());
    assert_eq!(plain.slot_count, 0);
    assert!(!plain.has_abs_xy && !plain.has_finger_tools && !plain.hi_res_vertical);
    assert!(plain.snapshot_abs_codes().is_empty() && plain.snapshot_mt_codes().is_empty());

    let touch = layout(&touch_spec());
    assert_eq!(touch.slot_count, 5);
    assert!(touch.has_abs_xy && touch.has_pressure && touch.has_finger_tools);
    assert!(!touch.has_touch_major && !touch.has_touch_minor && !touch.has_tool_type);
    assert_eq!(touch.snapshot_abs_codes(), [ABS_X, ABS_Y, ABS_MT_SLOT]);
    assert_eq!(
        touch.snapshot_mt_codes(),
        [
            ABS_MT_TRACKING_ID,
            ABS_MT_POSITION_X,
            ABS_MT_POSITION_Y,
            ABS_MT_PRESSURE
        ]
    );
}

#[test]
fn a_framer_reports_the_layout_it_was_built_with() {
    let l = layout(&touch_spec());
    assert_eq!(*Framer::new(l).layout(), l);
}

#[test]
fn multitouch_without_slots_is_protocol_a() {
    let spec = Spec {
        abs: vec![ABS_MT_POSITION_X, ABS_MT_POSITION_Y],
        ..Spec::default()
    };
    assert_eq!(
        Layout::from_info(&info(&spec)),
        Err(FrameError::MultitouchWithoutSlots)
    );
}

#[test]
fn slots_without_positions_or_tracking_ids_are_refused() {
    for missing in [ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TRACKING_ID] {
        let mut spec = touch_spec();
        spec.abs.retain(|&c| c != missing);
        assert_eq!(
            Layout::from_info(&info(&spec)),
            Err(FrameError::MultitouchWithoutPosition),
            "without {missing:#x}"
        );
    }
}

#[test]
fn a_slot_range_with_no_slots_is_refused() {
    let spec = Spec {
        slot_max: Some(-1),
        ..touch_spec()
    };
    assert_eq!(
        Layout::from_info(&info(&spec)),
        Err(FrameError::EmptySlotRange)
    );
    let spec = Spec {
        slot_max: Some(i32::MIN),
        ..touch_spec()
    };
    assert_eq!(
        Layout::from_info(&info(&spec)),
        Err(FrameError::EmptySlotRange)
    );
    let spec = Spec {
        slot_max: Some(0),
        ..touch_spec()
    };
    assert_eq!(layout(&spec).slot_count, 1);
}

#[test]
fn a_device_with_the_largest_slot_range_does_not_overflow() {
    let spec = Spec {
        slot_max: Some(i32::MAX),
        ..touch_spec()
    };
    assert_eq!(layout(&spec).slot_count, i32::MAX as u32 + 1);
}

fn bits(codes: &[u16]) -> BitSet {
    BitSet::from_codes(codes)
}

fn snapshot_for(spec: &Spec) -> Snapshot {
    let l = layout(spec);
    Snapshot {
        keys: BitSet::default(),
        switches: BitSet::default(),
        abs: l.snapshot_abs_codes().into_iter().map(|c| (c, 0)).collect(),
        mt: l
            .snapshot_mt_codes()
            .into_iter()
            .map(|code| MtSlotValues {
                code,
                values: vec![
                    if code == ABS_MT_TRACKING_ID { -1 } else { 0 };
                    l.slot_count as usize
                ],
            })
            .collect(),
    }
}

fn resync(rig: &mut Rig, snapshot: &Snapshot) -> Frame {
    rig.clock += 1000;
    rig.framer
        .apply_snapshot(snapshot, rig.clock)
        .expect("a valid snapshot")
}

#[test]
fn a_dropped_stream_discards_the_partial_frame_and_asks_for_a_resync_at_the_next_report() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(rig.push(rel(REL_X, 50)), Ok(None));
    assert_eq!(rig.push(key(BTN_LEFT, 1)), Ok(None));
    assert_eq!(rig.push((EV_SYN, SYN_DROPPED, 0)), Ok(None));
    // everything up to the report is thrown away, even what would be an error
    assert_eq!(rig.push(rel(REL_Y, 5)), Ok(None));
    assert_eq!(rig.push((0x20, 0, 0)), Ok(None));
    let at = rig.clock + 1000;
    match rig.push(SYN).unwrap() {
        Some(FramerOutput::ResyncNeeded { time_us }) => assert_eq!(time_us, at),
        other => panic!("{other:?}"),
    }
    // the caller reads the device's state: nothing is held now, and none of the discarded motion came through
    let frame = resync(&mut rig, &snapshot_for(&mouse_spec()));
    assert!(frame.resynced);
    assert_eq!(frame.motion, Motion::default());
    assert!(frame.held_buttons.is_empty());
    // the discarded press was never reported, so there is nothing to release
    assert!(frame.keys.is_empty());
    let frame = rig.frame(&[rel(REL_X, 1)]);
    assert_eq!(frame.motion, Motion { dx: 1, dy: 0 });
}

fn drop_and_resync(rig: &mut Rig, snapshot: &Snapshot) -> Frame {
    rig.push((EV_SYN, SYN_DROPPED, 0)).unwrap();
    assert!(matches!(
        rig.push(SYN).unwrap(),
        Some(FramerOutput::ResyncNeeded { .. })
    ));
    resync(rig, snapshot)
}

#[test]
fn a_press_discarded_by_a_drop_and_up_in_the_snapshot_is_no_transition() {
    let mut rig = Rig::new(&mouse_spec());
    rig.push(key(BTN_LEFT, 1)).unwrap();
    let frame = drop_and_resync(&mut rig, &snapshot_for(&mouse_spec()));
    assert!(frame.keys.is_empty());
    assert!(frame.held_buttons.is_empty());
}

#[test]
fn a_press_discarded_by_a_drop_and_held_in_the_snapshot_comes_out_as_a_press() {
    let mut rig = Rig::new(&mouse_spec());
    rig.push(key(BTN_LEFT, 1)).unwrap();
    let snapshot = Snapshot {
        keys: bits(&[BTN_LEFT]),
        ..snapshot_for(&mouse_spec())
    };
    let frame = drop_and_resync(&mut rig, &snapshot);
    assert_eq!(
        frame.keys,
        [KeyEvent {
            code: BTN_LEFT,
            state: KeyState::Pressed
        }]
    );
}

#[test]
fn a_reported_press_whose_release_is_discarded_is_released_by_the_snapshot() {
    let mut rig = Rig::new(&mouse_spec());
    rig.frame(&[key(BTN_LEFT, 1)]);
    rig.push(key(BTN_LEFT, 0)).unwrap();
    let frame = drop_and_resync(&mut rig, &snapshot_for(&mouse_spec()));
    assert_eq!(
        frame.keys,
        [KeyEvent {
            code: BTN_LEFT,
            state: KeyState::Released
        }]
    );
    assert!(frame.held_buttons.is_empty());
}

#[test]
fn a_switch_change_discarded_by_a_drop_comes_out_of_the_snapshot() {
    let spec = Spec {
        switches: vec![SW_LID],
        ..mouse_spec()
    };
    let mut rig = Rig::new(&spec);
    rig.push((EV_SW, SW_LID, 1)).unwrap();
    let snapshot = Snapshot {
        switches: bits(&[SW_LID]),
        ..snapshot_for(&spec)
    };
    let frame = drop_and_resync(&mut rig, &snapshot);
    assert_eq!(
        frame.switches,
        [SwitchEvent {
            switch: Switch::Lid,
            on: true
        }]
    );
}

#[test]
fn a_snapshot_over_a_partial_release_reports_the_release() {
    let mut rig = Rig::new(&mouse_spec());
    rig.frame(&[key(BTN_LEFT, 1)]);
    rig.push(key(BTN_LEFT, 0)).unwrap();
    let frame = resync(&mut rig, &snapshot_for(&mouse_spec()));
    assert_eq!(
        frame.keys,
        [KeyEvent {
            code: BTN_LEFT,
            state: KeyState::Released
        }]
    );
    assert!(frame.held_buttons.is_empty());
}

#[test]
fn a_second_drop_before_the_report_changes_nothing() {
    let mut rig = Rig::new(&mouse_spec());
    rig.push((EV_SYN, SYN_DROPPED, 0)).unwrap();
    rig.push(rel(REL_X, 1)).unwrap();
    rig.push((EV_SYN, SYN_DROPPED, 0)).unwrap();
    assert!(matches!(
        rig.push(SYN).unwrap(),
        Some(FramerOutput::ResyncNeeded { .. })
    ));
    assert!(matches!(
        rig.push(SYN).unwrap(),
        Some(FramerOutput::Frame(_))
    ));
}

#[test]
fn a_snapshot_makes_a_resynced_frame_of_the_difference() {
    let mut rig = Rig::new(&mouse_spec());
    rig.frame(&[key(30, 1), key(BTN_LEFT, 1)]);
    // the device now holds BTN_RIGHT and KEY_B (48); KEY_A and BTN_LEFT were released while events were lost
    let snapshot = Snapshot {
        keys: bits(&[BTN_RIGHT, 48]),
        ..snapshot_for(&mouse_spec())
    };
    let frame = resync(&mut rig, &snapshot);
    assert!(frame.resynced);
    assert_eq!(frame.motion, Motion::default());
    assert_eq!(
        frame.keys,
        [
            KeyEvent {
                code: 30,
                state: KeyState::Released
            },
            KeyEvent {
                code: 48,
                state: KeyState::Pressed
            },
            KeyEvent {
                code: BTN_LEFT,
                state: KeyState::Released
            },
            KeyEvent {
                code: BTN_RIGHT,
                state: KeyState::Pressed
            },
        ]
    );
    assert_eq!(frame.held_buttons, [BTN_RIGHT]);
}

#[test]
fn a_snapshot_of_an_unchanged_device_has_no_transitions() {
    let mut rig = Rig::new(&mouse_spec());
    rig.frame(&[key(BTN_LEFT, 1)]);
    let snapshot = Snapshot {
        keys: bits(&[BTN_LEFT]),
        ..snapshot_for(&mouse_spec())
    };
    let frame = resync(&mut rig, &snapshot);
    assert!(frame.resynced && frame.keys.is_empty());
    assert_eq!(frame.held_buttons, [BTN_LEFT]);
}

#[test]
fn the_first_snapshot_reports_what_is_already_held_and_on() {
    let spec = Spec {
        switches: vec![SW_LID, SW_TABLET_MODE],
        ..mouse_spec()
    };
    let mut rig = Rig::new(&spec);
    let snapshot = Snapshot {
        keys: bits(&[BTN_RIGHT]),
        switches: bits(&[SW_TABLET_MODE]),
        ..snapshot_for(&spec)
    };
    let frame = resync(&mut rig, &snapshot);
    assert_eq!(
        frame.keys,
        [KeyEvent {
            code: BTN_RIGHT,
            state: KeyState::Pressed
        }]
    );
    assert_eq!(
        frame.switches,
        [SwitchEvent {
            switch: Switch::TabletMode,
            on: true
        }]
    );
    // replayed events of that state are not transitions again
    let frame = rig.frame(&[key(BTN_RIGHT, 1), (EV_SW, SW_TABLET_MODE, 1)]);
    assert!(frame.keys.is_empty() && frame.switches.is_empty());
}

#[test]
fn a_snapshot_takes_the_absolute_position_and_the_finger_count() {
    let mut rig = Rig::new(&touch_spec());
    let mut snapshot = snapshot_for(&touch_spec());
    snapshot.abs = vec![(ABS_X, 321), (ABS_Y, 654), (ABS_MT_SLOT, 0)];
    snapshot.keys = bits(&[BTN_TOOL_FINGER, BTN_TOOL_DOUBLETAP, BTN_TOUCH]);
    let frame = resync(&mut rig, &snapshot);
    assert_eq!(frame.abs_pointer, Some(AbsPoint { x: 321, y: 654 }));
    assert_eq!(frame.finger_count, Some(2), "the highest finger tool held");
    assert_eq!(
        frame.keys,
        [KeyEvent {
            code: BTN_TOUCH,
            state: KeyState::Pressed
        }]
    );
}

#[test]
fn a_snapshot_takes_every_active_contact_and_the_selected_slot() {
    let mut rig = Rig::new(&touch_spec());
    rig.frame(&touch(0, 1, 1, 1, 1));
    let mut snapshot = snapshot_for(&touch_spec());
    snapshot.abs = vec![(ABS_X, 0), (ABS_Y, 0), (ABS_MT_SLOT, 2)];
    let set = |snapshot: &mut Snapshot, code: u16, values: [i32; 5]| {
        snapshot
            .mt
            .iter_mut()
            .find(|m| m.code == code)
            .unwrap()
            .values = values.to_vec();
    };
    set(&mut snapshot, ABS_MT_TRACKING_ID, [-1, 70, -1, 71, -1]);
    set(&mut snapshot, ABS_MT_POSITION_X, [9, 10, 11, 12, 13]);
    set(&mut snapshot, ABS_MT_POSITION_Y, [19, 20, 21, 22, 23]);
    set(&mut snapshot, ABS_MT_PRESSURE, [0, 50, 0, 60, 0]);
    let frame = resync(&mut rig, &snapshot);
    // the contact the old state had in slot 0 is gone, and inactive slots' data does not show
    assert_eq!(
        frame.contacts,
        [contact(1, 70, 10, 20, 50), contact(3, 71, 12, 22, 60)]
    );
    // the next data goes to slot 2, the selected one: still inactive, so no contact appears
    let frame = rig.frame(&[abs(ABS_MT_POSITION_X, 99)]);
    assert_eq!(frame.contacts.len(), 2);
    // and a contact there starts without a slot event
    let frame = rig.frame(&[
        abs(ABS_MT_TRACKING_ID, 72),
        abs(ABS_MT_POSITION_X, 5),
        abs(ABS_MT_POSITION_Y, 6),
    ]);
    assert_eq!(
        frame.contacts.iter().map(|c| c.slot).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}

#[test]
fn a_snapshot_missing_an_axis_or_with_the_wrong_slot_count_is_refused_and_changes_nothing() {
    let mut rig = Rig::new(&touch_spec());
    rig.frame(&[key(BTN_LEFT, 1)]);
    let good = snapshot_for(&touch_spec());

    let mut no_x = good.clone();
    no_x.abs.retain(|&(c, _)| c != ABS_X);
    assert_eq!(
        rig.framer.apply_snapshot(&no_x, 5),
        Err(FrameError::MissingSnapshotAxis { code: ABS_X })
    );

    let mut no_ids = good.clone();
    no_ids.mt.retain(|m| m.code != ABS_MT_TRACKING_ID);
    assert_eq!(
        rig.framer.apply_snapshot(&no_ids, 5),
        Err(FrameError::MissingSnapshotAxis {
            code: ABS_MT_TRACKING_ID
        })
    );

    let mut short = good.clone();
    short
        .mt
        .iter_mut()
        .find(|m| m.code == ABS_MT_POSITION_X)
        .unwrap()
        .values
        .pop();
    assert_eq!(
        rig.framer.apply_snapshot(&short, 5),
        Err(FrameError::SnapshotSlotCount {
            code: ABS_MT_POSITION_X,
            actual: 4,
            expected: 5
        })
    );

    let mut bad_slot = good.clone();
    bad_slot
        .abs
        .iter_mut()
        .find(|(c, _)| *c == ABS_MT_SLOT)
        .unwrap()
        .1 = 5;
    assert_eq!(
        rig.framer.apply_snapshot(&bad_slot, 5),
        Err(FrameError::SlotOutOfRange {
            slot: 5,
            slot_count: 5
        })
    );

    let mut bad_id = good.clone();
    bad_id
        .mt
        .iter_mut()
        .find(|m| m.code == ABS_MT_TRACKING_ID)
        .unwrap()
        .values[1] = -5;
    assert_eq!(
        rig.framer.apply_snapshot(&bad_id, 5),
        Err(FrameError::InvalidTrackingId { value: -5 })
    );

    // none of the refusals disturbed the held button
    let frame = rig.frame(&[]);
    assert_eq!(frame.held_buttons, [BTN_LEFT]);
}

#[test]
fn a_snapshot_on_a_device_without_multitouch_needs_no_slot_axes() {
    let mut rig = Rig::new(&mouse_spec());
    let frame = resync(&mut rig, &snapshot_for(&mouse_spec()));
    assert!(frame.resynced && frame.contacts.is_empty());
}

#[test]
fn a_snapshot_ends_the_dropping_state() {
    let mut rig = Rig::new(&mouse_spec());
    rig.push((EV_SYN, SYN_DROPPED, 0)).unwrap();
    resync(&mut rig, &snapshot_for(&mouse_spec()));
    // events are framed again at once
    assert_eq!(rig.frame(&[rel(REL_X, 4)]).motion.dx, 4);
}

#[test]
fn a_snapshot_discards_the_partial_frame() {
    let mut rig = Rig::new(&mouse_spec());
    rig.push(rel(REL_X, 40)).unwrap();
    rig.push(rel(REL_WHEEL, 2)).unwrap();
    let frame = resync(&mut rig, &snapshot_for(&mouse_spec()));
    assert_eq!(frame.motion, Motion::default());
    assert_eq!(frame.wheel, Wheel::default());
    assert_eq!(rig.frame(&[]).motion, Motion::default());
}

#[test]
fn a_resynced_frame_keeps_the_time_floor() {
    let mut rig = Rig::new(&mouse_spec());
    let before = rig.frame(&[]);
    let frame = rig
        .framer
        .apply_snapshot(&snapshot_for(&mouse_spec()), 5)
        .unwrap();
    assert_eq!(frame.time_us, before.time_us);
}
