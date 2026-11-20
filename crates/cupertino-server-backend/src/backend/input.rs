//! Input for the TTY backend: evdev nodes opened through the session, hotplug from kernel uevents,
//! and an [`InputBackend`] that turns evdev frames into the events `events.rs` consumes.
//!
//! Units on the way out:
//! - Mouse motion is raw device counts.
//! - Touchpad contacts are converted to millimetres with the axis resolution and run through the
//!   `cupertino-input` touchpad model, whose pointer motion and scroll leave in trackpad counts
//!   (400 per inch), the unit `PointerDevice` accelerates.
//! - Swipe deltas are in thousandths of an inch, the unit of `GestureConfig`.
//!
//! Every handler runs on the main loop. The shared [`Inputs`] table is borrowed only while frames
//! are translated; events are delivered to the compositor after the borrow ends.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use crate::session::LogindSession;
use anyhow::{anyhow, Context, Result};
use cupertino_devices::evdev::codes::{
    is_button, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_PRESSURE, ABS_MT_SLOT, ABS_X, ABS_Y,
    BTN_LEFT, BTN_RIGHT, BTN_TASK,
};
use cupertino_devices::evdev::{
    AbsPoint, Contact, Device as EvdevDevice, DeviceInfo, Frame, KeyEvent as EvdevKey,
    KeyState as EvdevKeyState, Resolution, Switch as EvdevSwitch, SwitchEvent as EvdevSwitchEvent,
    Tool,
};
use cupertino_devices::{
    classify_device, enumerate, Action, DevT, Device as SysDevice, DeviceEvent, InputClass,
    Subsystem,
};
use cupertino_input::{
    SwipeAxis, TouchContact, Touchpad, TouchpadButton, TouchpadEvent, TouchpadFrame,
    TouchpadGeometry, TouchpadSettings, SWIPE_COMPLETE_MM,
};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisRelativeDirection, AxisSource, ButtonState, Device,
    DeviceCapability, Event, GestureBeginEvent, GestureEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
    GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, Keycode,
    PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent, PointerMotionEvent, Switch,
    SwitchState, SwitchToggleEvent, UnusedEvent,
};
use smithay::backend::session::Session;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::rustix::io::Errno;

/// Compositor hooks called by the evdev input backend as devices open, emit events, and close.
pub trait EvdevHost: 'static {
    /// Current compositor monotonic clock time.
    fn evdev_clock_now(&self) -> Duration;
    /// Whether natural scrolling is enabled for touchpads (`touchpad == true`) or mice (`false`).
    fn evdev_natural_scrolling_for(&self, touchpad: bool) -> bool;
    /// Mark a resynced already-pressed keycode so accessibility key filters pass it through.
    fn evdev_pass_unfiltered_key(&mut self, code: u32);
    /// Deliver one translated evdev input event to the compositor.
    fn evdev_process_input_event(&mut self, event: InputEvent<EvdevBackend>);
}

const SYSFS: &str = "/sys";

/// Size assumed for a touchpad axis that reports no resolution, in millimetres (width, height).
///
/// approximate: libinput assumes a 69 mm by 50 mm sensor for a touchpad with no resolution, no
/// size hint and no hardware-database entry, because such pads are old and small
/// (`tp_init_default_resolution` in `src/evdev-mt-touchpad.c`; the 69 mm is "1 under palm
/// detection"). We take the same size so the pad behaves as it did under libinput.
const FALLBACK_PAD_SIZE_MM: (f64, f64) = (69.0, 50.0);

/// Tracking ids of the synthesised contacts start here, above any id a kernel driver assigns.
const FAKE_CONTACT_ID_BASE: u32 = 1 << 31;

/// Swipe deltas are measured in thousandths of an inch, the unit the gesture recognizer is tuned in.
const GESTURE_UNITS_PER_MM: f64 = 1000.0 / 25.4;

// ---- the input backend ---------------------------------------------------------------------------

/// The [`InputBackend`] of the TTY session: events come from evdev frames.
#[derive(Debug)]
pub struct EvdevBackend;

/// What an input node is, from its class and identity at open time.
#[derive(Debug)]
struct DeviceData {
    sysname: String,
    name: String,
    syspath: PathBuf,
    /// `(product, vendor)`, the order of [`Device::usb_id`].
    usb_id: (u32, u32),
    class: InputClass,
}

/// One evdev node. Clones share the identity; two devices are equal when their node names are.
#[derive(Clone, Debug)]
pub struct InputDevice(Rc<DeviceData>);

impl InputDevice {
    fn new(node: &SysDevice, class: InputClass, info: &DeviceInfo) -> Self {
        Self(Rc::new(DeviceData {
            sysname: node.sysname().to_owned(),
            name: info.name.clone(),
            syspath: node.syspath().to_owned(),
            usb_id: (u32::from(info.id.product), u32::from(info.id.vendor)),
            class,
        }))
    }
}

impl PartialEq for InputDevice {
    fn eq(&self, other: &Self) -> bool {
        self.0.sysname == other.0.sysname
    }
}

impl Eq for InputDevice {}

impl Hash for InputDevice {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.sysname.hash(state);
    }
}

/// Whether the compositor routes anything from a node of this class. Touch screens, tablets,
/// joysticks and motion sensors are not opened.
fn is_routed(class: &InputClass) -> bool {
    class.key || is_pointer(class) || class.switch
}

fn is_pointer(class: &InputClass) -> bool {
    class.mouse || class.touchpad || class.pointing_stick
}

impl Device for InputDevice {
    fn id(&self) -> String {
        self.0.sysname.clone()
    }

    fn name(&self) -> String {
        self.0.name.clone()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        let class = &self.0.class;
        match capability {
            DeviceCapability::Keyboard => class.key,
            DeviceCapability::Pointer => is_pointer(class),
            DeviceCapability::Gesture => class.touchpad,
            DeviceCapability::Switch => class.switch,
            DeviceCapability::Touch
            | DeviceCapability::TabletTool
            | DeviceCapability::TabletPad => false,
        }
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        Some(self.0.usb_id)
    }

    fn syspath(&self) -> Option<PathBuf> {
        Some(self.0.syspath.clone())
    }
}

/// The device and timestamp every event carries.
#[derive(Clone, Debug)]
struct Stamp {
    device: InputDevice,
    time_us: u64,
}

macro_rules! impl_event {
    ($($name:ident),+ $(,)?) => {$(
        impl Event<EvdevBackend> for $name {
            fn time(&self) -> u64 {
                self.stamp.time_us
            }

            fn device(&self) -> InputDevice {
                self.stamp.device.clone()
            }
        }
    )+};
}

#[derive(Debug)]
pub struct KeyEvent {
    stamp: Stamp,
    /// The evdev key code.
    code: u32,
    state: KeyState,
    /// Keys held on this device after the event.
    count: u32,
    /// The transition comes from re-reading the device (at open or after `SYN_DROPPED`), not from a key the user
    /// pressed or released just now: a key held while the session became active comes out as such a press.
    resynced: bool,
}

#[derive(Debug)]
pub struct MotionEvent {
    stamp: Stamp,
    /// Counts, not accelerated: the compositor applies the curves.
    delta: (f64, f64),
}

#[derive(Debug)]
pub struct AbsoluteMotionEvent {
    stamp: Stamp,
    /// Position as a fraction of the axis range, 0 to just below 1.
    position: (f64, f64),
}

#[derive(Debug)]
pub struct ButtonEvent {
    stamp: Stamp,
    code: u32,
    state: ButtonState,
}

/// Per-axis values of an axis event; `None` for an axis the event does not carry.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Axes {
    horizontal: Option<f64>,
    vertical: Option<f64>,
}

impl Axes {
    fn get(&self, axis: Axis) -> Option<f64> {
        match axis {
            Axis::Horizontal => self.horizontal,
            Axis::Vertical => self.vertical,
        }
    }

    /// An axis that did not move is not part of the event.
    fn moved(horizontal: f64, vertical: f64) -> Self {
        let present = |value: f64| (value != 0.0).then_some(value);
        Self {
            horizontal: present(horizontal),
            vertical: present(vertical),
        }
    }
}

#[derive(Debug)]
pub struct AxisEvent {
    stamp: Stamp,
    source: AxisSource,
    /// Content displacement for finger scrolling; `None` for a wheel.
    amount: Axes,
    /// Wheel travel in 1/120 detents; `None` for finger scrolling.
    v120: Axes,
    direction: AxisRelativeDirection,
}

/// Begin of a swipe or pinch.
#[derive(Debug)]
pub struct GestureBegin {
    stamp: Stamp,
    fingers: u32,
}

/// End of a swipe or pinch.
#[derive(Debug)]
pub struct GestureEnd {
    stamp: Stamp,
    cancelled: bool,
}

#[derive(Debug)]
pub struct SwipeUpdate {
    stamp: Stamp,
    delta: (f64, f64),
}

#[derive(Debug)]
pub struct PinchUpdate {
    stamp: Stamp,
    delta: (f64, f64),
    scale: f64,
    /// Degrees since the previous update.
    rotation: f64,
}

#[derive(Debug)]
pub struct SwitchEvent {
    stamp: Stamp,
    switch: Switch,
    state: SwitchState,
}

impl_event!(
    KeyEvent,
    MotionEvent,
    AbsoluteMotionEvent,
    ButtonEvent,
    AxisEvent,
    GestureBegin,
    GestureEnd,
    SwipeUpdate,
    PinchUpdate,
    SwitchEvent
);

impl KeyboardKeyEvent<EvdevBackend> for KeyEvent {
    fn key_code(&self) -> Keycode {
        // xkb keycodes are evdev codes plus 8
        Keycode::new(self.code + 8)
    }

    fn state(&self) -> KeyState {
        self.state
    }

    fn count(&self) -> u32 {
        self.count
    }
}

impl PointerMotionEvent<EvdevBackend> for MotionEvent {
    fn delta_x(&self) -> f64 {
        self.delta.0
    }

    fn delta_y(&self) -> f64 {
        self.delta.1
    }

    fn delta_x_unaccel(&self) -> f64 {
        self.delta.0
    }

    fn delta_y_unaccel(&self) -> f64 {
        self.delta.1
    }
}

impl AbsolutePositionEvent<EvdevBackend> for AbsoluteMotionEvent {
    fn x(&self) -> f64 {
        self.position.0
    }

    fn y(&self) -> f64 {
        self.position.1
    }

    fn x_transformed(&self, width: i32) -> f64 {
        self.position.0 * f64::from(width)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        self.position.1 * f64::from(height)
    }
}

impl PointerMotionAbsoluteEvent<EvdevBackend> for AbsoluteMotionEvent {}

impl PointerButtonEvent<EvdevBackend> for ButtonEvent {
    fn button_code(&self) -> u32 {
        self.code
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

impl PointerAxisEvent<EvdevBackend> for AxisEvent {
    fn amount(&self, axis: Axis) -> Option<f64> {
        self.amount.get(axis)
    }

    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        self.v120.get(axis)
    }

    fn source(&self) -> AxisSource {
        self.source
    }

    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        self.direction
    }
}

impl GestureBeginEvent<EvdevBackend> for GestureBegin {
    fn fingers(&self) -> u32 {
        self.fingers
    }
}

impl GestureSwipeBeginEvent<EvdevBackend> for GestureBegin {}

impl GesturePinchBeginEvent<EvdevBackend> for GestureBegin {}

impl GestureEndEvent<EvdevBackend> for GestureEnd {
    fn cancelled(&self) -> bool {
        self.cancelled
    }
}

impl GestureSwipeEndEvent<EvdevBackend> for GestureEnd {}

impl GesturePinchEndEvent<EvdevBackend> for GestureEnd {}

impl GestureSwipeUpdateEvent<EvdevBackend> for SwipeUpdate {
    fn delta_x(&self) -> f64 {
        self.delta.0
    }

    fn delta_y(&self) -> f64 {
        self.delta.1
    }
}

impl GesturePinchUpdateEvent<EvdevBackend> for PinchUpdate {
    fn delta_x(&self) -> f64 {
        self.delta.0
    }

    fn delta_y(&self) -> f64 {
        self.delta.1
    }

    fn scale(&self) -> f64 {
        self.scale
    }

    fn rotation(&self) -> f64 {
        self.rotation
    }
}

impl SwitchToggleEvent<EvdevBackend> for SwitchEvent {
    fn switch(&self) -> Option<Switch> {
        Some(self.switch)
    }

    fn state(&self) -> SwitchState {
        self.state
    }
}

impl InputBackend for EvdevBackend {
    type Device = InputDevice;
    type KeyboardKeyEvent = KeyEvent;
    type PointerAxisEvent = AxisEvent;
    type PointerButtonEvent = ButtonEvent;
    type PointerMotionEvent = MotionEvent;
    type PointerMotionAbsoluteEvent = AbsoluteMotionEvent;
    type GestureSwipeBeginEvent = GestureBegin;
    type GestureSwipeUpdateEvent = SwipeUpdate;
    type GestureSwipeEndEvent = GestureEnd;
    type GesturePinchBeginEvent = GestureBegin;
    type GesturePinchUpdateEvent = PinchUpdate;
    type GesturePinchEndEvent = GestureEnd;
    // The touchpad model has no hold gesture; touch screens and tablets are not opened.
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = SwitchEvent;
    type SpecialEvent = UnusedEvent;
}

type Event_ = InputEvent<EvdevBackend>;

// ---- frames to events ----------------------------------------------------------------------------

/// Axis range of an absolute pointer.
mod translate;
pub use translate::*;
