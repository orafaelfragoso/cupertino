//! The model against fake sysfs trees written by the test, through the public reading API.

use crate::common;

use std::fs;

use common::{write_tree, BACKLIGHT, DISCHARGING_BAT0, KEYBOARD_BACKLIGHT};
use cupertino_powerd::backlight::{BacklightType, KeyboardBacklight};
use cupertino_powerd::supply::State;
use cupertino_powerd::sysfs::{
    read_backlights, read_keyboard_backlights, read_supplies, Lid, Snapshot,
};
use cupertino_powerd::view::{on_battery, DeviceType, DeviceView};

#[test]
fn one_battery_discharging_with_energy_files() {
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &DISCHARGING_BAT0);
    let supplies = read_supplies(root.path()).unwrap();
    assert_eq!(supplies.len(), 1);

    let view = DeviceView::of_supply(&supplies[0]);
    assert_eq!(view.native_path, "BAT0");
    assert_eq!(
        (view.vendor.as_str(), view.model.as_str()),
        ("SMP", "L19M4PF4")
    );
    assert_eq!(view.kind, DeviceType::Battery);
    assert_eq!(view.state, State::Discharging);
    assert_eq!(view.percentage, 50.0);
    assert_eq!(
        (view.energy, view.energy_full, view.energy_full_design),
        (25.0, 50.0, 62.5)
    );
    assert_eq!((view.energy_rate, view.voltage), (10.0, 11.8));
    assert_eq!((view.time_to_empty, view.time_to_full), (9000, 0));
    assert_eq!(view.capacity, 80.0);
    assert_eq!(view.icon_name, "battery-good-symbolic");
    assert!(on_battery(&supplies));
}

#[test]
fn one_battery_charging_with_charge_files_and_voltage() {
    let root = tempfile::tempdir().unwrap();
    write_tree(
        root.path(),
        &[
            ("class/power_supply/BAT1/type", "Battery\n"),
            ("class/power_supply/BAT1/status", "Charging\n"),
            ("class/power_supply/BAT1/charge_now", "2000000\n"),
            ("class/power_supply/BAT1/charge_full", "4000000\n"),
            ("class/power_supply/BAT1/charge_full_design", "5000000\n"),
            ("class/power_supply/BAT1/voltage_now", "12000000\n"),
            // Drivers differ on the sign of the current; the state gives the direction.
            ("class/power_supply/BAT1/current_now", "-1000000\n"),
        ],
    );
    let supplies = read_supplies(root.path()).unwrap();
    let view = DeviceView::of_supply(&supplies[0]);
    assert_eq!(view.state, State::Charging);
    assert_eq!(view.percentage, 50.0);
    assert_eq!(
        (view.energy, view.energy_full, view.energy_full_design),
        (24.0, 48.0, 60.0)
    );
    assert_eq!(view.energy_rate, 12.0);
    assert_eq!((view.time_to_empty, view.time_to_full), (0, 7200));
    assert_eq!(view.capacity, 80.0);
    assert_eq!(view.icon_name, "battery-good-charging-symbolic");
    assert!(!on_battery(&supplies));
}

#[test]
fn two_batteries_aggregate_and_a_device_battery_does_not() {
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &DISCHARGING_BAT0);
    write_tree(
        root.path(),
        &[
            ("class/power_supply/BAT1/type", "Battery\n"),
            ("class/power_supply/BAT1/status", "Discharging\n"),
            ("class/power_supply/BAT1/energy_now", "10000000\n"),
            ("class/power_supply/BAT1/energy_full", "30000000\n"),
            ("class/power_supply/BAT1/energy_full_design", "37500000\n"),
            ("class/power_supply/BAT1/power_now", "5000000\n"),
            // A wireless mouse: a battery of an attached device, not of the machine.
            ("class/power_supply/hidpp_battery_0/type", "Battery\n"),
            ("class/power_supply/hidpp_battery_0/scope", "Device\n"),
            ("class/power_supply/hidpp_battery_0/status", "Discharging\n"),
            ("class/power_supply/hidpp_battery_0/capacity", "20\n"),
        ],
    );
    let supplies = read_supplies(root.path()).unwrap();
    let names: Vec<&str> = supplies.iter().map(|supply| supply.name.as_str()).collect();
    assert_eq!(names, ["BAT0", "BAT1", "hidpp_battery_0"]);

    let display = DeviceView::display(&supplies);
    assert_eq!(display.kind, DeviceType::Battery);
    assert_eq!(display.state, State::Discharging);
    assert_eq!(
        (
            display.energy,
            display.energy_full,
            display.energy_full_design
        ),
        (35.0, 80.0, 100.0)
    );
    assert_eq!(display.energy_rate, 15.0);
    assert_eq!(display.percentage, 43.75);
    assert_eq!(display.time_to_empty, 8400);
    assert_eq!(display.capacity, 80.0);
    assert!(display.is_present && display.power_supply);

    let mouse = DeviceView::of_supply(&supplies[2]);
    assert!(!mouse.power_supply);
    assert_eq!((mouse.kind, mouse.percentage), (DeviceType::Battery, 20.0));
}

#[test]
fn mains_online_with_a_full_battery() {
    let root = tempfile::tempdir().unwrap();
    write_tree(
        root.path(),
        &[
            ("class/power_supply/AC/type", "Mains\n"),
            ("class/power_supply/AC/online", "1\n"),
            ("class/power_supply/BAT0/type", "Battery\n"),
            ("class/power_supply/BAT0/status", "Full\n"),
            ("class/power_supply/BAT0/capacity", "100\n"),
            ("class/power_supply/BAT0/energy_now", "50000000\n"),
            ("class/power_supply/BAT0/energy_full", "50000000\n"),
            ("class/power_supply/BAT0/energy_full_design", "50000000\n"),
        ],
    );
    let supplies = read_supplies(root.path()).unwrap();
    let mains = DeviceView::of_supply(&supplies[0]);
    assert_eq!(
        (mains.kind, mains.online, mains.is_present),
        (DeviceType::LinePower, true, true)
    );
    assert_eq!(mains.icon_name, "ac-adapter-symbolic");

    let display = DeviceView::display(&supplies);
    assert_eq!(
        (display.state, display.percentage),
        (State::FullyCharged, 100.0)
    );
    assert_eq!((display.time_to_empty, display.time_to_full), (0, 0));
    assert_eq!(display.icon_name, "battery-full-charged-symbolic");
    assert!(!on_battery(&supplies));
}

#[test]
fn a_machine_without_power_classes_has_no_display_battery() {
    let root = tempfile::tempdir().unwrap();
    let snapshot = Snapshot::read(
        root.path(),
        Lid {
            present: false,
            closed: false,
        },
    )
    .unwrap();
    assert!(
        snapshot.supplies.is_empty()
            && snapshot.backlights.is_empty()
            && snapshot.keyboards.is_empty()
    );
    let display = DeviceView::display(&snapshot.supplies);
    assert_eq!(
        (display.kind, display.is_present, display.icon_name.as_str()),
        (DeviceType::Unknown, false, "battery-missing-symbolic")
    );
    assert!(!on_battery(&snapshot.supplies));
}

#[test]
fn an_unknown_status_names_the_supply() {
    let root = tempfile::tempdir().unwrap();
    write_tree(
        root.path(),
        &[
            ("class/power_supply/BAT0/type", "Battery\n"),
            ("class/power_supply/BAT0/status", "Exploding\n"),
        ],
    );
    let error = read_supplies(root.path()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "power supply BAT0: unknown battery status \"Exploding\""
    );
}

#[test]
fn several_backlights_are_chosen_by_type() {
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &BACKLIGHT);
    write_tree(
        root.path(),
        &[
            ("class/backlight/acpi_video0/type", "platform\n"),
            ("class/backlight/acpi_video0/brightness", "5\n"),
            ("class/backlight/acpi_video0/max_brightness", "15\n"),
            ("class/backlight/intel_backlight/type", "raw\n"),
            ("class/backlight/intel_backlight/brightness", "60000\n"),
            // The level the hardware reports wins over the level last requested.
            (
                "class/backlight/intel_backlight/actual_brightness",
                "59000\n",
            ),
            ("class/backlight/intel_backlight/max_brightness", "120000\n"),
        ],
    );
    let pick = || {
        let backlights = read_backlights(root.path()).unwrap();
        let chosen = cupertino_powerd::backlight::select(&backlights)
            .unwrap()
            .clone();
        (
            chosen.name,
            chosen.kind,
            chosen.brightness,
            chosen.max_brightness,
        )
    };
    assert_eq!(
        pick(),
        (
            "nvidia_wmi_ec_backlight".to_owned(),
            BacklightType::Firmware,
            40,
            100
        )
    );
    fs::remove_dir_all(root.path().join("class/backlight/nvidia_wmi_ec_backlight")).unwrap();
    assert_eq!(
        pick(),
        ("acpi_video0".to_owned(), BacklightType::Platform, 5, 15)
    );
    fs::remove_dir_all(root.path().join("class/backlight/acpi_video0")).unwrap();
    assert_eq!(
        pick(),
        (
            "intel_backlight".to_owned(),
            BacklightType::Raw,
            59000,
            120000
        )
    );
}

#[test]
fn the_keyboard_backlight_led_is_found_among_other_leds() {
    let root = tempfile::tempdir().unwrap();
    write_tree(root.path(), &KEYBOARD_BACKLIGHT);
    assert_eq!(
        read_keyboard_backlights(root.path()).unwrap(),
        [KeyboardBacklight {
            name: "tpacpi::kbd_backlight".to_owned(),
            brightness: 1,
            max_brightness: 2
        }]
    );
}
