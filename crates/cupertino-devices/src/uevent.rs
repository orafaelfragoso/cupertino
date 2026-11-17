//! `KEY=VALUE` property parsing shared by sysfs `uevent` files and kernel datagrams.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::dev_t::DevT;
use crate::error::ParseError;

/// The `uevent` properties of a device, by key.
pub type Properties = BTreeMap<String, String>;

/// Where the kernel's `DEVNAME` is rooted (it omits the `/dev/` prefix).
pub const DEV_ROOT: &str = "/dev";

/// Parses a sysfs `uevent` file: one `KEY=VALUE` per line.
pub fn parse_uevent_file(text: &str) -> Result<Properties, ParseError> {
    collect(text.lines())
}

/// Collects entries into a map. An entry without `=` or an empty entry is an error.
pub(crate) fn collect<'a>(
    entries: impl Iterator<Item = &'a str>,
) -> Result<Properties, ParseError> {
    let mut properties = Properties::new();
    for entry in entries {
        if entry.is_empty() {
            return Err(ParseError::EmptyProperty);
        }
        let (key, value) = entry
            .split_once('=')
            .ok_or_else(|| ParseError::MalformedProperty(entry.to_owned()))?;
        properties.insert(key.to_owned(), value.to_owned());
    }
    Ok(properties)
}

/// The device number from `MAJOR` and `MINOR`; `None` when the device has no node.
pub fn devnum(properties: &Properties) -> Result<Option<DevT>, ParseError> {
    let number = |key: &'static str| -> Result<Option<u32>, ParseError> {
        properties
            .get(key)
            .map(|value| {
                value.parse::<u32>().map_err(|_| ParseError::InvalidNumber {
                    key,
                    value: value.clone(),
                })
            })
            .transpose()
    };
    match (number("MAJOR")?, number("MINOR")?) {
        (Some(major), Some(minor)) => Ok(Some(DevT::new(major, minor))),
        (None, None) => Ok(None),
        _ => Err(ParseError::IncompleteDevnum),
    }
}

/// The node path under `/dev` from `DEVNAME`.
pub fn devnode(properties: &Properties) -> Option<PathBuf> {
    properties
        .get("DEVNAME")
        .map(|name| PathBuf::from(DEV_ROOT).join(name))
}
