//! Shared helpers: a fake sysfs tree written into a temporary directory.
// Each test file uses a different part.

use std::fs;
use std::path::Path;

/// Writes `files` (path relative to `root`, content) and creates the directories on the way.
pub fn write_tree(root: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

/// A 50 Wh battery at 25 Wh, discharging at 10 W: 50 percent, 9000 s left, health 80 percent.
/// The firmware `capacity` says 49 to show that the amounts win.
pub const DISCHARGING_BAT0: [(&str, &str); 11] = [
    ("class/power_supply/BAT0/type", "Battery\n"),
    ("class/power_supply/BAT0/status", "Discharging\n"),
    ("class/power_supply/BAT0/present", "1\n"),
    ("class/power_supply/BAT0/capacity", "49\n"),
    ("class/power_supply/BAT0/energy_now", "25000000\n"),
    ("class/power_supply/BAT0/energy_full", "50000000\n"),
    ("class/power_supply/BAT0/energy_full_design", "62500000\n"),
    ("class/power_supply/BAT0/power_now", "10000000\n"),
    ("class/power_supply/BAT0/voltage_now", "11800000\n"),
    ("class/power_supply/BAT0/manufacturer", "SMP\n"),
    ("class/power_supply/BAT0/model_name", "L19M4PF4\n"),
];

/// A display backlight at 40 of 100.
pub const BACKLIGHT: [(&str, &str); 3] = [
    ("class/backlight/nvidia_wmi_ec_backlight/type", "firmware\n"),
    ("class/backlight/nvidia_wmi_ec_backlight/brightness", "40\n"),
    (
        "class/backlight/nvidia_wmi_ec_backlight/max_brightness",
        "100\n",
    ),
];

/// A keyboard backlight at 1 of 2, and a lock-key LED that is not one.
pub const KEYBOARD_BACKLIGHT: [(&str, &str); 5] = [
    ("class/leds/tpacpi::kbd_backlight/brightness", "1\n"),
    ("class/leds/tpacpi::kbd_backlight/max_brightness", "2\n"),
    ("class/leds/input3::capslock/brightness", "0\n"),
    ("class/leds/input3::capslock/max_brightness", "1\n"),
    ("class/leds/input3::capslock/trigger", "none\n"),
];
