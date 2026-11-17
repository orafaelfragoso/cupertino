//! The framer: recorded event sequences in, frames out, with no device.

use cupertino_devices::evdev::codes::*;
use cupertino_devices::evdev::{
    AbsAxis, AbsInfo, AbsPoint, BitSet, Contact, DeviceId, DeviceInfo, Frame, FrameError, Framer,
    FramerOutput, InputEvent, KeyEvent, KeyState, Layout, Motion, MtSlotValues, Resolution,
    Snapshot, Switch, SwitchEvent, Tool, Wheel,
};

const MT_SLOTS: i32 = 5;

#[derive(Clone, Default)]
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

fn lid_spec() -> Spec {
    Spec {
        switches: vec![SW_LID, SW_TABLET_MODE],
        ..mouse_spec()
    }
}

/// A framer with a running clock: each `SYN_REPORT` is stamped 1000 microseconds after the last.
struct Rig {
    framer: Framer,
    clock: i64,
}

type Ev = (u16, u16, i32);

const SYN: Ev = (EV_SYN, SYN_REPORT, 0);
const DROPPED: Ev = (EV_SYN, SYN_DROPPED, 0);

fn key(code: u16, value: i32) -> Ev {
    (EV_KEY, code, value)
}
fn rel(code: u16, value: i32) -> Ev {
    (EV_REL, code, value)
}
fn abs(code: u16, value: i32) -> Ev {
    (EV_ABS, code, value)
}
fn sw(code: u16, value: i32) -> Ev {
    (EV_SW, code, value)
}

fn pressed(code: u16) -> KeyEvent {
    KeyEvent {
        code,
        state: KeyState::Pressed,
    }
}
fn released(code: u16) -> KeyEvent {
    KeyEvent {
        code,
        state: KeyState::Released,
    }
}
fn repeat(code: u16) -> KeyEvent {
    KeyEvent {
        code,
        state: KeyState::Repeat,
    }
}
fn switched(switch: Switch, on: bool) -> SwitchEvent {
    SwitchEvent { switch, on }
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

    fn push_all(&mut self, events: &[Ev]) {
        for &ev in events {
            assert_eq!(self.push(ev), Ok(None), "{ev:?} closes nothing");
        }
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

    fn resync(&mut self, snapshot: &Snapshot) -> Frame {
        self.clock += 1000;
        self.framer
            .apply_snapshot(snapshot, self.clock)
            .expect("a valid snapshot")
    }
}

#[test]
fn a_report_closes_a_frame_stamped_with_its_time_never_earlier_than_the_last() {
    let mut rig = Rig::new(&mouse_spec());
    assert_eq!(
        rig.push(rel(REL_X, 3)),
        Ok(None),
        "events before the report"
    );
    assert_eq!(rig.push(key(BTN_LEFT, 1)), Ok(None));
    let mut rig = Rig::new(&mouse_spec());
    let first = rig.frame(&[]);
    assert_eq!(first.time_us, 1_001_000);
    assert!(!first.resynced);
    assert!(first.keys.is_empty() && first.switches.is_empty() && first.contacts.is_empty());
    assert_eq!(first.motion, Motion::default());
    assert_eq!(first.wheel, Wheel::default());
    assert_eq!(first.abs_pointer, None);
    assert_eq!(first.finger_count, None);
    assert!(first.held_buttons.is_empty());

    let second = rig.frame(&[]);
    assert!(second.time_us > first.time_us);
    // a report stamped earlier than the last one is clamped up to it, keeping its changes
    let at = |usec, kind, code, value| InputEvent {
        sec: 0,
        usec,
        kind,
        code,
        value,
    };
    rig.framer.push(&at(5, EV_REL, REL_X, 9)).unwrap();
    match rig.framer.push(&at(10, EV_SYN, SYN_REPORT, 0)).unwrap() {
        Some(FramerOutput::Frame(frame)) => {
            assert_eq!(frame.time_us, second.time_us);
            assert_eq!(frame.motion.dx, 9);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn relative_motion_and_wheels_are_summed_within_a_frame_saturating_and_reset_after() {
    let hi_res = Spec {
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
    let vertical_hi_res = Spec {
        rel: vec![REL_WHEEL, REL_HWHEEL, REL_WHEEL_HI_RES],
        ..Spec::default()
    };
    let wheel = |vertical, horizontal| Wheel {
        vertical,
        horizontal,
    };
    let motion = |dx, dy| Motion { dx, dy };
    for (name, spec, frames) in [
        (
            "summed and reset",
            mouse_spec(),
            vec![
                (
                    vec![rel(REL_X, 3), rel(REL_Y, -2), rel(REL_X, 4), rel(REL_Y, -1)],
                    motion(7, -3),
                    wheel(0, 0),
                ),
                (vec![], motion(0, 0), wheel(0, 0)),
            ],
        ),
        (
            "saturating instead of wrapping",
            mouse_spec(),
            vec![(
                vec![
                    rel(REL_X, i32::MAX),
                    rel(REL_X, 10),
                    rel(REL_Y, i32::MIN),
                    rel(REL_Y, -10),
                ],
                motion(i32::MAX, i32::MIN),
                wheel(0, 0),
            )],
        ),
        (
            "a legacy wheel step is 120",
            mouse_spec(),
            vec![
                (
                    vec![rel(REL_WHEEL, 1), rel(REL_WHEEL, 2), rel(REL_HWHEEL, -1)],
                    motion(0, 0),
                    wheel(360, -120),
                ),
                (
                    vec![rel(REL_WHEEL, i32::MAX)],
                    motion(0, 0),
                    wheel(i32::MAX, 0),
                ),
            ],
        ),
        (
            // the kernel sends both for one detent; counting both would double it
            "a high-resolution wheel read through its own axis only",
            hi_res,
            vec![
                (
                    vec![
                        rel(REL_WHEEL_HI_RES, 60),
                        rel(REL_WHEEL_HI_RES, 60),
                        rel(REL_WHEEL, 1),
                    ],
                    motion(0, 0),
                    wheel(120, 0),
                ),
                (
                    vec![rel(REL_HWHEEL_HI_RES, -30), rel(REL_HWHEEL, -1)],
                    motion(0, 0),
                    wheel(0, -30),
                ),
            ],
        ),
        (
            "only the declared wheel axis goes high resolution",
            vertical_hi_res,
            vec![(
                vec![
                    rel(REL_WHEEL_HI_RES, 40),
                    rel(REL_WHEEL, 1),
                    rel(REL_HWHEEL, 1),
                ],
                motion(0, 0),
                wheel(40, 120),
            )],
        ),
        (
            "axes not modelled (REL_DIAL, REL_MISC) skipped",
            mouse_spec(),
            vec![(
                vec![rel(0x07, 5), rel(0x09, 5), rel(REL_X, 1)],
                motion(1, 0),
                wheel(0, 0),
            )],
        ),
    ] {
        let mut rig = Rig::new(&spec);
        for (index, (events, motion, wheel)) in frames.into_iter().enumerate() {
            let frame = rig.frame(&events);
            assert_eq!(
                (frame.motion, frame.wheel),
                (motion, wheel),
                "{name}, frame {index}"
            );
        }
    }
}

#[test]
fn keys_are_transitions_in_event_order_and_held_buttons_the_state_after_each_frame() {
    for (name, spec, frames) in [
        (
            "transitions in order",
            mouse_spec(),
            vec![(
                vec![key(30, 1), key(BTN_LEFT, 1), key(30, 0)],
                vec![pressed(30), pressed(BTN_LEFT), released(30)],
                vec![BTN_LEFT],
            )],
        ),
        (
            "held buttons exclude keyboard keys and stay held",
            mouse_spec(),
            vec![
                (
                    vec![key(BTN_RIGHT, 1), key(30, 1), key(BTN_LEFT, 1)],
                    vec![pressed(BTN_RIGHT), pressed(30), pressed(BTN_LEFT)],
                    vec![BTN_LEFT, BTN_RIGHT],
                ),
                (
                    vec![key(BTN_LEFT, 0)],
                    vec![released(BTN_LEFT)],
                    vec![BTN_RIGHT],
                ),
                (vec![], vec![], vec![BTN_RIGHT]),
                (vec![key(BTN_RIGHT, 0)], vec![released(BTN_RIGHT)], vec![]),
            ],
        ),
        (
            "auto-repeat changes no state",
            mouse_spec(),
            vec![
                (vec![key(30, 1)], vec![pressed(30)], vec![]),
                (
                    vec![key(30, 2), key(30, 2)],
                    vec![repeat(30), repeat(30)],
                    vec![],
                ),
                (vec![key(30, 0)], vec![released(30)], vec![]),
            ],
        ),
        (
            "a press of a held key and a release of a released one are replays",
            mouse_spec(),
            vec![
                (vec![key(30, 0)], vec![], vec![]),
                (vec![key(30, 1)], vec![pressed(30)], vec![]),
                (vec![key(30, 1)], vec![], vec![]),
                (vec![key(30, 0)], vec![released(30)], vec![]),
            ],
        ),
        (
            "the highest key code",
            mouse_spec(),
            vec![(vec![key(KEY_MAX, 1)], vec![pressed(KEY_MAX)], vec![])],
        ),
        (
            "the touch button is an ordinary held button",
            touch_spec(),
            vec![(
                vec![key(BTN_TOUCH, 1), key(BTN_LEFT, 1)],
                vec![pressed(BTN_TOUCH), pressed(BTN_LEFT)],
                vec![BTN_LEFT, BTN_TOUCH],
            )],
        ),
    ] {
        let mut rig = Rig::new(&spec);
        for (index, (events, keys, held)) in frames.into_iter().enumerate() {
            let frame = rig.frame(&events);
            assert_eq!(frame.keys, keys, "{name}, frame {index}");
            assert_eq!(frame.held_buttons, held, "{name}, frame {index}");
        }
    }
}

#[test]
fn finger_tools_are_a_count_not_key_events_even_past_the_slots() {
    let mut rig = Rig::new(&touch_spec());
    // the driver presses the new count, then releases the old one
    for (events, count) in [
        (
            vec![abs(ABS_X, 1), abs(ABS_Y, 1), key(BTN_TOOL_FINGER, 1)],
            1,
        ),
        (vec![key(BTN_TOOL_DOUBLETAP, 1), key(BTN_TOOL_FINGER, 0)], 2),
        (
            vec![key(BTN_TOOL_TRIPLETAP, 1), key(BTN_TOOL_DOUBLETAP, 0)],
            3,
        ),
        (vec![key(BTN_TOOL_TRIPLETAP, 0)], 0),
        (vec![], 0),
    ] {
        let frame = rig.frame(&events);
        assert_eq!(frame.finger_count, Some(count), "{events:?}");
        assert!(frame.keys.is_empty() && frame.held_buttons.is_empty());
    }

    let spec = Spec {
        slot_max: Some(1),
        ..touch_spec()
    };
    let mut rig = Rig::new(&spec);
    let mut events = touch(0, 1, 1, 1, 1);
    events.extend(touch(1, 2, 2, 2, 1));
    events.push(key(BTN_TOOL_TRIPLETAP, 1));
    let frame = rig.frame(&events);
    assert_eq!((frame.contacts.len(), frame.finger_count), (2, Some(3)));
}

#[test]
fn the_absolute_pointer_is_present_with_both_axes_and_keeps_its_value() {
    let spec = Spec {
        keys: vec![BTN_LEFT],
        abs: vec![ABS_X, ABS_Y, ABS_PRESSURE],
        ..Spec::default()
    };
    let mut rig = Rig::new(&spec);
    for (events, x, y) in [
        (vec![], 0, 0),
        (vec![abs(ABS_X, 120), abs(ABS_Y, -4)], 120, -4),
        (vec![abs(ABS_X, 7)], 7, -4),
        (vec![], 7, -4),
        // an axis not modelled is skipped
        (vec![abs(ABS_PRESSURE, 400), abs(ABS_X, 5)], 5, -4),
    ] {
        assert_eq!(
            rig.frame(&events).abs_pointer,
            Some(AbsPoint { x, y }),
            "{events:?}"
        );
    }
    let one_axis = Spec {
        abs: vec![ABS_X],
        ..Spec::default()
    };
    assert_eq!(Rig::new(&one_axis).frame(&[]).abs_pointer, None);
}

#[test]
fn switches_report_their_transitions_and_only_the_switches_the_framer_knows() {
    let mut rig = Rig::new(&lid_spec());
    for (events, expected) in [
        (vec![sw(SW_LID, 0)], vec![]),
        (vec![sw(SW_LID, 1)], vec![switched(Switch::Lid, true)]),
        (vec![sw(SW_LID, 1)], vec![]),
        (
            vec![sw(SW_TABLET_MODE, 1), sw(SW_LID, 0)],
            vec![
                switched(Switch::TabletMode, true),
                switched(Switch::Lid, false),
            ],
        ),
        (vec![], vec![]),
        // SW_HEADPHONE_INSERT is not one the framer reports
        (vec![sw(0x02, 1)], vec![]),
    ] {
        assert_eq!(rig.frame(&events).switches, expected, "{events:?}");
    }
}

#[test]
fn events_outside_the_modelled_types_are_skipped() {
    let mut rig = Rig::new(&mouse_spec());
    // EV_MSC scancode, EV_LED, EV_REP, EV_FF, SYN_CONFIG, the last valid type
    let frame = rig.frame(&[
        (0x04, 0x04, 458_792),
        (0x11, 0, 1),
        (0x14, 0, 250),
        (0x15, 0, 1),
        (EV_SYN, 1, 0),
        (EV_MAX, 0, 0),
        rel(REL_X, 2),
    ]);
    assert_eq!(frame.motion.dx, 2);
    assert!(frame.keys.is_empty());
}

#[test]
fn an_event_the_devices_layout_cannot_carry_is_refused() {
    let unexpected = |kind, code| FrameError::UnexpectedEvent { kind, code };
    let one_axis = Spec {
        abs: vec![ABS_X],
        ..Spec::default()
    };
    for (name, spec, events, expected) in [
        (
            "an undeclared high-resolution wheel",
            mouse_spec(),
            vec![rel(REL_WHEEL_HI_RES, 1)],
            unexpected(EV_REL, REL_WHEEL_HI_RES),
        ),
        (
            "an undeclared horizontal high-resolution wheel",
            mouse_spec(),
            vec![rel(REL_HWHEEL_HI_RES, 1)],
            unexpected(EV_REL, REL_HWHEEL_HI_RES),
        ),
        (
            "a key value of 3",
            mouse_spec(),
            vec![key(30, 3)],
            FrameError::InvalidKeyValue { code: 30, value: 3 },
        ),
        (
            "a negative key value",
            mouse_spec(),
            vec![key(30, -1)],
            FrameError::InvalidKeyValue {
                code: 30,
                value: -1,
            },
        ),
        (
            "a key code past the range",
            mouse_spec(),
            vec![key(KEY_MAX + 1, 1)],
            unexpected(EV_KEY, KEY_MAX + 1),
        ),
        (
            "a finger tool on a device without them",
            mouse_spec(),
            vec![key(BTN_TOOL_FINGER, 1)],
            unexpected(EV_KEY, BTN_TOOL_FINGER),
        ),
        (
            "one absolute axis",
            one_axis,
            vec![abs(ABS_X, 1)],
            unexpected(EV_ABS, ABS_X),
        ),
        (
            "an event type above the maximum",
            mouse_spec(),
            vec![(0x20, 0, 0)],
            FrameError::UnknownEventType { kind: 0x20 },
        ),
        (
            "the largest event type",
            mouse_spec(),
            vec![(0xffff, 0, 0)],
            FrameError::UnknownEventType { kind: 0xffff },
        ),
        (
            "the multitouch report of protocol A",
            mouse_spec(),
            vec![(EV_SYN, SYN_MT_REPORT, 0)],
            FrameError::ProtocolA,
        ),
        (
            "a switch value of 2",
            lid_spec(),
            vec![sw(SW_LID, 2)],
            FrameError::InvalidSwitchValue {
                code: SW_LID,
                value: 2,
            },
        ),
        (
            "a slot past the declared count",
            touch_spec(),
            vec![abs(ABS_MT_SLOT, MT_SLOTS)],
            FrameError::SlotOutOfRange {
                slot: MT_SLOTS,
                slot_count: MT_SLOTS as u32,
            },
        ),
        (
            "a negative slot",
            touch_spec(),
            vec![abs(ABS_MT_SLOT, -1)],
            FrameError::SlotOutOfRange {
                slot: -1,
                slot_count: 5,
            },
        ),
        (
            "a tracking id below -1",
            touch_spec(),
            vec![abs(ABS_MT_TRACKING_ID, -2)],
            FrameError::InvalidTrackingId { value: -2 },
        ),
        (
            "an undeclared touch major",
            touch_spec(),
            vec![abs(ABS_MT_TOUCH_MAJOR, 4)],
            unexpected(EV_ABS, ABS_MT_TOUCH_MAJOR),
        ),
        (
            "an undeclared tool type",
            touch_spec(),
            vec![abs(ABS_MT_TOOL_TYPE, 0)],
            unexpected(EV_ABS, ABS_MT_TOOL_TYPE),
        ),
        (
            "a slot on a device without slots",
            mouse_spec(),
            vec![abs(ABS_MT_SLOT, 0)],
            unexpected(EV_ABS, ABS_MT_SLOT),
        ),
        (
            "a position on a device without slots",
            mouse_spec(),
            vec![abs(ABS_MT_POSITION_X, 0)],
            unexpected(EV_ABS, ABS_MT_POSITION_X),
        ),
    ] {
        assert_eq!(Rig::new(&spec).error(&events), expected, "{name}");
    }
    assert!(Rig::new(&touch_spec())
        .push(abs(ABS_MT_SLOT, MT_SLOTS - 1))
        .is_ok());
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
fn contacts_follow_their_slots_and_tracking_ids_listed_by_slot() {
    let three = [
        touch(3, 40, 30, 30, 1),
        touch(1, 41, 10, 10, 2),
        touch(2, 42, 20, 20, 3),
    ]
    .concat();
    let two = [touch(0, 1, 5, 5, 1), touch(1, 2, 6, 6, 1)].concat();
    for (name, frames) in [
        (
            "a contact starts, moves and ends",
            vec![
                (
                    touch(0, 17, 100, 200, 30),
                    vec![contact(0, 17, 100, 200, 30)],
                ),
                // a contact stays in every frame while down
                (
                    vec![abs(ABS_MT_POSITION_X, 110)],
                    vec![contact(0, 17, 110, 200, 30)],
                ),
                (vec![abs(ABS_MT_TRACKING_ID, -1)], vec![]),
                (vec![], vec![]),
            ],
        ),
        (
            "listed by slot whatever the order of arrival",
            vec![(
                three,
                vec![
                    contact(1, 41, 10, 10, 2),
                    contact(2, 42, 20, 20, 3),
                    contact(3, 40, 30, 30, 1),
                ],
            )],
        ),
        (
            "data applies to the slot last selected",
            vec![
                (two, vec![contact(0, 1, 5, 5, 1), contact(1, 2, 6, 6, 1)]),
                (
                    vec![abs(ABS_MT_POSITION_Y, 99)],
                    vec![contact(0, 1, 5, 5, 1), contact(1, 2, 6, 99, 1)],
                ),
                (
                    vec![abs(ABS_MT_SLOT, 0), abs(ABS_MT_POSITION_Y, 77)],
                    vec![contact(0, 1, 5, 77, 1), contact(1, 2, 6, 99, 1)],
                ),
            ],
        ),
        (
            "a slot reused with a new tracking id",
            vec![
                (touch(0, 5, 1, 1, 1), vec![contact(0, 5, 1, 1, 1)]),
                (vec![abs(ABS_MT_TRACKING_ID, -1)], vec![]),
                (
                    vec![
                        abs(ABS_MT_TRACKING_ID, 6),
                        abs(ABS_MT_POSITION_X, 9),
                        abs(ABS_MT_POSITION_Y, 9),
                    ],
                    vec![contact(0, 6, 9, 9, 1)],
                ),
            ],
        ),
        (
            "data on an inactive slot makes no contact",
            vec![
                (
                    vec![
                        abs(ABS_MT_POSITION_X, 5),
                        abs(ABS_MT_POSITION_Y, 5),
                        abs(ABS_MT_PRESSURE, 9),
                    ],
                    vec![],
                ),
                (touch(0, 1, 1, 1, 1), vec![contact(0, 1, 1, 1, 1)]),
                // ended and updated in the same frame
                (
                    vec![abs(ABS_MT_TRACKING_ID, -1), abs(ABS_MT_POSITION_X, 50)],
                    vec![],
                ),
            ],
        ),
    ] {
        let mut rig = Rig::new(&touch_spec());
        for (index, (events, expected)) in frames.into_iter().enumerate() {
            assert_eq!(
                rig.frame(&events).contacts,
                expected,
                "{name}, frame {index}"
            );
        }
    }
}

#[test]
fn optional_contact_axes_appear_only_when_the_device_has_them_and_the_tool_defaults_to_a_finger() {
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
    for (raw, tool) in [(7, Tool::Other(7)), (MT_TOOL_PEN, Tool::Pen)] {
        assert_eq!(
            rig.frame(&[abs(ABS_MT_TOOL_TYPE, raw)]).contacts[0].tool,
            tool
        );
    }
    assert_eq!(
        Rig::new(&touch_spec())
            .frame(&touch(0, 1, 1, 1, 1))
            .contacts[0]
            .tool,
        Tool::Finger
    );
}

#[test]
fn the_layout_is_derived_from_the_declared_capabilities_and_kept_by_the_framer() {
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
    assert_eq!(*Framer::new(touch).layout(), touch);

    for (slot_max, count) in [(0, 1), (i32::MAX, i32::MAX as u32 + 1)] {
        let spec = Spec {
            slot_max: Some(slot_max),
            ..touch_spec()
        };
        assert_eq!(layout(&spec).slot_count, count, "{slot_max}");
    }
}

#[test]
fn a_layout_of_protocol_a_slots_without_positions_or_an_empty_slot_range_is_refused() {
    let without = |missing: u16| {
        let mut spec = touch_spec();
        spec.abs.retain(|&c| c != missing);
        spec
    };
    for (name, spec, expected) in [
        (
            "multitouch without slots",
            Spec {
                abs: vec![ABS_MT_POSITION_X, ABS_MT_POSITION_Y],
                ..Spec::default()
            },
            FrameError::MultitouchWithoutSlots,
        ),
        (
            "no x",
            without(ABS_MT_POSITION_X),
            FrameError::MultitouchWithoutPosition,
        ),
        (
            "no y",
            without(ABS_MT_POSITION_Y),
            FrameError::MultitouchWithoutPosition,
        ),
        (
            "no tracking id",
            without(ABS_MT_TRACKING_ID),
            FrameError::MultitouchWithoutPosition,
        ),
        (
            "a slot maximum of -1",
            Spec {
                slot_max: Some(-1),
                ..touch_spec()
            },
            FrameError::EmptySlotRange,
        ),
        (
            "the lowest slot maximum",
            Spec {
                slot_max: Some(i32::MIN),
                ..touch_spec()
            },
            FrameError::EmptySlotRange,
        ),
    ] {
        assert_eq!(Layout::from_info(&info(&spec)), Err(expected), "{name}");
    }
}

fn bits(codes: &[u16]) -> BitSet {
    BitSet::from_codes(codes)
}

/// A snapshot of `spec`'s device at rest: nothing held, every axis zero, every slot inactive.
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

#[test]
fn a_dropped_stream_discards_everything_up_to_the_next_report_and_asks_for_a_resync() {
    let mut rig = Rig::new(&mouse_spec());
    rig.push_all(&[rel(REL_X, 50), key(BTN_LEFT, 1), DROPPED]);
    // everything up to the report is thrown away, even what would be an error, and a second drop
    rig.push_all(&[rel(REL_Y, 5), (0x20, 0, 0), DROPPED]);
    let at = rig.clock + 1000;
    match rig.push(SYN).unwrap() {
        Some(FramerOutput::ResyncNeeded { time_us }) => assert_eq!(time_us, at),
        other => panic!("{other:?}"),
    }
    let frame = rig.resync(&snapshot_for(&mouse_spec()));
    assert!(frame.resynced);
    assert_eq!(frame.motion, Motion::default());
    assert!(frame.keys.is_empty() && frame.held_buttons.is_empty());
    // the snapshot ends the dropping state: events are framed again at once
    assert_eq!(rig.frame(&[rel(REL_X, 1)]).motion, Motion { dx: 1, dy: 0 });

    // without a snapshot the report after the resync request is a frame again
    let mut rig = Rig::new(&mouse_spec());
    rig.push_all(&[DROPPED]);
    assert!(matches!(
        rig.push(SYN).unwrap(),
        Some(FramerOutput::ResyncNeeded { .. })
    ));
    assert!(matches!(
        rig.push(SYN).unwrap(),
        Some(FramerOutput::Frame(_))
    ));
}

/// The regression cases of the discarded partial frame: its key and switch transitions are
/// undone, so the snapshot is compared with what was reported, not with what was half-applied.
#[test]
fn a_snapshot_reports_the_difference_from_the_state_the_frames_reported() {
    type Row = (
        &'static str,
        Spec,
        Vec<Vec<Ev>>,
        Vec<Ev>,
        &'static [u16],
        &'static [u16],
        Vec<KeyEvent>,
        Vec<SwitchEvent>,
        Vec<u16>,
    );
    let rows: Vec<Row> = vec![
        (
            "a press discarded by a drop and up in the snapshot",
            mouse_spec(),
            vec![],
            vec![key(BTN_LEFT, 1), DROPPED, SYN],
            &[],
            &[],
            vec![],
            vec![],
            vec![],
        ),
        (
            "a press discarded by a drop and held in the snapshot",
            mouse_spec(),
            vec![],
            vec![key(BTN_LEFT, 1), DROPPED, SYN],
            &[BTN_LEFT],
            &[],
            vec![pressed(BTN_LEFT)],
            vec![],
            vec![BTN_LEFT],
        ),
        (
            "a reported press whose release is discarded",
            mouse_spec(),
            vec![vec![key(BTN_LEFT, 1)]],
            vec![key(BTN_LEFT, 0), DROPPED, SYN],
            &[],
            &[],
            vec![released(BTN_LEFT)],
            vec![],
            vec![],
        ),
        (
            "a switch change discarded by a drop",
            lid_spec(),
            vec![],
            vec![sw(SW_LID, 1), DROPPED, SYN],
            &[],
            &[SW_LID],
            vec![],
            vec![switched(Switch::Lid, true)],
            vec![],
        ),
        (
            "a partial release without a drop",
            mouse_spec(),
            vec![vec![key(BTN_LEFT, 1)]],
            vec![key(BTN_LEFT, 0)],
            &[],
            &[],
            vec![released(BTN_LEFT)],
            vec![],
            vec![],
        ),
        (
            // KEY_A and BTN_LEFT were released and BTN_RIGHT and KEY_B (48) pressed while events were lost
            "the difference in code order",
            mouse_spec(),
            vec![vec![key(30, 1), key(BTN_LEFT, 1)]],
            vec![],
            &[BTN_RIGHT, 48],
            &[],
            vec![
                released(30),
                pressed(48),
                released(BTN_LEFT),
                pressed(BTN_RIGHT),
            ],
            vec![],
            vec![BTN_RIGHT],
        ),
        (
            "an unchanged device",
            mouse_spec(),
            vec![vec![key(BTN_LEFT, 1)]],
            vec![],
            &[BTN_LEFT],
            &[],
            vec![],
            vec![],
            vec![BTN_LEFT],
        ),
        (
            "the first snapshot reports what is already held and on",
            lid_spec(),
            vec![],
            vec![],
            &[BTN_RIGHT],
            &[SW_TABLET_MODE],
            vec![pressed(BTN_RIGHT)],
            vec![switched(Switch::TabletMode, true)],
            vec![BTN_RIGHT],
        ),
    ];
    for (name, spec, frames, partial, keys, switches, expected_keys, expected_switches, held) in
        rows
    {
        let mut rig = Rig::new(&spec);
        for events in &frames {
            rig.frame(events);
        }
        for &ev in &partial {
            rig.push(ev).unwrap();
        }
        let snapshot = Snapshot {
            keys: bits(keys),
            switches: bits(switches),
            ..snapshot_for(&spec)
        };
        let frame = rig.resync(&snapshot);
        assert!(frame.resynced, "{name}");
        assert_eq!(frame.keys, expected_keys, "{name}");
        assert_eq!(frame.switches, expected_switches, "{name}");
        assert_eq!(frame.held_buttons, held, "{name}");
        // replayed events of the snapshot's state are not transitions again
        let replay: Vec<Ev> = keys
            .iter()
            .map(|&code| key(code, 1))
            .chain(switches.iter().map(|&code| sw(code, 1)))
            .collect();
        let after = rig.frame(&replay);
        assert!(
            after.keys.is_empty() && after.switches.is_empty(),
            "{name}: {after:?}"
        );
    }
}

#[test]
fn a_snapshot_takes_the_pointer_the_finger_count_and_every_active_contact_and_selected_slot() {
    let mut rig = Rig::new(&touch_spec());
    rig.frame(&touch(0, 1, 1, 1, 1));
    let mut snapshot = snapshot_for(&touch_spec());
    snapshot.abs = vec![(ABS_X, 321), (ABS_Y, 654), (ABS_MT_SLOT, 2)];
    snapshot.keys = bits(&[BTN_TOOL_FINGER, BTN_TOOL_DOUBLETAP, BTN_TOUCH]);
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
    let frame = rig.resync(&snapshot);
    assert_eq!(frame.abs_pointer, Some(AbsPoint { x: 321, y: 654 }));
    assert_eq!(frame.finger_count, Some(2), "the highest finger tool held");
    assert_eq!(frame.keys, [pressed(BTN_TOUCH)]);
    // the contact the old state had in slot 0 is gone, and inactive slots' data does not show
    assert_eq!(
        frame.contacts,
        [contact(1, 70, 10, 20, 50), contact(3, 71, 12, 22, 60)]
    );
    // the next data goes to slot 2, the selected one: still inactive, so no contact appears
    assert_eq!(rig.frame(&[abs(ABS_MT_POSITION_X, 99)]).contacts.len(), 2);
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
fn a_snapshot_discards_the_partial_frame_keeps_the_time_floor_and_needs_slot_axes_only_for_multitouch(
) {
    let mut rig = Rig::new(&mouse_spec());
    let before = rig.frame(&[]);
    rig.push_all(&[rel(REL_X, 40), rel(REL_WHEEL, 2)]);
    let frame = rig
        .framer
        .apply_snapshot(&snapshot_for(&mouse_spec()), 5)
        .unwrap();
    assert_eq!(frame.time_us, before.time_us);
    assert!(frame.resynced && frame.contacts.is_empty());
    assert_eq!(
        (frame.motion, frame.wheel),
        (Motion::default(), Wheel::default())
    );
    assert_eq!(rig.frame(&[]).motion, Motion::default());
}

#[test]
fn a_snapshot_missing_an_axis_or_with_the_wrong_slots_is_refused_and_changes_nothing() {
    let mut rig = Rig::new(&touch_spec());
    rig.frame(&[key(BTN_LEFT, 1)]);
    let good = snapshot_for(&touch_spec());
    let changed = |change: &dyn Fn(&mut Snapshot)| {
        let mut snapshot = good.clone();
        change(&mut snapshot);
        snapshot
    };
    fn mt(snapshot: &mut Snapshot, code: u16) -> &mut Vec<i32> {
        &mut snapshot
            .mt
            .iter_mut()
            .find(|m| m.code == code)
            .unwrap()
            .values
    }
    for (name, snapshot, expected) in [
        (
            "no x",
            changed(&|s| s.abs.retain(|&(c, _)| c != ABS_X)),
            FrameError::MissingSnapshotAxis { code: ABS_X },
        ),
        (
            "no tracking ids",
            changed(&|s| s.mt.retain(|m| m.code != ABS_MT_TRACKING_ID)),
            FrameError::MissingSnapshotAxis {
                code: ABS_MT_TRACKING_ID,
            },
        ),
        (
            "a slot short",
            changed(&|s| {
                mt(s, ABS_MT_POSITION_X).pop();
            }),
            FrameError::SnapshotSlotCount {
                code: ABS_MT_POSITION_X,
                actual: 4,
                expected: 5,
            },
        ),
        (
            "a selected slot past the count",
            changed(&|s| {
                s.abs.iter_mut().find(|(c, _)| *c == ABS_MT_SLOT).unwrap().1 = 5;
            }),
            FrameError::SlotOutOfRange {
                slot: 5,
                slot_count: 5,
            },
        ),
        (
            "a tracking id below -1",
            changed(&|s| mt(s, ABS_MT_TRACKING_ID)[1] = -5),
            FrameError::InvalidTrackingId { value: -5 },
        ),
    ] {
        assert_eq!(
            rig.framer.apply_snapshot(&snapshot, 5),
            Err(expected),
            "{name}"
        );
    }
    // none of the refusals disturbed the held button
    assert_eq!(rig.frame(&[]).held_buttons, [BTN_LEFT]);
}
