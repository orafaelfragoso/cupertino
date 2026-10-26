//! The values of an `org.freedesktop.UPower.Device`, computed from supplies, and the machine-wide
//! facts built from several supplies (the display device, on-battery).
//!
//! The property names, types and enum numbering are those of UPower's D-Bus documentation
//! (<https://upower.freedesktop.org/docs/Device.html>). How several batteries combine follows
//! UPower's documented description of the display device: one virtual device that stands for all
//! power-supply batteries.

use crate::supply::{estimate, Battery, Kind, State, Supply};

/// `org.freedesktop.UPower.Device.Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Unknown = 0,
    LinePower = 1,
    Battery = 2,
    Ups = 3,
}

/// Every value of a device that the interface serves. Zero or empty means "not known", as in UPower.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceView {
    pub native_path: String,
    pub vendor: String,
    pub model: String,
    pub serial: String,
    pub kind: DeviceType,
    pub power_supply: bool,
    pub online: bool,
    /// Wh.
    pub energy: f64,
    /// Wh.
    pub energy_empty: f64,
    /// Wh.
    pub energy_full: f64,
    /// Wh.
    pub energy_full_design: f64,
    /// W.
    pub energy_rate: f64,
    /// V.
    pub voltage: f64,
    /// Seconds; 0 when unknown.
    pub time_to_empty: i64,
    /// Seconds; 0 when unknown.
    pub time_to_full: i64,
    /// 0 to 100.
    pub percentage: f64,
    pub is_present: bool,
    pub state: State,
    pub is_rechargeable: bool,
    /// Health: full capacity as a percentage of the design capacity.
    pub capacity: f64,
    pub icon_name: String,
}

/// The icon for a charge level. approximate: the icon names are from the freedesktop
/// icon naming specification (`battery-{empty,caution,low,good,full}); the percentage bounds
/// between them are our placeholders.
fn battery_icon(percentage: f64, state: State, is_present: bool) -> String {
    if !is_present {
        return "battery-missing-symbolic".to_owned();
    }
    match state {
        State::FullyCharged => return "battery-full-charged-symbolic".to_owned(),
        State::Empty => return "battery-empty-symbolic".to_owned(),
        _ => {}
    }
    let level = match percentage {
        p if p < 5.0 => "empty",
        p if p < 20.0 => "caution",
        p if p < 40.0 => "low",
        p if p < 80.0 => "good",
        _ => "full",
    };
    let charging = if state == State::Charging {
        "-charging"
    } else {
        ""
    };
    format!("battery-{level}{charging}-symbolic")
}

impl DeviceView {
    /// A device about which nothing is known.
    fn blank(kind: DeviceType) -> Self {
        Self {
            native_path: String::new(),
            vendor: String::new(),
            model: String::new(),
            serial: String::new(),
            kind,
            power_supply: false,
            online: false,
            energy: 0.0,
            energy_empty: 0.0,
            energy_full: 0.0,
            energy_full_design: 0.0,
            energy_rate: 0.0,
            voltage: 0.0,
            time_to_empty: 0,
            time_to_full: 0,
            percentage: 0.0,
            is_present: false,
            state: State::Unknown,
            is_rechargeable: false,
            capacity: 0.0,
            icon_name: String::new(),
        }
    }

    /// The device for one supply.
    pub fn of_supply(supply: &Supply) -> Self {
        let kind = match supply.kind {
            Kind::Unknown => DeviceType::Unknown,
            Kind::LinePower => DeviceType::LinePower,
            Kind::Battery => DeviceType::Battery,
            Kind::Ups => DeviceType::Ups,
        };
        let mut view = Self::blank(kind);
        view.native_path = supply.name.clone();
        view.vendor = supply.vendor.clone().unwrap_or_default();
        view.model = supply.model.clone().unwrap_or_default();
        view.serial = supply.serial.clone().unwrap_or_default();
        view.power_supply = supply.power_supply;
        view.online = supply.online.unwrap_or(false);
        match &supply.battery {
            Some(battery) => {
                view.energy = battery.energy().unwrap_or(0.0);
                view.energy_full = battery.energy_full().unwrap_or(0.0);
                view.energy_full_design = battery.energy_full_design().unwrap_or(0.0);
                view.energy_rate = battery.energy_rate().unwrap_or(0.0);
                view.voltage = battery.voltage.unwrap_or(0.0);
                view.time_to_empty = battery.time_to_empty().map_or(0, |seconds| seconds as i64);
                view.time_to_full = battery.time_to_full().map_or(0, |seconds| seconds as i64);
                view.percentage = battery.percentage().unwrap_or(0.0);
                view.is_present = battery.present;
                view.state = battery.state();
                view.is_rechargeable = true;
                view.capacity = battery.health().unwrap_or(0.0);
                view.icon_name = battery_icon(view.percentage, view.state, view.is_present);
            }
            None if supply.kind == Kind::LinePower => {
                view.is_present = true;
                view.icon_name = "ac-adapter-symbolic".to_owned();
            }
            None => {}
        }
        view
    }

    /// The display device: all present power-supply batteries as one. Energies and rates add up.
    /// The percentage is total energy over total full energy, or the mean of the batteries'
    /// percentages when some battery has no energy figure. The state is charging if any battery
    /// charges, else discharging if any discharges, else fully charged if all are, else empty,
    /// pending charge or unknown in that order. The time to empty divides the total energy by the
    /// rate of the discharging batteries; the time to full divides the missing energy by the rate
    /// of the charging ones.
    pub fn display(supplies: &[Supply]) -> Self {
        let batteries: Vec<&Battery> = supplies
            .iter()
            .filter(|supply| supply.power_supply)
            .filter_map(|supply| supply.battery.as_ref())
            .filter(|battery| battery.present)
            .collect();
        if batteries.is_empty() {
            let mut view = Self::blank(DeviceType::Unknown);
            view.power_supply = true;
            view.icon_name = battery_icon(0.0, State::Unknown, false);
            return view;
        }
        let total = |value: fn(&Battery) -> Option<f64>| {
            batteries
                .iter()
                .map(|battery| value(battery))
                .sum::<Option<f64>>()
        };
        let (energy, energy_full) = (total(Battery::energy), total(Battery::energy_full));
        let percentage = match (energy, energy_full) {
            (Some(now), Some(full)) if full > 0.0 => (now / full * 100.0).min(100.0),
            _ => batteries
                .iter()
                .map(|battery| battery.percentage())
                .sum::<Option<f64>>()
                .map_or(0.0, |sum| sum / batteries.len() as f64),
        };
        let states: Vec<State> = batteries.iter().map(|battery| battery.state()).collect();
        let any = |wanted: State| states.contains(&wanted);
        let state = if any(State::Charging) {
            State::Charging
        } else if any(State::Discharging) {
            State::Discharging
        } else if states.iter().all(|state| *state == State::FullyCharged) {
            State::FullyCharged
        } else if any(State::Empty) {
            State::Empty
        } else if any(State::PendingCharge) {
            State::PendingCharge
        } else {
            State::Unknown
        };
        let rate_of = |wanted: State| {
            batteries
                .iter()
                .filter(|battery| battery.state() == wanted)
                .map(|battery| battery.energy_rate())
                .sum::<Option<f64>>()
        };
        let time_to_empty = match (state, energy, rate_of(State::Discharging)) {
            (State::Discharging, Some(energy), Some(rate)) => estimate(energy, rate),
            _ => None,
        };
        let time_to_full = match (state, energy, energy_full, rate_of(State::Charging)) {
            (State::Charging, Some(energy), Some(full), Some(rate)) => {
                estimate(full - energy, rate)
            }
            _ => None,
        };
        let design = total(Battery::energy_full_design);

        let mut view = Self::blank(DeviceType::Battery);
        view.power_supply = true;
        view.is_present = true;
        view.is_rechargeable = true;
        view.energy = energy.unwrap_or(0.0);
        view.energy_full = energy_full.unwrap_or(0.0);
        view.energy_full_design = design.unwrap_or(0.0);
        view.energy_rate = total(Battery::energy_rate).unwrap_or(0.0);
        view.percentage = percentage;
        view.state = state;
        view.time_to_empty = time_to_empty.map_or(0, |seconds| seconds as i64);
        view.time_to_full = time_to_full.map_or(0, |seconds| seconds as i64);
        view.capacity = match (energy_full, design) {
            (Some(full), Some(design)) if design > 0.0 => full / design * 100.0,
            _ => 0.0,
        };
        view.icon_name = battery_icon(percentage, state, true);
        view
    }
}

/// UPower's `OnBattery`: the machine runs on battery when no line power is online and some
/// present power-supply battery is discharging or empty. A machine without batteries is never on
/// battery.
pub fn on_battery(supplies: &[Supply]) -> bool {
    let line_power_online = supplies
        .iter()
        .any(|supply| supply.kind == Kind::LinePower && supply.online == Some(true));
    let draining = supplies
        .iter()
        .filter(|supply| supply.power_supply)
        .filter_map(|supply| supply.battery.as_ref())
        .any(|battery| matches!(battery.state(), State::Discharging | State::Empty));
    !line_power_online && draining
}
