//! The low-battery notifications of the session agent: which notification to post, replace or withdraw as the power
//! daemon's warning level (`org.freedesktop.UPower.Device.WarningLevel` of the display device) changes.
//!
//! The levels and when they rise are the power daemon's (`cupertino-powerd`, the design's rules ). The
//! reference posts a notification at its early and final levels; its wording (`LOW_BATTERY_BODY`) is not
//! in the control panel's string tables of this payload (searched: every `.loctable` of the control panel app, unknown), so
//! the words below are approximate and our own. Withdrawing the notification when external power clears the level is
//! the compositor's own rule.

/// UPower's `WarningLevel`, as far as notifications care.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    None,
    /// `WarningLevel` 3: the early warning.
    Low,
    /// `WarningLevel` 4 or 5 (critical, action): the final warning.
    Critical,
}

impl Level {
    /// The level of a `WarningLevel` value: 0 unknown, 1 none and 2 (discharging, for UPS only) give no warning.
    pub fn from_upower(value: u32) -> Level {
        match value {
            3 => Level::Low,
            4 | 5 => Level::Critical,
            _ => Level::None,
        }
    }
}

/// A notification to show through `org.freedesktop.Notifications`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub summary: String,
    pub body: String,
    /// Freedesktop icon name.
    pub icon: &'static str,
    /// Urgency hint: 1 normal, 2 critical.
    pub urgency: u8,
}

/// What to do with the notification service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// `Notify`, replacing the notification `replaces` (0 for none).
    Post { replaces: u32, notice: Notice },
    /// `CloseNotification`.
    Close(u32),
}

/// The notification on screen and the level it stands for.
#[derive(Debug, Default)]
pub struct LowBattery {
    level: Option<Level>,
    shown: Option<u32>,
}

/// Name the notifications are posted under.
pub const APP_NAME: &str = "Battery";

impl LowBattery {
    pub fn new() -> Self {
        Self::default()
    }

    /// The daemon reported `level` with the battery at `percent`; returns the call to make, if any. A level above the
    /// one shown posts (replacing the shown one), none withdraws it, anything else changes nothing.
    pub fn report(&mut self, level: Level, percent: f64) -> Option<Action> {
        let before = self.level.replace(level).unwrap_or(Level::None);
        match level {
            Level::None => self.shown.take().map(Action::Close),
            _ if level > before => Some(Action::Post { replaces: self.shown.unwrap_or(0), notice: notice(level, percent) }),
            _ => None,
        }
    }

    /// The service answered a post with `id`.
    pub fn posted(&mut self, id: u32) {
        self.shown = Some(id);
    }
}

fn notice(level: Level, percent: f64) -> Notice {
    let remains = format!("{}% of the battery remains.", percent.round().clamp(0.0, 100.0) as u32);
    match level {
        Level::Critical => Notice { summary: "Battery Almost Empty".into(), body: format!("{remains} Connect the computer to power now."), icon: "battery-empty-symbolic", urgency: 2 },
        Level::Low | Level::None => Notice { summary: "Low Battery".into(), body: format!("{remains} Connect the computer to power soon."), icon: "battery-caution-symbolic", urgency: 1 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upower_levels_3_to_5_warn() {
        assert_eq!([0, 1, 2, 3, 4, 5, 6].map(Level::from_upower), [Level::None, Level::None, Level::None, Level::Low, Level::Critical, Level::Critical, Level::None]);
    }

    #[test]
    fn the_early_warning_posts_the_final_one_replaces_it_and_power_withdraws_it() {
        let mut b = LowBattery::new();
        assert_eq!(b.report(Level::None, 50.0), None);
        let Some(Action::Post { replaces: 0, notice }) = b.report(Level::Low, 9.4) else { panic!("the early warning posts") };
        assert_eq!((notice.summary.as_str(), notice.urgency), ("Low Battery", 1));
        assert!(notice.body.starts_with("9% "), "{}", notice.body);
        b.posted(7);
        assert_eq!(b.report(Level::Low, 8.0), None, "the same level again changes nothing");
        let Some(Action::Post { replaces: 7, notice }) = b.report(Level::Critical, 2.0) else { panic!("the final warning replaces the early one") };
        assert_eq!(notice.urgency, 2);
        b.posted(7);
        assert_eq!(b.report(Level::None, 2.0), Some(Action::Close(7)));
        assert_eq!(b.report(Level::None, 3.0), None);
    }

    #[test]
    fn a_session_that_starts_at_the_final_level_posts_it_once() {
        let mut b = LowBattery::new();
        assert!(matches!(b.report(Level::Critical, 1.0), Some(Action::Post { replaces: 0, .. })));
        b.posted(3);
        assert_eq!(b.report(Level::Low, 1.0), None, "a lower level does not post");
    }
}
