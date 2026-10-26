//! Which kernel uevents make the daemon read sysfs again.

use cupertino_devices::DeviceEvent;

/// The uevent subsystems that carry power, backlight and keyboard backlight changes.
const SUBSYSTEMS: [&str; 3] = ["power_supply", "backlight", "leds"];

/// Whether `event` is about a device this daemon reports.
pub fn concerns_power(event: &DeviceEvent) -> bool {
    SUBSYSTEMS.contains(&event.subsystem.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_devices::parse_datagram;

    fn event(datagram: &[u8]) -> DeviceEvent {
        parse_datagram(datagram).unwrap().unwrap()
    }

    #[test]
    fn power_backlight_and_led_changes_count_and_other_devices_do_not() {
        assert!(concerns_power(&event(b"change@/devices/LNXSYSTM:00/PNP0C0A:00/power_supply/BAT0\0SUBSYSTEM=power_supply\0POWER_SUPPLY_NAME=BAT0\0")));
        assert!(concerns_power(&event(b"change@/devices/pci0000:00/0000:00:02.0/drm/card0/card0-eDP-1/intel_backlight\0SUBSYSTEM=backlight\0")));
        assert!(concerns_power(&event(b"change@/devices/platform/thinkpad_acpi/leds/tpacpi::kbd_backlight\0SUBSYSTEM=leds\0")));
        assert!(!concerns_power(&event(b"add@/devices/virtual/input/input9\0SUBSYSTEM=input\0")));
    }
}
