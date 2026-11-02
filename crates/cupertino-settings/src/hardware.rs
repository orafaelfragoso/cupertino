//! What the machine has, read where the kernel and udev publish it: a battery of its own and a touchpad decide
//! whether their panes show.

use std::fs;
use std::path::{Path, PathBuf};

/// Where the facts are read from; the system's own folders, or a test's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sources {
    /// `/sys`.
    pub sysfs: PathBuf,
    /// udev's database, `/run/udev/data`.
    pub udev_data: PathBuf,
}

impl Default for Sources {
    fn default() -> Self {
        Sources {
            sysfs: PathBuf::from("/sys"),
            udev_data: PathBuf::from("/run/udev/data"),
        }
    }
}

/// The hardware the panes depend on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hardware {
    /// A present battery that powers the machine; a wireless mouse's or headset's battery is not one.
    pub battery: bool,
    /// An input device udev classifies as a touchpad.
    pub touchpad: bool,
}

impl Hardware {
    pub fn read(sources: &Sources) -> Self {
        Hardware {
            battery: has_battery(&sources.sysfs),
            touchpad: has_touchpad(&sources.sysfs, &sources.udev_data),
        }
    }
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

fn attribute(dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(dir.join(name))
        .ok()
        .map(|value| value.trim().to_owned())
}

/// A `power_supply` of type `Battery` whose scope is not `Device` and that reports itself present.
fn has_battery(sysfs: &Path) -> bool {
    entries(&sysfs.join("class/power_supply"))
        .iter()
        .any(|supply| {
            attribute(supply, "type").as_deref() == Some("Battery")
                && attribute(supply, "scope").as_deref() != Some("Device")
                && attribute(supply, "present").as_deref() != Some("0")
        })
}

/// An `event*` node whose udev record says `ID_INPUT_TOUCHPAD=1`.
fn has_touchpad(sysfs: &Path, udev_data: &Path) -> bool {
    entries(&sysfs.join("class/input")).iter().any(|node| {
        let is_event = node
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("event"));
        is_event
            && attribute(node, "dev").is_some_and(|dev| {
                fs::read_to_string(udev_data.join(format!("c{dev}")))
                    .is_ok_and(|record| record.lines().any(|l| l == "E:ID_INPUT_TOUCHPAD=1"))
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: PathBuf, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn supply(root: &Path, name: &str, kind: &str, scope: Option<&str>) {
        let dir = root.join("sys/class/power_supply").join(name);
        write(dir.join("type"), &format!("{kind}\n"));
        if let Some(scope) = scope {
            write(dir.join("scope"), &format!("{scope}\n"));
        }
    }

    fn input(root: &Path, event: &str, dev: &str, record: &str) {
        write(
            root.join("sys/class/input").join(event).join("dev"),
            &format!("{dev}\n"),
        );
        write(root.join("udev").join(format!("c{dev}")), record);
    }

    fn read(root: &Path) -> Hardware {
        Hardware::read(&Sources {
            sysfs: root.join("sys"),
            udev_data: root.join("udev"),
        })
    }

    #[test]
    fn a_desktop_with_a_wireless_mouse_battery_and_a_keyboard_has_no_battery_and_no_touchpad() {
        let root = tempfile::tempdir().unwrap();
        supply(root.path(), "hidpp_battery_0", "Battery", Some("Device"));
        supply(root.path(), "ACAD", "Mains", None);
        input(
            root.path(),
            "event7",
            "13:71",
            "E:ID_INPUT=1\nE:ID_INPUT_MOUSE=1\nE:ID_INPUT_KEY=1\n",
        );
        assert_eq!(read(root.path()), Hardware::default());
    }

    #[test]
    fn a_laptop_has_its_battery_and_its_touchpad() {
        let root = tempfile::tempdir().unwrap();
        supply(root.path(), "BAT0", "Battery", Some("System"));
        input(
            root.path(),
            "event9",
            "13:73",
            "E:ID_INPUT=1\nE:ID_INPUT_TOUCHPAD=1\n",
        );
        assert_eq!(
            read(root.path()),
            Hardware {
                battery: true,
                touchpad: true
            }
        );
    }

    #[test]
    fn a_battery_slot_reporting_no_battery_is_none() {
        let root = tempfile::tempdir().unwrap();
        supply(root.path(), "BAT1", "Battery", None);
        write(
            root.path().join("sys/class/power_supply/BAT1/present"),
            "0\n",
        );
        assert!(!read(root.path()).battery);
    }
}
