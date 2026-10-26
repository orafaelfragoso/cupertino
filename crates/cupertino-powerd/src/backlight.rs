//! Pure model of the display backlights (`/sys/class/backlight`) and keyboard backlights
//! (`/sys/class/leds/*::kbd_backlight`).
//!
//! Reference: `Documentation/ABI/stable/sysfs-class-backlight` and
//! `Documentation/ABI/testing/sysfs-class-led`.

use std::cmp::Reverse;
use std::fmt;

/// A backlight attribute that does not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// `type` is none of `firmware`, `platform`, `raw`.
    UnknownType(String),
    /// A brightness attribute is not an unsigned integer.
    NotANumber {
        attribute: &'static str,
        value: String,
    },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownType(value) => write!(f, "unknown backlight type {value:?}"),
            Self::NotANumber { attribute, value } => {
                write!(f, "{attribute} is not an unsigned integer: {value:?}")
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// The `type` of a display backlight, from most to least preferred. The ABI document says a
/// `firmware` interface is the best choice, `platform` next, and `raw` (hardware registers
/// driven directly) the last resort, so userspace that controls one of several backlights
/// picks in that order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BacklightType {
    Raw,
    Platform,
    Firmware,
}

impl BacklightType {
    pub fn parse(value: &str) -> Result<Self, ParseError> {
        match value {
            "firmware" => Ok(Self::Firmware),
            "platform" => Ok(Self::Platform),
            "raw" => Ok(Self::Raw),
            other => Err(ParseError::UnknownType(other.to_owned())),
        }
    }
}

/// Parses a brightness attribute.
pub fn brightness_value(attribute: &'static str, text: &str) -> Result<u32, ParseError> {
    text.parse().map_err(|_| ParseError::NotANumber {
        attribute,
        value: text.to_owned(),
    })
}

/// One display backlight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlight {
    /// The directory name, for example `intel_backlight`.
    pub name: String,
    pub kind: BacklightType,
    /// `actual_brightness` when the driver has it (the level the hardware reports), else
    /// `brightness` (the level last requested).
    pub brightness: u32,
    pub max_brightness: u32,
}

/// The backlight to control: the most preferred type, and the lowest name among equals so the
/// choice does not depend on directory order.
pub fn select(backlights: &[Backlight]) -> Option<&Backlight> {
    backlights
        .iter()
        .min_by(|a, b| (Reverse(a.kind), &a.name).cmp(&(Reverse(b.kind), &b.name)))
}

/// One keyboard backlight LED.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardBacklight {
    /// The directory name, for example `tpacpi::kbd_backlight`.
    pub name: String,
    pub brightness: u32,
    pub max_brightness: u32,
}

/// The suffix that marks a keyboard backlight LED name (`devicename:colour:function`, with the
/// function `kbd_backlight` in the LED naming scheme).
pub const KEYBOARD_BACKLIGHT_SUFFIX: &str = "::kbd_backlight";

#[cfg(test)]
mod tests {
    use super::*;

    fn backlight(name: &str, kind: BacklightType) -> Backlight {
        Backlight {
            name: name.to_owned(),
            kind,
            brightness: 0,
            max_brightness: 100,
        }
    }

    #[test]
    fn firmware_beats_platform_beats_raw_and_names_break_ties() {
        let all = [
            backlight("intel_backlight", BacklightType::Raw),
            backlight("acpi_video1", BacklightType::Platform),
            backlight("acpi_video0", BacklightType::Platform),
        ];
        assert_eq!(select(&all).unwrap().name, "acpi_video0");
        let with_firmware = [
            all.as_slice(),
            &[backlight("vendor_ec", BacklightType::Firmware)],
        ]
        .concat();
        assert_eq!(select(&with_firmware).unwrap().name, "vendor_ec");
        assert_eq!(select(&[]), None);
    }
}
