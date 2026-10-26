//! Input device classification: which kind of device an `input` node is.
//!
//! The decision rules follow systemd's udev `input_id` builtin
//! (<https://github.com/systemd/systemd/blob/main/src/udev/udev-builtin-input_id.c>, GPL-2.0-or-later,
//! functions `test_pointers` and `test_key`), so that this crate agrees with what `libinput` and
//! `udev` report on the same hardware. The code here is a Rust formulation of those rules, not a
//! transliteration. The inputs are the capability bitmasks the kernel prints in sysfs.

use std::path::Path;

use cupertino_evdev::codes;

use crate::capabilities::CapabilityMask;
use crate::device::Device;
use crate::error::{Error, Result};
use crate::sysfs;

// The kernel codes live in `cupertino_evdev::codes` as `u16`; the capability masks index with `u32`.
macro_rules! widen {
    ($($name:ident),+ $(,)?) => {$(const $name: u32 = codes::$name as u32;)+};
}

widen!(
    EV_KEY, EV_REL, EV_SW, REL_X, REL_Y, REL_HWHEEL, REL_WHEEL, ABS_X, ABS_Y, ABS_Z, ABS_RX, ABS_PRESSURE, ABS_MT_SLOT, ABS_MT_POSITION_X,
    ABS_MT_POSITION_Y, BTN_MISC, BTN_0, BTN_1, BTN_MOUSE, BTN_JOYSTICK, BTN_DIGI, BTN_TOOL_PEN, BTN_TOOL_FINGER, BTN_TOUCH, BTN_STYLUS, KEY_OK,
    BTN_DPAD_UP, BTN_DPAD_RIGHT, KEY_ALS_TOGGLE, BTN_TRIGGER_HAPPY1, BTN_TRIGGER_HAPPY40, INPUT_PROP_DIRECT, INPUT_PROP_BUTTONPAD,
    INPUT_PROP_POINTING_STICK, INPUT_PROP_ACCELEROMETER,
);

/// `BUS_I2C`: a device on this bus that looks like a mouse is a pointing stick.
const BUS_I2C: u16 = 0x18;

/// Keys that a keyboard has and a joystick does not (control, caps lock, num lock, insert, mute,
/// calculator, file, mail, play/pause, brightness down).
const WELL_KNOWN_KEYBOARD_KEYS: [u32; 10] = [29, 58, 69, 110, 113, 140, 144, 155, 164, 224];

/// The kernel capability data of one `input` device (the directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputCapabilities {
    /// `capabilities/ev`: supported event types.
    pub ev: CapabilityMask,
    /// `capabilities/key`: supported keys and buttons.
    pub key: CapabilityMask,
    /// `capabilities/rel`: supported relative axes.
    pub rel: CapabilityMask,
    /// `capabilities/abs`: supported absolute axes.
    pub abs: CapabilityMask,
    /// `properties`: `INPUT_PROP_*` flags.
    pub props: CapabilityMask,
    /// `id/bustype`.
    pub bustype: u16,
}

/// What kind of device an input node is. Several flags can be set at once (a keyboard with a
/// touchpad built in reports both).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputClass {
    /// Has any key (udev `ID_INPUT_KEY`).
    pub key: bool,
    /// Has the full block of alphanumeric keys (`ID_INPUT_KEYBOARD`).
    pub keyboard: bool,
    /// A mouse, relative or absolute (`ID_INPUT_MOUSE`).
    pub mouse: bool,
    /// An indirect multi-touch or single-touch pad (`ID_INPUT_TOUCHPAD`).
    pub touchpad: bool,
    /// A direct touch surface (`ID_INPUT_TOUCHSCREEN`).
    pub touchscreen: bool,
    /// A pen tablet or tablet pad (`ID_INPUT_TABLET`).
    pub tablet: bool,
    /// The button and ring part of a tablet (`ID_INPUT_TABLET_PAD`).
    pub tablet_pad: bool,
    /// A joystick or gamepad (`ID_INPUT_JOYSTICK`).
    pub joystick: bool,
    /// A pointing stick (`ID_INPUT_POINTINGSTICK`).
    pub pointing_stick: bool,
    /// A motion sensor exposed as an input device (`ID_INPUT_ACCELEROMETER`).
    pub accelerometer: bool,
    /// Has switch events such as the lid or tablet mode (`ID_INPUT_SWITCH`).
    pub switch: bool,
    /// The surface is one physical button (`INPUT_PROP_BUTTONPAD`).
    pub clickpad: bool,
}

/// Reads the capabilities of `dir`, an directory.
fn read_capabilities(dir: &Path) -> Result<InputCapabilities> {
    let mask = |relative: &str| -> Result<CapabilityMask> {
        let path = dir.join(relative);
        CapabilityMask::parse(&sysfs::read_attr(&path)?)
            .map_err(|source| Error::Parse { origin: path.display().to_string(), source })
    };
    Ok(InputCapabilities {
        ev: mask("capabilities/ev")?,
        key: mask("capabilities/key")?,
        rel: mask("capabilities/rel")?,
        abs: mask("capabilities/abs")?,
        props: mask("properties")?,
        bustype: sysfs::read_hex_u16(&dir.join("id/bustype"), "bustype")?,
    })
}

/// Classifies an `event*` node, like udev's `input_id` run on it. The capabilities belong to the
/// device above the node, so this walks up to the first directory that has
/// `capabilities/ev`. `None` when the device has no such ancestor (it is not an input device).
pub fn classify_device(device: &Device, sysfs_root: &Path) -> Result<Option<InputClass>> {
    let devices_dir = sysfs::devices_dir(sysfs_root)?;
    let mut candidates = vec![device.syspath().to_owned()];
    candidates.extend(sysfs::ancestor_dirs(device.syspath(), &devices_dir));
    candidates
        .iter()
        .find(|dir| dir.join("capabilities/ev").is_file())
        .map(|dir| Ok(classify(&read_capabilities(dir)?)))
        .transpose()
}

/// Classifies a device from its capabilities.
pub fn classify(caps: &InputCapabilities) -> InputClass {
    let mut class = InputClass::default();
    let is_pointer = classify_pointer(caps, &mut class);
    let is_key = classify_key(caps, &mut class);

    // Some evdev nodes carry only a scroll wheel; they count as key devices.
    let has_wheel_axis = caps.ev.test(EV_REL)
        && (caps.rel.test(REL_WHEEL) || caps.rel.test(REL_HWHEEL));
    if !is_pointer && !is_key && has_wheel_axis {
        class.key = true;
    }
    class.switch = caps.ev.test(EV_SW);
    class.clickpad = caps.props.test(INPUT_PROP_BUTTONPAD);
    class
}

/// Sets the pointer-like flags; returns whether the device is a pointer of any kind.
fn classify_pointer(caps: &InputCapabilities, class: &mut InputClass) -> bool {
    let key = |bit| caps.key.test(bit);
    let abs = |bit| caps.abs.test(bit);

    let has_keys = caps.ev.test(EV_KEY);
    let has_abs_xy = abs(ABS_X) && abs(ABS_Y);
    let has_abs_xyz = has_abs_xy && abs(ABS_Z);

    // A device with three absolute axes and no keys is a motion sensor.
    if caps.props.test(INPUT_PROP_ACCELEROMETER) || (!has_keys && has_abs_xyz) {
        class.accelerometer = true;
        return true;
    }

    let mut pointing_stick = caps.props.test(INPUT_PROP_POINTING_STICK);
    let has_stylus = key(BTN_STYLUS);
    let has_pen = key(BTN_TOOL_PEN);
    let finger_but_no_pen = key(BTN_TOOL_FINGER) && !has_pen;
    let has_mouse_button = caps.key.any_in(BTN_MOUSE..BTN_JOYSTICK);
    let has_rel_xy = caps.ev.test(EV_REL) && caps.rel.test(REL_X) && caps.rel.test(REL_Y);
    let is_direct = caps.props.test(INPUT_PROP_DIRECT);
    let has_touch = key(BTN_TOUCH);
    let has_pad_buttons = key(BTN_0) && key(BTN_1) && !has_pen;
    let has_wheel = caps.ev.test(EV_REL) && (caps.rel.test(REL_WHEEL) || caps.rel.test(REL_HWHEEL));

    // Multi-touch coordinates, unless the device claims every axis (then it is a fake).
    let has_mt_xy = abs(ABS_MT_POSITION_X)
        && abs(ABS_MT_POSITION_Y)
        && !(abs(ABS_MT_SLOT) && abs(ABS_MT_SLOT - 1));

    // A mouse with more than 16 buttons runs into the joystick button range; skip those.
    let num_joystick_buttons = if key(BTN_JOYSTICK - 1) {
        0
    } else {
        caps.key.count_in(BTN_JOYSTICK..BTN_DIGI)
            + caps.key.count_in(BTN_TRIGGER_HAPPY1..BTN_TRIGGER_HAPPY40 + 1)
            + caps.key.count_in(BTN_DPAD_UP..BTN_DPAD_RIGHT + 1)
    };
    let num_joystick_axes = caps.abs.count_in(ABS_RX..ABS_PRESSURE);
    let has_joystick_inputs = num_joystick_buttons > 0 || num_joystick_axes > 0;

    let mut absolute_mouse = false;
    if has_abs_xy {
        if has_stylus || has_pen {
            class.tablet = true;
        } else if finger_but_no_pen && !is_direct {
            class.touchpad = true;
        } else if has_mouse_button {
            // E.g. a virtual machine's pointer: absolute axes but no touch button.
            absolute_mouse = true;
        } else if has_touch || is_direct {
            class.touchscreen = true;
        } else if has_joystick_inputs {
            class.joystick = true;
        }
    } else if has_joystick_inputs {
        class.joystick = true;
    }
    if has_mt_xy {
        if has_stylus || has_pen {
            class.tablet = true;
        } else if finger_but_no_pen && !is_direct {
            class.touchpad = true;
        } else if has_touch || is_direct {
            class.touchscreen = true;
        }
    }

    if class.tablet && has_pad_buttons {
        class.tablet_pad = true;
    }
    // A pad with buttons and a wheel but no pointer motion is a tablet pad on its own.
    if has_pad_buttons && has_wheel && !has_rel_xy {
        class.tablet = true;
        class.tablet_pad = true;
    }

    let mut mouse = !class.tablet
        && !class.touchpad
        && !class.joystick
        && has_mouse_button
        && (has_rel_xy || !has_abs_xy);
    // There is no such thing as an I2C mouse.
    if mouse && caps.bustype == BUS_I2C {
        pointing_stick = true;
    }

    // Some keyboards set random joystick buttons; a joystick needs at least two inputs and few
    // well-known keyboard keys.
    if class.joystick {
        let well_known_keys = if has_keys {
            WELL_KNOWN_KEYBOARD_KEYS.iter().filter(|&&code| key(code)).count()
        } else {
            0
        };
        if well_known_keys >= 4 || num_joystick_buttons + num_joystick_axes < 2 {
            class.joystick = false;
        }
        if has_wheel && has_pad_buttons {
            class.joystick = false;
        }
    }

    mouse |= absolute_mouse;
    class.mouse = mouse;
    class.pointing_stick = pointing_stick;
    class.tablet
        || class.mouse
        || class.touchpad
        || class.touchscreen
        || class.joystick
        || class.pointing_stick
}

/// Sets `key` and `keyboard`; returns whether the device counts as a key device.
fn classify_key(caps: &InputCapabilities, class: &mut InputClass) -> bool {
    if !caps.ev.test(EV_KEY) {
        return false;
    }
    // Any KEY_* below the button range, or in one of the two key blocks above it.
    class.key = caps.key.any_in(0..BTN_MISC)
        || caps.key.any_in(KEY_OK..BTN_DPAD_UP)
        || caps.key.any_in(KEY_ALS_TOGGLE..BTN_TRIGGER_HAPPY1);
    // The first 32 bits are ESC, the digits and Q to D; all of them (KEY_RESERVED aside) make a
    // full keyboard.
    class.keyboard = caps.key.low_word32() & 0xffff_fffe == 0xffff_fffe;
    class.keyboard || class.key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(bits: &[u32]) -> CapabilityMask {
        CapabilityMask::from_bits(bits)
    }

    fn caps(ev: &[u32], key: &[u32], rel: &[u32], abs: &[u32], props: &[u32]) -> InputCapabilities {
        InputCapabilities {
            ev: mask(ev),
            key: mask(key),
            rel: mask(rel),
            abs: mask(abs),
            props: mask(props),
            bustype: 0x03,
        }
    }

    fn only(f: impl FnOnce(&mut InputClass)) -> InputClass {
        let mut class = InputClass::default();
        f(&mut class);
        class
    }

    #[test]
    fn full_keyboard() {
        let keys: Vec<u32> = (1..=31).collect();
        let class = classify(&caps(&[EV_KEY], &keys, &[], &[], &[]));
        assert_eq!(class, only(|c| { c.key = true; c.keyboard = true; }));
    }

    #[test]
    fn mouse_with_wheel() {
        let class = classify(&caps(
            &[EV_KEY, EV_REL],
            &[BTN_MOUSE, BTN_MOUSE + 1],
            &[REL_X, REL_Y, REL_WHEEL],
            &[],
            &[],
        ));
        assert_eq!(class, only(|c| c.mouse = true));
    }

    #[test]
    fn clickpad_touchpad() {
        let class = classify(&caps(
            &[EV_KEY, 3],
            &[BTN_MOUSE, BTN_TOOL_FINGER, BTN_TOUCH],
            &[],
            &[ABS_X, ABS_Y, ABS_MT_POSITION_X, ABS_MT_POSITION_Y],
            &[INPUT_PROP_BUTTONPAD],
        ));
        assert_eq!(class, only(|c| { c.touchpad = true; c.clickpad = true; }));
    }

    #[test]
    fn touchscreen_has_touch_and_no_finger_tool() {
        let class = classify(&caps(
            &[EV_KEY, 3],
            &[BTN_TOUCH],
            &[],
            &[ABS_X, ABS_Y, ABS_MT_POSITION_X, ABS_MT_POSITION_Y],
            &[INPUT_PROP_DIRECT],
        ));
        assert_eq!(class, only(|c| c.touchscreen = true));
    }

    #[test]
    fn pen_tablet_with_pad_buttons_is_tablet_and_pad() {
        // A stylus button makes it a tablet; BTN_0 and BTN_1 without a pen tool add the pad.
        let class = classify(&caps(
            &[EV_KEY, 3],
            &[BTN_0, BTN_1, BTN_STYLUS],
            &[],
            &[ABS_X, ABS_Y, ABS_PRESSURE],
            &[],
        ));
        assert_eq!(class, only(|c| { c.tablet = true; c.tablet_pad = true; }));
    }

    #[test]
    fn joystick_with_buttons_and_axes() {
        let class = classify(&caps(
            &[EV_KEY, 3],
            &[BTN_JOYSTICK, BTN_JOYSTICK + 1],
            &[],
            &[ABS_X, ABS_Y, ABS_RX],
            &[],
        ));
        assert_eq!(class, only(|c| c.joystick = true));
    }

    #[test]
    fn keyboard_with_stray_joystick_buttons_is_not_a_joystick() {
        // Left control, caps lock, num lock and insert are four well-known keys.
        let mut keys: Vec<u32> = (1..=31).collect();
        keys.extend([58, 69, 110, BTN_JOYSTICK, BTN_JOYSTICK + 1]);
        let class = classify(&caps(&[EV_KEY], &keys, &[], &[], &[]));
        assert_eq!(class, only(|c| { c.key = true; c.keyboard = true; }));
    }

    #[test]
    fn lone_joystick_button_is_not_a_joystick() {
        let class = classify(&caps(&[EV_KEY], &[BTN_JOYSTICK], &[], &[], &[]));
        assert_eq!(class, InputClass::default());
    }

    #[test]
    fn three_axes_without_keys_is_an_accelerometer() {
        let class = classify(&caps(&[3], &[], &[], &[ABS_X, ABS_Y, ABS_Z], &[]));
        assert_eq!(class, only(|c| c.accelerometer = true));
    }

    #[test]
    fn lid_switch() {
        let class = classify(&caps(&[EV_SW], &[], &[], &[], &[]));
        assert_eq!(class, only(|c| c.switch = true));
    }

    #[test]
    fn i2c_mouse_is_also_a_pointing_stick() {
        let mut stick = caps(&[EV_KEY, EV_REL], &[BTN_MOUSE], &[REL_X, REL_Y], &[], &[]);
        stick.bustype = BUS_I2C;
        assert_eq!(classify(&stick), only(|c| { c.mouse = true; c.pointing_stick = true; }));
    }

    #[test]
    fn wheel_only_node_counts_as_keys() {
        let class = classify(&caps(&[EV_REL], &[], &[REL_WHEEL], &[], &[]));
        assert_eq!(class, only(|c| c.key = true));
    }
}
