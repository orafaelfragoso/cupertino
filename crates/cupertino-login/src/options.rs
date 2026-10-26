//! The command line of the process.

use std::time::Duration;

pub const USAGE: &str = "usage: cupertino-login [--idle-timeout SECONDS] [--retries-until-hint N] [--power-button-sleeps-system BOOL]\n       cupertino-login --greeter";

/// `RetriesUntilHint` when the option is absent (absent = 0).
const RETRIES_UNTIL_HINT_ABSENT: u32 = 0;

/// `PowerButtonSleepsSystem` when the option is absent (default NO).
const POWER_BUTTON_SLEEPS_SYSTEM_ABSENT: bool = false;

/// What the process runs as.
#[derive(Debug, PartialEq, Eq)]
pub enum Options {
    /// The lock screen and session agent of a user's session.
    Session(SessionOptions),
    /// The login screen of the session manager's greeter session. It takes no options: the accounts, the power items and the
    /// hint setting come from the manager.
    Greeter,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SessionOptions {
    /// Lock after the seat has been idle this long. Absent: no idle lock of this process's own; the server locks after the
    /// display sleeps and the password delay passes, which asks this process through `Lock`.
    pub idle_timeout: Option<Duration>,
    /// Failed attempts until the password help shows with its hint, see [`crate::hint`]; 0 is the setting absent.
    pub retries_until_hint: u32,
    /// `PowerButtonSleepsSystem`: a release of the power key between the debounce and the hold time sleeps the
    /// system. The setting absent is off, the design's default.
    pub power_button_sleeps_system: bool,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut args = args;
        let mut greeter = false;
        let mut idle_timeout = None;
        let mut retries_until_hint = None;
        let mut power_button_sleeps_system = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--greeter" => greeter = true,
                "--idle-timeout" => {
                    let seconds = args.next().ok_or("--idle-timeout needs a number of seconds")?;
                    let seconds: u64 = seconds.parse().map_err(|_| format!("{seconds:?} is not a number of seconds"))?;
                    if seconds == 0 {
                        return Err("--idle-timeout must be at least one second".into());
                    }
                    idle_timeout = Some(Duration::from_secs(seconds));
                }
                "--retries-until-hint" => {
                    let count = args.next().ok_or("--retries-until-hint needs a number of failed attempts")?;
                    retries_until_hint = Some(count.parse::<u32>().map_err(|_| format!("{count:?} is not a number of failed attempts"))?);
                }
                "--power-button-sleeps-system" => power_button_sleeps_system = Some(parse_flag(&arg, args.next())?),
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        if greeter {
            if idle_timeout.is_some() || retries_until_hint.is_some() || power_button_sleeps_system.is_some() {
                return Err("--greeter takes no other option: the session manager supplies the settings of the login screen".into());
            }
            return Ok(Options::Greeter);
        }
        Ok(Options::Session(SessionOptions {
            idle_timeout,
            retries_until_hint: retries_until_hint.unwrap_or(RETRIES_UNTIL_HINT_ABSENT),
            power_button_sleeps_system: power_button_sleeps_system.unwrap_or(POWER_BUTTON_SLEEPS_SYSTEM_ABSENT),
        }))
    }
}

/// The `true` or `false` that follows the option `name`.
fn parse_flag(name: &str, value: Option<String>) -> Result<bool, String> {
    match value.as_deref() {
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!("{name} takes true or false, not {other:?}")),
        None => Err(format!("{name} needs true or false")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        Options::parse(args.iter().map(|a| (*a).to_owned()))
    }

    #[test]
    fn no_arguments_run_the_session_without_an_idle_lock() {
        assert_eq!(parse(&[]), Ok(Options::Session(SessionOptions { idle_timeout: None, retries_until_hint: 0, power_button_sleeps_system: false })));
        assert_eq!(parse(&["--idle-timeout", "30"]), Ok(Options::Session(SessionOptions { idle_timeout: Some(Duration::from_secs(30)), retries_until_hint: 0, power_button_sleeps_system: false })));
        assert!(parse(&["--idle-timeout", "0"]).is_err());
    }

    #[test]
    fn the_hint_threshold_is_a_count() {
        assert_eq!(parse(&["--retries-until-hint", "2"]), Ok(Options::Session(SessionOptions { idle_timeout: None, retries_until_hint: 2, power_button_sleeps_system: false })));
        assert!(parse(&["--retries-until-hint", "-1"]).is_err());
        assert!(parse(&["--retries-until-hint"]).is_err());
    }

    #[test]
    fn power_button_sleeps_system_is_a_flag_and_off_when_absent() {
        assert_eq!(parse(&["--power-button-sleeps-system", "true"]), Ok(Options::Session(SessionOptions { idle_timeout: None, retries_until_hint: 0, power_button_sleeps_system: true })));
        assert_eq!(parse(&["--power-button-sleeps-system", "false"]), parse(&[]));
        assert!(parse(&["--power-button-sleeps-system", "1"]).is_err());
        assert!(parse(&["--power-button-sleeps-system"]).is_err());
    }

    #[test]
    fn the_greeter_takes_no_other_option() {
        assert_eq!(parse(&["--greeter"]), Ok(Options::Greeter));
        for extra in [&["--idle-timeout", "5"][..], &["--retries-until-hint", "3"], &["--power-button-sleeps-system", "true"]] {
            let mut args = vec!["--greeter"];
            args.extend_from_slice(extra);
            assert!(parse(&args).is_err(), "{extra:?}");
        }
        // the settings the manager owns are no options at all
        for gone in ["--user", "--session-command", "--hidden-users", "--restart-disabled"] {
            assert!(parse(&["--greeter", gone, "x"]).is_err(), "{gone}");
        }
    }
}
