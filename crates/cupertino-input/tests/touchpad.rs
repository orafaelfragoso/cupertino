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

fn finger(id: u32, x: f64, y: f64) -> TouchContact {
    TouchContact {
        id,
        position_mm: (x, y),
        pressure: None,
        palm: false,
    }
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
        Pad {
            pad: Touchpad::new(GEOMETRY, settings),
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

fn tapping() -> TouchpadSettings {
    TouchpadSettings {
        tap_to_click: true,
        ..TouchpadSettings::default()
    }
}

#[test]
fn one_finger_moving_moves_the_pointer_in_its_direction() {
    let mut p = Pad::new(TouchpadSettings::default());
    p.step(vec![finger(0, 20.0, 20.0)]);
    assert!(p.take().is_empty(), "touching down does nothing");
    p.drag(&[(20.0, 20.0)], 30.0, -10.0, 15);
    let (dx, dy) = motion(&p.take());
    assert!(dx > 0.0 && dy < 0.0, "({dx}, {dy})");
    assert!((dx / dy + 3.0).abs() < 0.3, "the direction is kept");
}

#[test]
fn a_still_finger_does_not_move_the_pointer() {
    let mut p = Pad::new(TouchpadSettings::default());
    p.hold(&[finger(0, 20.0, 20.0)], 0.5);
    assert!(p.take().is_empty());
}

#[test]
fn pointer_motion_is_reported_in_device_counts_whatever_the_speed() {
    let travel = |frames: usize| {
        let mut p = Pad::new(TouchpadSettings::default());
        p.step(vec![finger(0, 10.0, 30.0)]);
        p.take();
        p.drag(&[(10.0, 30.0)], 60.0, 0.0, frames);
        motion(&p.take()).0
    };
    // Acceleration belongs to the caller (`TouchpadSettings::pointer_accel`); the model reports 60 mm in counts.
    let counts = 60.0 / MM_PER_COUNT;
    assert!((travel(8) - counts).abs() < 1.0, "{}", travel(8));
    assert!((travel(80) - counts).abs() < 1.0, "{}", travel(80));
}

#[test]
fn a_quick_touch_clicks_when_tap_to_click_is_on() {
    let mut p = Pad::new(tapping());
    p.hold(&[finger(0, 30.0, 20.0)], 0.1);
    p.lift();
    assert_eq!(
        clicks(&p.take()),
        [(TouchpadButton::Left, true), (TouchpadButton::Left, false)]
    );
}

#[test]
fn a_touch_does_not_click_with_tap_to_click_off() {
    let mut p = Pad::new(TouchpadSettings::default());
    p.hold(&[finger(0, 30.0, 20.0)], 0.1);
    p.lift();
    assert!(clicks(&p.take()).is_empty());
}

#[test]
fn a_long_touch_is_not_a_tap() {
    let mut p = Pad::new(tapping());
    p.hold(&[finger(0, 30.0, 20.0)], 0.4);
    p.lift();
    assert!(clicks(&p.take()).is_empty());
}

#[test]
fn a_two_finger_tap_is_a_secondary_click() {
    let mut p = Pad::new(tapping());
    let two = [finger(0, 30.0, 20.0), finger(1, 45.0, 20.0)];
    p.hold(&two, 0.1);
    p.lift();
    assert_eq!(
        clicks(&p.take()),
        [
            (TouchpadButton::Right, true),
            (TouchpadButton::Right, false)
        ]
    );
}

#[test]
fn a_touch_that_travels_is_not_a_tap() {
    let mut p = Pad::new(tapping());
    p.step(vec![finger(0, 20.0, 20.0)]);
    p.drag(&[(20.0, 20.0)], 40.0, 0.0, 12);
    p.lift();
    assert!(clicks(&p.take()).is_empty());
}

#[test]
fn a_physical_click_presses_and_releases_the_left_button() {
    let mut p = Pad::new(TouchpadSettings::default());
    let one = vec![finger(0, 30.0, 20.0)];
    p.step(one.clone());
    p.step_button(one.clone(), true);
    p.step_button(one.clone(), true);
    p.step_button(one, false);
    assert_eq!(
        clicks(&p.take()),
        [(TouchpadButton::Left, true), (TouchpadButton::Left, false)]
    );
}

#[test]
fn a_click_with_two_fingers_down_is_a_secondary_click_when_enabled() {
    let two = vec![finger(0, 30.0, 20.0), finger(1, 45.0, 20.0)];
    let mut p = Pad::new(TouchpadSettings::default());
    p.step(two.clone());
    p.step_button(two.clone(), true);
    p.step_button(two.clone(), false);
    assert_eq!(
        clicks(&p.take()),
        [
            (TouchpadButton::Right, true),
            (TouchpadButton::Right, false)
        ]
    );
    let mut off = Pad::new(TouchpadSettings {
        secondary_click: false,
        ..TouchpadSettings::default()
    });
    off.step(two.clone());
    off.step_button(two, true);
    assert_eq!(clicks(&off.take()), [(TouchpadButton::Left, true)]);
}

#[test]
fn a_click_in_a_configured_corner_is_a_secondary_click() {
    let corner = vec![finger(0, 105.0, 66.0)];
    let mut p = Pad::new(TouchpadSettings {
        corner_click: CornerClick::BottomRight,
        ..TouchpadSettings::default()
    });
    p.step(corner.clone());
    p.step_button(corner.clone(), true);
    assert_eq!(clicks(&p.take()), [(TouchpadButton::Right, true)]);
    let mut other = Pad::new(TouchpadSettings {
        corner_click: CornerClick::BottomLeft,
        ..TouchpadSettings::default()
    });
    other.step(corner.clone());
    other.step_button(corner, true);
    assert_eq!(clicks(&other.take()), [(TouchpadButton::Left, true)]);
}

#[test]
fn a_pad_with_its_own_buttons_always_reports_the_left_one() {
    let mut p = Pad {
        pad: Touchpad::new(
            TouchpadGeometry {
                clickpad: false,
                ..GEOMETRY
            },
            TouchpadSettings::default(),
        ),
        t: 1.0,
        events: Vec::new(),
    };
    let two = vec![finger(0, 30.0, 20.0), finger(1, 45.0, 20.0)];
    p.step(two.clone());
    p.step_button(two, true);
    assert_eq!(clicks(&p.take()), [(TouchpadButton::Left, true)]);
}

#[test]
fn two_fingers_moving_together_scroll() {
    let mut p = Pad::new(TouchpadSettings::default());
    let start = [(30.0, 20.0), (45.0, 20.0)];
    p.step(vec![finger(0, 30.0, 20.0), finger(1, 45.0, 20.0)]);
    p.drag(&start, 0.0, 25.0, 20);
    let events = p.take();
    assert!(events.contains(&TouchpadEvent::ScrollBegin));
    let dy: f64 = events
        .iter()
        .filter_map(|e| match e {
            TouchpadEvent::ScrollUpdate { dy, .. } => Some(*dy),
            _ => None,
        })
        .sum();
    assert!(dy != 0.0);
    p.lift();
    let end = p.take();
    assert!(end
        .iter()
        .any(|e| matches!(e, TouchpadEvent::ScrollEnd { .. })));
}

#[test]
fn scroll_direction_follows_the_natural_scrolling_setting() {
    let total = |natural: bool| {
        let mut p = Pad::new(TouchpadSettings {
            natural_scrolling: natural,
            ..TouchpadSettings::default()
        });
        let start = [(30.0, 20.0), (45.0, 20.0)];
        p.step(vec![finger(0, 30.0, 20.0), finger(1, 45.0, 20.0)]);
        p.drag(&start, 0.0, 25.0, 20);
        p.events
            .iter()
            .filter_map(|e| match e {
                TouchpadEvent::ScrollUpdate { dy, .. } => Some(*dy),
                _ => None,
            })
            .sum::<f64>()
    };
    let (natural, classic) = (total(true), total(false));
    assert!(natural * classic < 0.0, "{natural} {classic}");
}

#[test]
fn two_fingers_spreading_pinch() {
    let mut p = Pad::new(TouchpadSettings::default());
    p.step(vec![finger(0, 40.0, 30.0), finger(1, 60.0, 30.0)]);
    for i in 1..=20 {
        let spread = f64::from(i);
        p.step(vec![
            finger(0, 40.0 - spread / 2.0, 30.0),
            finger(1, 60.0 + spread / 2.0, 30.0),
        ]);
    }
    let events = p.take();
    assert!(events.contains(&TouchpadEvent::PinchBegin { fingers: 2 }));
    let scale = events.iter().rev().find_map(|e| match e {
        TouchpadEvent::PinchUpdate { scale, .. } => Some(*scale),
        _ => None,
    });
    assert!((scale.expect("an update") - 2.0).abs() < 0.05);
    assert!(!events.contains(&TouchpadEvent::ScrollBegin));
}

#[test]
fn with_pinch_and_rotate_off_two_fingers_apart_scroll_instead() {
    let mut p = Pad::new(TouchpadSettings {
        pinch: false,
        rotate: false,
        ..TouchpadSettings::default()
    });
    p.step(vec![finger(0, 40.0, 30.0), finger(1, 60.0, 30.0)]);
    for i in 1..=20 {
        let spread = f64::from(i);
        p.step(vec![
            finger(0, 40.0 - spread / 2.0, 30.0 + spread),
            finger(1, 60.0 + spread / 2.0, 30.0 + spread),
        ]);
    }
    let events = p.take();
    assert!(!events
        .iter()
        .any(|e| matches!(e, TouchpadEvent::PinchBegin { .. })));
    assert!(events.contains(&TouchpadEvent::ScrollBegin));
}

#[test]
fn three_fingers_swiping_sideways_drive_a_horizontal_swipe() {
    let mut p = Pad::new(TouchpadSettings::default());
    let start = [(30.0, 20.0), (45.0, 20.0), (60.0, 20.0)];
    p.step(
        start
            .iter()
            .enumerate()
            .map(|(i, (x, y))| finger(i as u32, *x, *y))
            .collect(),
    );
    p.drag(&start, SWIPE_COMPLETE_MM, 0.0, 30);
    let events = p.take();
    assert!(events.contains(&TouchpadEvent::SwipeBegin {
        fingers: 3,
        axis: SwipeAxis::Horizontal
    }));
    let last = events.iter().rev().find_map(|e| match e {
        TouchpadEvent::SwipeUpdate {
            fingers: 3,
            progress,
        } => Some(*progress),
        _ => None,
    });
    let progress = last.expect("updates");
    assert!(progress.abs() > 0.5 && progress.abs() <= 1.5, "{progress}");
    p.lift();
    let end = p.take();
    let committed = end.iter().find_map(|e| match e {
        TouchpadEvent::SwipeEnd { committed, .. } => Some(*committed),
        _ => None,
    });
    assert_eq!(committed, Some(true));
}

#[test]
fn a_short_swipe_is_cancelled_on_lift() {
    let mut p = Pad::new(TouchpadSettings::default());
    let start = [(30.0, 20.0), (45.0, 20.0), (60.0, 20.0)];
    p.step(
        start
            .iter()
            .enumerate()
            .map(|(i, (x, y))| finger(i as u32, *x, *y))
            .collect(),
    );
    p.drag(&start, 0.0, -15.0, 30);
    let events = p.take();
    assert!(events.contains(&TouchpadEvent::SwipeBegin {
        fingers: 3,
        axis: SwipeAxis::Vertical
    }));
    p.hold(
        &start
            .iter()
            .enumerate()
            .map(|(i, (x, y))| finger(i as u32, *x, y - 15.0))
            .collect::<Vec<_>>(),
        0.6,
    );
    p.lift();
    let committed = p.take().into_iter().find_map(|e| match e {
        TouchpadEvent::SwipeEnd { committed, .. } => Some(committed),
        _ => None,
    });
    assert_eq!(committed, Some(false));
}

#[test]
fn swipes_with_a_finger_count_that_is_turned_off_do_not_start() {
    let mut p = Pad::new(TouchpadSettings {
        three_finger_swipe: false,
        ..TouchpadSettings::default()
    });
    let start = [(30.0, 20.0), (45.0, 20.0), (60.0, 20.0)];
    p.step(
        start
            .iter()
            .enumerate()
            .map(|(i, (x, y))| finger(i as u32, *x, *y))
            .collect(),
    );
    p.drag(&start, 40.0, 0.0, 20);
    assert!(!p
        .take()
        .iter()
        .any(|e| matches!(e, TouchpadEvent::SwipeBegin { .. })));
}

#[test]
fn a_palm_contact_is_ignored() {
    let mut p = Pad::new(tapping());
    let palm = TouchContact {
        palm: true,
        ..finger(1, 80.0, 60.0)
    };
    p.step(vec![finger(0, 30.0, 20.0), palm]);
    p.drag(&[(30.0, 20.0)], 20.0, 0.0, 10);
    let events = p.take();
    assert!(
        motion(&events).0 > 0.0,
        "the other finger still moves the pointer"
    );
    assert!(!events.contains(&TouchpadEvent::ScrollBegin));
}

#[test]
fn a_thumb_resting_in_the_bottom_zone_does_not_count_as_a_finger() {
    let mut p = Pad::new(TouchpadSettings::default());
    let thumb = finger(1, 55.0, 68.0);
    // The thumb lands first and rests for longer than a tap; then a finger lands.
    p.hold(&[thumb], 0.4);
    p.step(vec![thumb, finger(0, 20.0, 20.0)]);
    p.take();
    for i in 1..=10 {
        p.step(vec![thumb, finger(0, 20.0 + f64::from(i) * 3.0, 20.0)]);
    }
    let events = p.take();
    assert!(
        motion(&events).0 > 0.0,
        "one counted finger moves the pointer"
    );
    assert!(!events.contains(&TouchpadEvent::ScrollBegin));
}

#[test]
fn a_hard_press_is_a_force_click_with_hysteresis() {
    let mut p = Pad::new(TouchpadSettings::default());
    let at = |pressure: f64| {
        vec![TouchContact {
            pressure: Some(pressure),
            ..finger(0, 30.0, 20.0)
        }]
    };
    p.step(at(0.2));
    assert!(p.take().is_empty());
    p.step(at(0.7));
    assert_eq!(p.take(), [TouchpadEvent::ForceClick { active: true }]);
    p.step(at(0.55));
    assert!(
        p.take().is_empty(),
        "stays active between the release and activation levels"
    );
    p.step(at(0.3));
    assert_eq!(p.take(), [TouchpadEvent::ForceClick { active: false }]);
}

#[test]
fn the_click_pressure_setting_moves_the_force_threshold() {
    let force_at = |pressure: ClickPressure, level: f64| {
        let mut p = Pad::new(TouchpadSettings {
            click_pressure: pressure,
            ..TouchpadSettings::default()
        });
        p.step(vec![TouchContact {
            pressure: Some(level),
            ..finger(0, 30.0, 20.0)
        }]);
        p.take()
            .contains(&TouchpadEvent::ForceClick { active: true })
    };
    assert!(force_at(ClickPressure::Light, 0.5));
    assert!(!force_at(ClickPressure::Medium, 0.5));
    assert!(!force_at(ClickPressure::Firm, 0.65));
    assert!(force_at(ClickPressure::Firm, 0.75));
    let mut off = Pad::new(TouchpadSettings {
        force_click: false,
        ..TouchpadSettings::default()
    });
    off.step(vec![TouchContact {
        pressure: Some(1.0),
        ..finger(0, 30.0, 20.0)
    }]);
    assert!(off.take().is_empty());
}

#[test]
fn a_tap_and_a_touch_within_the_window_starts_a_drag() {
    let mut p = Pad::new(TouchpadSettings {
        tap_to_click: true,
        tap_drag: true,
        ..TouchpadSettings::default()
    });
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
fn a_single_tap_with_tap_drag_clicks_after_the_hold_time() {
    let mut p = Pad::new(TouchpadSettings {
        tap_to_click: true,
        tap_drag: true,
        ..TouchpadSettings::default()
    });
    p.hold(&[finger(0, 30.0, 20.0)], 0.08);
    p.lift();
    let due = p.pad.next_deadline().expect("a held-back click");
    assert!(p.pad.tick(due - 0.01).is_empty());
    let out = p.pad.tick(due);
    assert_eq!(
        clicks(&out),
        [(TouchpadButton::Left, true), (TouchpadButton::Left, false)]
    );
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

#[test]
fn an_idle_pad_has_nothing_due() {
    let mut pad = Touchpad::new(GEOMETRY, TouchpadSettings::default());
    assert_eq!(pad.next_deadline(), None);
    assert!(pad.tick(10.0).is_empty());
}
