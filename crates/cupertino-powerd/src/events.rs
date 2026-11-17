//! Which kernel uevents make the daemon read sysfs again.

use cupertino_devices::DeviceEvent;

/// The uevent subsystems that carry power, backlight and keyboard backlight changes.
const SUBSYSTEMS: [&str; 3] = ["power_supply", "backlight", "leds"];

/// Whether `event` is about a device this daemon reports.
pub fn concerns_power(event: &DeviceEvent) -> bool {
    SUBSYSTEMS.contains(&event.subsystem.as_str())
}
