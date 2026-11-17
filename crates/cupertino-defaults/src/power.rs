//! Power settings (`power.conf`, `$CUPERTINO_POWER_CONF` overrides the path): when the display dims and sleeps, when the
//! system sleeps, and how soon a sleeping display asks for the password. One `key = value` per line, `# starts a comment.
//!
//! | Key | Value | Default |
//! |---|---|---|
//! | `display_sleep_ac`, `display_sleep_battery` | seconds of no input, or `never` | 600 and 120 |
//! | `system_sleep_ac`, `system_sleep_battery` | seconds of no input, or `never` | `never` and 600 |
//! | `dim_before_display_sleep` | `on` or `off` | on |
//! | `require_password_after` | seconds after the display sleeps, `0` for at once, or `never` | 300 |
//! | `lock_screen_display_sleep` | seconds of no input on the lock screen | 30 |
//!
//! The design stores the timers in minutes and its settings pane offers 1 to 180 minutes; seconds here let a test use
//! short timers. Defaults: display sleep 10 min on mains and 2 min on battery, dim on (the built-in power table, unchanged
//! per model); system sleep never on mains and 10 min on battery, because a build, a download or a remote session takes
//! no sleep inhibitor and nothing else says the machine is busy; the password 300 s after the display sleeps
//! (the new-user template's); the lock screen's own display timeout 30 s.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::conf::{self, parse_switch, switch_text, Key, Problem};

/// Environment variable that redirects the settings file.
pub const POWER_CONF_ENV: &str = "CUPERTINO_POWER_CONF";

/// A timer that may be switched off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timer {
    After(Duration),
    Never,
}

impl Timer {
    pub fn duration(self) -> Option<Duration> {
        match self {
            Timer::After(d) => Some(d),
            Timer::Never => None,
        }
    }
}

impl fmt::Display for Timer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Timer::After(d) => write!(f, "{} s", d.as_secs()),
            Timer::Never => write!(f, "never"),
        }
    }
}

/// The timers of one power source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceTimers {
    pub display_sleep: Timer,
    pub system_sleep: Timer,
}

/// Everything in `power.conf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PowerSettings {
    pub ac: SourceTimers,
    pub battery: SourceTimers,
    /// Dim the display before it sleeps ("Display Sleep Uses Dim").
    pub dim_before_display_sleep: bool,
    /// How long after the display goes to sleep the session locks.
    pub require_password_after: Timer,
    /// Display sleep while the session is locked.
    pub lock_screen_display_sleep: Duration,
}

impl Default for PowerSettings {
    fn default() -> Self {
        let minutes = |m: u64| Timer::After(Duration::from_secs(m * 60));
        Self {
            ac: SourceTimers {
                display_sleep: minutes(10),
                system_sleep: Timer::Never,
            },
            battery: SourceTimers {
                display_sleep: minutes(2),
                system_sleep: minutes(10),
            },
            dim_before_display_sleep: true,
            require_password_after: Timer::After(Duration::from_secs(300)),
            lock_screen_display_sleep: Duration::from_secs(30),
        }
    }
}

impl PowerSettings {
    /// The timers of the mains or of the battery.
    pub fn timers(&self, on_battery: bool) -> SourceTimers {
        if on_battery {
            self.battery
        } else {
            self.ac
        }
    }

    /// Where the file is: `$CUPERTINO_POWER_CONF`, else `power.conf` in the settings directory.
    pub fn path() -> Option<PathBuf> {
        crate::config_file(POWER_CONF_ENV, "power.conf")
    }

    /// The defaults with the lines of `text` applied, and the problems of its lines; with `last_good`, a key whose line
    /// is wrong keeps its last good value.
    pub fn parse(text: &str, last_good: Option<&Self>) -> (Self, Vec<Problem>) {
        conf::read(
            &conf::ConfText::parse(text),
            KEYS,
            &Self::default(),
            last_good,
            None,
        )
    }

    /// [`Self::parse`] of the file at `path`; a missing file gives the defaults, one that cannot be read is an error.
    pub fn load(path: &Path, last_good: Option<&Self>) -> io::Result<(Self, Vec<Problem>)> {
        conf::load(path, KEYS, &Self::default(), last_good, None)
    }

    /// Write the keys whose value differs from the file's into `path` atomically; comments and other lines stay.
    pub fn store(&self, path: &Path) -> io::Result<Vec<Problem>> {
        conf::store(path, KEYS, &Self::default(), None, self)
    }
}

fn seconds(value: &str) -> Result<Duration, String> {
    value
        .parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|_| format!("expected seconds, got {value:?}"))
}

/// A sleep timer: seconds (at least 1) or `never`.
fn parse_timer(value: &str) -> Result<Timer, String> {
    match value {
        "never" => Ok(Timer::Never),
        v => {
            let d = seconds(v)?;
            if d.is_zero() {
                return Err("expected at least 1 second or never, got 0".to_string());
            }
            Ok(Timer::After(d))
        }
    }
}

fn timer_text(timer: Timer) -> String {
    match timer {
        Timer::After(d) => d.as_secs().to_string(),
        Timer::Never => "never".to_string(),
    }
}

const KEYS: &[Key<PowerSettings>] = &[
    Key {
        name: "display_sleep_ac",
        read: |s, v| parse_timer(v).map(|t| s.ac.display_sleep = t),
        write: |s| Some(timer_text(s.ac.display_sleep)),
        keep: |s, g| s.ac.display_sleep = g.ac.display_sleep,
    },
    Key {
        name: "display_sleep_battery",
        read: |s, v| parse_timer(v).map(|t| s.battery.display_sleep = t),
        write: |s| Some(timer_text(s.battery.display_sleep)),
        keep: |s, g| s.battery.display_sleep = g.battery.display_sleep,
    },
    Key {
        name: "system_sleep_ac",
        read: |s, v| parse_timer(v).map(|t| s.ac.system_sleep = t),
        write: |s| Some(timer_text(s.ac.system_sleep)),
        keep: |s, g| s.ac.system_sleep = g.ac.system_sleep,
    },
    Key {
        name: "system_sleep_battery",
        read: |s, v| parse_timer(v).map(|t| s.battery.system_sleep = t),
        write: |s| Some(timer_text(s.battery.system_sleep)),
        keep: |s, g| s.battery.system_sleep = g.battery.system_sleep,
    },
    Key {
        name: "dim_before_display_sleep",
        read: |s, v| parse_switch(v).map(|b| s.dim_before_display_sleep = b),
        write: |s| Some(switch_text(s.dim_before_display_sleep)),
        keep: |s, g| s.dim_before_display_sleep = g.dim_before_display_sleep,
    },
    // 0 is allowed here: the session locks as the display goes to sleep
    Key {
        name: "require_password_after",
        read: |s, v| {
            s.require_password_after = if v == "never" {
                Timer::Never
            } else {
                Timer::After(seconds(v)?)
            };
            Ok(())
        },
        write: |s| Some(timer_text(s.require_password_after)),
        keep: |s, g| s.require_password_after = g.require_password_after,
    },
    Key {
        name: "lock_screen_display_sleep",
        read: |s, v| {
            let d = seconds(v)?;
            if d.is_zero() {
                return Err("expected at least 1 second, got 0".to_string());
            }
            s.lock_screen_display_sleep = d;
            Ok(())
        },
        write: |s| Some(s.lock_screen_display_sleep.as_secs().to_string()),
        keep: |s, g| s.lock_screen_display_sleep = g.lock_screen_display_sleep,
    },
];
