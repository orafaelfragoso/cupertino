//! A touchpad model turns frames of finger contacts into pointer motion, clicks, scrolls, swipes and pinches.

use cupertino_input::{
    ClickPressure, CornerClick, SwipeAxis, TouchContact, Touchpad, TouchpadButton, TouchpadEvent,
    TouchpadFrame, TouchpadGeometry, TouchpadSettings, MM_PER_COUNT, SWIPE_COMPLETE_MM,
};

const GEOMETRY: TouchpadGeometry = TouchpadGeometry {
    width_mm: 110.0,
    height_mm: 70.0,
    clickpad: true,
};

/// One frame every 8 ms, the pace of a 120 Hz pad.
const FRAME_S: f64 = 1.0 / 120.0;

const LEFT_CLICK: [(TouchpadButton, bool); 2] =
    [(TouchpadButton::Left, true), (TouchpadButton::Left, false)];

fn finger(id: u32, x: f64, y: f64) -> TouchContact {
    TouchContact {
        id,
        position_mm: (x, y),
        pressure: None,
        palm: false,
    }
}

fn fingers(at: &[(f64, f64)]) -> Vec<TouchContact> {
    at.iter()
        .enumerate()
        .map(|(id, (x, y))| finger(id as u32, *x, *y))
        .collect()
}

fn frame(time_s: f64, contacts: Vec<TouchContact>) -> TouchpadFrame {
    TouchpadFrame {
        time_s,
        contacts,
        button_down: false,
    }
}

struct Pad {
    pad: Touchpad,
    t: f64,
    events: Vec<TouchpadEvent>,
}

impl Pad {
    fn new(settings: TouchpadSettings) -> Self {
        Self::with_geometry(GEOMETRY, settings)
    }

    fn with_geometry(geometry: TouchpadGeometry, settings: TouchpadSettings) -> Self {
        Pad {
            pad: Touchpad::new(geometry, settings),
            t: 1.0,
            events: Vec::new(),
        }
    }

    fn step(&mut self, contacts: Vec<TouchContact>) {
        self.events
            .extend(self.pad.process_frame(&frame(self.t, contacts)));
        self.t += FRAME_S;
    }

    fn step_button(&mut self, contacts: Vec<TouchContact>, down: bool) {
        let mut f = frame(self.t, contacts);
        f.button_down = down;
        self.events.extend(self.pad.process_frame(&f));
        self.t += FRAME_S;
    }

    /// Hold still for `seconds`.
    fn hold(&mut self, contacts: &[TouchContact], seconds: f64) {
        for _ in 0..(seconds / FRAME_S).ceil() as usize {
            self.step(contacts.to_vec());
        }
    }

    fn lift(&mut self) {
        self.step(Vec::new());
    }

    fn take(&mut self) -> Vec<TouchpadEvent> {
        std::mem::take(&mut self.events)
    }

    /// Move `fingers` by (dx, dy) in total over `frames` frames, from `start` positions.
    fn drag(&mut self, start: &[(f64, f64)], dx: f64, dy: f64, frames: usize) {
        for i in 1..=frames {
            let k = i as f64 / frames as f64;
            let contacts = start
                .iter()
                .enumerate()
                .map(|(id, (x, y))| finger(id as u32, x + dx * k, y + dy * k))
                .collect();
            self.step(contacts);
        }
    }
}

fn clicks(events: &[TouchpadEvent]) -> Vec<(TouchpadButton, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            TouchpadEvent::Button { button, pressed } => Some((*button, *pressed)),
            _ => None,
        })
        .collect()
}

fn motion(events: &[TouchpadEvent]) -> (f64, f64) {
    events.iter().fold((0.0, 0.0), |(x, y), e| match e {
        TouchpadEvent::PointerMotion { dx, dy } => (x + dx, y + dy),
        _ => (x, y),
    })
}

fn scrolled(events: &[TouchpadEvent]) -> f64 {
    events
        .iter()
        .filter_map(|e| match e {
            TouchpadEvent::ScrollUpdate { dy, .. } => Some(*dy),
            _ => None,
        })
        .sum()
}

fn tapping() -> TouchpadSettings {
    TouchpadSettings {
        tap_to_click: true,
        ..TouchpadSettings::default()
    }
}

fn tap_drag() -> TouchpadSettings {
    TouchpadSettings {
        tap_to_click: true,
        tap_drag: true,
        ..TouchpadSettings::default()
    }
}

#[test]
fn one_finger_moving_moves_the_pointer_in_its_direction_and_a_still_one_does_not() {
    let mut still = Pad::new(TouchpadSettings::default());
    still.hold(&[finger(0, 20.0, 20.0)], 0.5);
    assert!(still.take().is_empty());

    let mut p = Pad::new(TouchpadSettings::default());
    p.step(vec![finger(0, 20.0, 20.0)]);
    assert!(p.take().is_empty(), "touching down does nothing");
    p.drag(&[(20.0, 20.0)], 30.0, -10.0, 15);
    let (dx, dy) = motion(&p.take());
    assert!(dx > 0.0 && dy < 0.0, "({dx}, {dy})");
    assert!((dx / dy + 3.0).abs() < 0.3, "the direction is kept");
}

#[test]
fn pointer_motion_is_reported_in_device_counts_whatever_the_speed() {
    // Acceleration belongs to the caller (`TouchpadSettings::pointer_accel`); the model reports 60 mm in counts.
    let counts = 60.0 / MM_PER_COUNT;
    for frames in [8, 80] {
        let mut p = Pad::new(TouchpadSettings::default());
        p.step(vec![finger(0, 10.0, 30.0)]);
        p.take();
        p.drag(&[(10.0, 30.0)], 60.0, 0.0, frames);
        let travel = motion(&p.take()).0;
        assert!((travel - counts).abs() < 1.0, "{frames} frames: {travel}");
    }
}

#[test]
fn a_quick_still_touch_is_a_tap_when_tap_to_click_is_on() {
    let one = [(30.0, 20.0)];
    let two = [(30.0, 20.0), (45.0, 20.0)];
    let right = [
        (TouchpadButton::Right, true),
        (TouchpadButton::Right, false),
    ];
    for (name, settings, at, seconds, travel, expected) in [
        (
            "a quick touch",
            tapping(),
            &one[..],
            0.1,
            0.0,
            &LEFT_CLICK[..],
        ),
        (
            "tap to click off",
            TouchpadSettings::default(),
            &one[..],
            0.1,
            0.0,
            &[][..],
        ),
        ("a long touch", tapping(), &one[..], 0.4, 0.0, &[][..]),
        ("two fingers", tapping(), &two[..], 0.1, 0.0, &right[..]),
        (
            "a touch that travels",
            tapping(),
            &one[..],
            0.0,
            40.0,
            &[][..],
        ),
    ] {
        let mut p = Pad::new(settings);
        if seconds > 0.0 {
            p.hold(&fingers(at), seconds);
        } else {
            p.step(fingers(at));
            p.drag(at, travel, 0.0, 12);
        }
        p.lift();
        assert_eq!(clicks(&p.take()), expected, "{name}");
    }
}

#[test]
fn a_physical_click_is_secondary_with_two_fingers_or_in_the_configured_corner() {
    let one = [(30.0, 20.0)];
    let two = [(30.0, 20.0), (45.0, 20.0)];
    let corner = [(105.0, 66.0)];
    let buttons = TouchpadGeometry {
        clickpad: false,
        ..GEOMETRY
    };
    let corner_click = |corner_click| TouchpadSettings {
        corner_click,
        ..TouchpadSettings::default()
    };
    let no_secondary = TouchpadSettings {
        secondary_click: false,
        ..TouchpadSettings::default()
    };
    for (name, geometry, settings, at, button) in [
        (
            "one finger",
            GEOMETRY,
            TouchpadSettings::default(),
            &one[..],
            TouchpadButton::Left,
        ),
        (
            "two fingers",
            GEOMETRY,
            TouchpadSettings::default(),
            &two[..],
            TouchpadButton::Right,
        ),
        (
            "two fingers, secondary click off",
            GEOMETRY,
            no_secondary,
            &two[..],
            TouchpadButton::Left,
        ),
        (
            "the configured corner",
            GEOMETRY,
            corner_click(CornerClick::BottomRight),
            &corner[..],
            TouchpadButton::Right,
        ),
        (
            "another corner than the configured one",
            GEOMETRY,
            corner_click(CornerClick::BottomLeft),
            &corner[..],
            TouchpadButton::Left,
        ),
        (
            "a pad with its own buttons",
            buttons,
            TouchpadSettings::default(),
            &two[..],
            TouchpadButton::Left,
        ),
    ] {
        let mut p = Pad::with_geometry(geometry, settings);
        p.step(fingers(at));
        p.step_button(fingers(at), true);
        p.step_button(fingers(at), true);
        p.step_button(fingers(at), false);
        assert_eq!(
            clicks(&p.take()),
            [(button, true), (button, false)],
            "{name}"
        );
    }
}

#[test]
fn two_fingers_moving_together_scroll_in_the_natural_scrolling_direction() {
    let start = [(30.0, 20.0), (45.0, 20.0)];
    let mut totals = Vec::new();
    for natural in [true, false] {
        let mut p = Pad::new(TouchpadSettings {
            natural_scrolling: natural,
            ..TouchpadSettings::default()
        });
        p.step(fingers(&start));
        p.drag(&start, 0.0, 25.0, 20);
        let events = p.take();
        assert!(events.contains(&TouchpadEvent::ScrollBegin), "{natural}");
        totals.push(scrolled(&events));
        p.lift();
        assert!(
            p.take()
                .iter()
                .any(|e| matches!(e, TouchpadEvent::ScrollEnd { .. })),
            "{natural}"
        );
    }
    assert!(totals[0] * totals[1] < 0.0, "{totals:?}");
}

#[test]
fn two_fingers_spreading_pinch_or_scroll_with_pinch_and_rotate_off() {
    for pinch in [true, false] {
        let mut p = Pad::new(TouchpadSettings {
            pinch,
            rotate: pinch,
            ..TouchpadSettings::default()
        });
        p.step(fingers(&[(40.0, 30.0), (60.0, 30.0)]));
        for i in 1..=20 {
            let spread = f64::from(i);
            // Without pinch the pair also moves down, so it can scroll.
            let down = if pinch { 0.0 } else { spread };
            p.step(fingers(&[
                (40.0 - spread / 2.0, 30.0 + down),
                (60.0 + spread / 2.0, 30.0 + down),
            ]));
        }
        let events = p.take();
        let scale = events.iter().rev().find_map(|e| match e {
            TouchpadEvent::PinchUpdate { scale, .. } => Some(*scale),
            _ => None,
        });
        if pinch {
            assert!(events.contains(&TouchpadEvent::PinchBegin { fingers: 2 }));
            assert!((scale.expect("an update") - 2.0).abs() < 0.05);
            assert!(!events.contains(&TouchpadEvent::ScrollBegin));
        } else {
            assert_eq!(scale, None);
            assert!(events.contains(&TouchpadEvent::ScrollBegin));
        }
    }
}

#[test]
fn three_fingers_swipe_along_an_axis_and_commit_only_when_far_enough() {
    let start = [(30.0, 20.0), (45.0, 20.0), (60.0, 20.0)];
    for (name, dx, dy, axis, rest, committed) in [
        (
            "a full sideways swipe",
            SWIPE_COMPLETE_MM,
            0.0,
            SwipeAxis::Horizontal,
            0.0,
            true,
        ),
        (
            "a short swipe up, held",
            0.0,
            -15.0,
            SwipeAxis::Vertical,
            0.6,
            false,
        ),
    ] {
        let mut p = Pad::new(TouchpadSettings::default());
        p.step(fingers(&start));
        p.drag(&start, dx, dy, 30);
        let events = p.take();
        assert!(
            events.contains(&TouchpadEvent::SwipeBegin { fingers: 3, axis }),
            "{name}"
        );
        if committed {
            let progress = events
                .iter()
                .rev()
                .find_map(|e| match e {
                    TouchpadEvent::SwipeUpdate {
                        fingers: 3,
                        progress,
                    } => Some(*progress),
                    _ => None,
                })
                .expect("updates");
            assert!(progress.abs() > 0.5 && progress.abs() <= 1.5, "{progress}");
        }
        let moved: Vec<(f64, f64)> = start.iter().map(|(x, y)| (x + dx, y + dy)).collect();
        p.hold(&fingers(&moved), rest);
        p.lift();
        let end = p.take().into_iter().find_map(|e| match e {
            TouchpadEvent::SwipeEnd { committed, .. } => Some(committed),
            _ => None,
        });
        assert_eq!(end, Some(committed), "{name}");
    }

    let mut off = Pad::new(TouchpadSettings {
        three_finger_swipe: false,
        ..TouchpadSettings::default()
    });
    off.step(fingers(&start));
    off.drag(&start, 40.0, 0.0, 20);
    assert!(
        !off.take()
            .iter()
            .any(|e| matches!(e, TouchpadEvent::SwipeBegin { .. })),
        "a finger count that is turned off does not start"
    );
}

#[test]
fn a_palm_or_a_resting_thumb_does_not_count_as_a_finger() {
    let mut palm = Pad::new(tapping());
    let resting_palm = TouchContact {
        palm: true,
        ..finger(1, 80.0, 60.0)
    };
    palm.step(vec![finger(0, 30.0, 20.0), resting_palm]);
    palm.drag(&[(30.0, 20.0)], 20.0, 0.0, 10);

    let mut thumb = Pad::new(TouchpadSettings::default());
    let resting_thumb = finger(1, 55.0, 68.0);
    // The thumb lands in the bottom zone and rests for longer than a tap; then a finger lands.
    thumb.hold(&[resting_thumb], 0.4);
    thumb.step(vec![resting_thumb, finger(0, 20.0, 20.0)]);
    thumb.take();
    for i in 1..=10 {
        thumb.step(vec![
            resting_thumb,
            finger(0, 20.0 + f64::from(i) * 3.0, 20.0),
        ]);
    }

    for (name, mut p) in [("palm", palm), ("thumb", thumb)] {
        let events = p.take();
        assert!(
            motion(&events).0 > 0.0,
            "{name}: the finger moves the pointer"
        );
        assert!(!events.contains(&TouchpadEvent::ScrollBegin), "{name}");
    }
}

#[test]
fn a_hard_press_is_a_force_click_with_hysteresis() {
    let mut p = Pad::new(TouchpadSettings::default());
    for (pressure, expected) in [
        (0.2, None),
        (0.7, Some(true)),
        // Stays active between the release and activation levels.
        (0.55, None),
        (0.3, Some(false)),
    ] {
        p.step(vec![TouchContact {
            pressure: Some(pressure),
            ..finger(0, 30.0, 20.0)
        }]);
        let expected: Vec<TouchpadEvent> = expected
            .map(|active| TouchpadEvent::ForceClick { active })
            .into_iter()
            .collect();
        assert_eq!(p.take(), expected, "{pressure}");
    }
}

#[test]
fn the_click_pressure_setting_moves_the_force_threshold() {
    for (pressure, force_click, level, fires) in [
        (ClickPressure::Light, true, 0.5, true),
        (ClickPressure::Medium, true, 0.5, false),
        (ClickPressure::Firm, true, 0.65, false),
        (ClickPressure::Firm, true, 0.75, true),
        (ClickPressure::Medium, false, 1.0, false),
    ] {
        let mut p = Pad::new(TouchpadSettings {
            click_pressure: pressure,
            force_click,
            ..TouchpadSettings::default()
        });
        p.step(vec![TouchContact {
            pressure: Some(level),
            ..finger(0, 30.0, 20.0)
        }]);
        assert_eq!(
            p.take()
                .contains(&TouchpadEvent::ForceClick { active: true }),
            fires,
            "{pressure:?} force click {force_click} at {level}"
        );
    }
}

#[test]
fn a_tap_and_a_touch_within_the_window_starts_a_drag() {
    let mut p = Pad::new(tap_drag());
    p.hold(&[finger(0, 30.0, 20.0)], 0.08);
    p.lift();
    assert!(
        clicks(&p.take()).is_empty(),
        "the click is held back to see if a drag follows"
    );
    assert!(p.pad.next_deadline().is_some());
    p.hold(&[], 0.1);
    p.hold(&[finger(0, 30.5, 20.0)], 0.04);
    // The first tap is clicked, then the button goes down again for the drag.
    assert_eq!(
        clicks(&p.take()),
        [
            (TouchpadButton::Left, true),
            (TouchpadButton::Left, false),
            (TouchpadButton::Left, true)
        ]
    );
    p.drag(&[(30.5, 20.0)], 20.0, 0.0, 10);
    p.lift();
    // Releasing ends the drag after a short bridge, so the button comes up on a later tick.
    let mut released = clicks(&p.take());
    released.extend(clicks(&p.pad.tick(p.t + 1.0)));
    assert_eq!(released, [(TouchpadButton::Left, false)]);
}

#[test]
fn a_single_tap_with_tap_drag_clicks_when_its_deadline_is_due() {
    let mut idle = Touchpad::new(GEOMETRY, TouchpadSettings::default());
    assert_eq!(idle.next_deadline(), None, "an idle pad has nothing due");
    assert!(idle.tick(10.0).is_empty());

    let mut p = Pad::new(tap_drag());
    p.hold(&[finger(0, 30.0, 20.0)], 0.08);
    p.lift();
    let due = p.pad.next_deadline().expect("a held-back click");
    assert!(p.pad.tick(due - 0.01).is_empty());
    assert_eq!(clicks(&p.pad.tick(due)), LEFT_CLICK);
}

#[test]
fn time_going_backwards_is_a_bug_in_the_caller() {
    let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
    pad.process_frame(&frame(2.0, Vec::new()));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pad.process_frame(&frame(1.0, Vec::new()))
    }));
    assert!(result.is_err());
}
