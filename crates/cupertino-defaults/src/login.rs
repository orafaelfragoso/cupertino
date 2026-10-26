//! Session manager settings (`login.conf`, `$CUPERTINO_LOGIN_CONF` overrides the path): the login screen's VT and accounts,
//! the commands the manager starts, which accounts and power items the screen offers. One `key = value` per line, `# starts
//! a comment. System configuration, owned by root: unlike the user settings, a missing file or a bad line is an error, and the
//! manager refuses to start on it (no silent fallback).
//!
//! | Key | Value | Default |
//! |---|---|---|
//! | `vt` | the login screen's virtual terminal, 1 to 63 | 1 |
//! | `greeter-user` | the account the login screen runs as | `cupertino-greeter` |
//! | `greeter-command`, `session-command` | program (an absolute path) and arguments, split at spaces, no quoting | required |
//! | `hidden-users`, `deny-list` | account names separated by commas | none |
//! | `allow-list` | account names separated by commas; given and empty lists nobody | absent |
//! | `hide-admin-users`, `show-other-users` | `true` or `false` | false, and absent (the screen's rule) |
//! | `restart-disabled`, `shut-down-disabled`, `sleep-disabled`, `power-off-disabled` | `true` or `false` | absent |
//! | `retries-until-hint` | failed attempts until the password help shows, 0 for the setting absent | 0 |
//! | `auto-login` | the account logged in without a password once per boot (semantics unknown in the design); the manager refuses it unless this file is root-owned and writable by root alone | off |
//!
//! The filter and power keys are the login window preferences of the design (`HiddenUsersList`, `HideAdminUsers`,
//! `AllowList`, `DenyList`, `SHOWOTHERUSERS_MANAGED`, `RestartDisabled`, `ShutDownDisabled`, `SleepDisabled`,
//! `PowerOffDisabled`, `RetriesUntilHint`). The VT, the user and the commands are ours
//! (the design has one console and starts its login window from a launch daemon).

use std::path::PathBuf;

/// Environment variable that redirects the settings file.
pub const LOGIN_CONF_ENV: &str = "CUPERTINO_LOGIN_CONF";

/// Where the settings file is when [`LOGIN_CONF_ENV`] is not set.
pub const LOGIN_CONF_PATH: &str = "/etc/cupertino/login.conf";

/// The login screen's VT when the file does not name one (the first console, where a getty would run).
const DEFAULT_VT: u32 = 1;
/// The account the login screen runs as when the file does not name one (the sysusers entry of the package).
const DEFAULT_GREETER_USER: &str = "cupertino-greeter";
/// The number of virtual terminals Linux has (`MAX_NR_CONSOLES`, `vt(4)).
const MAX_VT: u32 = 63;

/// The settings of `login.conf`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginConfig {
    pub vt: u32,
    pub greeter_user: String,
    /// Program and arguments of the login screen's compositor.
    pub greeter_command: Vec<String>,
    /// Program and arguments of a user's session.
    pub session_command: Vec<String>,
    pub hidden_users: Vec<String>,
    pub hide_admin_users: bool,
    pub allow_list: Option<Vec<String>>,
    pub deny_list: Vec<String>,
    pub show_other_users: Option<bool>,
    pub restart_disabled: Option<bool>,
    pub shut_down_disabled: Option<bool>,
    pub sleep_disabled: Option<bool>,
    pub power_off_disabled: Option<bool>,
    pub retries_until_hint: u32,
    /// The account that is logged in without a password once per boot; `None` (the default) is off. The manager refuses the key
    /// unless the file is root-owned and writable by no one else.
    pub auto_login: Option<String>,
}

/// A line that was not understood, as `(line number, message)`; a missing required key is line 0.
pub type Problem = (usize, String);

impl LoginConfig {
    /// The settings file: [`LOGIN_CONF_ENV`] when set and not empty, else [`LOGIN_CONF_PATH`].
    pub fn path() -> PathBuf {
        std::env::var_os(LOGIN_CONF_ENV).filter(|p| !p.is_empty()).map_or_else(|| PathBuf::from(LOGIN_CONF_PATH), PathBuf::from)
    }

    /// Read and parse the settings file; every problem comes back, in the order of the file.
    pub fn load() -> Result<Self, Vec<Problem>> {
        let path = Self::path();
        let text = std::fs::read_to_string(&path).map_err(|e| vec![(0, format!("{}: {e}", path.display()))])?;
        Self::parse(&text)
    }

    /// Parse the text of a settings file.
    pub fn parse(text: &str) -> Result<Self, Vec<Problem>> {
        let mut config = LoginConfig {
            vt: DEFAULT_VT,
            greeter_user: DEFAULT_GREETER_USER.to_owned(),
            greeter_command: Vec::new(),
            session_command: Vec::new(),
            hidden_users: Vec::new(),
            hide_admin_users: false,
            allow_list: None,
            deny_list: Vec::new(),
            show_other_users: None,
            restart_disabled: None,
            shut_down_disabled: None,
            sleep_disabled: None,
            power_off_disabled: None,
            retries_until_hint: 0,
            auto_login: None,
        };
        let mut problems = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                problems.push((i + 1, format!("expected `key = value`, got {line:?}")));
                continue;
            };
            if let Err(message) = config.apply(key.trim(), value.trim()) {
                problems.push((i + 1, message));
            }
        }
        for (key, command) in [("greeter-command", &config.greeter_command), ("session-command", &config.session_command)] {
            if command.is_empty() {
                problems.push((0, format!("{key} is required")));
            }
        }
        if problems.is_empty() {
            Ok(config)
        } else {
            Err(problems)
        }
    }

    fn apply(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "vt" => {
                let vt: u32 = value.parse().map_err(|_| format!("{key}: expected a number, got {value:?}"))?;
                if !(1..=MAX_VT).contains(&vt) {
                    return Err(format!("{key}: expected 1 to {MAX_VT}, got {vt}"));
                }
                self.vt = vt;
            }
            "greeter-user" => self.greeter_user = account(key, value)?,
            "auto-login" => self.auto_login = Some(account(key, value)?),
            "greeter-command" => self.greeter_command = command(key, value)?,
            "session-command" => self.session_command = command(key, value)?,
            "hidden-users" => self.hidden_users = names(key, value)?,
            "deny-list" => self.deny_list = names(key, value)?,
            "allow-list" => self.allow_list = Some(names(key, value)?),
            "hide-admin-users" => self.hide_admin_users = flag(key, value)?,
            "show-other-users" => self.show_other_users = Some(flag(key, value)?),
            "restart-disabled" => self.restart_disabled = Some(flag(key, value)?),
            "shut-down-disabled" => self.shut_down_disabled = Some(flag(key, value)?),
            "sleep-disabled" => self.sleep_disabled = Some(flag(key, value)?),
            "power-off-disabled" => self.power_off_disabled = Some(flag(key, value)?),
            "retries-until-hint" => self.retries_until_hint = value.parse().map_err(|_| format!("{key}: expected a number of failed attempts, got {value:?}"))?,
            other => return Err(format!("unknown setting {other:?}")),
        }
        Ok(())
    }
}

/// One account name.
fn account(key: &str, value: &str) -> Result<String, String> {
    if value.is_empty() || value.contains(|c: char| c.is_whitespace() || c == ':') {
        return Err(format!("{key}: expected an account name, got {value:?}"));
    }
    Ok(value.to_owned())
}

fn flag(key: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("{key}: expected true or false, got {other:?}")),
    }
}

/// Account names separated by commas; the empty text is the empty list, an empty or spaced entry is an error.
fn names(key: &str, value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = value.split(',').map(str::to_owned).collect();
    if names.iter().any(|name| name.is_empty() || name.contains(char::is_whitespace)) {
        return Err(format!("{key}: expected account names separated by commas, got {value:?}"));
    }
    Ok(names)
}

/// A program and its arguments. The program is an absolute path: the manager runs as root and does not search `PATH`.
fn command(key: &str, value: &str) -> Result<Vec<String>, String> {
    let words: Vec<String> = value.split_whitespace().map(str::to_owned).collect();
    match words.first() {
        Some(program) if program.starts_with('/') => Ok(words),
        _ => Err(format!("{key}: expected an absolute program path and arguments, got {value:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: &str = "greeter-command = /usr/local/bin/greeter --a\nsession-command = /usr/local/bin/session\n";

    #[test]
    fn the_required_commands_alone_give_the_defaults() {
        let config = LoginConfig::parse(REQUIRED).unwrap();
        assert_eq!((config.vt, config.greeter_user.as_str()), (1, "cupertino-greeter"));
        assert_eq!(config.greeter_command, ["/usr/local/bin/greeter", "--a"]);
        assert_eq!((config.allow_list, config.show_other_users, config.hide_admin_users), (None, None, false));
    }

    #[test]
    fn keys_override_the_defaults() {
        let text = format!("{REQUIRED}vt = 5 # login screen\nhidden-users = mallory,eve\nallow-list =\nhide-admin-users = true\nsleep-disabled = false\nretries-until-hint = 3\n");
        let config = LoginConfig::parse(&text).unwrap();
        assert_eq!(config.vt, 5);
        assert_eq!(config.hidden_users, ["mallory", "eve"]);
        assert_eq!(config.allow_list, Some(Vec::new()));
        assert!(config.hide_admin_users);
        assert_eq!((config.sleep_disabled, config.restart_disabled, config.retries_until_hint), (Some(false), None, 3));
    }

    #[test]
    fn auto_login_is_off_unless_the_file_names_one_account() {
        assert_eq!(LoginConfig::parse(REQUIRED).unwrap().auto_login, None);
        assert_eq!(LoginConfig::parse(&format!("{REQUIRED}auto-login = alice\n")).unwrap().auto_login.as_deref(), Some("alice"));
        let problems = LoginConfig::parse(&format!("{REQUIRED}auto-login =\nauto-login = a b\n")).unwrap_err();
        assert_eq!(problems.iter().map(|(line, _)| *line).collect::<Vec<_>>(), [3, 4]);
    }

    #[test]
    fn every_bad_line_is_reported_by_number_and_a_missing_command_is_line_zero() {
        let text = "vt = 0\nvt = 64\nbogus = 1\nhide-admin-users = yes\nhidden-users = a,,b\nsession-command = relative/path\nno equals sign\n";
        let problems = LoginConfig::parse(text).unwrap_err();
        assert_eq!(problems.iter().map(|(line, _)| *line).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6, 7, 0, 0]);
        assert_eq!(problems[7].1, "greeter-command is required");
    }
}
