//! The integration tests of `cupertino-devices`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "evdev-framer.rs"]
mod evdev_framer;
#[path = "evdev-records.rs"]
mod evdev_records;
#[path = "input-class.rs"]
mod input_class;
mod monitor;
mod parsing;
mod sysfs;
