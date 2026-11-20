//! The command line of the process.

use std::time::Duration;

pub const USAGE: &str = "usage: cupertino-login [--idle-timeout SECONDS] [--retries-until-hint N]\n       cupertino-login --greeter";

/// `RetriesUntilHint` when the option is absent (absent = 0).
const RETRIES_UNTIL_HINT_ABSENT: u32 = 0;

/// What the process runs as.
#[derive(Debug, PartialEq, Eq)]
pub enum Options {
    /// The lock screen of a user's session.
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
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut args = args;
        let mut greeter = false;
        let mut idle_timeout = None;
        let mut retries_until_hint = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--greeter" => greeter = true,
                "--idle-timeout" => {
                    let seconds = args
                        .next()
                        .ok_or("--idle-timeout needs a number of seconds")?;
                    let seconds: u64 = seconds
                        .parse()
                        .map_err(|_| format!("{seconds:?} is not a number of seconds"))?;
                    if seconds == 0 {
                        return Err("--idle-timeout must be at least one second".into());
                    }
                    idle_timeout = Some(Duration::from_secs(seconds));
                }
                "--retries-until-hint" => {
                    let count = args
                        .next()
                        .ok_or("--retries-until-hint needs a number of failed attempts")?;
                    retries_until_hint =
                        Some(count.parse::<u32>().map_err(|_| {
                            format!("{count:?} is not a number of failed attempts")
                        })?);
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        if greeter {
            if idle_timeout.is_some() || retries_until_hint.is_some() {
                return Err("--greeter takes no other option: the session manager supplies the settings of the login screen".into());
            }
            return Ok(Options::Greeter);
        }
        Ok(Options::Session(SessionOptions {
            idle_timeout,
            retries_until_hint: retries_until_hint.unwrap_or(RETRIES_UNTIL_HINT_ABSENT),
        }))
    }
}
