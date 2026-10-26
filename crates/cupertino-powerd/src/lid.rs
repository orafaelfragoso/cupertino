//! The lid switch: the evdev device that reports `SW_LID`.

use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use cupertino_devices::{classify_device, enumerate, Subsystem};
use cupertino_evdev::codes::SW_LID;
use cupertino_evdev::Device;
use rustix::fs::{open, Mode, OFlags};

use crate::error::{Error, Result};

/// An opened lid switch device.
pub struct LidSwitch {
    device: Device,
    path: PathBuf,
}

impl LidSwitch {
    /// Opens the first input device below `sysfs_root` that supports `SW_LID`, lowest node
    /// number first. Several switch devices can exist (tablet mode, docking); only a lid
    /// matters here. `None` when the machine has no lid. Opening `/dev/input/event*` needs root
    /// or the `input` group.
    pub fn find(sysfs_root: &Path) -> Result<Option<Self>> {
        for node in enumerate(sysfs_root, Subsystem::Input).map_err(Error::Devices)? {
            let class = classify_device(&node, sysfs_root).map_err(Error::Devices)?;
            let (Some(class), Some(path)) = (class, node.devnode()) else {
                continue;
            };
            if !class.switch {
                continue;
            }
            let fd = open(path, OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty()).map_err(|errno| Error::Open { path: path.to_owned(), errno })?;
            let evdev_error = |source| Error::Evdev { path: path.to_owned(), source };
            let device = Device::open(fd).map_err(evdev_error)?;
            if device.info().switches.contains(SW_LID) {
                return Ok(Some(Self { device, path: path.to_owned() }));
            }
        }
        Ok(None)
    }

    /// Whether the lid is closed now (`EVIOCGSW`).
    pub fn closed(&self) -> Result<bool> {
        let snapshot = self.device.snapshot().map_err(|source| Error::Evdev { path: self.path.clone(), source })?;
        Ok(snapshot.switches.contains(SW_LID))
    }

    /// Reads and drops the queued events so the descriptor stops being readable; the state
    /// comes from [`LidSwitch::closed`].
    pub fn drain(&mut self) -> Result<()> {
        self.device.read_frames(&mut Vec::new()).map_err(|source| Error::Evdev { path: self.path.clone(), source })
    }
}

impl AsFd for LidSwitch {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }
}
