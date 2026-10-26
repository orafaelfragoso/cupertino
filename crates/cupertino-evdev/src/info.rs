//! Device identity, capabilities and absolute-axis geometry.

use std::num::NonZeroU32;
use std::os::fd::BorrowedFd;

use crate::bitset::BitSet;
use crate::codes::{ABS_MAX, EV_ABS, EV_KEY, EV_MAX, EV_REL, EV_SW, INPUT_PROP_MAX, KEY_MAX, REL_MAX, SW_MAX};
use crate::error::Error;
use crate::ioctl;

/// `struct input_id`: where the device is attached and who made it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceId {
    /// Bus type (`BUS_USB`, `BUS_I2C`, `BUS_VIRTUAL`, ...).
    pub bustype: u16,
    /// Vendor id.
    pub vendor: u16,
    /// Product id.
    pub product: u16,
    /// Product version.
    pub version: u16,
}

/// Axis resolution in units per millimetre (units per radian for rotational axes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The driver reported 0: the physical size is not known. Callers must not guess one.
    Unknown,
    /// Device units per millimetre.
    UnitsPerMm(NonZeroU32),
}

impl Resolution {
    /// Parses the raw `input_absinfo.resolution`. A non-positive value is [`Resolution::Unknown`]: some drivers
    /// report a negative resolution, and the kernel and libevdev accept any value, so it must not fail the device.
    pub fn from_raw(raw: i32) -> Self {
        u32::try_from(raw).ok().and_then(NonZeroU32::new).map_or(Self::Unknown, Self::UnitsPerMm)
    }
}

/// `struct input_absinfo`: the range and geometry of one absolute axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbsInfo {
    /// The value when the info was queried.
    pub value: i32,
    /// Smallest value the axis reports.
    pub min: i32,
    /// Largest value the axis reports.
    pub max: i32,
    /// Noise the driver filters out, in axis units.
    pub fuzz: i32,
    /// Dead zone around the centre, in axis units.
    pub flat: i32,
    /// Units per millimetre.
    pub resolution: Resolution,
}

impl AbsInfo {
    /// Converts a length in device units to millimetres; `None` when the resolution is unknown.
    pub fn units_to_mm(&self, units: i32) -> Option<f64> {
        match self.resolution {
            Resolution::Unknown => None,
            Resolution::UnitsPerMm(per_mm) => Some(f64::from(units) / f64::from(per_mm.get())),
        }
    }
}

/// An absolute axis the device supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbsAxis {
    /// The axis code (`ABS_*`).
    pub code: u16,
    /// Its range and geometry.
    pub info: AbsInfo,
}

/// What an evdev device is and what it can report. Query it once with [`DeviceInfo::query`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Device name (`EVIOCGNAME`).
    pub name: String,
    /// Bus, vendor, product and version (`EVIOCGID`).
    pub id: DeviceId,
    /// Input properties (`INPUT_PROP_*`, `EVIOCGPROP`).
    pub properties: BitSet,
    /// Supported event types (`EV_*`).
    pub event_types: BitSet,
    /// Supported keys and buttons.
    pub keys: BitSet,
    /// Supported relative axes.
    pub rel_axes: BitSet,
    /// Supported switches.
    pub switches: BitSet,
    /// Supported absolute axes with their geometry, in ascending code order.
    pub abs_axes: Vec<AbsAxis>,
}

impl DeviceInfo {
    /// Reads the name, id, properties, capability bitmaps and every absolute axis from an evdev fd.
    pub fn query(fd: BorrowedFd<'_>) -> Result<Self, Error> {
        let event_types = BitSet::from_bytes(ioctl::event_bits(fd, 0, EV_MAX)?);
        let bits = |ev_type: u16, max: u16| -> Result<BitSet, Error> {
            if event_types.contains(ev_type) {
                Ok(BitSet::from_bytes(ioctl::event_bits(fd, ev_type, max)?))
            } else {
                Ok(BitSet::default())
            }
        };
        let keys = bits(EV_KEY, KEY_MAX)?;
        let rel_axes = bits(EV_REL, REL_MAX)?;
        let switches = bits(EV_SW, SW_MAX)?;
        let abs_codes = bits(EV_ABS, ABS_MAX)?;
        let abs_axes = abs_codes
            .iter()
            .map(|code| Ok(AbsAxis { code, info: ioctl::abs_info(fd, code)? }))
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            name: ioctl::name(fd)?,
            id: ioctl::id(fd)?,
            properties: BitSet::from_bytes(ioctl::properties(fd, INPUT_PROP_MAX)?),
            event_types,
            keys,
            rel_axes,
            switches,
            abs_axes,
        })
    }

    /// The geometry of absolute axis `code`, if the device has it.
    pub fn abs(&self, code: u16) -> Option<&AbsInfo> {
        self.abs_axes.iter().find(|axis| axis.code == code).map(|axis| &axis.info)
    }

    /// Whether the device declares input property `property` (`INPUT_PROP_*`).
    pub fn has_property(&self, property: u16) -> bool {
        self.properties.contains(property)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_resolution_is_unknown() {
        assert_eq!(Resolution::from_raw(0), Resolution::Unknown);
        assert_eq!(Resolution::from_raw(-1), Resolution::Unknown);
        assert_eq!(Resolution::from_raw(i32::MIN), Resolution::Unknown);
        assert_eq!(Resolution::from_raw(40), Resolution::UnitsPerMm(NonZeroU32::new(40).unwrap()));
        let info = |raw| AbsInfo {
            value: 0,
            min: 0,
            max: 4000,
            fuzz: 0,
            flat: 0,
            resolution: Resolution::from_raw(raw),
        };
        assert_eq!(info(0).units_to_mm(400), None);
        assert_eq!(info(40).units_to_mm(400), Some(10.0));
    }
}
