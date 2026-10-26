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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_node_uevent() {
        let properties = parse_uevent_file("MAJOR=13\nMINOR=67\nDEVNAME=input/event3\n").unwrap();
        assert_eq!(devnum(&properties).unwrap(), Some(DevT::new(13, 67)));
        assert_eq!(
            devnode(&properties),
            Some(PathBuf::from("/dev/input/event3"))
        );
    }

    #[test]
    fn value_keeps_inner_equals_signs() {
        let properties = parse_uevent_file("MODALIAS=a=b\n").unwrap();
        assert_eq!(properties["MODALIAS"], "a=b");
    }

    #[test]
    fn entry_without_equals_is_an_error() {
        assert_eq!(
            parse_uevent_file("MAJOR=13\nGARBAGE\n"),
            Err(ParseError::MalformedProperty("GARBAGE".to_owned()))
        );
    }

    #[test]
    fn blank_line_is_an_error() {
        assert_eq!(
            parse_uevent_file("A=1\n\nB=2\n"),
            Err(ParseError::EmptyProperty)
        );
    }

    #[test]
    fn lone_major_is_an_error() {
        let properties = parse_uevent_file("MAJOR=13\n").unwrap();
        assert_eq!(devnum(&properties), Err(ParseError::IncompleteDevnum));
    }

    #[test]
    fn non_numeric_minor_is_an_error() {
        let properties = parse_uevent_file("MAJOR=13\nMINOR=x\n").unwrap();
        assert_eq!(
            devnum(&properties),
            Err(ParseError::InvalidNumber {
                key: "MINOR",
                value: "x".to_owned()
            })
        );
    }

    #[test]
    fn device_without_node_has_no_devnum() {
        let properties = parse_uevent_file("DRIVER=i915\n").unwrap();
        assert_eq!(devnum(&properties).unwrap(), None);
        assert_eq!(devnode(&properties), None);
    }
}
