//! Linux evdev input for the compositor, without libinput, libevdev or libudev.
//!
//! The session opens `/dev/input/eventN` and hands over an fd; [`Device::open`] queries the capabilities
//! ([`DeviceInfo`]), and [`Device::read_frames`] turns the kernel's `input_event` stream into typed [`Frame`]s.
//! The framing logic lives in [`Framer`], a state machine over [`InputEvent`]s that needs no device, so recorded
//! event sequences can drive it in tests.

pub mod bitset;
pub mod codes;
mod device;
mod error;
mod event;
mod frame;
mod framer;
mod info;
mod ioctl;

pub use bitset::BitSet;
pub use device::{read_events, Device};
pub use error::{Error, FrameError};
pub use event::{InputEvent, EVENT_SIZE};
pub use frame::{
    AbsPoint, Contact, Frame, KeyEvent, KeyState, Motion, Switch, SwitchEvent, Tool, Wheel,
};
pub use framer::{Framer, FramerOutput, Layout, MtSlotValues, Snapshot};
pub use info::{AbsAxis, AbsInfo, DeviceId, DeviceInfo, Resolution};
