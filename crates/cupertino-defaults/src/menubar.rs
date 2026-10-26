//! `menubar.conf` (`$CUPERTINO_MENUBAR_CONFIG` overrides the path): the menu bar clock. A key the file does not set
//! reads as the design reads an unset key ([`ClockSettings::UNSET`], read rules).
//!
//! | Key | Values | Unset |
//! |---|---|---|
//! | `clock.show_seconds` | `on`, `off` | `off` |
//! | `clock.show_day_of_week` | `on`, `off` | `off` |
//! | `clock.show_date` | `when-space-allows`, `always`, `never` | `never` |
//! | `clock.show_am_pm` | `on`, `off` (12-hour cycle only) | `on` |
//! | `clock.flash_separators` | `on`, `off` | `off` |
//! | `clock.hour_cycle` | `12`, `24` | `12` |

use std::io;
use std::path::{Path, PathBuf};

use crate::conf::{self, parse_switch, switch_text, Key, Problem};

/// Environment variable that redirects the settings file.
pub const MENUBAR_CONF_ENV: &str = "CUPERTINO_MENUBAR_CONFIG";

/// When the day and short month show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowDate {
    /// `0`: when there is room. The bar does not shrink the clock, so this shows the date (approximate: the design
    /// falls back to the compact formatter when the bar is full; that rule is open).
    WhenSpaceAllows,
    /// `1`.
    Always,
    /// `2`.
    Never,
}

/// The hour cycle of the time part. The design takes it from the locale (`j` in the skeleton); the prototype has no
/// locale data, so it is a setting (approximate; default 12, the English locale whose join strings are recorded).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HourCycle {
    H12,
    H24,
}

/// The clock options (the design clock preference domain, `clock.* keys here).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockSettings {
    pub show_seconds: bool,
    pub show_day_of_week: bool,
    pub show_date: ShowDate,
    /// Honoured in the 12-hour cycle only.
    pub show_am_pm: bool,
    pub flash_separators: bool,
    pub hour_cycle: HourCycle,
}

impl ClockSettings {
    /// What an empty domain reads as (read rules): time only, with the day period.
    pub const UNSET: Self = Self { show_seconds: false, show_day_of_week: false, show_date: ShowDate::Never, show_am_pm: true, flash_separators: false, hour_cycle: HourCycle::H12 };
}

/// Everything in `menubar.conf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuBarSettings {
    pub clock: ClockSettings,
}

const KEYS: &[Key<MenuBarSettings>] = &[
    Key { name: "clock.show_seconds", read: |s, v| parse_switch(v).map(|on| s.clock.show_seconds = on), write: |s| Some(switch_text(s.clock.show_seconds)), keep: |s, g| s.clock.show_seconds = g.clock.show_seconds },
    Key { name: "clock.show_day_of_week", read: |s, v| parse_switch(v).map(|on| s.clock.show_day_of_week = on), write: |s| Some(switch_text(s.clock.show_day_of_week)), keep: |s, g| s.clock.show_day_of_week = g.clock.show_day_of_week },
    Key {
        name: "clock.show_date",
        read: |s, v| {
            s.clock.show_date = match v {
                "when-space-allows" => ShowDate::WhenSpaceAllows,
                "always" => ShowDate::Always,
                "never" => ShowDate::Never,
                other => return Err(format!("must be when-space-allows, always or never, not {other:?}")),
            };
            Ok(())
        },
        write: |s| Some(match s.clock.show_date {
            ShowDate::WhenSpaceAllows => "when-space-allows",
            ShowDate::Always => "always",
            ShowDate::Never => "never",
        }
        .to_string()),
        keep: |s, g| s.clock.show_date = g.clock.show_date,
    },
    Key { name: "clock.show_am_pm", read: |s, v| parse_switch(v).map(|on| s.clock.show_am_pm = on), write: |s| Some(switch_text(s.clock.show_am_pm)), keep: |s, g| s.clock.show_am_pm = g.clock.show_am_pm },
    Key { name: "clock.flash_separators", read: |s, v| parse_switch(v).map(|on| s.clock.flash_separators = on), write: |s| Some(switch_text(s.clock.flash_separators)), keep: |s, g| s.clock.flash_separators = g.clock.flash_separators },
    Key {
        name: "clock.hour_cycle",
        read: |s, v| {
            s.clock.hour_cycle = match v {
                "12" => HourCycle::H12,
                "24" => HourCycle::H24,
                other => return Err(format!("must be 12 or 24, not {other:?}")),
            };
            Ok(())
        },
        write: |s| Some(match s.clock.hour_cycle {
            HourCycle::H12 => "12",
            HourCycle::H24 => "24",
        }
        .to_string()),
        keep: |s, g| s.clock.hour_cycle = g.clock.hour_cycle,
    },
];

impl MenuBarSettings {
    pub const UNSET: Self = Self { clock: ClockSettings::UNSET };

    /// Where the file is: `$CUPERTINO_MENUBAR_CONFIG`, else `menubar.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(MENUBAR_CONF_ENV, "menubar.conf")
    }

    /// The settings of `text` and the problems of its lines; with `last_good`, a key whose line is wrong keeps its
    /// last good value.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        conf::read(&conf::ConfText::parse(text), KEYS, &Self::UNSET, last_good, None)
    }

    /// [`Self::parse`] of the file at `path`; a missing file is every key unset.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<Problem>)> {
        conf::load(path, KEYS, &Self::UNSET, last_good, None)
    }

    /// Write the keys whose value differs from the file's into `path` atomically; comments and other lines stay.
    pub fn store(&self, path: &Path) -> io::Result<Vec<Problem>> {
        conf::store(path, KEYS, &Self::UNSET, None, self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_keys_are_read_and_bad_lines_are_reported_without_losing_the_rest() {
        let text = "# clock\nclock.show_seconds = on\nclock.show_date = always # with the month\nclock.hour_cycle = 25\nclock.zoom = 3\nnonsense\nclock.hour_cycle=24\n";
        let (file, problems) = MenuBarSettings::parse(text, None);
        assert!(file.clock.show_seconds);
        assert_eq!(file.clock.show_date, ShowDate::Always);
        assert_eq!(file.clock.hour_cycle, HourCycle::H24);
        assert_eq!(problems.iter().map(|p| p.line).collect::<Vec<_>>(), vec![4, 5, 6]);
        assert_eq!(MenuBarSettings::parse("", None).0, MenuBarSettings::UNSET);
    }

    #[test]
    fn a_wrong_value_keeps_the_last_good_one() {
        let good = MenuBarSettings { clock: ClockSettings { hour_cycle: HourCycle::H24, ..ClockSettings::UNSET } };
        let (s, problems) = MenuBarSettings::parse("clock.hour_cycle = 25\n", Some(&good));
        assert_eq!(s.clock.hour_cycle, HourCycle::H24);
        assert_eq!(problems[0].message, "clock.hour_cycle: must be 12 or 24, not \"25\"");
    }

    #[test]
    fn a_store_round_trips_and_keeps_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("menubar.conf");
        std::fs::write(&path, "# my clock\nclock.show_seconds = on # tick\n").unwrap();
        let mut s = MenuBarSettings::load(&path, None).unwrap().0;
        s.clock.show_date = ShowDate::WhenSpaceAllows;
        s.clock.show_seconds = false;
        assert!(s.store(&path).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# my clock\nclock.show_seconds = off # tick\nclock.show_date = when-space-allows\n");
        assert_eq!(MenuBarSettings::load(&path, None).unwrap(), (s, vec![]));
    }
}
