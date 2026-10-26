use std::path::{Path, PathBuf};

use crate::dev_t::DevT;
use crate::error::{Error, Result};
use crate::seat;
use crate::sysfs;
use crate::uevent::{self, Properties};

/// A device class this crate enumerates, with the name of its device nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subsystem {
    /// `/sys/class/input`; only `event*` nodes.
    Input,
    /// `/sys/class/drm`; only `card*` nodes.
    Drm,
}

impl Subsystem {
    /// The directory name under `<sysfs>/class`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Drm => "drm",
        }
    }

    /// The node name prefix that matters: `event` or `card`.
    fn node_prefix(self) -> &'static str {
        match self {
            Self::Input => "event",
            Self::Drm => "card",
        }
    }

    /// The node number if `sysname` is exactly the prefix and digits (`card1` yes, `card1-DP-1` no).
    fn node_number(self, sysname: &str) -> Option<u32> {
        sysname.strip_prefix(self.node_prefix())?.parse().ok()
    }
}

/// A device read from sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    syspath: PathBuf,
    sysname: String,
    properties: Properties,
    devnode: Option<PathBuf>,
    devnum: Option<DevT>,
}

impl Device {
    /// Reads the device at `syspath`, a canonical directory under `<sysfs>/devices` that has a
    /// `uevent` file.
    pub fn from_syspath(syspath: &Path) -> Result<Self> {
        let uevent_path = syspath.join("uevent");
        let parse_error = |source| Error::Parse {
            origin: uevent_path.display().to_string(),
            source,
        };
        let properties =
            uevent::parse_uevent_file(&sysfs::read_attr(&uevent_path)?).map_err(parse_error)?;
        let devnum = uevent::devnum(&properties).map_err(parse_error)?;
        let devnode = uevent::devnode(&properties);
        let sysname = syspath
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .ok_or_else(|| Error::Io {
                path: syspath.to_owned(),
                source: std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "syspath has no final component",
                ),
            })?;
        Ok(Self {
            syspath: syspath.to_owned(),
            sysname,
            properties,
            devnode,
            devnum,
        })
    }

    /// The canonical sysfs directory, e.g. `/sys/devices/pci0000:00/.../drm/card1`.
    pub fn syspath(&self) -> &Path {
        &self.syspath
    }

    /// The last component of the syspath, e.g. `event3`.
    pub fn sysname(&self) -> &str {
        &self.sysname
    }

    /// The device node under `/dev` (from `DEVNAME`), if the device has one.
    pub fn devnode(&self) -> Option<&Path> {
        self.devnode.as_deref()
    }

    /// The device number (from `MAJOR` and `MINOR`), if the device has a node.
    pub fn devnum(&self) -> Option<DevT> {
        self.devnum
    }

    /// The `uevent` properties.
    pub fn properties(&self) -> &Properties {
        &self.properties
    }

    /// One `uevent` property.
    pub fn property(&self, key: &str) -> Option<&str> {
        self.properties.get(key).map(String::as_str)
    }

    /// The seat this device belongs to; always `seat0` here (see [`crate::seat`]).
    pub fn seat(&self) -> &str {
        seat::seat_name(&self.properties)
    }

    /// The nearest ancestor that is a device (has a `uevent` file). Container directories such as
    /// `input/` or `drm/` are skipped, as udev does. `None` at the top of `<sysfs>/devices`.
    pub fn parent(&self, sysfs_root: &Path) -> Result<Option<Device>> {
        let devices_dir = sysfs::devices_dir(sysfs_root)?;
        sysfs::ancestor_dirs(&self.syspath, &devices_dir)
            .into_iter()
            .find(|dir| dir.join("uevent").is_file())
            .map(|dir| Device::from_syspath(&dir))
            .transpose()
    }

    /// Every ancestor device, nearest first.
    pub fn parents(&self, sysfs_root: &Path) -> Result<Vec<Device>> {
        let mut chain = Vec::new();
        let mut current = self.parent(sysfs_root)?;
        while let Some(device) = current {
            current = device.parent(sysfs_root)?;
            chain.push(device);
        }
        Ok(chain)
    }
}

/// Lists the devices of a subsystem from `<sysfs_root>/class/<subsystem>`, lowest node number
/// first. Only `event*` nodes (input) or `card*` nodes (drm) are returned. Production passes
/// `/sys` as `sysfs_root`.
pub fn enumerate(sysfs_root: &Path, subsystem: Subsystem) -> Result<Vec<Device>> {
    let class_dir = sysfs_root.join("class").join(subsystem.as_str());
    let entries = std::fs::read_dir(&class_dir).map_err(|source| Error::Io {
        path: class_dir.clone(),
        source,
    })?;

    let mut nodes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: class_dir.clone(),
            source,
        })?;
        let name = entry.file_name();
        let Some(number) = name.to_str().and_then(|name| subsystem.node_number(name)) else {
            continue;
        };
        nodes.push((number, sysfs::canonicalize(&entry.path())?));
    }
    nodes.sort();
    nodes
        .iter()
        .map(|(_, syspath)| Device::from_syspath(syspath))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_names() {
        assert_eq!(Subsystem::Drm.node_number("card1"), Some(1));
        assert_eq!(Subsystem::Drm.node_number("card1-DP-1"), None);
        assert_eq!(Subsystem::Drm.node_number("renderD128"), None);
        assert_eq!(Subsystem::Input.node_number("event10"), Some(10));
        assert_eq!(Subsystem::Input.node_number("mouse2"), None);
        assert_eq!(Subsystem::Input.node_number("input3"), None);
        assert_eq!(Subsystem::Input.node_number("event"), None);
    }
}
