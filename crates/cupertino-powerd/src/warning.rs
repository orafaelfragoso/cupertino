//! The low-battery warning level of the machine, by the design's rules (from the design's
//! power daemon):
//!
//! * the percent is `lround(100 × now / full) clamped to 1 … 100, and it only rises while the battery charges or
//!   while the ignore window is open;
//! * on battery, a percent of 2 or less is the final warning and 3 to 10 the early one; a level is raised only above
//!   the one already reached, and external power resets it to none;
//! * a change of the charging or full state opens a 60 s window during which no warning is raised.
//!
//! The levels map onto UPower's `WarningLevel` (early = low, final = critical). Pure: the daemon passes the time.

use crate::supply::{State, Supply};
use crate::view::{on_battery, DeviceView};

/// Seconds after a change of the charging or full state during which no warning is raised.
pub const IGNORE_WINDOW_S: f64 = 60.0;
/// Highest percent of the final warning.
pub const FINAL_PERCENT: u32 = 2;
/// Highest percent of the early warning.
pub const EARLY_PERCENT: u32 = 10;

/// The design's warning levels, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    None,
    Early,
    Final,
}

impl Level {
    /// `org.freedesktop.UPower.Device.WarningLevel`: 1 none, 3 low, 4 critical.
    pub fn upower(self) -> u32 {
        match self {
            Level::None => 1,
            Level::Early => 3,
            Level::Final => 4,
        }
    }
}

/// What the warning rules read from the machine's batteries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// UPower's `OnBattery`: no line power and a battery drains.
    pub on_battery: bool,
    pub charging: bool,
    pub full: bool,
    /// The display device's percentage, 0 … 100.
    pub percentage: f64,
}

impl Reading {
    /// The reading of the power-supply batteries; `None` without one.
    pub fn of(supplies: &[Supply]) -> Option<Reading> {
        let view = DeviceView::display(supplies);
        view.is_present.then(|| Reading {
            on_battery: on_battery(supplies),
            charging: view.state == State::Charging,
            full: view.state == State::FullyCharged,
            percentage: view.percentage,
        })
    }
}

/// The warning state between readings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Warnings {
    level: Option<Level>,
    percent: Option<u32>,
    /// Charging and full of the last reading.
    flags: Option<(bool, bool)>,
    ignore_until: Option<f64>,
}

impl Warnings {
    pub fn new() -> Self {
        Self::default()
    }

    /// The level after `reading` at `now` (seconds on a monotonic clock).
    pub fn update(&mut self, now: f64, reading: Option<Reading>) -> Level {
        let Some(r) = reading else {
            *self = Self::default();
            return Level::None;
        };
        let flags = (r.charging, r.full);
        if self.flags.is_some_and(|before| before != flags) {
            self.ignore_until = Some(now + IGNORE_WINDOW_S);
        }
        self.flags = Some(flags);
        if self.ignore_until.is_some_and(|until| now >= until) {
            self.ignore_until = None;
        }
        let ignoring = self.ignore_until.is_some();
        let measured = r.percentage.round().clamp(1.0, 100.0) as u32;
        let percent = match self.percent {
            Some(before) if !r.charging && !ignoring && measured > before => before,
            _ => measured,
        };
        self.percent = Some(percent);
        let level = if !r.on_battery {
            Level::None
        } else if ignoring {
            self.level.unwrap_or(Level::None)
        } else {
            let reached = match percent {
                p if p <= FINAL_PERCENT => Level::Final,
                p if p <= EARLY_PERCENT => Level::Early,
                _ => Level::None,
            };
            reached.max(self.level.unwrap_or(Level::None))
        };
        self.level = Some(level);
        level
    }

    /// The level of the last update.
    pub fn level(&self) -> Level {
        self.level.unwrap_or(Level::None)
    }

    /// When the ignore window closes, so the daemon reads the batteries again then.
    pub fn deadline(&self) -> Option<f64> {
        self.ignore_until
    }
}
