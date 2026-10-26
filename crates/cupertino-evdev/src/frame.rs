//! The typed output of the framer: everything the device reported between two `SYN_REPORT`s.

use crate::codes::{MT_TOOL_FINGER, MT_TOOL_PALM, MT_TOOL_PEN};

/// What happened to a key or button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyState {
    /// Went down (value 1).
    Pressed,
    /// Went up (value 0).
    Released,
    /// Auto-repeat while held (value 2).
    Repeat,
}

/// A key or button transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// The key or button code (`KEY_*`, `BTN_*`).
    pub code: u16,
    /// The transition.
    pub state: KeyState,
}

/// Summed relative pointer motion of one frame, in device counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Motion {
    /// `REL_X`.
    pub dx: i32,
    /// `REL_Y`.
    pub dy: i32,
}

/// Summed wheel motion of one frame in 1/120 of a detent; the sign is the kernel's (positive is away from the user
/// for the vertical wheel, to the right for the horizontal one).
///
/// A device with `REL_WHEEL_HI_RES` is read through that axis only; its legacy `REL_WHEEL` events are ignored so a
/// detent is never counted twice. A device without it reports each legacy step as 120.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wheel {
    /// Vertical wheel.
    pub vertical: i32,
    /// Horizontal wheel.
    pub horizontal: i32,
}

/// An absolute pointer position (`ABS_X`/`ABS_Y`) in device units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbsPoint {
    /// `ABS_X`.
    pub x: i32,
    /// `ABS_Y`.
    pub y: i32,
}

/// What a multitouch contact is, from `ABS_MT_TOOL_TYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// `MT_TOOL_FINGER`; also what a device without `ABS_MT_TOOL_TYPE` implies.
    Finger,
    /// `MT_TOOL_PEN`.
    Pen,
    /// `MT_TOOL_PALM`.
    Palm,
    /// Any other `MT_TOOL_*` value.
    Other(i32),
}

impl Tool {
    /// Maps a raw `ABS_MT_TOOL_TYPE` value.
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            MT_TOOL_FINGER => Self::Finger,
            MT_TOOL_PEN => Self::Pen,
            MT_TOOL_PALM => Self::Palm,
            other => Self::Other(other),
        }
    }
}

/// One touch contact. Positions are in device units; convert with the axis resolution from
/// [`DeviceInfo`](crate::DeviceInfo).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contact {
    /// The multitouch slot holding the contact.
    pub slot: u32,
    /// The kernel tracking id; unique per touch, while slots are reused.
    pub tracking_id: u32,
    /// `ABS_MT_POSITION_X`.
    pub x: i32,
    /// `ABS_MT_POSITION_Y`.
    pub y: i32,
    /// `ABS_MT_PRESSURE`, when the device has the axis.
    pub pressure: Option<i32>,
    /// `ABS_MT_TOUCH_MAJOR`, when the device has the axis.
    pub touch_major: Option<i32>,
    /// `ABS_MT_TOUCH_MINOR`, when the device has the axis.
    pub touch_minor: Option<i32>,
    /// The contact's tool.
    pub tool: Tool,
}

/// A switch the framer reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// `SW_LID`: on means the lid is closed.
    Lid,
    /// `SW_TABLET_MODE`: on means tablet mode.
    TabletMode,
}

/// A switch transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchEvent {
    /// Which switch.
    pub switch: Switch,
    /// Its new state.
    pub on: bool,
}

/// Everything a device reported up to one `SYN_REPORT`.
///
/// Key, motion, wheel and switch fields are the changes of this frame. Contacts, buttons, the absolute pointer and
/// the finger count are the full current state, so a consumer never has to track them itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The `SYN_REPORT` timestamp in microseconds, on the clock set with `EVIOCSCLOCKID` (monotonic).
    pub time_us: i64,
    /// True when the frame was synthesised by a resync (open or `SYN_DROPPED`): `keys` and `switches` then hold the
    /// differences to the state before it, and everything else is the device's state now.
    pub resynced: bool,
    /// Key and button transitions, in event order.
    pub keys: Vec<KeyEvent>,
    /// Buttons (`BTN_MISC`..=`BTN_GEAR_UP`, except the finger-count tools) held after this frame, ascending. On a
    /// clickpad `BTN_LEFT` here is the physical click.
    pub held_buttons: Vec<u16>,
    /// Relative motion.
    pub motion: Motion,
    /// Wheel motion.
    pub wheel: Wheel,
    /// The single-touch or absolute pointer position; `Some` iff the device has `ABS_X` and `ABS_Y`.
    pub abs_pointer: Option<AbsPoint>,
    /// Every active multitouch contact, ascending by slot. Empty for devices without multitouch.
    pub contacts: Vec<Contact>,
    /// Fingers down from `BTN_TOOL_FINGER`..`BTN_TOOL_QUINTTAP` (0 for none); `Some` iff the device has any of them.
    /// It exceeds `contacts.len()` when the device has fewer slots than fingers.
    pub finger_count: Option<u8>,
    /// Switch transitions.
    pub switches: Vec<SwitchEvent>,
}
