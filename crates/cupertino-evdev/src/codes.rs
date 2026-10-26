//! Event codes and sizes from the kernel uAPI headers `linux/input.h` and `linux/input-event-codes.h`.

// Event types (input-event-codes.h, "Event types").
pub const EV_SYN: u16 = 0x00;
pub const EV_KEY: u16 = 0x01;
pub const EV_REL: u16 = 0x02;
pub const EV_ABS: u16 = 0x03;
pub const EV_SW: u16 = 0x05;
/// Highest event type (`EV_MAX`).
pub const EV_MAX: u16 = 0x1f;

// Synchronization events (input-event-codes.h, "Synchronization events").
pub const SYN_REPORT: u16 = 0;
pub const SYN_MT_REPORT: u16 = 2;
pub const SYN_DROPPED: u16 = 3;

// Keys and buttons (input-event-codes.h).
/// Highest key code (`KEY_MAX`).
pub const KEY_MAX: u16 = 0x2ff;
/// First button code (`BTN_MISC`); codes below it are keyboard keys.
pub const BTN_MISC: u16 = 0x100;
pub const BTN_0: u16 = 0x100;
pub const BTN_1: u16 = 0x101;
/// First mouse button (`BTN_MOUSE`, the same code as `BTN_LEFT`).
pub const BTN_MOUSE: u16 = 0x110;
pub const BTN_LEFT: u16 = 0x110;
pub const BTN_RIGHT: u16 = 0x111;
/// Last of the mouse buttons (`BTN_LEFT`..=`BTN_TASK`).
pub const BTN_TASK: u16 = 0x117;
/// First joystick button (`BTN_JOYSTICK`); the mouse block ends below it.
pub const BTN_JOYSTICK: u16 = 0x120;
/// First digitizer code (`BTN_DIGI`, the same code as `BTN_TOOL_PEN`).
pub const BTN_DIGI: u16 = 0x140;
pub const BTN_TOOL_PEN: u16 = 0x140;
/// Last button of the contiguous `BTN_MISC`..`BTN_GEAR_UP` block.
pub const BTN_GEAR_UP: u16 = 0x151;
/// First key code above the button block; codes from here are keyboard keys again.
pub const KEY_OK: u16 = 0x160;
pub const KEY_ALS_TOGGLE: u16 = 0x230;
pub const BTN_DPAD_UP: u16 = 0x220;
pub const BTN_DPAD_RIGHT: u16 = 0x223;
pub const BTN_TRIGGER_HAPPY1: u16 = 0x2c0;
pub const BTN_TRIGGER_HAPPY40: u16 = 0x2e7;
pub const BTN_TOUCH: u16 = 0x14a;
pub const BTN_STYLUS: u16 = 0x14b;
pub const BTN_TOOL_FINGER: u16 = 0x145;
pub const BTN_TOOL_QUINTTAP: u16 = 0x148;
pub const BTN_TOOL_DOUBLETAP: u16 = 0x14d;
pub const BTN_TOOL_TRIPLETAP: u16 = 0x14e;
pub const BTN_TOOL_QUADTAP: u16 = 0x14f;

// Relative axes (input-event-codes.h, "Relative axes").
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_HWHEEL: u16 = 0x06;
pub const REL_WHEEL: u16 = 0x08;
pub const REL_WHEEL_HI_RES: u16 = 0x0b;
pub const REL_HWHEEL_HI_RES: u16 = 0x0c;
/// Highest relative axis (`REL_MAX`).
pub const REL_MAX: u16 = 0x0f;

// Absolute axes (input-event-codes.h, "Absolute axes").
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_Z: u16 = 0x02;
pub const ABS_RX: u16 = 0x03;
pub const ABS_PRESSURE: u16 = 0x18;
pub const ABS_MT_SLOT: u16 = 0x2f;
pub const ABS_MT_TOUCH_MAJOR: u16 = 0x30;
pub const ABS_MT_TOUCH_MINOR: u16 = 0x31;
pub const ABS_MT_POSITION_X: u16 = 0x35;
pub const ABS_MT_POSITION_Y: u16 = 0x36;
pub const ABS_MT_TOOL_TYPE: u16 = 0x37;
pub const ABS_MT_TRACKING_ID: u16 = 0x39;
pub const ABS_MT_PRESSURE: u16 = 0x3a;
/// Highest absolute axis (`ABS_MAX`).
pub const ABS_MAX: u16 = 0x3f;

// Switches (input-event-codes.h, "Switch events").
pub const SW_LID: u16 = 0x00;
pub const SW_TABLET_MODE: u16 = 0x01;
/// Highest switch (`SW_MAX`).
pub const SW_MAX: u16 = 0x10;

// Multitouch tool types (input-event-codes.h, `MT_TOOL_*`).
pub const MT_TOOL_FINGER: i32 = 0;
pub const MT_TOOL_PEN: i32 = 1;
pub const MT_TOOL_PALM: i32 = 2;

// Input properties (input-event-codes.h, `INPUT_PROP_*`).
pub const INPUT_PROP_POINTER: u16 = 0x00;
pub const INPUT_PROP_DIRECT: u16 = 0x01;
pub const INPUT_PROP_BUTTONPAD: u16 = 0x02;
pub const INPUT_PROP_SEMI_MT: u16 = 0x03;
pub const INPUT_PROP_POINTING_STICK: u16 = 0x05;
pub const INPUT_PROP_ACCELEROMETER: u16 = 0x06;
/// Highest input property (`INPUT_PROP_MAX`).
pub const INPUT_PROP_MAX: u16 = 0x1f;

/// `CLOCK_MONOTONIC` (`linux/time.h`), the clock passed to `EVIOCSCLOCKID`.
pub const CLOCK_MONOTONIC: i32 = 1;

/// Whether `code` is a button (`BTN_MISC`..=`BTN_GEAR_UP`) rather than a keyboard key. The single definition of the
/// button range for every consumer of this crate.
pub const fn is_button(code: u16) -> bool {
    code >= BTN_MISC && code <= BTN_GEAR_UP
}

/// Size of the bitmap needed for codes `0..=max`, in bytes.
pub(crate) const fn bitmap_bytes(max: u16) -> usize {
    max as usize / 8 + 1
}
