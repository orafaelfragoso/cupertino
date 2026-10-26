//! Reading the power, backlight and LED classes from a sysfs root (`/sys` in production, a
//! temporary tree in tests), and writing a brightness.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rustix::io::Errno;

use crate::backlight::{
    self, Backlight, BacklightType, KeyboardBacklight, KEYBOARD_BACKLIGHT_SUFFIX,
};
use crate::error::{Error, Result};
use crate::supply::{Attributes, Supply, ATTRIBUTES};

/// Whether the lid switch exists and what it reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lid {
    pub present: bool,
    pub closed: bool,
}

/// Everything the daemon knows about the hardware at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Ordered by directory name.
    pub supplies: Vec<Supply>,
    /// Ordered by directory name.
    pub backlights: Vec<Backlight>,
    /// Ordered by directory name.
    pub keyboards: Vec<KeyboardBacklight>,
    pub lid: Lid,
}

impl Snapshot {
    /// Reads the three sysfs classes under `sysfs_root`. The lid is not in sysfs; the caller
    /// passes what the switch device reported.
    pub fn read(sysfs_root: &Path, lid: Lid) -> Result<Self> {
        Ok(Self {
            supplies: read_supplies(sysfs_root)?,
            backlights: read_backlights(sysfs_root)?,
            keyboards: read_keyboard_backlights(sysfs_root)?,
            lid,
        })
    }

    /// The display backlight under control.
    pub fn backlight(&self) -> Option<&Backlight> {
        backlight::select(&self.backlights)
    }
}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_owned(),
        source,
    }
}

/// The entries of `<root>/class/<class>` as `(name, directory)`, ordered by name. A missing class
/// directory means the kernel has no device of that class (the directory is created when the
/// first one registers), so it lists nothing.
fn class_entries(sysfs_root: &Path, class: &str) -> Result<Vec<(String, PathBuf)>> {
    let class_dir = sysfs_root.join("class").join(class);
    let entries = match fs::read_dir(&class_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(&class_dir)(source)),
    };
    let mut listed = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error(&class_dir))?;
        // Names that are not UTF-8 cannot be object path components or sysfs names the kernel
        // creates; they are an error, not a skip.
        let name = entry.file_name().into_string().map_err(|name| Error::Io {
            path: class_dir.join(name),
            source: io::Error::new(io::ErrorKind::InvalidData, "directory name is not UTF-8"),
        })?;
        listed.push((name, entry.path()));
    }
    listed.sort();
    Ok(listed)
}

/// Reads one attribute, trimmed. `None` when the file does not exist, and when the driver cannot
/// report the property right now: the kernel answers such a read with `ENODATA` or `ENODEV`.
fn read_attribute(dir: &Path, attribute: &str) -> Result<Option<String>> {
    let path = dir.join(attribute);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text.trim().to_owned())),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source)
            if matches!(
                source.raw_os_error().map(Errno::from_raw_os_error),
                Some(Errno::NODATA | Errno::NODEV)
            ) =>
        {
            Ok(None)
        }
        Err(source) => Err(io_error(&path)(source)),
    }
}

fn require_attribute(dir: &Path, attribute: &str) -> Result<String> {
    read_attribute(dir, attribute)?.ok_or_else(|| {
        let path = dir.join(attribute);
        io_error(&path)(io::Error::new(
            io::ErrorKind::NotFound,
            "required attribute is missing",
        ))
    })
}

/// Reads every supply of `<root>/class/power_supply`.
pub fn read_supplies(sysfs_root: &Path) -> Result<Vec<Supply>> {
    class_entries(sysfs_root, "power_supply")?
        .into_iter()
        .map(|(name, dir)| {
            let mut attributes = Attributes::new();
            for attribute in ATTRIBUTES {
                if let Some(text) = read_attribute(&dir, attribute)? {
                    attributes.insert(attribute.to_owned(), text);
                }
            }
            Supply::parse(&name, &attributes).map_err(|source| Error::Supply { name, source })
        })
        .collect()
}

/// Reads every backlight of `<root>/class/backlight`.
pub fn read_backlights(sysfs_root: &Path) -> Result<Vec<Backlight>> {
    class_entries(sysfs_root, "backlight")?
        .into_iter()
        .map(|(name, dir)| {
            let parse_error = |source| Error::Backlight {
                name: name.clone(),
                source,
            };
            let kind =
                BacklightType::parse(&require_attribute(&dir, "type")?).map_err(parse_error)?;
            let max_brightness = backlight::brightness_value(
                "max_brightness",
                &require_attribute(&dir, "max_brightness")?,
            )
            .map_err(parse_error)?;
            let (attribute, text) = match read_attribute(&dir, "actual_brightness")? {
                Some(text) => ("actual_brightness", text),
                None => ("brightness", require_attribute(&dir, "brightness")?),
            };
            let brightness = backlight::brightness_value(attribute, &text).map_err(parse_error)?;
            Ok(Backlight {
                name: name.clone(),
                kind,
                brightness,
                max_brightness,
            })
        })
        .collect()
}

/// Reads every keyboard backlight LED of `<root>/class/leds`.
pub fn read_keyboard_backlights(sysfs_root: &Path) -> Result<Vec<KeyboardBacklight>> {
    class_entries(sysfs_root, "leds")?
        .into_iter()
        .filter(|(name, _)| name.ends_with(KEYBOARD_BACKLIGHT_SUFFIX))
        .map(|(name, dir)| {
            let parse_error = |source| Error::Backlight {
                name: name.clone(),
                source,
            };
            let brightness =
                backlight::brightness_value("brightness", &require_attribute(&dir, "brightness")?)
                    .map_err(parse_error)?;
            let max_brightness = backlight::brightness_value(
                "max_brightness",
                &require_attribute(&dir, "max_brightness")?,
            )
            .map_err(parse_error)?;
            Ok(KeyboardBacklight {
                name: name.clone(),
                brightness,
                max_brightness,
            })
        })
        .collect()
}

/// The directory of a display backlight.
pub fn backlight_dir(sysfs_root: &Path, name: &str) -> PathBuf {
    sysfs_root.join("class/backlight").join(name)
}

/// The directory of a keyboard backlight LED.
pub fn led_dir(sysfs_root: &Path, name: &str) -> PathBuf {
    sysfs_root.join("class/leds").join(name)
}

/// Writes `brightness` in `dir`. This needs root on a real system.
pub fn write_brightness(dir: &Path, value: u32) -> Result<()> {
    let path = dir.join("brightness");
    fs::write(&path, value.to_string()).map_err(io_error(&path))
}
