//! Input classification from capability bitmasks, with udev's `input_id` rules.

use cupertino_devices::evdev::codes::*;
use cupertino_devices::{classify, CapabilityMask, InputCapabilities, InputClass};

/// The sysfs text of a mask with `bits` set.
fn mask(bits: &[u16]) -> CapabilityMask {
    let mut words = [0u64; 12];
    for &bit in bits {
        words[usize::from(bit) / 64] |= 1 << (bit % 64);
    }
    let text: Vec<String> = words.iter().rev().map(|w| format!("{w:x}")).collect();
    CapabilityMask::parse(&text.join(" ")).unwrap()
}

#[derive(Default)]
struct Caps {
    ev: Vec<u16>,
    key: Vec<u16>,
    rel: Vec<u16>,
    abs: Vec<u16>,
    props: Vec<u16>,
    bustype: u16,
}

impl Caps {
    fn build(&self) -> InputCapabilities {
        InputCapabilities {
            ev: mask(&self.ev),
            key: mask(&self.key),
            rel: mask(&self.rel),
            abs: mask(&self.abs),
            props: mask(&self.props),
            bustype: self.bustype,
        }
    }

    fn class(&self) -> InputClass {
        classify(&self.build())
    }
}

const KEY_A: u16 = 30;
const BTN_THUMB: u16 = 0x121;

fn keyboard_keys() -> Vec<u16> {
    (1..32).collect()
}

fn mouse() -> Caps {
    Caps {
        ev: vec![EV_KEY, EV_REL],
        key: vec![BTN_LEFT, BTN_RIGHT],
        rel: vec![REL_X, REL_Y, REL_WHEEL],
        ..Caps::default()
    }
}

fn touchpad() -> Caps {
    Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_LEFT, BTN_TOOL_FINGER, BTN_TOUCH],
        abs: vec![
            ABS_X,
            ABS_Y,
            ABS_MT_SLOT,
            ABS_MT_POSITION_X,
            ABS_MT_POSITION_Y,
        ],
        props: vec![INPUT_PROP_POINTER],
        ..Caps::default()
    }
}

#[test]
fn a_device_with_nothing_is_nothing() {
    assert_eq!(Caps::default().class(), InputClass::default());
}

#[test]
fn a_full_alphanumeric_block_is_a_keyboard() {
    let class = Caps {
        ev: vec![EV_KEY],
        key: keyboard_keys(),
        ..Caps::default()
    }
    .class();
    assert!(class.keyboard && class.key);
    assert!(!class.mouse && !class.touchpad && !class.joystick);
}

#[test]
fn a_missing_key_in_the_first_block_makes_it_a_key_device_not_a_keyboard() {
    let mut keys = keyboard_keys();
    keys.retain(|&k| k != 16);
    let class = Caps {
        ev: vec![EV_KEY],
        key: keys,
        ..Caps::default()
    }
    .class();
    assert!(class.key && !class.keyboard);
}

#[test]
fn a_power_button_is_a_key_device() {
    let class = Caps {
        ev: vec![EV_KEY],
        key: vec![116],
        ..Caps::default()
    }
    .class();
    assert!(class.key && !class.keyboard);
}

#[test]
fn keys_are_ignored_without_the_key_event_type() {
    let class = Caps {
        ev: vec![],
        key: keyboard_keys(),
        ..Caps::default()
    }
    .class();
    assert!(!class.key && !class.keyboard);
}

#[test]
fn a_relative_pointer_with_buttons_is_a_mouse() {
    let class = mouse().class();
    assert!(class.mouse);
    assert!(!class.key && !class.touchpad && !class.pointing_stick);
}

#[test]
fn a_keyboard_with_a_mouse_function_is_both() {
    let mut caps = mouse();
    caps.key.extend(keyboard_keys());
    let class = caps.class();
    assert!(class.mouse && class.keyboard && class.key);
}

#[test]
fn a_scroll_wheel_alone_counts_as_a_key_device() {
    let class = Caps {
        ev: vec![EV_REL],
        rel: vec![REL_WHEEL],
        ..Caps::default()
    }
    .class();
    assert!(class.key && !class.mouse);
    let class = Caps {
        ev: vec![EV_REL],
        rel: vec![REL_HWHEEL],
        ..Caps::default()
    }
    .class();
    assert!(class.key);
    // motion without a wheel is neither
    let class = Caps {
        ev: vec![EV_REL],
        rel: vec![REL_X, REL_Y],
        ..Caps::default()
    }
    .class();
    assert_eq!(class, InputClass::default());
}

#[test]
fn a_finger_pad_is_a_touchpad_and_a_direct_one_a_touchscreen() {
    let class = touchpad().class();
    assert!(class.touchpad && !class.touchscreen && !class.mouse);

    let mut screen = touchpad();
    screen.key = vec![BTN_TOUCH];
    screen.props = vec![INPUT_PROP_DIRECT];
    let class = screen.class();
    assert!(class.touchscreen && !class.touchpad);

    // a finger tool on a direct device is still a screen
    let mut both = touchpad();
    both.props = vec![INPUT_PROP_DIRECT];
    assert!(both.class().touchscreen && !both.class().touchpad);
}

#[test]
fn a_button_pad_is_a_clickpad() {
    let mut caps = touchpad();
    assert!(!caps.class().clickpad);
    caps.props = vec![INPUT_PROP_POINTER, INPUT_PROP_BUTTONPAD];
    let class = caps.class();
    assert!(class.clickpad && class.touchpad);
}

#[test]
fn a_single_touch_pad_with_only_absolute_axes_is_a_touchpad() {
    let class = Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_TOOL_FINGER, BTN_TOUCH],
        abs: vec![ABS_X, ABS_Y],
        ..Caps::default()
    }
    .class();
    assert!(class.touchpad);
}

#[test]
fn multitouch_axes_that_claim_every_slot_axis_are_a_fake() {
    let mut caps = touchpad();
    caps.abs = vec![
        ABS_MT_SLOT,
        ABS_MT_SLOT - 1,
        ABS_MT_POSITION_X,
        ABS_MT_POSITION_Y,
    ];
    assert!(!caps.class().touchpad);
}

#[test]
fn a_pen_makes_a_tablet_and_not_a_mouse() {
    let class = Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_TOOL_PEN, BTN_STYLUS, BTN_TOUCH, BTN_LEFT],
        abs: vec![ABS_X, ABS_Y, ABS_PRESSURE],
        ..Caps::default()
    }
    .class();
    assert!(class.tablet);
    assert!(!class.mouse && !class.touchpad && !class.tablet_pad);
}

#[test]
fn pad_buttons_make_a_tablet_pad_only_without_a_pen_tool() {
    let class = Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_TOOL_PEN, BTN_STYLUS, BTN_0, BTN_1],
        abs: vec![ABS_X, ABS_Y],
        ..Caps::default()
    }
    .class();
    assert!(
        class.tablet && !class.tablet_pad,
        "a pen turns the buttons into the pen's, not a pad's"
    );

    // the stylus without a pen tool, with pad buttons
    let class = Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_STYLUS, BTN_0, BTN_1],
        abs: vec![ABS_X, ABS_Y],
        ..Caps::default()
    }
    .class();
    assert!(class.tablet && class.tablet_pad);
}

#[test]
fn buttons_and_a_wheel_without_motion_are_a_tablet_pad() {
    let class = Caps {
        ev: vec![EV_KEY, EV_REL],
        key: vec![BTN_0, BTN_1],
        rel: vec![REL_WHEEL],
        ..Caps::default()
    }
    .class();
    assert!(class.tablet && class.tablet_pad);
    assert!(!class.joystick && !class.mouse);
}

#[test]
fn a_virtual_machines_absolute_pointer_with_buttons_is_a_mouse() {
    let class = Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_LEFT, BTN_RIGHT],
        abs: vec![ABS_X, ABS_Y],
        ..Caps::default()
    }
    .class();
    assert!(class.mouse);
    assert!(!class.touchscreen && !class.tablet);
}

#[test]
fn a_mouse_on_the_i2c_bus_is_a_pointing_stick() {
    let mut caps = mouse();
    caps.bustype = 0x18;
    let class = caps.class();
    assert!(class.mouse && class.pointing_stick);
    caps.bustype = 0x03;
    assert!(!caps.class().pointing_stick);
}

#[test]
fn the_pointing_stick_property_marks_a_pointing_stick() {
    let mut caps = mouse();
    caps.props = vec![INPUT_PROP_POINTER, 5];
    assert!(caps.class().pointing_stick);
}

#[test]
fn a_motion_sensor_is_an_accelerometer() {
    let by_property = Caps {
        ev: vec![EV_ABS],
        abs: vec![ABS_X, ABS_Y],
        props: vec![INPUT_PROP_ACCELEROMETER],
        ..Caps::default()
    }
    .class();
    assert!(by_property.accelerometer && !by_property.mouse);

    let by_axes = Caps {
        ev: vec![EV_ABS],
        abs: vec![ABS_X, ABS_Y, ABS_Z],
        ..Caps::default()
    }
    .class();
    assert!(by_axes.accelerometer);

    // three axes on a device with keys is something else
    let with_keys = Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_TOUCH],
        abs: vec![ABS_X, ABS_Y, ABS_Z],
        ..Caps::default()
    }
    .class();
    assert!(!with_keys.accelerometer);
}

fn gamepad() -> Caps {
    Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_JOYSTICK, BTN_THUMB, BTN_JOYSTICK + 2],
        abs: vec![ABS_X, ABS_Y, ABS_RX],
        ..Caps::default()
    }
}

#[test]
fn a_device_with_stick_axes_and_buttons_is_a_joystick() {
    let class = gamepad().class();
    assert!(class.joystick);
    assert!(!class.mouse && !class.touchpad && !class.keyboard);
}

#[test]
fn a_joystick_without_the_axes_but_with_buttons_is_one_too() {
    let class = Caps {
        ev: vec![EV_KEY],
        key: vec![BTN_JOYSTICK, BTN_THUMB],
        ..Caps::default()
    }
    .class();
    assert!(class.joystick);
}

#[test]
fn a_single_stray_joystick_button_is_not_a_joystick() {
    let class = Caps {
        ev: vec![EV_KEY],
        key: vec![BTN_JOYSTICK],
        ..Caps::default()
    }
    .class();
    assert!(!class.joystick);
}

#[test]
fn a_keyboard_with_stray_joystick_buttons_stays_a_keyboard() {
    let mut caps = gamepad();
    // control, caps lock, num lock, insert: four of the well-known keyboard keys
    caps.key.extend([29, 58, 69, 110]);
    caps.key.extend(keyboard_keys());
    let class = caps.class();
    assert!(class.keyboard);
    assert!(!class.joystick);
}

#[test]
fn a_gamepad_with_a_few_keyboard_keys_is_still_a_joystick() {
    let mut caps = gamepad();
    caps.key.extend([KEY_A, 29, 58]);
    assert!(caps.class().joystick);
}

#[test]
fn a_mouse_with_many_buttons_is_not_taken_for_a_joystick() {
    let mut caps = mouse();
    // the button range runs on into the joystick's
    caps.key = (BTN_LEFT..BTN_LEFT + 20).collect();
    let class = caps.class();
    assert!(class.mouse && !class.joystick);
}

#[test]
fn a_switch_device_reports_switches() {
    let class = Caps {
        ev: vec![EV_SW],
        ..Caps::default()
    }
    .class();
    assert!(class.switch);
    assert!(!class.key);
}

#[test]
fn a_device_can_be_several_things_at_once() {
    let mut caps = touchpad();
    caps.ev.extend([EV_SW]);
    caps.key.extend(keyboard_keys());
    let class = caps.class();
    assert!(class.touchpad && class.keyboard && class.switch);
}
