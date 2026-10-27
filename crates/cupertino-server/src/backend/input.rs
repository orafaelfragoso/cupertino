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

use anyhow::{anyhow, Context, Result};
use cupertino_devices::{
    classify_device, enumerate, Action, DevT, Device as SysDevice, DeviceEvent, InputClass,
    Subsystem,
};
use cupertino_evdev::codes::{
    is_button, ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_PRESSURE, ABS_MT_SLOT, ABS_X, ABS_Y,
    BTN_LEFT, BTN_RIGHT, BTN_TASK,
};
use cupertino_evdev::{
    AbsPoint, Contact, Device as EvdevDevice, DeviceInfo, Frame, KeyEvent as EvdevKey,
    KeyState as EvdevKeyState, Resolution, Switch as EvdevSwitch, SwitchEvent as EvdevSwitchEvent,
    Tool,
};
use cupertino_input::{
    SwipeAxis, TouchContact, Touchpad, TouchpadButton, TouchpadEvent, TouchpadFrame,
    TouchpadGeometry, TouchpadSettings, SWIPE_COMPLETE_MM,
};
use cupertino_seat::LogindSession;
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

use crate::state::Cupertino;

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
struct AbsRange {
    x: (i32, i32),
    y: (i32, i32),
}

impl AbsRange {
    fn position(&self, point: AbsPoint) -> (f64, f64) {
        let fraction =
            |value: i32, (min, max): (i32, i32)| f64::from(value - min) / f64::from(max - min + 1);
        (fraction(point.x, self.x), fraction(point.y, self.y))
    }
}

/// One touchpad axis: where it starts and how many device units make a millimetre.
struct PadAxis {
    min: i32,
    units_per_mm: f64,
}

impl PadAxis {
    /// The axis `code` and its length in millimetres. An axis without a resolution gets
    /// `fallback_mm` as its length, and the log says so.
    fn new(info: &DeviceInfo, code: u16, fallback_mm: f64) -> Result<(Self, f64)> {
        let axis = info
            .abs(code)
            .with_context(|| format!("{} has no absolute axis {code:#x}", info.name))?;
        let range = f64::from(axis.max - axis.min);
        match axis.resolution {
            Resolution::UnitsPerMm(per_mm) => {
                let units_per_mm = f64::from(per_mm.get());
                Ok((
                    Self {
                        min: axis.min,
                        units_per_mm,
                    },
                    range / units_per_mm,
                ))
            }
            Resolution::Unknown => {
                tracing::warn!(device = %info.name, axis = code, size_mm = fallback_mm, "touchpad axis reports no resolution, assuming the libinput fallback size");
                Ok((
                    Self {
                        min: axis.min,
                        units_per_mm: range / fallback_mm,
                    },
                    fallback_mm,
                ))
            }
        }
    }

    fn mm(&self, value: i32) -> f64 {
        f64::from(value - self.min) / self.units_per_mm
    }
}

/// The touchpad model of one device with the conversion from device units to its input.
struct Pad {
    model: Touchpad,
    x: PadAxis,
    y: PadAxis,
    /// Range of `ABS_MT_PRESSURE`, when the device has a usable one.
    pressure: Option<(i32, i32)>,
    /// The device reports contacts in multitouch slots; otherwise it is a single-touch pad whose
    /// one position (`ABS_X`/`ABS_Y`) stands for every finger down.
    multitouch: bool,
    /// Contacts the device can report at once.
    slots: usize,
}

impl Pad {
    /// `Err` when the device has no position axes at all.
    fn new(info: &DeviceInfo, clickpad: bool, settings: TouchpadSettings) -> Result<Self> {
        let multitouch =
            info.abs(ABS_MT_POSITION_X).is_some() && info.abs(ABS_MT_POSITION_Y).is_some();
        let (x_code, y_code) = if multitouch {
            (ABS_MT_POSITION_X, ABS_MT_POSITION_Y)
        } else {
            (ABS_X, ABS_Y)
        };
        let (x, width_mm) = PadAxis::new(info, x_code, FALLBACK_PAD_SIZE_MM.0)?;
        let (y, height_mm) = PadAxis::new(info, y_code, FALLBACK_PAD_SIZE_MM.1)?;
        let pressure = info
            .abs(ABS_MT_PRESSURE)
            .filter(|axis| axis.max > axis.min)
            .map(|axis| (axis.min, axis.max));
        let slots = match info.abs(ABS_MT_SLOT) {
            Some(axis) if multitouch => {
                usize::try_from(axis.max + 1).expect("the framer rejects an empty slot range")
            }
            _ => 1,
        };
        Ok(Self {
            model: Touchpad::new(
                TouchpadGeometry {
                    width_mm,
                    height_mm,
                    clickpad,
                },
                settings,
            ),
            x,
            y,
            pressure,
            multitouch,
            slots,
        })
    }

    /// The model's view of a contact; pens and unknown tools are not fingers.
    fn contact(&self, contact: &Contact) -> Option<TouchContact> {
        let palm = match contact.tool {
            Tool::Finger => false,
            Tool::Palm => true,
            Tool::Pen | Tool::Other(_) => return None,
        };
        let pressure = contact
            .pressure
            .zip(self.pressure)
            .map(|(value, (min, max))| {
                (f64::from(value - min) / f64::from(max - min)).clamp(0.0, 1.0)
            });
        Some(TouchContact {
            id: contact.tracking_id,
            position_mm: (self.x.mm(contact.x), self.y.mm(contact.y)),
            pressure,
            palm,
        })
    }

    /// The fingers down. A single-touch pad reports one position and says how many fingers
    /// (`BTN_TOOL_DOUBLETAP` and up) make it; a multitouch pad with fewer slots than fingers does
    /// the same for the fingers it cannot track. The missing fingers are "fake fingers", as
    /// libinput calls them: each sits where the top-most real finger is.
    fn contacts(&self, frame: &Frame) -> Vec<TouchContact> {
        let fingers = usize::from(frame.finger_count.unwrap_or(0));
        let mut contacts: Vec<TouchContact> = if self.multitouch {
            frame
                .contacts
                .iter()
                .filter_map(|c| self.contact(c))
                .collect()
        } else {
            let single = |point: AbsPoint| TouchContact {
                id: 0,
                position_mm: (self.x.mm(point.x), self.y.mm(point.y)),
                pressure: None,
                palm: false,
            };
            frame
                .abs_pointer
                .filter(|_| fingers > 0)
                .map(single)
                .into_iter()
                .collect()
        };
        if fingers > self.slots {
            let top = contacts
                .iter()
                .filter(|c| !c.palm)
                .min_by(|a, b| a.position_mm.1.total_cmp(&b.position_mm.1))
                .copied();
            if let Some(top) = top {
                for index in contacts.len()..fingers {
                    contacts.push(TouchContact {
                        id: FAKE_CONTACT_ID_BASE + index as u32,
                        ..top
                    });
                }
            }
        }
        contacts
    }

    fn frame(&self, frame: &Frame) -> TouchpadFrame {
        TouchpadFrame {
            time_s: frame.time_us as f64 / 1e6,
            contacts: self.contacts(frame),
            // On a clickpad the left button is the physical click.
            button_down: frame.held_buttons.contains(&BTN_LEFT),
        }
    }
}

/// Timestamps come from `CLOCK_MONOTONIC`, which is never negative.
fn micros(time_us: i64) -> u64 {
    u64::try_from(time_us).expect("evdev timestamps are not negative")
}

fn micros_from_seconds(seconds: f64) -> u64 {
    (seconds * 1e6) as u64
}

/// Turns the frames of one device into input events.
struct Translator {
    device: InputDevice,
    natural_scrolling: bool,
    /// Keys and buttons reported down and not yet up, to release them when the device goes away.
    held: BTreeSet<u16>,
    abs: Option<AbsRange>,
    last_abs: Option<AbsPoint>,
    pad: Option<Pad>,
    /// Axis and last progress of the running three- or four-finger swipe.
    swipe: Option<(SwipeAxis, f64)>,
    /// A finger scroll has begun and not ended.
    scrolling: bool,
    /// A pinch has begun and not ended.
    pinching: bool,
    /// Rotation reported so far in the running pinch, degrees.
    rotation_deg: f64,
}

impl Translator {
    /// Applies the natural scrolling setting to the wheel and the touchpad from the next frame on.
    fn set_natural_scrolling(&mut self, natural_scrolling: bool) {
        self.natural_scrolling = natural_scrolling;
        if let Some(pad) = &mut self.pad {
            pad.model.settings.natural_scrolling = natural_scrolling;
        }
    }

    /// A touchpad that cannot be modelled (no position axes) still gets its buttons: it is driven
    /// as a plain pointer, and the log says why.
    fn new(device: InputDevice, info: &DeviceInfo, natural_scrolling: bool) -> Self {
        let class = device.0.class;
        let pad = class.touchpad.then(|| Pad::new(info, class.clickpad, TouchpadSettings { tap_to_click: true, natural_scrolling, ..TouchpadSettings::default() })).and_then(|pad| {
            pad.map_err(|e| tracing::warn!(device = %info.name, error = %format!("{e:#}"), "touchpad cannot be modelled, using its buttons only")).ok()
        });
        let abs = match (info.abs(ABS_X), info.abs(ABS_Y)) {
            (Some(x), Some(y)) if class.mouse => Some(AbsRange {
                x: (x.min, x.max),
                y: (y.min, y.max),
            }),
            _ => None,
        };
        Self {
            device,
            natural_scrolling,
            held: BTreeSet::new(),
            abs,
            last_abs: None,
            pad,
            swipe: None,
            scrolling: false,
            pinching: false,
            rotation_deg: 0.0,
        }
    }

    fn stamp(&self, time_us: u64) -> Stamp {
        Stamp {
            device: self.device.clone(),
            time_us,
        }
    }

    fn frame(&mut self, frame: &Frame, out: &mut Vec<Event_>) {
        let time_us = micros(frame.time_us);
        for key in &frame.keys {
            self.key(*key, frame.resynced, time_us, out);
        }
        if is_pointer(&self.device.0.class) && self.pad.is_none() {
            self.pointer(frame, time_us, out);
        }
        for switch in &frame.switches {
            self.switch(*switch, time_us, out);
        }
        let events = self
            .pad
            .as_mut()
            .map(|pad| pad.model.process_frame(&pad.frame(frame)));
        for event in events.into_iter().flatten() {
            self.pad_event(event, time_us, out);
        }
    }

    /// Runs the touchpad timers (a held-back tap, a drag release) due at `now_s`.
    fn tick(&mut self, now_s: f64, out: &mut Vec<Event_>) {
        let events = self.pad.as_mut().map(|pad| pad.model.tick(now_s));
        for event in events.into_iter().flatten() {
            self.pad_event(event, micros_from_seconds(now_s), out);
        }
    }

    /// When the touchpad model next needs [`Translator::tick`], in seconds of the frame clock.
    fn next_deadline(&self) -> Option<f64> {
        self.pad.as_ref().and_then(|pad| pad.model.next_deadline())
    }

    /// Cancels the running gesture and releases everything still held, before the device goes away.
    fn release_all(&mut self, time_us: u64, out: &mut Vec<Event_>) {
        let stamp = self.stamp(time_us);
        if self.swipe.take().is_some() {
            out.push(InputEvent::GestureSwipeEnd {
                event: GestureEnd {
                    stamp: stamp.clone(),
                    cancelled: true,
                },
            });
        }
        if std::mem::take(&mut self.pinching) {
            out.push(InputEvent::GesturePinchEnd {
                event: GestureEnd {
                    stamp: stamp.clone(),
                    cancelled: true,
                },
            });
        }
        if std::mem::take(&mut self.scrolling) {
            out.push(self.scroll_stop(stamp));
        }
        for code in std::mem::take(&mut self.held) {
            if is_key_code(code) {
                self.push_key(code, false, false, time_us, out);
            } else {
                self.push_button(code, false, time_us, out);
            }
        }
    }

    fn key(&mut self, key: EvdevKey, resynced: bool, time_us: u64, out: &mut Vec<Event_>) {
        let pressed = match key.state {
            EvdevKeyState::Pressed => true,
            EvdevKeyState::Released => false,
            // clients repeat keys themselves
            EvdevKeyState::Repeat => return,
        };
        let class = self.device.0.class;
        if is_key_code(key.code) {
            if class.key {
                self.set_held(key.code, pressed);
                self.push_key(key.code, pressed, resynced, time_us, out);
            }
        } else if (BTN_LEFT..=BTN_TASK).contains(&key.code) {
            // The touchpad model turns the physical click into button events.
            if is_pointer(&class) && !(self.pad.is_some() && key.code == BTN_LEFT) {
                self.set_held(key.code, pressed);
                self.push_button(key.code, pressed, time_us, out);
            }
        }
        // The remaining codes are touch, tool and stylus buttons: nothing consumes them.
    }

    fn set_held(&mut self, code: u16, pressed: bool) {
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    fn push_key(
        &self,
        code: u16,
        pressed: bool,
        resynced: bool,
        time_us: u64,
        out: &mut Vec<Event_>,
    ) {
        let count = self.held.iter().filter(|c| is_key_code(**c)).count() as u32;
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        out.push(InputEvent::Keyboard {
            event: KeyEvent {
                stamp: self.stamp(time_us),
                code: u32::from(code),
                state,
                count,
                resynced,
            },
        });
    }

    fn push_button(&self, code: u16, pressed: bool, time_us: u64, out: &mut Vec<Event_>) {
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        out.push(InputEvent::PointerButton {
            event: ButtonEvent {
                stamp: self.stamp(time_us),
                code: u32::from(code),
                state,
            },
        });
    }

    /// Relative motion, wheels and an absolute position of a mouse.
    fn pointer(&mut self, frame: &Frame, time_us: u64, out: &mut Vec<Event_>) {
        if frame.motion.dx != 0 || frame.motion.dy != 0 {
            let delta = (f64::from(frame.motion.dx), f64::from(frame.motion.dy));
            out.push(InputEvent::PointerMotion {
                event: MotionEvent {
                    stamp: self.stamp(time_us),
                    delta,
                },
            });
        }
        // The kernel counts a wheel turned away from the user as positive; the axis points the
        // other way. Natural scrolling flips both wheels, as it flips the touchpad.
        let sign = if self.natural_scrolling { -1.0 } else { 1.0 };
        let v120 = Axes::moved(
            f64::from(frame.wheel.horizontal) * sign,
            -f64::from(frame.wheel.vertical) * sign,
        );
        if v120 != Axes::default() {
            let axis = AxisEvent {
                stamp: self.stamp(time_us),
                source: AxisSource::Wheel,
                amount: Axes::default(),
                v120,
                direction: self.direction(),
            };
            out.push(InputEvent::PointerAxis { event: axis });
        }
        if let (Some(range), Some(point)) = (&self.abs, frame.abs_pointer) {
            // The first frame of a device only reports where it is.
            if !frame.resynced && self.last_abs != Some(point) {
                out.push(InputEvent::PointerMotionAbsolute {
                    event: AbsoluteMotionEvent {
                        stamp: self.stamp(time_us),
                        position: range.position(point),
                    },
                });
            }
            self.last_abs = Some(point);
        }
    }

    fn direction(&self) -> AxisRelativeDirection {
        if self.natural_scrolling {
            AxisRelativeDirection::Inverted
        } else {
            AxisRelativeDirection::Identical
        }
    }

    fn switch(&self, switch: EvdevSwitchEvent, time_us: u64, out: &mut Vec<Event_>) {
        let kind = match switch.switch {
            EvdevSwitch::Lid => Switch::Lid,
            EvdevSwitch::TabletMode => Switch::TabletMode,
        };
        let state = if switch.on {
            SwitchState::On
        } else {
            SwitchState::Off
        };
        out.push(InputEvent::SwitchToggle {
            event: SwitchEvent {
                stamp: self.stamp(time_us),
                switch: kind,
                state,
            },
        });
    }

    /// Zero on both axes ends a finger scroll; clients run their own momentum.
    fn scroll_stop(&self, stamp: Stamp) -> Event_ {
        let amount = Axes {
            horizontal: Some(0.0),
            vertical: Some(0.0),
        };
        InputEvent::PointerAxis {
            event: AxisEvent {
                stamp,
                source: AxisSource::Finger,
                amount,
                v120: Axes::default(),
                direction: self.direction(),
            },
        }
    }

    fn pad_event(&mut self, event: TouchpadEvent, time_us: u64, out: &mut Vec<Event_>) {
        let stamp = self.stamp(time_us);
        match event {
            TouchpadEvent::PointerMotion { dx, dy } => out.push(InputEvent::PointerMotion {
                event: MotionEvent {
                    stamp,
                    delta: (dx, dy),
                },
            }),
            TouchpadEvent::Button { button, pressed } => {
                let code = match button {
                    TouchpadButton::Left => BTN_LEFT,
                    TouchpadButton::Right => BTN_RIGHT,
                };
                self.set_held(code, pressed);
                self.push_button(code, pressed, time_us, out);
            }
            // A scroll starts with its first update.
            TouchpadEvent::ScrollBegin => {}
            TouchpadEvent::ScrollUpdate { dx, dy } => {
                self.scrolling = true;
                // The model reports how the content moves; an axis value is the opposite.
                let amount = Axes::moved(-dx, -dy);
                out.push(InputEvent::PointerAxis {
                    event: AxisEvent {
                        stamp,
                        source: AxisSource::Finger,
                        amount,
                        v120: Axes::default(),
                        direction: self.direction(),
                    },
                });
            }
            TouchpadEvent::ScrollEnd { .. } => {
                self.scrolling = false;
                out.push(self.scroll_stop(stamp));
            }
            TouchpadEvent::SwipeBegin { fingers, axis } => {
                self.swipe = Some((axis, 0.0));
                out.push(InputEvent::GestureSwipeBegin {
                    event: GestureBegin { stamp, fingers },
                });
            }
            TouchpadEvent::SwipeUpdate { progress, .. } => {
                let (axis, last) = self
                    .swipe
                    .as_mut()
                    .expect("the touchpad model begins a swipe before updating it");
                let travel = (progress - *last) * SWIPE_COMPLETE_MM * GESTURE_UNITS_PER_MM;
                *last = progress;
                let delta = match axis {
                    SwipeAxis::Horizontal => (travel, 0.0),
                    SwipeAxis::Vertical => (0.0, travel),
                };
                out.push(InputEvent::GestureSwipeUpdate {
                    event: SwipeUpdate { stamp, delta },
                });
            }
            TouchpadEvent::SwipeEnd { committed, .. } => {
                self.swipe = None;
                out.push(InputEvent::GestureSwipeEnd {
                    event: GestureEnd {
                        stamp,
                        cancelled: !committed,
                    },
                });
            }
            TouchpadEvent::PinchBegin { fingers } => {
                self.pinching = true;
                self.rotation_deg = 0.0;
                out.push(InputEvent::GesturePinchBegin {
                    event: GestureBegin { stamp, fingers },
                });
            }
            TouchpadEvent::PinchUpdate {
                scale,
                rotation_deg,
                ..
            } => {
                let rotation = rotation_deg - self.rotation_deg;
                self.rotation_deg = rotation_deg;
                out.push(InputEvent::GesturePinchUpdate {
                    event: PinchUpdate {
                        stamp,
                        delta: (0.0, 0.0),
                        scale,
                        rotation,
                    },
                });
            }
            TouchpadEvent::PinchEnd { committed, .. } => {
                self.pinching = false;
                out.push(InputEvent::GesturePinchEnd {
                    event: GestureEnd {
                        stamp,
                        cancelled: !committed,
                    },
                });
            }
            // Nothing consumes the force click yet.
            TouchpadEvent::ForceClick { .. } => {}
        }
    }
}

/// Keyboard keys: everything outside the button block.
fn is_key_code(code: u16) -> bool {
    !is_button(code)
}

// ---- devices on the main loop ----------------------------------------------------------------------

/// An open device: the translator, its loop source and the descriptor the session handed out.
struct Opened {
    translator: Translator,
    /// The node it was opened from, to tell a reused device number from the same device.
    devnode: PathBuf,
    token: RegistrationToken,
    /// Returned to the session when the device closes. The loop source reads a duplicate.
    session_fd: OwnedFd,
}

/// The pending touchpad timer.
struct ArmedTimer {
    token: RegistrationToken,
    deadline: f64,
}

pub struct Inputs {
    handle: LoopHandle<'static, Cupertino>,
    session: LogindSession,
    devices: HashMap<DevT, Opened>,
    timer: Option<ArmedTimer>,
}

pub type SharedInputs = Rc<RefCell<Inputs>>;

/// An empty device table; [`open_all`] fills it.
pub fn new_inputs(handle: LoopHandle<'static, Cupertino>, session: LogindSession) -> SharedInputs {
    Rc::new(RefCell::new(Inputs {
        handle,
        session,
        devices: HashMap::new(),
        timer: None,
    }))
}

fn now(state: &Cupertino) -> Duration {
    Duration::from(state.clock.now())
}

fn deliver(state: &mut Cupertino, events: Vec<Event_>) {
    for event in events {
        // A key that was already down is state, not a keystroke: it bypasses the accessibility filters (sticky keys
        // would latch the modifiers of the chord that switched to this session).
        if let InputEvent::Keyboard { event: key } = &event {
            if key.resynced && key.state == KeyState::Pressed {
                state.key_filters.pass_unfiltered(key.code);
            }
        }
        state.process_input_event(event);
    }
}

fn close_fd(session: &mut LogindSession, fd: OwnedFd) {
    if let Err(e) = session.close(fd) {
        tracing::warn!(error = %e, "closing an input device failed");
    }
}

/// Reads what the device has queued and translates it.
fn read(
    evdev: &mut EvdevDevice,
    translator: &mut Translator,
    out: &mut Vec<Event_>,
) -> Result<(), cupertino_evdev::Error> {
    let mut frames = Vec::new();
    evdev.read_frames(&mut frames)?;
    for frame in &frames {
        translator.frame(frame, out);
    }
    Ok(())
}

/// Opens every input node and announces it.
pub fn open_all(shared: &SharedInputs, state: &mut Cupertino) -> Result<()> {
    let nodes = enumerate(Path::new(SYSFS), Subsystem::Input)
        .map_err(|e| anyhow!("listing input devices: {e}"))?;
    for node in &nodes {
        open_reporting(shared, node, state);
    }
    Ok(())
}

/// Closes every device, releasing what they hold, for a session that is paused.
pub fn close_all(shared: &SharedInputs, state: &mut Cupertino) {
    let devnums: Vec<DevT> = shared.borrow().devices.keys().copied().collect();
    for devnum in devnums {
        close_device(shared, devnum, true, state);
    }
}

/// Kernel uevent for the `input` subsystem: a node appeared or went away.
pub fn hotplug(shared: &SharedInputs, event: &DeviceEvent, state: &mut Cupertino) {
    let Some(devnum) = event.devnum else { return };
    match event.action {
        Action::Added if event.devnode.as_deref().is_some_and(is_event_node) => {
            let syspath = Path::new(SYSFS).join(event.devpath.trim_start_matches('/'));
            let handle = shared.borrow().handle.clone();
            let shared = shared.clone();
            let path = syspath.clone();
            retry(
                &handle,
                HOTPLUG_RETRY_INTERVAL,
                HOTPLUG_RETRIES,
                state,
                move |state| {
                    // Unplugged again while waiting: nothing to open, and not an error.
                    if !path.exists() {
                        return Ok(());
                    }
                    let node = SysDevice::from_syspath(&path)
                        .map_err(|e| anyhow!("reading the new input device: {e}"))?;
                    open_device(&shared, &node, state)
                },
                move |e| tracing::error!(path = %syspath.display(), error = %format!("{e:#}"), "skipping input device"),
            );
        }
        Action::Removed => close_device(shared, devnum, true, state),
        _ => {}
    }
}

/// How long and how often a hot-plugged node is retried (approximate: udevd normally finishes within
/// milliseconds). The kernel announces a node before udevd has processed it, and logind refuses
/// `TakeDevice` for it (`ENODEV`) until then.
const HOTPLUG_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const HOTPLUG_RETRIES: u32 = 40;

/// Runs `attempt` now and, while it fails, again every `interval` on the loop, up to `retries` more
/// times. `give_up` receives the last failure.
fn retry<S: 'static>(
    handle: &LoopHandle<'static, S>,
    interval: Duration,
    retries: u32,
    state: &mut S,
    mut attempt: impl FnMut(&mut S) -> Result<()> + 'static,
    give_up: impl FnOnce(anyhow::Error) + 'static,
) {
    let Err(failure) = attempt(state) else { return };
    if retries == 0 {
        give_up(failure);
        return;
    }
    let next_handle = handle.clone();
    // The loop wants a `FnMut`, but the timer fires once and hands both callbacks on.
    let mut pending = Some((attempt, give_up));
    let inserted = handle.insert_source(Timer::from_duration(interval), move |_, _, state| {
        if let Some((attempt, give_up)) = pending.take() {
            retry(&next_handle, interval, retries - 1, state, attempt, give_up);
        }
        TimeoutAction::Drop
    });
    if let Err(e) = inserted {
        tracing::error!(error = %e, failure = %format!("{failure:#}"), "scheduling the retry of a failed attempt failed");
    }
}

/// An input node as the kernel names it: its device number and its file. Both must match for a
/// node to be the device that is open, because the kernel reuses device numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NodeKey {
    devnum: DevT,
    devnode: PathBuf,
}

/// What to do to bring the open set in line with the nodes that exist.
#[derive(Debug, Default, PartialEq, Eq)]
struct NodeDiff {
    /// Open devices whose node is gone, or whose device number now belongs to another node.
    close: Vec<NodeKey>,
    /// Nodes that are not open.
    open: Vec<NodeKey>,
}

/// Compares the open devices with the nodes present now. Close what [`NodeDiff::close`] lists
/// before opening what [`NodeDiff::open`] lists: a reused device number is in both.
fn diff_nodes(open: &[NodeKey], present: &[NodeKey]) -> NodeDiff {
    NodeDiff {
        close: open
            .iter()
            .filter(|key| !present.contains(key))
            .cloned()
            .collect(),
        open: present
            .iter()
            .filter(|key| !open.contains(key))
            .cloned()
            .collect(),
    }
}

/// Brings the open devices in line with the nodes that exist, after uevents were lost: opens the
/// new ones and closes the gone ones.
pub fn resync(shared: &SharedInputs, state: &mut Cupertino) {
    let nodes = match enumerate(Path::new(SYSFS), Subsystem::Input) {
        Ok(nodes) => nodes,
        Err(e) => {
            tracing::error!(error = %e, "listing input devices to resynchronise failed");
            return;
        }
    };
    let present: Vec<(NodeKey, &SysDevice)> = nodes
        .iter()
        .filter_map(|node| {
            Some((
                NodeKey {
                    devnum: node.devnum()?,
                    devnode: node.devnode()?.to_owned(),
                },
                node,
            ))
        })
        .collect();
    let open: Vec<NodeKey> = shared
        .borrow()
        .devices
        .iter()
        .map(|(devnum, opened)| NodeKey {
            devnum: *devnum,
            devnode: opened.devnode.clone(),
        })
        .collect();
    let diff = diff_nodes(
        &open,
        &present
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
    );
    for key in &diff.close {
        close_device(shared, key.devnum, true, state);
    }
    for (key, node) in &present {
        if diff.open.contains(key) {
            open_reporting(shared, node, state);
        }
    }
}

fn is_event_node(devnode: &Path) -> bool {
    devnode
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("event"))
        .is_some_and(|number| number.parse::<u32>().is_ok())
}

fn open_reporting(shared: &SharedInputs, node: &SysDevice, state: &mut Cupertino) {
    if let Err(e) = open_device(shared, node, state) {
        tracing::error!(device = node.sysname(), error = %format!("{e:#}"), "skipping input device");
    }
}

/// Opens one node through the session and registers it with the loop. A node the compositor routes
/// nothing from, one that is already open, or any node while the session is paused, is skipped.
fn open_device(shared: &SharedInputs, node: &SysDevice, state: &mut Cupertino) -> Result<()> {
    let devnum = node.devnum().context("the node has no device number")?;
    let devnode = node
        .devnode()
        .context("the node has no device file")?
        .to_owned();
    let Some(class) = classify_device(node, Path::new(SYSFS))
        .map_err(|e| anyhow!("classifying the device: {e}"))?
    else {
        return Ok(());
    };
    if !is_routed(&class) || shared.borrow().devices.contains_key(&devnum) {
        return Ok(());
    }
    let mut session = shared.borrow().session.clone();
    // A node that appears while the session is paused would be handed out revoked; `open_all`
    // opens it on the activation.
    if !session.is_active() {
        return Ok(());
    }
    let fd = session
        .open(&devnode, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NONBLOCK)
        .map_err(|e| anyhow!("opening {}: {e}", devnode.display()))?;
    match register(node, class, &fd, state.config.pointer.natural_scrolling) {
        Ok((evdev, translator, mut events)) => {
            let device = translator.device.clone();
            let handle = shared.borrow().handle.clone();
            let token = match insert_device(shared, devnum, evdev, &handle) {
                Ok(token) => token,
                Err(e) => {
                    close_fd(&mut session, fd);
                    return Err(e);
                }
            };
            shared.borrow_mut().devices.insert(
                devnum,
                Opened {
                    translator,
                    devnode,
                    token,
                    session_fd: fd,
                },
            );
            events.insert(0, InputEvent::DeviceAdded { device });
            deliver(state, events);
            arm_timer(shared, state);
            Ok(())
        }
        Err(e) => {
            close_fd(&mut session, fd);
            Err(e)
        }
    }
}

/// Everything about a new device that can fail before it joins the loop: the evdev handle, its
/// translator, and the events of its open-time state (held keys, the lid).
fn register(
    node: &SysDevice,
    class: InputClass,
    fd: &OwnedFd,
    natural_scrolling: bool,
) -> Result<(EvdevDevice, Translator, Vec<Event_>)> {
    let mut evdev = EvdevDevice::open(fd.try_clone().context("duplicating the descriptor")?)
        .map_err(|e| anyhow!("reading the device capabilities: {e}"))?;
    let device = InputDevice::new(node, class, evdev.info());
    let mut translator = Translator::new(device, evdev.info(), natural_scrolling);
    let mut events = Vec::new();
    read(&mut evdev, &mut translator, &mut events)
        .map_err(|e| anyhow!("reading the device state: {e}"))?;
    Ok((evdev, translator, events))
}

fn insert_device(
    shared: &SharedInputs,
    devnum: DevT,
    evdev: EvdevDevice,
    handle: &LoopHandle<'static, Cupertino>,
) -> Result<RegistrationToken> {
    let shared = shared.clone();
    handle
        .insert_source(
            Generic::new(evdev, Interest::READ, Mode::Level),
            move |_, evdev, state| {
                // SAFETY: the descriptor is only read; the device is never replaced or dropped here.
                let evdev = unsafe { evdev.get_mut() };
                Ok(read_ready(&shared, devnum, evdev, state))
            },
        )
        .map_err(|e| anyhow!("registering the device with the event loop: {e}"))
}

/// The device became readable.
fn read_ready(
    shared: &SharedInputs,
    devnum: DevT,
    evdev: &mut EvdevDevice,
    state: &mut Cupertino,
) -> PostAction {
    let mut events = Vec::new();
    let result = {
        let mut inputs = shared.borrow_mut();
        let opened = inputs
            .devices
            .get_mut(&devnum)
            .expect("a registered device has an entry");
        opened
            .translator
            .set_natural_scrolling(state.config.pointer.natural_scrolling);
        read(evdev, &mut opened.translator, &mut events)
    };
    match result {
        Ok(()) => {
            deliver(state, events);
            arm_timer(shared, state);
            PostAction::Continue
        }
        Err(e) => {
            match &e {
                cupertino_evdev::Error::Io {
                    errno: Errno::NODEV,
                    ..
                } => tracing::info!(devnum = %devnum, "input device unplugged"),
                _ => {
                    tracing::error!(devnum = %devnum, error = %e, "reading an input device failed, closing it")
                }
            }
            // The loop drops this source when the callback returns.
            close_device(shared, devnum, false, state);
            PostAction::Remove
        }
    }
}

/// Removes a device, releases what it held and announces its removal. `remove_source` is false
/// when called from the device's own loop callback, which removes the source by its return value.
fn close_device(shared: &SharedInputs, devnum: DevT, remove_source: bool, state: &mut Cupertino) {
    let (opened, mut session, handle) = {
        let mut inputs = shared.borrow_mut();
        let Some(opened) = inputs.devices.remove(&devnum) else {
            return;
        };
        (opened, inputs.session.clone(), inputs.handle.clone())
    };
    if remove_source {
        handle.remove(opened.token);
    }
    close_fd(&mut session, opened.session_fd);
    let mut translator = opened.translator;
    let mut events = Vec::new();
    translator.release_all(micros_from_seconds(now(state).as_secs_f64()), &mut events);
    events.push(InputEvent::DeviceRemoved {
        device: translator.device,
    });
    deliver(state, events);
}

/// Makes sure a timer fires when the earliest touchpad deadline passes.
fn arm_timer(shared: &SharedInputs, state: &Cupertino) {
    let mut inputs = shared.borrow_mut();
    let Some(deadline) = inputs
        .devices
        .values()
        .filter_map(|opened| opened.translator.next_deadline())
        .reduce(f64::min)
    else {
        return;
    };
    // A pending timer that fires first re-arms for this deadline.
    if inputs
        .timer
        .as_ref()
        .is_some_and(|timer| timer.deadline <= deadline)
    {
        return;
    }
    if let Some(timer) = inputs.timer.take() {
        inputs.handle.remove(timer.token);
    }
    let delay = Duration::from_secs_f64((deadline - now(state).as_secs_f64()).max(0.0));
    let callback_shared = shared.clone();
    let inserted = inputs
        .handle
        .insert_source(Timer::from_duration(delay), move |_, _, state| {
            let now_s = now(state).as_secs_f64();
            let mut events = Vec::new();
            {
                let mut inputs = callback_shared.borrow_mut();
                inputs.timer = None;
                for opened in inputs.devices.values_mut() {
                    opened.translator.tick(now_s, &mut events);
                }
            }
            deliver(state, events);
            arm_timer(&callback_shared, state);
            TimeoutAction::Drop
        });
    match inserted {
        Ok(token) => inputs.timer = Some(ArmedTimer { token, deadline }),
        Err(e) => tracing::error!(error = %e, "arming the touchpad timer failed"),
    }
}

#[cfg(test)]
mod tests {
    use cupertino_evdev::{AbsAxis, AbsInfo, BitSet, DeviceId, Motion, Wheel};

    use super::*;

    fn device(class: InputClass) -> InputDevice {
        InputDevice(Rc::new(DeviceData {
            sysname: "event3".into(),
            name: "test".into(),
            syspath: PathBuf::from("/sys/test"),
            usb_id: (2, 1),
            class,
        }))
    }

    #[test]
    fn a_mouse_being_plugged_or_removed_changes_the_mice_property_and_is_announced() {
        use crate::quit::{BusOut, MouseEntry};
        let event_loop: smithay::reexports::calloop::EventLoop<'static, Cupertino> =
            smithay::reexports::calloop::EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            smithay::reexports::wayland_server::Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            crate::config::Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        let (tx, rx) = async_channel::unbounded();
        state.quit.signals = Some(tx);
        let mouse = device(InputClass {
            mouse: true,
            ..InputClass::default()
        });
        let pad = InputDevice(Rc::new(DeviceData {
            sysname: "event4".into(),
            name: "pad".into(),
            syspath: PathBuf::from("/sys/pad"),
            usb_id: (4, 3),
            class: InputClass {
                touchpad: true,
                ..InputClass::default()
            },
        }));
        let mice = |out: Option<BusOut>| match out {
            Some(BusOut::Settings(settings)) => settings.mice,
            other => panic!("expected the settings, got {other:?}"),
        };

        state.process_input_event::<EvdevBackend>(InputEvent::DeviceAdded {
            device: mouse.clone(),
        });
        assert_eq!(
            mice(rx.try_recv().ok()),
            vec![MouseEntry {
                key: "0001:0002:test".into(),
                name: "test".into(),
                dpi: 1000,
                stored: false
            }]
        );
        state.process_input_event::<EvdevBackend>(InputEvent::DeviceAdded {
            device: pad.clone(),
        });
        assert!(rx.try_recv().is_err(), "a touchpad is not announced");
        assert_eq!(state.session_settings().mice.len(), 1);
        state.process_input_event::<EvdevBackend>(InputEvent::DeviceRemoved { device: mouse });
        assert!(mice(rx.try_recv().ok()).is_empty());
    }

    fn info(abs_axes: Vec<AbsAxis>) -> DeviceInfo {
        DeviceInfo {
            name: "test".into(),
            id: DeviceId {
                bustype: 3,
                vendor: 1,
                product: 2,
                version: 0,
            },
            properties: BitSet::default(),
            event_types: BitSet::default(),
            keys: BitSet::default(),
            rel_axes: BitSet::default(),
            switches: BitSet::default(),
            abs_axes,
        }
    }

    fn axis(code: u16, max: i32, resolution: i32) -> AbsAxis {
        AbsAxis {
            code,
            info: AbsInfo {
                value: 0,
                min: 0,
                max,
                fuzz: 0,
                flat: 0,
                resolution: Resolution::from_raw(resolution),
            },
        }
    }

    fn frame(time_us: i64) -> Frame {
        Frame {
            time_us,
            resynced: false,
            keys: Vec::new(),
            held_buttons: Vec::new(),
            motion: Motion::default(),
            wheel: Wheel::default(),
            abs_pointer: None,
            contacts: Vec::new(),
            finger_count: None,
            switches: Vec::new(),
        }
    }

    fn translate(translator: &mut Translator, frame: &Frame) -> Vec<Event_> {
        let mut out = Vec::new();
        translator.frame(frame, &mut out);
        out
    }

    fn keyboard() -> Translator {
        Translator::new(
            device(InputClass {
                key: true,
                keyboard: true,
                ..InputClass::default()
            }),
            &info(Vec::new()),
            false,
        )
    }

    fn mouse(natural_scrolling: bool) -> Translator {
        Translator::new(
            device(InputClass {
                mouse: true,
                ..InputClass::default()
            }),
            &info(Vec::new()),
            natural_scrolling,
        )
    }

    #[test]
    fn a_key_press_becomes_a_keyboard_event_and_is_released_with_the_device() {
        let mut translator = keyboard();
        let mut press = frame(1_500_000);
        press.keys.push(EvdevKey {
            code: 30,
            state: EvdevKeyState::Pressed,
        });
        let events = translate(&mut translator, &press);
        let [InputEvent::Keyboard { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(
            (
                event.key_code(),
                KeyboardKeyEvent::state(event),
                event.count(),
                event.time()
            ),
            (Keycode::new(38), KeyState::Pressed, 1, 1_500_000)
        );

        let mut out = Vec::new();
        translator.release_all(2_000_000, &mut out);
        let [InputEvent::Keyboard { event }] = out.as_slice() else {
            panic!("{out:?}")
        };
        assert_eq!(
            (
                event.key_code(),
                KeyboardKeyEvent::state(event),
                event.count(),
                event.time()
            ),
            (Keycode::new(38), KeyState::Released, 0, 2_000_000)
        );
        assert!(!event.resynced);
    }

    #[test]
    fn a_key_found_held_when_the_device_is_read_is_marked_resynced() {
        let mut translator = keyboard();
        let mut open = frame(10);
        open.resynced = true;
        open.keys.push(EvdevKey {
            code: 29,
            state: EvdevKeyState::Pressed,
        });
        let events = translate(&mut translator, &open);
        let [InputEvent::Keyboard { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert!(event.resynced && KeyboardKeyEvent::state(event) == KeyState::Pressed);
        let mut release = frame(20);
        release.keys.push(EvdevKey {
            code: 29,
            state: EvdevKeyState::Released,
        });
        let events = translate(&mut translator, &release);
        let [InputEvent::Keyboard { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert!(!event.resynced);
    }

    #[test]
    fn relative_motion_is_reported_in_counts() {
        let mut motion = frame(10);
        motion.motion = Motion { dx: 3, dy: -2 };
        let events = translate(&mut mouse(false), &motion);
        let [InputEvent::PointerMotion { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(
            (event.delta_unaccel().x, event.delta_unaccel().y),
            (3.0, -2.0)
        );
    }

    #[test]
    fn a_hi_res_wheel_is_v120_from_a_wheel() {
        // Two detents away from the user, one to the right.
        let mut wheel = frame(10);
        wheel.wheel = Wheel {
            vertical: 240,
            horizontal: 120,
        };
        let events = translate(&mut mouse(false), &wheel);
        let [InputEvent::PointerAxis { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(event.source(), AxisSource::Wheel);
        assert_eq!(
            (
                event.amount_v120(Axis::Vertical),
                event.amount_v120(Axis::Horizontal)
            ),
            (Some(-240.0), Some(120.0))
        );
        assert_eq!(event.amount(Axis::Vertical), None);
        assert_eq!(
            event.relative_direction(Axis::Vertical),
            AxisRelativeDirection::Identical
        );

        // Natural scrolling flips the vertical axis and says so; it applies to the next frame of a running device.
        let mut live = mouse(false);
        live.set_natural_scrolling(true);
        let events = translate(&mut live, &wheel);
        let [InputEvent::PointerAxis { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(
            (
                event.amount_v120(Axis::Vertical),
                event.amount_v120(Axis::Horizontal)
            ),
            (Some(240.0), Some(-120.0))
        );
        assert_eq!(
            event.relative_direction(Axis::Vertical),
            AxisRelativeDirection::Inverted
        );
        live.set_natural_scrolling(false);
        let events = translate(&mut live, &wheel);
        let [InputEvent::PointerAxis { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(event.amount_v120(Axis::Vertical), Some(-240.0));
    }

    #[test]
    fn the_lid_switch_is_a_switch_toggle() {
        let mut lid = frame(10);
        lid.switches.push(EvdevSwitchEvent {
            switch: EvdevSwitch::Lid,
            on: true,
        });
        let class = InputClass {
            switch: true,
            ..InputClass::default()
        };
        let events = translate(
            &mut Translator::new(device(class), &info(Vec::new()), false),
            &lid,
        );
        let [InputEvent::SwitchToggle { event }] = events.as_slice() else {
            panic!("{events:?}")
        };
        assert_eq!(
            (event.switch(), SwitchToggleEvent::state(event)),
            (Some(Switch::Lid), SwitchState::On)
        );
    }

    fn touchpad() -> Translator {
        // 40 units per millimetre on both axes, 100 mm by 60 mm.
        let axes = vec![
            axis(ABS_MT_POSITION_X, 4_000, 40),
            axis(ABS_MT_POSITION_Y, 2_400, 40),
        ];
        Translator::new(
            device(InputClass {
                touchpad: true,
                clickpad: true,
                ..InputClass::default()
            }),
            &info(axes),
            true,
        )
    }

    fn two_fingers(time_us: i64, y: i32) -> Frame {
        let finger = |slot: u32, x: i32| Contact {
            slot,
            tracking_id: slot,
            x,
            y,
            pressure: None,
            touch_major: None,
            touch_minor: None,
            tool: Tool::Finger,
        };
        let mut f = frame(time_us);
        f.contacts = vec![finger(0, 1_600), finger(1, 2_000)];
        f.finger_count = Some(2);
        f
    }

    #[test]
    fn two_fingers_moving_down_scroll_with_a_finger_source_until_they_lift() {
        let mut translator = touchpad();
        let mut axis_events = Vec::new();
        // Fingers go down at y = 20 mm and move down 0.5 mm (20 units) every 10 ms.
        for step in 0..8 {
            for event in translate(
                &mut translator,
                &two_fingers(10_000 * (step + 1), 800 + 20 * step as i32),
            ) {
                if let InputEvent::PointerAxis { event } = event {
                    axis_events.push(event);
                }
            }
        }
        for event in translate(&mut translator, &frame(100_000)) {
            if let InputEvent::PointerAxis { event } = event {
                axis_events.push(event);
            }
        }
        assert!(axis_events.iter().all(|e| e.source() == AxisSource::Finger
            && e.relative_direction(Axis::Vertical) == AxisRelativeDirection::Inverted));
        let scrolled: Vec<Option<f64>> = axis_events
            .iter()
            .map(|e| e.amount(Axis::Vertical))
            .collect();
        // Natural scrolling: fingers down is content down, a negative axis value. Scrolling starts
        // after 1 mm of travel (1 / 0.0635 counts), then every 0.5 mm step is 7.874 counts.
        assert_eq!(
            scrolled,
            [
                Some(-15.748031496062993),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(0.0)
            ]
        );
    }

    #[test]
    fn a_single_touch_pad_without_a_resolution_scrolls_with_fake_fingers() {
        // ABS_X/ABS_Y only, no resolution: libinput's 69 mm by 50 mm fallback makes y 48 units per mm.
        let axes = vec![axis(ABS_X, 4_000, 0), axis(ABS_Y, 2_400, 0)];
        let mut translator = Translator::new(
            device(InputClass {
                touchpad: true,
                ..InputClass::default()
            }),
            &info(axes),
            true,
        );
        let two_fingers_at = |time_us: i64, y: i32| {
            let mut f = frame(time_us);
            f.abs_pointer = Some(AbsPoint { x: 2_000, y });
            f.finger_count = Some(2);
            f
        };
        let mut scrolled = Vec::new();
        // Both fingers report one position: it goes down 0.5 mm (24 units) every 10 ms from y = 20 mm.
        for step in 0..8 {
            for event in translate(
                &mut translator,
                &two_fingers_at(10_000 * (step + 1), 960 + 24 * step as i32),
            ) {
                if let InputEvent::PointerAxis { event } = event {
                    scrolled.push(event.amount(Axis::Vertical));
                }
            }
        }
        for event in translate(&mut translator, &frame(100_000)) {
            if let InputEvent::PointerAxis { event } = event {
                scrolled.push(event.amount(Axis::Vertical));
            }
        }
        assert_eq!(
            scrolled,
            [
                Some(-15.748031496062993),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(-7.874015748031496),
                Some(0.0)
            ]
        );
    }

    #[test]
    fn a_touchpad_that_cannot_be_modelled_keeps_its_buttons() {
        let mut translator = Translator::new(
            device(InputClass {
                touchpad: true,
                clickpad: true,
                ..InputClass::default()
            }),
            &info(Vec::new()),
            true,
        );
        let mut press = frame(10);
        press.keys = vec![
            EvdevKey {
                code: BTN_LEFT,
                state: EvdevKeyState::Pressed,
            },
            EvdevKey {
                code: BTN_RIGHT,
                state: EvdevKeyState::Pressed,
            },
        ];
        let events = translate(&mut translator, &press);
        let buttons: Vec<(u32, ButtonState)> = events
            .iter()
            .map(|event| match event {
                InputEvent::PointerButton { event } => (event.button_code(), event.state()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            buttons,
            [(0x110, ButtonState::Pressed), (0x111, ButtonState::Pressed)]
        );
    }

    #[test]
    fn closing_a_device_cancels_the_running_gestures_before_releasing_it() {
        let mut translator = touchpad();
        let mut out = Vec::new();
        translator.pad_event(
            TouchpadEvent::SwipeBegin {
                fingers: 3,
                axis: SwipeAxis::Horizontal,
            },
            1_000,
            &mut out,
        );
        translator.pad_event(TouchpadEvent::PinchBegin { fingers: 2 }, 1_000, &mut out);
        translator.pad_event(
            TouchpadEvent::ScrollUpdate { dx: 0.0, dy: 2.0 },
            1_000,
            &mut out,
        );
        out.clear();

        translator.release_all(2_000, &mut out);
        let shapes: Vec<String> = out
            .iter()
            .map(|event| match event {
                InputEvent::GestureSwipeEnd { event } => format!(
                    "swipe end cancelled={} at {}",
                    event.cancelled(),
                    event.time()
                ),
                InputEvent::GesturePinchEnd { event } => format!(
                    "pinch end cancelled={} at {}",
                    event.cancelled(),
                    event.time()
                ),
                InputEvent::PointerAxis { event } => format!(
                    "axis {:?} {:?}/{:?}",
                    event.source(),
                    event.amount(Axis::Horizontal),
                    event.amount(Axis::Vertical)
                ),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            shapes,
            [
                "swipe end cancelled=true at 2000",
                "pinch end cancelled=true at 2000",
                "axis Finger Some(0.0)/Some(0.0)"
            ]
        );

        // Nothing is left to cancel.
        out.clear();
        translator.release_all(3_000, &mut out);
        assert!(out.is_empty());
    }

    fn node(major: u32, minor: u32, devnode: &str) -> NodeKey {
        NodeKey {
            devnum: DevT::new(major, minor),
            devnode: PathBuf::from(devnode),
        }
    }

    #[test]
    fn the_node_diff_opens_new_nodes_closes_gone_ones_and_replaces_a_reused_device_number() {
        let open = [
            node(13, 64, "/dev/input/event0"),
            node(13, 65, "/dev/input/event1"),
            node(13, 66, "/dev/input/event2"),
        ];
        // event1 is gone; event2 was unplugged and its number reused by event7; event9 is new.
        let present = [
            node(13, 64, "/dev/input/event0"),
            node(13, 66, "/dev/input/event7"),
            node(13, 73, "/dev/input/event9"),
        ];
        assert_eq!(
            diff_nodes(&open, &present),
            NodeDiff {
                close: vec![
                    node(13, 65, "/dev/input/event1"),
                    node(13, 66, "/dev/input/event2")
                ],
                open: vec![
                    node(13, 66, "/dev/input/event7"),
                    node(13, 73, "/dev/input/event9")
                ],
            }
        );
        assert_eq!(diff_nodes(&present, &present), NodeDiff::default());
    }

    /// Dispatches `event_loop` until `done` holds, failing the test after two seconds.
    fn run_until<S>(
        event_loop: &mut smithay::reexports::calloop::EventLoop<'static, S>,
        state: &mut S,
        done: impl Fn(&S) -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done(state) {
            assert!(
                std::time::Instant::now() < deadline,
                "the loop did not finish in time"
            );
            event_loop
                .dispatch(Some(Duration::from_millis(5)), state)
                .unwrap();
        }
    }

    #[derive(Default)]
    struct Attempts {
        made: u32,
    }

    #[test]
    fn a_failing_attempt_is_repeated_until_it_succeeds() {
        let mut event_loop = smithay::reexports::calloop::EventLoop::<Attempts>::try_new().unwrap();
        let mut state = Attempts::default();
        // The node is refused twice (udevd has not tagged it yet), then accepted.
        retry(
            &event_loop.handle(),
            Duration::from_millis(1),
            5,
            &mut state,
            |state| {
                state.made += 1;
                if state.made < 3 {
                    Err(anyhow!("TakeDevice: ENODEV"))
                } else {
                    Ok(())
                }
            },
            |_| panic!("the third attempt succeeds"),
        );
        assert_eq!(state.made, 1, "the first attempt runs at once");
        run_until(&mut event_loop, &mut state, |state| state.made == 3);
        // No fourth attempt follows a success.
        for _ in 0..10 {
            event_loop
                .dispatch(Some(Duration::from_millis(2)), &mut state)
                .unwrap();
        }
        assert_eq!(state.made, 3);
    }

    #[test]
    fn the_last_failure_is_reported_when_the_retries_run_out() {
        let mut event_loop = smithay::reexports::calloop::EventLoop::<Attempts>::try_new().unwrap();
        let mut state = Attempts::default();
        let reported = Rc::new(RefCell::new(None));
        let sink = reported.clone();
        retry(
            &event_loop.handle(),
            Duration::from_millis(1),
            2,
            &mut state,
            |state| {
                state.made += 1;
                Err(anyhow!("attempt {}", state.made))
            },
            move |failure| *sink.borrow_mut() = Some(failure.to_string()),
        );
        run_until(&mut event_loop, &mut state, |state| state.made == 3);
        assert_eq!(reported.borrow().as_deref(), Some("attempt 3"));
    }
}
