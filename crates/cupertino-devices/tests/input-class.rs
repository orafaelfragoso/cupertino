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

#[derive(Clone, Default)]
struct Caps {
    ev: Vec<u16>,
    key: Vec<u16>,
    rel: Vec<u16>,
    abs: Vec<u16>,
    props: Vec<u16>,
    bustype: u16,
}

impl Caps {
    fn class(&self) -> InputClass {
        classify(&InputCapabilities {
            ev: mask(&self.ev),
            key: mask(&self.key),
            rel: mask(&self.rel),
            abs: mask(&self.abs),
            props: mask(&self.props),
            bustype: self.bustype,
        })
    }
}

/// Every flag of a class by its name.
fn flags(class: &InputClass) -> [(&'static str, bool); 12] {
    [
        ("key", class.key),
        ("keyboard", class.keyboard),
        ("mouse", class.mouse),
        ("touchpad", class.touchpad),
        ("touchscreen", class.touchscreen),
        ("tablet", class.tablet),
        ("tablet_pad", class.tablet_pad),
        ("joystick", class.joystick),
        ("pointing_stick", class.pointing_stick),
        ("accelerometer", class.accelerometer),
        ("switch", class.switch),
        ("clickpad", class.clickpad),
    ]
}

const KEY_A: u16 = 30;
const BTN_THUMB: u16 = 0x121;

fn keyboard_keys() -> Vec<u16> {
    (1..32).collect()
}

fn keys(key: Vec<u16>) -> Caps {
    Caps {
        ev: vec![EV_KEY],
        key,
        ..Caps::default()
    }
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

fn gamepad() -> Caps {
    Caps {
        ev: vec![EV_KEY, EV_ABS],
        key: vec![BTN_JOYSTICK, BTN_THUMB, BTN_JOYSTICK + 2],
        abs: vec![ABS_X, ABS_Y, ABS_RX],
        ..Caps::default()
    }
}

fn with(mut caps: Caps, change: impl FnOnce(&mut Caps)) -> Caps {
    change(&mut caps);
    caps
}

#[test]
fn a_device_is_classified_by_udevs_input_id_rules() {
    let rows: Vec<(&str, Caps, &[&str], &[&str])> = vec![
        ("nothing", Caps::default(), &[], &[]),
        (
            "a full alphanumeric block is a keyboard",
            keys(keyboard_keys()),
            &["keyboard", "key"],
            &["mouse", "touchpad", "joystick"],
        ),
        (
            "a missing key in the first block is a key device",
            keys(keyboard_keys().into_iter().filter(|&k| k != 16).collect()),
            &["key"],
            &["keyboard"],
        ),
        ("a power button", keys(vec![116]), &["key"], &["keyboard"]),
        (
            "keys without the key event type",
            with(keys(keyboard_keys()), |c| c.ev.clear()),
            &[],
            &["key", "keyboard"],
        ),
        (
            "a relative pointer with buttons is a mouse",
            mouse(),
            &["mouse"],
            &["key", "touchpad", "pointing_stick"],
        ),
        (
            "a keyboard with a mouse function",
            with(mouse(), |c| c.key.extend(keyboard_keys())),
            &["mouse", "keyboard", "key"],
            &[],
        ),
        (
            "a scroll wheel alone",
            Caps {
                ev: vec![EV_REL],
                rel: vec![REL_WHEEL],
                ..Caps::default()
            },
            &["key"],
            &["mouse"],
        ),
        (
            "a horizontal wheel alone",
            Caps {
                ev: vec![EV_REL],
                rel: vec![REL_HWHEEL],
                ..Caps::default()
            },
            &["key"],
            &[],
        ),
        (
            "motion without a wheel or buttons",
            Caps {
                ev: vec![EV_REL],
                rel: vec![REL_X, REL_Y],
                ..Caps::default()
            },
            &[],
            &["key", "mouse"],
        ),
        (
            "a finger pad is a touchpad",
            touchpad(),
            &["touchpad"],
            &["touchscreen", "mouse", "clickpad"],
        ),
        (
            "a direct pad is a touchscreen",
            with(touchpad(), |c| {
                c.key = vec![BTN_TOUCH];
                c.props = vec![INPUT_PROP_DIRECT];
            }),
            &["touchscreen"],
            &["touchpad"],
        ),
        (
            "a finger tool on a direct device is still a screen",
            with(touchpad(), |c| c.props = vec![INPUT_PROP_DIRECT]),
            &["touchscreen"],
            &["touchpad"],
        ),
        (
            "a button pad is a clickpad",
            with(touchpad(), |c| {
                c.props = vec![INPUT_PROP_POINTER, INPUT_PROP_BUTTONPAD];
            }),
            &["clickpad", "touchpad"],
            &[],
        ),
        (
            "a single-touch pad with only absolute axes",
            Caps {
                ev: vec![EV_KEY, EV_ABS],
                key: vec![BTN_TOOL_FINGER, BTN_TOUCH],
                abs: vec![ABS_X, ABS_Y],
                ..Caps::default()
            },
            &["touchpad"],
            &[],
        ),
        (
            "multitouch axes claiming every slot axis are a fake",
            with(touchpad(), |c| {
                c.abs = vec![
                    ABS_MT_SLOT,
                    ABS_MT_SLOT - 1,
                    ABS_MT_POSITION_X,
                    ABS_MT_POSITION_Y,
                ];
            }),
            &[],
            &["touchpad"],
        ),
        (
            "a pen makes a tablet",
            Caps {
                ev: vec![EV_KEY, EV_ABS],
                key: vec![BTN_TOOL_PEN, BTN_STYLUS, BTN_TOUCH, BTN_LEFT],
                abs: vec![ABS_X, ABS_Y, ABS_PRESSURE],
                ..Caps::default()
            },
            &["tablet"],
            &["mouse", "touchpad", "tablet_pad"],
        ),
        (
            "a pen turns pad buttons into the pen's",
            Caps {
                ev: vec![EV_KEY, EV_ABS],
                key: vec![BTN_TOOL_PEN, BTN_STYLUS, BTN_0, BTN_1],
                abs: vec![ABS_X, ABS_Y],
                ..Caps::default()
            },
            &["tablet"],
            &["tablet_pad"],
        ),
        (
            "a stylus without a pen tool, with pad buttons",
            Caps {
                ev: vec![EV_KEY, EV_ABS],
                key: vec![BTN_STYLUS, BTN_0, BTN_1],
                abs: vec![ABS_X, ABS_Y],
                ..Caps::default()
            },
            &["tablet", "tablet_pad"],
            &[],
        ),
        (
            "buttons and a wheel without motion",
            Caps {
                ev: vec![EV_KEY, EV_REL],
                key: vec![BTN_0, BTN_1],
                rel: vec![REL_WHEEL],
                ..Caps::default()
            },
            &["tablet", "tablet_pad"],
            &["joystick", "mouse"],
        ),
        (
            "a virtual machine's absolute pointer",
            Caps {
                ev: vec![EV_KEY, EV_ABS],
                key: vec![BTN_LEFT, BTN_RIGHT],
                abs: vec![ABS_X, ABS_Y],
                ..Caps::default()
            },
            &["mouse"],
            &["touchscreen", "tablet"],
        ),
        (
            "a mouse on the i2c bus",
            with(mouse(), |c| c.bustype = 0x18),
            &["mouse", "pointing_stick"],
            &[],
        ),
        (
            "a mouse on usb",
            with(mouse(), |c| c.bustype = 0x03),
            &["mouse"],
            &["pointing_stick"],
        ),
        (
            "the pointing stick property",
            with(mouse(), |c| c.props = vec![INPUT_PROP_POINTER, 5]),
            &["pointing_stick"],
            &[],
        ),
        (
            "the accelerometer property",
            Caps {
                ev: vec![EV_ABS],
                abs: vec![ABS_X, ABS_Y],
                props: vec![INPUT_PROP_ACCELEROMETER],
                ..Caps::default()
            },
            &["accelerometer"],
            &["mouse"],
        ),
        (
            "three axes alone",
            Caps {
                ev: vec![EV_ABS],
                abs: vec![ABS_X, ABS_Y, ABS_Z],
                ..Caps::default()
            },
            &["accelerometer"],
            &[],
        ),
        (
            "three axes with keys",
            Caps {
                ev: vec![EV_KEY, EV_ABS],
                key: vec![BTN_TOUCH],
                abs: vec![ABS_X, ABS_Y, ABS_Z],
                ..Caps::default()
            },
            &[],
            &["accelerometer"],
        ),
        (
            "stick axes and buttons",
            gamepad(),
            &["joystick"],
            &["mouse", "touchpad", "keyboard"],
        ),
        (
            "joystick buttons without the axes",
            keys(vec![BTN_JOYSTICK, BTN_THUMB]),
            &["joystick"],
            &[],
        ),
        (
            "a single stray joystick button",
            keys(vec![BTN_JOYSTICK]),
            &[],
            &["joystick"],
        ),
        (
            "a keyboard with stray joystick buttons",
            with(gamepad(), |c| {
                // control, caps lock, num lock, insert: four of the well-known keyboard keys
                c.key.extend([29, 58, 69, 110]);
                c.key.extend(keyboard_keys());
            }),
            &["keyboard"],
            &["joystick"],
        ),
        (
            "a gamepad with a few keyboard keys",
            with(gamepad(), |c| c.key.extend([KEY_A, 29, 58])),
            &["joystick"],
            &[],
        ),
        (
            "a mouse whose buttons run into the joystick's",
            with(mouse(), |c| c.key = (BTN_LEFT..BTN_LEFT + 20).collect()),
            &["mouse"],
            &["joystick"],
        ),
        (
            "a switch device",
            Caps {
                ev: vec![EV_SW],
                ..Caps::default()
            },
            &["switch"],
            &["key"],
        ),
        (
            "several things at once",
            with(touchpad(), |c| {
                c.ev.push(EV_SW);
                c.key.extend(keyboard_keys());
            }),
            &["touchpad", "keyboard", "switch"],
            &[],
        ),
    ];
    for (name, caps, set, unset) in rows {
        let class = caps.class();
        if set.is_empty() && unset.is_empty() {
            assert_eq!(class, InputClass::default(), "{name}");
        }
        for (flag, value) in flags(&class) {
            if set.contains(&flag) {
                assert!(value, "{name}: {flag} should be set");
            }
            if unset.contains(&flag) {
                assert!(!value, "{name}: {flag} should be clear");
            }
        }
    }
}
