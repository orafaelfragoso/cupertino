//! GPU discovery: replaces smithay's `all_gpus` and `primary_gpu` for the udev backend.

use std::path::{Path, PathBuf};

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
    /// The `renderD*` node of the same device, as `/dev/dri/renderD128`; `None` on a display-only card.
    pub render_node: Option<PathBuf>,
}

/// How [`select_render`] chose the GPU that renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderChoice {
    /// No override is set, or it is empty: the primary GPU.
    Primary,
    /// The override names this card's node or its render node.
    Named,
    /// The override names no GPU: the primary one renders.
    Unmatched(PathBuf),
}

/// Every GPU, lowest card number first.
pub fn gpus(sysfs_root: &Path) -> Result<Vec<Gpu>> {
    let devices_dir = sysfs::devices_dir(sysfs_root)?;
    device::enumerate(sysfs_root, Subsystem::Drm)?
        .into_iter()
        .map(|device| {
            let boot_vga = boot_vga(device.syspath(), &devices_dir)?;
            let render_node = render_node(device.syspath());
            Ok(Gpu {
                device,
                boot_vga,
                render_node,
            })
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

/// Picks the GPU that renders from `gpus`: the card whose node or render node is `wanted`, else the primary one.
/// `None` without any card.
pub fn select_render(gpus: Vec<Gpu>, wanted: Option<&Path>) -> Option<(Gpu, RenderChoice)> {
    let Some(wanted) = wanted.filter(|w| !w.as_os_str().is_empty()) else {
        return select_primary(gpus).map(|gpu| (gpu, RenderChoice::Primary));
    };
    let named = gpus.iter().position(|gpu| {
        gpu.device.devnode() == Some(wanted) || gpu.render_node.as_deref() == Some(wanted)
    });
    match named {
        Some(index) => gpus
            .into_iter()
            .nth(index)
            .map(|gpu| (gpu, RenderChoice::Named)),
        None => select_primary(gpus).map(|gpu| (gpu, RenderChoice::Unmatched(wanted.to_owned()))),
    }
}

/// The `/dev` node of the first readable `renderD*` device next to the card, in the same `drm/` directory. Only the
/// override reads it, so a directory or device that cannot be read leaves it `None` instead of failing the listing.
fn render_node(card_syspath: &Path) -> Option<PathBuf> {
    let mut nodes: Vec<(u32, PathBuf)> = std::fs::read_dir(card_syspath.parent()?)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let number = entry
                .file_name()
                .to_str()?
                .strip_prefix("renderD")?
                .parse()
                .ok()?;
            Some((number, entry.path()))
        })
        .collect();
    nodes.sort();
    nodes.into_iter().find_map(|(_, syspath)| {
        Device::from_syspath(&syspath)
            .ok()?
            .devnode()
            .map(Path::to_owned)
    })
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
