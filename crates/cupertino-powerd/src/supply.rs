//! Pure model of one `/sys/class/power_supply` entry: attribute text in, typed values out.
//!
//! Reference: the kernel ABI document `Documentation/ABI/testing/sysfs-class-power`. Units there:
//! energy in µWh, charge in µAh, voltage in µV, power in µW, current in µA. This module converts
//! to Wh, Ah, V, W and A. Rules that are not a plain sysfs value say where they come from:
//! the ABI document, UPower's documented behaviour, or an approximate choice of ours.

use std::collections::BTreeMap;
use std::fmt;

/// The text of the attribute files of one supply, trimmed, by file name. A file the supply does
/// not have is absent.
pub type Attributes = BTreeMap<String, String>;

/// The attribute files this model reads.
pub const ATTRIBUTES: [&str; 19] = [
    "type",
    "scope",
    "status",
    "present",
    "online",
    "capacity",
    "technology",
    "manufacturer",
    "model_name",
    "serial_number",
    "energy_now",
    "energy_full",
    "energy_full_design",
    "charge_now",
    "charge_full",
    "charge_full_design",
    "voltage_now",
    "power_now",
    "current_now",
];

/// Smallest discharge or charge rate (W, or A for charge-based supplies) from which a time
/// estimate is computed. approximate: UPower ignores rates near zero because the quotient is
/// meaningless; 0.01 is our placeholder for its cut-off.
const MIN_RATE: f64 = 0.01;

/// Longest time estimate reported (seconds). approximate: UPower drops estimates beyond a day
/// as noise; 20 hours is our placeholder.
const MAX_ESTIMATE_SECONDS: f64 = 20.0 * 3600.0;

/// A value the model cannot interpret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// A numeric attribute is not an integer.
    NotANumber { attribute: &'static str, value: String },
    /// A boolean attribute (`present`, `online`) is neither 0 nor 1.
    NotABoolean { attribute: &'static str, value: String },
    /// `type` is not one of the values the ABI document lists.
    UnknownType(String),
    /// `scope` is neither `System` nor `Device`.
    UnknownScope(String),
    /// `status` is not one of the values the ABI document lists.
    UnknownStatus(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber { attribute, value } => write!(f, "{attribute} is not an integer: {value:?}"),
            Self::NotABoolean { attribute, value } => write!(f, "{attribute} is not 0 or 1: {value:?}"),
            Self::UnknownType(value) => write!(f, "unknown supply type {value:?}"),
            Self::UnknownScope(value) => write!(f, "unknown supply scope {value:?}"),
            Self::UnknownStatus(value) => write!(f, "unknown battery status {value:?}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// What a supply is, from `type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `Unknown`.
    Unknown,
    /// `Battery`.
    Battery,
    /// `UPS`.
    Ups,
    /// `Mains`, `Wireless`, `BrickID` and every `USB*` type: something that supplies power when
    /// plugged in. UPower reports all of these as line power.
    LinePower,
}

/// The `status` of a battery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Unknown,
    Charging,
    Discharging,
    /// `Not charging`: connected to power but deliberately not charging (for example at a
    /// charge limit).
    NotCharging,
    Full,
}

/// A battery state, with the numbering of `org.freedesktop.UPower.Device.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unknown = 0,
    Charging = 1,
    Discharging = 2,
    Empty = 3,
    FullyCharged = 4,
    PendingCharge = 5,
}

/// Which family of `*_now`/`*_full` attributes a battery reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// `energy_*`, in Wh.
    Energy,
    /// `charge_*`, in Ah. Needs the voltage to become energy.
    Charge,
}

/// The amounts a battery reports in one [`Unit`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Amounts {
    pub unit: Unit,
    pub now: Option<f64>,
    pub full: Option<f64>,
    pub full_design: Option<f64>,
}

/// The battery part of a supply of kind [`Kind::Battery`] or [`Kind::Ups`].
#[derive(Debug, Clone, PartialEq)]
pub struct Battery {
    /// `present`; a battery without the attribute is present (the ABI document defines it for
    /// removable batteries only).
    pub present: bool,
    pub status: Status,
    /// `capacity`, 0 to 100, as the firmware reports it.
    pub capacity: Option<f64>,
    pub amounts: Option<Amounts>,
    /// `voltage_now`, V.
    pub voltage: Option<f64>,
    /// `power_now`, W.
    pub power: Option<f64>,
    /// `current_now`, A, always positive: drivers disagree on the sign, and [`Battery::state`]
    /// already tells the direction.
    pub current: Option<f64>,
}

/// One power supply.
#[derive(Debug, Clone, PartialEq)]
pub struct Supply {
    /// The directory name, for example `BAT0`.
    pub name: String,
    pub kind: Kind,
    /// False for a battery of an attached device (`scope` = `Device`, for example a wireless
    /// mouse); true when the supply powers the system.
    pub power_supply: bool,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    /// `online`, for supplies that have it.
    pub online: Option<bool>,
    /// Some for [`Kind::Battery`] and [`Kind::Ups`].
    pub battery: Option<Battery>,
}

fn integer(attributes: &Attributes, attribute: &'static str) -> Result<Option<i64>, ParseError> {
    attributes
        .get(attribute)
        .map(|value| value.parse().map_err(|_| ParseError::NotANumber { attribute, value: value.clone() }))
        .transpose()
}

/// An attribute in micro-units, as a base unit.
fn micro(attributes: &Attributes, attribute: &'static str) -> Result<Option<f64>, ParseError> {
    Ok(integer(attributes, attribute)?.map(|value| value as f64 / 1e6))
}

fn boolean(attributes: &Attributes, attribute: &'static str) -> Result<Option<bool>, ParseError> {
    match attributes.get(attribute).map(String::as_str) {
        None => Ok(None),
        Some("0") => Ok(Some(false)),
        Some("1") => Ok(Some(true)),
        Some(value) => Err(ParseError::NotABoolean { attribute, value: value.to_owned() }),
    }
}

fn text(attributes: &Attributes, attribute: &str) -> Option<String> {
    attributes.get(attribute).filter(|value| !value.is_empty()).cloned()
}

impl Kind {
    fn parse(value: &str) -> Result<Self, ParseError> {
        match value {
            "Unknown" => Ok(Self::Unknown),
            "Battery" => Ok(Self::Battery),
            "UPS" => Ok(Self::Ups),
            "Mains" | "Wireless" | "BrickID" | "USB" | "USB_DCP" | "USB_CDP" | "USB_ACA" | "USB_C" | "USB_PD" | "USB_PD_DRP" => Ok(Self::LinePower),
            other => Err(ParseError::UnknownType(other.to_owned())),
        }
    }
}

impl Status {
    fn parse(value: &str) -> Result<Self, ParseError> {
        match value {
            "Unknown" => Ok(Self::Unknown),
            "Charging" => Ok(Self::Charging),
            "Discharging" => Ok(Self::Discharging),
            "Not charging" => Ok(Self::NotCharging),
            "Full" => Ok(Self::Full),
            other => Err(ParseError::UnknownStatus(other.to_owned())),
        }
    }
}

impl Amounts {
    /// The `energy_*` attributes when any is present, else the `charge_*` ones, else none.
    fn parse(attributes: &Attributes) -> Result<Option<Self>, ParseError> {
        let energy = Self {
            unit: Unit::Energy,
            now: micro(attributes, "energy_now")?,
            full: micro(attributes, "energy_full")?,
            full_design: micro(attributes, "energy_full_design")?,
        };
        let charge = Self {
            unit: Unit::Charge,
            now: micro(attributes, "charge_now")?,
            full: micro(attributes, "charge_full")?,
            full_design: micro(attributes, "charge_full_design")?,
        };
        let has_any = |amounts: &Self| amounts.now.is_some() || amounts.full.is_some() || amounts.full_design.is_some();
        Ok(if has_any(&energy) {
            Some(energy)
        } else if has_any(&charge) {
            Some(charge)
        } else {
            None
        })
    }
}

impl Battery {
    fn parse(attributes: &Attributes) -> Result<Self, ParseError> {
        Ok(Self {
            present: boolean(attributes, "present")?.unwrap_or(true),
            status: attributes.get("status").map(|value| Status::parse(value)).transpose()?.unwrap_or(Status::Unknown),
            capacity: integer(attributes, "capacity")?.map(|value| value as f64),
            amounts: Amounts::parse(attributes)?,
            voltage: micro(attributes, "voltage_now")?,
            power: micro(attributes, "power_now")?.map(f64::abs),
            current: micro(attributes, "current_now")?.map(f64::abs),
        })
    }

    /// Converts an amount in this battery's unit to Wh. Charge needs `voltage_now`.
    fn watt_hours(&self, amount: f64) -> Option<f64> {
        match self.amounts?.unit {
            Unit::Energy => Some(amount),
            Unit::Charge => self.voltage.map(|voltage| amount * voltage),
        }
    }

    /// Energy now, Wh.
    pub fn energy(&self) -> Option<f64> {
        self.watt_hours(self.amounts?.now?)
    }

    /// Energy when full, Wh.
    pub fn energy_full(&self) -> Option<f64> {
        self.watt_hours(self.amounts?.full?)
    }

    /// Energy when full as designed, Wh.
    pub fn energy_full_design(&self) -> Option<f64> {
        self.watt_hours(self.amounts?.full_design?)
    }

    /// The charge or discharge rate in W: `power_now`, else `current_now` times `voltage_now`.
    pub fn energy_rate(&self) -> Option<f64> {
        self.power.or_else(|| Some(self.current? * self.voltage?))
    }

    /// The rate in the unit of the amounts (W for energy, A for charge), so that
    /// `remaining / rate` is hours without any voltage in between when the driver reports the
    /// matching quantity.
    fn rate_in_unit(&self, unit: Unit) -> Option<f64> {
        match unit {
            Unit::Energy => self.energy_rate(),
            Unit::Charge => self.current.or_else(|| {
                let voltage = self.voltage.filter(|voltage| *voltage > 0.0)?;
                Some(self.power? / voltage)
            }),
        }
    }

    /// Percentage, 0 to 100. UPower derives it from the amounts (`now / full`), the most precise
    /// source, and uses the firmware's `capacity` only when the amounts are missing. The ratio
    /// is taken in the reported unit, so it needs no voltage.
    pub fn percentage(&self) -> Option<f64> {
        let ratio = self.amounts.and_then(|amounts| {
            let (now, full) = (amounts.now?, amounts.full?);
            (full > 0.0).then(|| (now / full * 100.0).min(100.0))
        });
        ratio.or(self.capacity)
    }

    /// The state. `Not charging` is UPower's "pending charge". A battery discharging at 0 percent
    /// is empty (the ABI document has no `Empty` status). A battery that is not present has
    /// no state.
    pub fn state(&self) -> State {
        if !self.present {
            return State::Unknown;
        }
        match self.status {
            Status::Unknown => State::Unknown,
            Status::Charging => State::Charging,
            Status::Discharging if self.percentage() == Some(0.0) => State::Empty,
            Status::Discharging => State::Discharging,
            Status::NotCharging => State::PendingCharge,
            Status::Full => State::FullyCharged,
        }
    }

    /// Seconds until empty at the current rate; only while discharging.
    pub fn time_to_empty(&self) -> Option<u64> {
        if self.state() != State::Discharging {
            return None;
        }
        let amounts = self.amounts?;
        estimate(amounts.now?, self.rate_in_unit(amounts.unit)?)
    }

    /// Seconds until full at the current rate; only while charging.
    pub fn time_to_full(&self) -> Option<u64> {
        if self.state() != State::Charging {
            return None;
        }
        let amounts = self.amounts?;
        estimate(amounts.full? - amounts.now?, self.rate_in_unit(amounts.unit)?)
    }

    /// Full capacity as a percentage of the design capacity (UPower's `Capacity`, the battery's
    /// health).
    pub fn health(&self) -> Option<f64> {
        let amounts = self.amounts?;
        let design = amounts.full_design.filter(|design| *design > 0.0)?;
        Some(amounts.full? / design * 100.0)
    }
}

/// `remaining / rate` hours as seconds; none for a negligible rate or an implausible result.
pub(crate) fn estimate(remaining: f64, rate: f64) -> Option<u64> {
    if rate < MIN_RATE || remaining < 0.0 {
        return None;
    }
    let seconds = remaining / rate * 3600.0;
    (seconds <= MAX_ESTIMATE_SECONDS).then(|| seconds.round() as u64)
}

impl Supply {
    /// Interprets the attributes of the supply directory `name`. A supply without `type` is
    /// [`Kind::Unknown`]: the attribute is mandatory in the ABI document, but this model must not
    /// refuse a driver that omits it.
    pub fn parse(name: &str, attributes: &Attributes) -> Result<Self, ParseError> {
        let kind = attributes.get("type").map(|value| Kind::parse(value)).transpose()?.unwrap_or(Kind::Unknown);
        let power_supply = match attributes.get("scope").map(String::as_str) {
            None | Some("System") => true,
            Some("Device") => false,
            Some(other) => return Err(ParseError::UnknownScope(other.to_owned())),
        };
        let battery = match kind {
            Kind::Battery | Kind::Ups => Some(Battery::parse(attributes)?),
            Kind::Unknown | Kind::LinePower => None,
        };
        Ok(Self {
            name: name.to_owned(),
            kind,
            power_supply,
            vendor: text(attributes, "manufacturer"),
            model: text(attributes, "model_name"),
            serial: text(attributes, "serial_number"),
            online: boolean(attributes, "online")?,
            battery,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributes(pairs: &[(&str, &str)]) -> Attributes {
        pairs.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect()
    }

    fn battery(pairs: &[(&str, &str)]) -> Battery {
        let mut all = vec![("type", "Battery")];
        all.extend_from_slice(pairs);
        Supply::parse("BAT0", &attributes(&all)).unwrap().battery.unwrap()
    }

    #[test]
    fn charge_without_voltage_still_gives_percentage_and_time() {
        let battery = battery(&[("status", "Discharging"), ("charge_now", "1500000"), ("charge_full", "3000000"), ("current_now", "500000")]);
        assert_eq!(battery.percentage(), Some(50.0));
        assert_eq!(battery.time_to_empty(), Some(10_800));
        assert_eq!(battery.energy(), None);
    }

    #[test]
    fn firmware_capacity_is_the_fallback_without_amounts() {
        let battery = battery(&[("status", "Discharging"), ("capacity", "42")]);
        assert_eq!(battery.percentage(), Some(42.0));
        assert_eq!(battery.time_to_empty(), None);
    }

    #[test]
    fn discharging_at_zero_is_empty_and_not_charging_is_pending() {
        assert_eq!(battery(&[("status", "Discharging"), ("capacity", "0")]).state(), State::Empty);
        assert_eq!(battery(&[("status", "Not charging"), ("capacity", "80")]).state(), State::PendingCharge);
    }

    #[test]
    fn estimates_ignore_a_negligible_rate_and_implausible_spans() {
        let idle = battery(&[("status", "Discharging"), ("energy_now", "25000000"), ("energy_full", "50000000"), ("power_now", "5000")]);
        assert_eq!(idle.time_to_empty(), None);
        let slow = battery(&[("status", "Discharging"), ("energy_now", "25000000"), ("energy_full", "50000000"), ("power_now", "1000000")]);
        assert_eq!(slow.time_to_empty(), None);
    }

    #[test]
    fn unknown_values_are_errors() {
        let parse = |pairs: &[(&str, &str)]| Supply::parse("X", &attributes(pairs)).unwrap_err();
        assert_eq!(parse(&[("type", "Fusion")]), ParseError::UnknownType("Fusion".to_owned()));
        assert_eq!(parse(&[("type", "Battery"), ("status", "Exploding")]), ParseError::UnknownStatus("Exploding".to_owned()));
        assert_eq!(parse(&[("type", "Mains"), ("online", "2")]), ParseError::NotABoolean { attribute: "online", value: "2".to_owned() });
        assert_eq!(parse(&[("type", "Battery"), ("capacity", "n/a")]), ParseError::NotANumber { attribute: "capacity", value: "n/a".to_owned() });
    }
}
