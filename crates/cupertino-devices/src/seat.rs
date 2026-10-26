//! Seat assignment.
//!
//! The seat of a device is the `ID_SEAT` property that udevd's rules derive from the `seat` tag;
//! no `ID_SEAT` means `seat0`. This crate reads sysfs and kernel uevents only, never udevd's
//! database, so `ID_SEAT` is never present and every device belongs to `seat0`. The lookup still
//! honours the property so that a caller feeding in properties from elsewhere gets udev's answer.

use crate::uevent::Properties;

/// The default seat.
pub const SEAT0: &str = "seat0";

/// The seat a device belongs to, given its properties.
pub fn seat_name(properties: &Properties) -> &str {
    properties.get("ID_SEAT").map_or(SEAT0, String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_seat0() {
        assert_eq!(seat_name(&Properties::new()), "seat0");
    }

    #[test]
    fn id_seat_overrides() {
        let properties = Properties::from([("ID_SEAT".to_owned(), "seat1".to_owned())]);
        assert_eq!(seat_name(&properties), "seat1");
    }
}
