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
