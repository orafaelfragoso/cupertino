//! An absolute pointing device whose events enter the server through `process_input_event`, as a real device's do:
//! each position is a share of the device's range, which the server spans over an output.
#![allow(dead_code)]

use smithay::backend::input::{
    AbsolutePositionEvent, DeviceCapability, Event, InputBackend, InputEvent,
    PointerMotionAbsoluteEvent, UnusedEvent,
};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Device;

impl smithay::backend::input::Device for Device {
    fn id(&self) -> String {
        "absolute-pointer".to_owned()
    }

    fn name(&self) -> String {
        "absolute pointer".to_owned()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        capability == DeviceCapability::Pointer
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

#[derive(Debug)]
pub struct Backend;

impl InputBackend for Backend {
    type Device = Device;
    type KeyboardKeyEvent = Key;
    type PointerAxisEvent = UnusedEvent;
    type PointerButtonEvent = Button;
    type PointerMotionEvent = Relative;
    type PointerMotionAbsoluteEvent = Absolute;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = Frame;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

pub struct Relative {
    dx: f64,
    dy: f64,
}
impl Event<Backend> for Relative {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> Device {
        Device
    }
}
impl smithay::backend::input::PointerMotionEvent<Backend> for Relative {
    fn delta_x(&self) -> f64 {
        self.dx
    }
    fn delta_y(&self) -> f64 {
        self.dy
    }
    fn delta_x_unaccel(&self) -> f64 {
        self.dx
    }
    fn delta_y_unaccel(&self) -> f64 {
        self.dy
    }
}
pub fn relative(dx: f64, dy: f64) -> InputEvent<Backend> {
    InputEvent::PointerMotion {
        event: Relative { dx, dy },
    }
}

pub struct Frame;
impl Event<Backend> for Frame {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> Device {
        Device
    }
}
impl smithay::backend::input::TouchFrameEvent<Backend> for Frame {}
pub fn frame() -> InputEvent<Backend> {
    InputEvent::TouchFrame { event: Frame }
}

/// A position as a share of the device's range, 0 to 1 along each axis.
pub struct Absolute {
    pub x: f64,
    pub y: f64,
}

impl Event<Backend> for Absolute {
    fn time(&self) -> u64 {
        0
    }

    fn device(&self) -> Device {
        Device
    }
}

impl AbsolutePositionEvent<Backend> for Absolute {
    fn x(&self) -> f64 {
        self.x
    }

    fn y(&self) -> f64 {
        self.y
    }

    fn x_transformed(&self, width: i32) -> f64 {
        self.x * f64::from(width)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        self.y * f64::from(height)
    }
}

impl PointerMotionAbsoluteEvent<Backend> for Absolute {}

pub struct Key {
    pub code: u32,
    pub pressed: bool,
}
impl Event<Backend> for Key {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> Device {
        Device
    }
}
impl smithay::backend::input::KeyboardKeyEvent<Backend> for Key {
    fn key_code(&self) -> smithay::input::keyboard::Keycode {
        smithay::input::keyboard::Keycode::new(self.code + 8)
    }
    fn state(&self) -> smithay::backend::input::KeyState {
        if self.pressed {
            smithay::backend::input::KeyState::Pressed
        } else {
            smithay::backend::input::KeyState::Released
        }
    }
    fn count(&self) -> u32 {
        u32::from(self.pressed)
    }
}
pub fn key(code: u32, pressed: bool) -> InputEvent<Backend> {
    InputEvent::Keyboard {
        event: Key { code, pressed },
    }
}
pub struct Button {
    pub pressed: bool,
}
impl Event<Backend> for Button {
    fn time(&self) -> u64 {
        0
    }
    fn device(&self) -> Device {
        Device
    }
}
impl smithay::backend::input::PointerButtonEvent<Backend> for Button {
    fn button_code(&self) -> u32 {
        272
    }
    fn state(&self) -> smithay::backend::input::ButtonState {
        if self.pressed {
            smithay::backend::input::ButtonState::Pressed
        } else {
            smithay::backend::input::ButtonState::Released
        }
    }
}
pub fn button(pressed: bool) -> InputEvent<Backend> {
    InputEvent::PointerButton {
        event: Button { pressed },
    }
}

/// The device's motion to `x`, `y` (shares of its range).
pub fn motion(x: f64, y: f64) -> InputEvent<Backend> {
    InputEvent::PointerMotionAbsolute {
        event: Absolute { x, y },
    }
}
