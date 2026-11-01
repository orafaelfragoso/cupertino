//! Device discovery for the compositor without libudev.
//!
//! Everything reads `/sys` and the kernel's uevent netlink socket directly:
//! - [`device`]: enumerate `input` (`event*`) and `drm` (`card*`) devices with syspath, node,
//!   device number, parents and `uevent` properties.
//! - [`input_id`]: classify input devices (keyboard, mouse, touchpad, touchscreen, ...) from their
//!   capability bitmasks, with the rules of udev's `input_id` builtin.
//! - [`gpu`]: all GPUs and the primary one.
//! - [`monitor`]: hotplug events from the kernel.
//! - [`preflight`]: what the primary GPU's driver needs before a compositor takes the display, with the fix for each
//!   setting that is wrong.
//! - [`seat`]: seat assignment (always `seat0` without udevd).
//! - [`udev_db`]: the properties udevd stored for a node (hardware database entries such as `MOUSE_DPI`).
//!
//! Every function that reads sysfs takes the sysfs root as a parameter; production passes `/sys`,
//! tests pass a fixture tree. Parsers take their input as data.

pub mod capabilities;
pub mod dev_t;
pub mod device;
pub mod error;
pub mod gpu;
pub mod input_id;
pub mod monitor;
pub mod preflight;
pub mod seat;
mod sysfs;
pub mod udev_db;
pub mod uevent;

pub use capabilities::CapabilityMask;
pub use dev_t::DevT;
pub use device::{enumerate, Device, Subsystem};
pub use error::{Error, ParseError, Result};
pub use gpu::{gpus, primary_gpu, select_primary, select_render, Gpu, RenderChoice};
pub use input_id::{classify, classify_device, InputCapabilities, InputClass};
pub use monitor::{parse_datagram, Action, DeviceEvent, Drained, Monitor};
pub use seat::{seat_name, SEAT0};
pub use udev_db::{parse_udev_db, read_udev_properties, UDEV_DATA_DIR};
pub use uevent::Properties;
