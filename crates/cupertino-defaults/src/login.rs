//! Session manager settings (`login.conf`, `$CUPERTINO_LOGIN_CONF` overrides the path): the login screen's VT and accounts,
//! the commands the manager starts, which accounts and power items the screen offers. One `key = value` per line, `# starts
//! a comment. System configuration, owned by root. A missing file, or a missing or bad `greeter-command` or
//! `session-command`, is an error and the manager refuses to start on it. Any other bad line is reported and the manager
//! starts: its key takes its safe value (the last column), so one typo cannot keep the machine without a login screen. A
//! line without `=` is a bad line of the key it starts with. `cupertino-sessiond --check-config` lists every bad line.
//!
//! A command that names a program earlier versions had and this one does not (a locally edited file the package manager
//! kept over an upgrade) runs its replacement, and the manager warns: `/usr/lib/cupertino/greeter` is
//! `/usr/bin/cupertino-server --udev`, `/usr/bin/cupertino-session [args]` is `/usr/bin/cupertino-server --session
//! [args]` ([`Loaded::replaced`]).
//!
//! Names in a list are separated by commas, with spaces around them allowed; a name with a space or `:` inside is bad.
//!
//! | Key | Value | Default | When the value is bad |
//! |---|---|---|---|
//! | `vt` | the login screen's virtual terminal, 1 to 63 | 1 | 1 |
//! | `greeter-user` | the account the login screen runs as | `cupertino-greeter` | `cupertino-greeter` |
//! | `greeter-command`, `session-command` | program (an absolute path) and arguments, split at spaces, no quoting | required | refused |
//! | `hidden-users`, `deny-list` | account names | none | the picker lists nobody and offers "Other…" |
//! | `allow-list` | account names; given and empty lists nobody | absent | the picker lists nobody and offers "Other…" |
//! | `hide-admin-users`, `show-other-users` | `true` or `false` | false, and absent (the screen's rule) | true, and true |
//! | `restart-disabled`, `shut-down-disabled`, `sleep-disabled`, `power-off-disabled` | `true` or `false` | absent | true |
//! | `retries-until-hint` | failed attempts until the password help shows, 0 for the setting absent | 0 | 0 |
//! | `admin-groups` | group names: a member of any of them is an administrator; the manager uses the key only when this file is root-owned and writable by root alone | the build's ([`DEFAULT_ADMIN_GROUPS`]) | no administrator |
//! | `auto-login` | the account logged in without a password once per boot (semantics unknown in the design); the manager refuses it unless this file is root-owned and writable by root alone | off | off |
//! | `lock-message` | a line of text the login and lock screens show near the bottom, at most 200 characters | none | none |
//! | `show-full-name` | `true` or `false`: the login screen starts with a name and a password field instead of the picker | false | false |
//! | `host-info` | `hostname`, `version` or `ip`: a status item of the login screen shows it | none | none |
//! | `show-input-menu` | `true` or `false`: the login and lock screens show the keyboard layout item | false | false |
//! | `hide-user-avatar-and-name` | `true` or `false`: the password screens show no avatar and no name | false | false |
//! | `large-clock` | `true` or `false`: the login and lock screens show the date and the large time | true | true |
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
/// The administrators' groups when the file does not name them: the build's `CUPERTINO_ADMIN_GROUPS` (for a system whose
/// administrators' group is not `wheel`), else `wheel`.
pub const DEFAULT_ADMIN_GROUPS: &str = match option_env!("CUPERTINO_ADMIN_GROUPS") {
    Some(groups) => groups,
    None => "wheel",
};

const _: () = assert!(
    is_name_list(DEFAULT_ADMIN_GROUPS),
    "CUPERTINO_ADMIN_GROUPS must be group names separated by commas"
);

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
    /// The groups whose members are administrators; `None` takes [`DEFAULT_ADMIN_GROUPS`]. The manager refuses the key
    /// unless the file is root-owned and writable by no one else.
    pub admin_groups: Option<Vec<String>>,
    /// What the login and lock screens show.
    pub screens: ScreenSettings,
}

/// The longest lock message, in characters.
pub const MAX_LOCK_MESSAGE_CHARS: usize = 200;

/// What a status item of the login screen says about the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostInfo {
    Hostname,
    Version,
    Ip,
}

/// The settings of `login.conf` the login and lock screens follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenSettings {
    pub lock_message: Option<String>,
    pub show_full_name: bool,
    pub host_info: Option<HostInfo>,
    pub show_input_menu: bool,
    pub hide_user_avatar_and_name: bool,
    pub large_clock: bool,
}

impl Default for ScreenSettings {
    fn default() -> Self {
        ScreenSettings {
            lock_message: None,
            show_full_name: false,
            host_info: None,
            show_input_menu: false,
            hide_user_avatar_and_name: false,
            large_clock: true,
        }
    }
}

impl ScreenSettings {
    /// The screens' settings of a settings file's text, every other key and every bad line left out: the screens read
    /// the file the manager reads, and follow what they understand of it.
    pub fn parse(text: &str) -> ScreenSettings {
        let mut settings = ScreenSettings::default();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if let Some((key, value)) = line.split_once('=') {
                let _ = settings.apply(key.trim(), value.trim());
            }
        }
        settings
    }

    /// The screens' settings of the settings file ([`LoginConfig::path`]); the defaults when it cannot be read.
    pub fn load() -> ScreenSettings {
        std::fs::read_to_string(LoginConfig::path())
            .map(|text| ScreenSettings::parse(&text))
            .unwrap_or_default()
    }

    /// Take `key = value` when it is one of the screens' keys: `None` for another key.
    fn apply(&mut self, key: &str, value: &str) -> Option<Result<(), String>> {
        let applied = match key {
            "lock-message" => lock_message(key, value).map(|m| self.lock_message = m),
            "show-full-name" => flag(key, value).map(|v| self.show_full_name = v),
            "host-info" => host_info(key, value).map(|v| self.host_info = Some(v)),
            "show-input-menu" => flag(key, value).map(|v| self.show_input_menu = v),
            "hide-user-avatar-and-name" => {
                flag(key, value).map(|v| self.hide_user_avatar_and_name = v)
            }
            "large-clock" => flag(key, value).map(|v| self.large_clock = v),
            _ => return None,
        };
        Some(applied)
    }

    /// The safe value of `key`, whose line was bad.
    fn fail_closed(&mut self, key: &str) {
        let safe = ScreenSettings::default();
        match key {
            "lock-message" => self.lock_message = safe.lock_message,
            "show-full-name" => self.show_full_name = safe.show_full_name,
            "host-info" => self.host_info = safe.host_info,
            "show-input-menu" => self.show_input_menu = safe.show_input_menu,
            "hide-user-avatar-and-name" => {
                self.hide_user_avatar_and_name = safe.hide_user_avatar_and_name
            }
            "large-clock" => self.large_clock = safe.large_clock,
            _ => {}
        }
    }
}

/// A lock message: printable text of at most [`MAX_LOCK_MESSAGE_CHARS`] characters; empty is none.
fn lock_message(key: &str, value: &str) -> Result<Option<String>, String> {
    if value.chars().any(char::is_control) {
        return Err(format!("{key}: control characters are not allowed"));
    }
    if value.chars().count() > MAX_LOCK_MESSAGE_CHARS {
        return Err(format!(
            "{key}: at most {MAX_LOCK_MESSAGE_CHARS} characters"
        ));
    }
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn host_info(key: &str, value: &str) -> Result<HostInfo, String> {
    match value {
        "hostname" => Ok(HostInfo::Hostname),
        "version" => Ok(HostInfo::Version),
        "ip" => Ok(HostInfo::Ip),
        _ => Err(format!(
            "{key}: expected hostname, version or ip, got {value:?}"
        )),
    }
}

/// A line that was not understood, as `(line number, message)`; a missing required key is line 0.
pub type Problem = (usize, String);

/// The settings of a file the manager starts with, and its lines that were not understood: each one's key took its safe
/// value.
#[derive(Debug)]
pub struct Loaded {
    pub config: LoginConfig,
    pub problems: Vec<Problem>,
    /// The commands that named a program earlier versions installed and this one does not, each run as its
    /// replacement: a locally edited file the package manager kept over an upgrade (the new one is beside it as
    /// `.pacnew`). The manager starts and warns.
    pub replaced: Vec<Replaced>,
}

/// A command of the file that names a removed program, and what runs in its place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replaced {
    /// `greeter-command` or `session-command`.
    pub key: &'static str,
    /// The removed program.
    pub program: &'static str,
    /// The command that runs instead.
    pub command: Vec<String>,
}

/// The login screen's program of earlier versions, a wrapper that took no arguments: it ran the compositor with
/// `--udev` and the login screen's settings file, which the manager now gives every greeter command.
const REMOVED_GREETER: &str = "/usr/lib/cupertino/greeter";
/// The session program of earlier versions: it passed its arguments on to the compositor, as `--session` does.
const REMOVED_SESSION: &str = "/usr/bin/cupertino-session";
/// The compositor, which runs the login screen (`--udev`) and a user's session (`--session`).
const SERVER: &str = "/usr/bin/cupertino-server";

/// `words` with a removed program of [`REMOVED_GREETER`] or [`REMOVED_SESSION`] replaced; `None` for any other command.
fn replace_removed(words: &[String]) -> Option<(&'static str, Vec<String>)> {
    let (program, args) = words.split_first()?;
    let (removed, mut command) = match program.as_str() {
        REMOVED_GREETER => return Some((REMOVED_GREETER, vec![SERVER.into(), "--udev".into()])),
        REMOVED_SESSION => (
            REMOVED_SESSION,
            vec![SERVER.to_owned(), "--session".to_owned()],
        ),
        _ => return None,
    };
    command.extend(args.iter().cloned());
    Some((removed, command))
}

impl LoginConfig {
    /// The settings file: [`LOGIN_CONF_ENV`] when set and not empty, else [`LOGIN_CONF_PATH`].
    pub fn path() -> PathBuf {
        std::env::var_os(LOGIN_CONF_ENV)
            .filter(|p| !p.is_empty())
            .map_or_else(|| PathBuf::from(LOGIN_CONF_PATH), PathBuf::from)
    }

    /// Read and parse the settings file; every problem comes back, in the order of the file.
    pub fn load() -> Result<Loaded, Vec<Problem>> {
        let path = Self::path();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| vec![(0, format!("{}: {e}", path.display()))])?;
        Self::parse(&text)
    }

    /// Parse the text of a settings file: an error only when a command is missing or bad.
    pub fn parse(text: &str) -> Result<Loaded, Vec<Problem>> {
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
            admin_groups: None,
            screens: ScreenSettings::default(),
        };
        let mut problems = Vec::new();
        let mut bad_keys = Vec::new();
        let mut fatal = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            // a line without `=` is bad for the key it starts with, so `hide-admin-users true` or `hide-admin-users: true`
            // does not leave it open
            let (key, applied) = match line.split_once('=') {
                Some((key, value)) => {
                    let key = key.trim();
                    (key, config.apply(key, value.trim()))
                }
                None => (
                    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                        .next()
                        .unwrap_or(line),
                    Err(format!("expected `key = value`, got {line:?}")),
                ),
            };
            if let Err(message) = applied {
                if matches!(key, "greeter-command" | "session-command") {
                    fatal.push((i + 1, message));
                } else {
                    problems.push((i + 1, message));
                }
                bad_keys.push(key.to_owned());
            }
        }
        // after every line, so a good line of the same key further down does not undo it
        for key in &bad_keys {
            config.fail_closed(key);
        }
        for (key, command) in [
            ("greeter-command", &config.greeter_command),
            ("session-command", &config.session_command),
        ] {
            if command.is_empty() && !bad_keys.iter().any(|bad| bad == key) {
                fatal.push((0, format!("{key} is required")));
            }
        }
        let mut replaced = Vec::new();
        for (key, command) in [
            ("greeter-command", &mut config.greeter_command),
            ("session-command", &mut config.session_command),
        ] {
            if let Some((program, new)) = replace_removed(command) {
                *command = new.clone();
                replaced.push(Replaced {
                    key,
                    program,
                    command: new,
                });
            }
        }
        if fatal.is_empty() {
            Ok(Loaded {
                config,
                problems,
                replaced,
            })
        } else {
            Err(fatal)
        }
    }

    /// The safe value of `key`, whose line was bad.
    fn fail_closed(&mut self, key: &str) {
        match key {
            "vt" => self.vt = DEFAULT_VT,
            "greeter-user" => self.greeter_user = DEFAULT_GREETER_USER.to_owned(),
            "auto-login" => self.auto_login = None,
            "admin-groups" => self.admin_groups = Some(Vec::new()),
            "hidden-users" | "deny-list" | "allow-list" => {
                self.allow_list = Some(Vec::new());
                self.show_other_users = Some(true);
            }
            "hide-admin-users" => self.hide_admin_users = true,
            "show-other-users" => self.show_other_users = Some(true),
            "restart-disabled" => self.restart_disabled = Some(true),
            "shut-down-disabled" => self.shut_down_disabled = Some(true),
            "sleep-disabled" => self.sleep_disabled = Some(true),
            "power-off-disabled" => self.power_off_disabled = Some(true),
            "retries-until-hint" => self.retries_until_hint = 0,
            other => self.screens.fail_closed(other),
        }
    }

    fn apply(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "vt" => {
                let vt: u32 = value
                    .parse()
                    .map_err(|_| format!("{key}: expected a number, got {value:?}"))?;
                if !(1..=MAX_VT).contains(&vt) {
                    return Err(format!("{key}: expected 1 to {MAX_VT}, got {vt}"));
                }
                self.vt = vt;
            }
            "greeter-user" => self.greeter_user = account(key, value)?,
            "auto-login" => self.auto_login = Some(account(key, value)?),
            "admin-groups" => self.admin_groups = Some(names(key, value)?),
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
            "retries-until-hint" => {
                self.retries_until_hint = value.parse().map_err(|_| {
                    format!("{key}: expected a number of failed attempts, got {value:?}")
                })?
            }
            other => {
                return self
                    .screens
                    .apply(other, value)
                    .unwrap_or_else(|| Err(format!("unknown setting {other:?}")))
            }
        }
        Ok(())
    }
}

/// [`DEFAULT_ADMIN_GROUPS`] as a list.
pub fn default_admin_groups() -> Vec<String> {
    DEFAULT_ADMIN_GROUPS.split(',').map(str::to_owned).collect()
}

/// Whether `list` is names separated by commas, each one not empty, without spaces or `:`.
const fn is_name_list(list: &str) -> bool {
    let bytes = list.as_bytes();
    let mut i = 0;
    let mut entry = 0;
    while i < bytes.len() {
        match bytes[i] {
            b',' if entry == 0 => return false,
            b',' => entry = 0,
            b' ' | b'\t' | b'\n' | b':' => return false,
            _ => entry += 1,
        }
        i += 1;
    }
    entry > 0
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

/// Account or group names separated by commas, with spaces around them allowed; the empty text is the empty list, an
/// empty entry or one with a space or `:` inside is an error.
fn names(key: &str, value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let names: Vec<String> = value
        .split(',')
        .map(|name| name.trim().to_owned())
        .collect();
    if names
        .iter()
        .any(|name| name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == ':'))
    {
        return Err(format!(
            "{key}: expected account names separated by commas, got {value:?}"
        ));
    }
    Ok(names)
}

/// A program and its arguments. The program is an absolute path: the manager runs as root and does not search `PATH`.
fn command(key: &str, value: &str) -> Result<Vec<String>, String> {
    let words: Vec<String> = value.split_whitespace().map(str::to_owned).collect();
    match words.first() {
        Some(program) if program.starts_with('/') => Ok(words),
        _ => Err(format!(
            "{key}: expected an absolute program path and arguments, got {value:?}"
        )),
    }
}
