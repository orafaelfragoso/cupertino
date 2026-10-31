//! GPU discovery: replaces smithay's `all_gpus` and `primary_gpu` for the udev backend.

use std::path::Path;

use crate::device::{self, Device, Subsystem};
use crate::error::{Error, ParseError, Result};
use crate::sysfs;

/// A DRM `card*` device and whether the firmware booted on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    /// The `card*` device; its node is `/dev/dri/card*`.
    pub device: Device,
    /// The `boot_vga` attribute of the nearest ancestor that has one (the PCI device). `false`
    /// when no ancestor has it, as on virtual or platform GPUs.
    pub boot_vga: bool,
}

/// Every GPU, lowest card number first.
pub fn gpus(sysfs_root: &Path) -> Result<Vec<Gpu>> {
    let devices_dir = sysfs::devices_dir(sysfs_root)?;
    device::enumerate(sysfs_root, Subsystem::Drm)?
        .into_iter()
        .map(|device| {
            let boot_vga = boot_vga(device.syspath(), &devices_dir)?;
            Ok(Gpu { device, boot_vga })
        })
        .collect()
}

/// The primary GPU: the one with `boot_vga` = 1, else the first card. `None` without any card.
pub fn primary_gpu(sysfs_root: &Path) -> Result<Option<Gpu>> {
    Ok(select_primary(gpus(sysfs_root)?))
}

/// Picks the primary GPU from `gpus` (ordered by card number): the first that booted the
/// firmware display, else the first one.
pub fn select_primary(gpus: Vec<Gpu>) -> Option<Gpu> {
    let index = gpus.iter().position(|gpu| gpu.boot_vga).unwrap_or(0);
    gpus.into_iter().nth(index)
}

/// Picks the GPU that renders from `gpus`: the card whose node is `wanted`, else the primary one.
pub fn select_render(gpus: Vec<Gpu>, wanted: Option<&Path>) -> Option<Gpu> {
    match gpus
        .iter()
        .position(|gpu| wanted.is_some() && gpu.device.devnode() == wanted)
    {
        Some(index) => gpus.into_iter().nth(index),
        None => select_primary(gpus),
    }
}

/// Reads `boot_vga` from the nearest ancestor of the card that has it.
fn boot_vga(syspath: &Path, devices_dir: &Path) -> Result<bool> {
    for dir in sysfs::ancestor_dirs(syspath, devices_dir) {
        let path = dir.join("boot_vga");
        let Some(text) = sysfs::read_attr_optional(&path)? else {
            continue;
        };
        return match text.trim() {
            "1" => Ok(true),
            "0" => Ok(false),
            other => Err(Error::Parse {
                origin: path.display().to_string(),
                source: ParseError::InvalidNumber {
                    key: "boot_vga",
                    value: other.to_owned(),
                },
            }),
        };
    }
    Ok(false)
}
