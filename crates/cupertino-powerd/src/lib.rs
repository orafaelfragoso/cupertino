//! Power and backlight for the desktop, replacing UPower and the backlight helpers.
//!
//! - [`supply`]: pure model of `/sys/class/power_supply` (percentage, state, time estimates).
//! - [`view`]: the values of a UPower device, the display device over several batteries,
//!   on-battery.
//! - [`backlight`]: pure model of display and keyboard backlights, and the choice among several.
//! - [`sysfs`]: reads those classes from an injectable sysfs root and writes a brightness.
//! - [`lid`]: the lid switch, an evdev device that reports `SW_LID`.
//! - [`history`]: the charge recorded over the last ten days, which `GetHistory` serves.
//! - [`events`]: which kernel uevents mean "read sysfs again".
//! - [`warning`]: the low-battery warning level (none, early, final) and its timing rules.
//! - `auth` (feature `dbus`): who may write a brightness, decided from logind.
//! - `daemon` and `interfaces` (feature `dbus`): the `org.freedesktop.UPower` subset and
//!   `org.cupertino.Power1` on the bus.
//!
//! Every function that reads sysfs takes the sysfs root as a parameter; production passes `/sys`.

pub mod backlight;
pub mod error;
pub mod events;
pub mod history;
pub mod lid;
pub mod supply;
pub mod sysfs;
pub mod view;
pub mod warning;

#[cfg(feature = "dbus")]
pub mod auth;
#[cfg(feature = "dbus")]
pub mod daemon;
#[cfg(feature = "dbus")]
pub mod interfaces;

pub use error::{Error, Result};
