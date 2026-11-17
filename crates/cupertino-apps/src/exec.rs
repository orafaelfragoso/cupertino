//! `Exec` parsing: quoting rules, field-code expansion and the command a launch turns into.
//!
//! Quoting follows the Desktop Entry Specification: arguments are separated by unquoted spaces,
//! double quotes group, and inside quotes `\"`, `` \` ``, `\$` and `\\` are escapes. Field codes
//! are expanded per argument: `%f %F %u %U` take the optional files/URIs of the request (and vanish
//! when there are none), `%i` becomes `--icon <Icon>`, `%c` the name, `%k` the desktop file path,
//! `%%` a percent sign, and deprecated or unknown codes are dropped.

use crate::entry::{DesktopAction, DesktopEntry};
use std::path::PathBuf;

/// Environment variable carrying an xdg-activation token to the launched program.
pub const ACTIVATION_TOKEN_ENV: &str = "XDG_ACTIVATION_TOKEN";
/// The older startup-notification variable some toolkits still read.
pub const STARTUP_ID_ENV: &str = "DESKTOP_STARTUP_ID";

/// What the host has to spawn. Plain data: the compositor decides how to execute it (spawn
/// through its own process management, with its own environment and session setup).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: Option<PathBuf>,
    /// Extra environment variables (activation token, ...).
    pub env: Vec<(String, String)>,
    /// Desktop file id the command came from.
    pub desktop_id: Option<String>,
    /// The entry asked for a terminal (`Terminal=true`); wrap with [`CommandSpec::in_terminal`].
    pub terminal: bool,
}

impl CommandSpec {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    /// The command of an argument list, the program first; `None` for an empty list.
    pub fn from_argv(argv: Vec<String>) -> Option<Self> {
        let mut it = argv.into_iter();
        Some(Self {
            program: it.next()?,
            args: it.collect(),
            ..Default::default()
        })
    }

    pub fn argv(&self) -> Vec<&str> {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect()
    }

    /// Add the activation token variables.
    pub fn with_activation_token(mut self, token: &str) -> Self {
        self.env
            .retain(|(k, _)| k != ACTIVATION_TOKEN_ENV && k != STARTUP_ID_ENV);
        self.env.push((ACTIVATION_TOKEN_ENV.into(), token.into()));
        self.env.push((STARTUP_ID_ENV.into(), token.into()));
        self
    }

    /// Run inside a terminal emulator: `terminal` is the emulator's argv prefix, for example
    /// `["xterm", "-e"]. The prefix is unknown for this system, so it is configuration.
    pub fn in_terminal(mut self, terminal: &[String]) -> Self {
        let Some((prog, rest)) = terminal.split_first() else {
            return self;
        };
        let mut args: Vec<String> = rest.to_vec();
        args.push(std::mem::take(&mut self.program));
        args.append(&mut self.args);
        self.program = prog.clone();
        self.args = args;
        self.terminal = false;
        self
    }

    pub fn to_command(&self) -> std::process::Command {
        let mut c = std::process::Command::new(&self.program);
        c.args(&self.args);
        if let Some(d) = &self.working_dir {
            c.current_dir(d);
        }
        c.envs(self.env.iter().map(|(k, v)| (k, v)));
        c
    }
}

/// Split an `Exec` value into arguments applying the quoting rules. `None` for an unterminated quote.
pub fn tokenize(exec: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut in_quote = false;
    let mut it = exec.chars().peekable();
    while let Some(c) = it.next() {
        match (in_quote, c) {
            (true, '"') => in_quote = false,
            (true, '\\') => match it.peek() {
                Some(&n) if matches!(n, '"' | '`' | '$' | '\\') => {
                    cur.push(n);
                    it.next();
                }
                _ => cur.push('\\'),
            },
            (true, c) => cur.push(c),
            (false, '"') => {
                in_quote = true;
                has = true;
            }
            (false, ' ' | '\t') => {
                if has || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (false, c) => cur.push(c),
        }
    }
    if in_quote {
        return None;
    }
    if has || !cur.is_empty() {
        args.push(cur);
    }
    Some(args)
}

/// Values the field codes expand to.
#[derive(Clone, Debug, Default)]
pub struct ExpandContext<'a> {
    pub name: &'a str,
    pub icon: Option<&'a str>,
    pub desktop_path: Option<&'a str>,
    /// Files or URIs passed to the application (`%f %F %u %U`).
    pub files: &'a [String],
}

/// Expand field codes of every argument. Returns the final argument list.
pub fn expand_args(args: &[String], ctx: &ExpandContext<'_>) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        if matches!(arg.as_str(), "%F" | "%U") {
            out.extend(ctx.files.iter().cloned());
            continue;
        }
        if arg == "%i" {
            if let Some(i) = ctx.icon {
                out.push("--icon".into());
                out.push(i.to_string());
            }
            continue;
        }
        let mut s = String::new();
        let mut had_code = false;
        let mut it = arg.chars();
        while let Some(c) = it.next() {
            if c != '%' {
                s.push(c);
                continue;
            }
            had_code = true;
            match it.next() {
                Some('%') => s.push('%'),
                Some('f' | 'u') => s.push_str(ctx.files.first().map_or("", String::as_str)),
                Some('F' | 'U') => s.push_str(&ctx.files.join(" ")),
                Some('i') => s.push_str(ctx.icon.unwrap_or("")),
                Some('c') => s.push_str(ctx.name),
                Some('k') => s.push_str(ctx.desktop_path.unwrap_or("")),
                Some(_) | None => {}
            }
        }
        if s.is_empty() && had_code {
            continue;
        }
        out.push(s);
    }
    out
}

/// Strip all field codes (no files): the command to run for a plain launch.
pub fn strip_field_codes(exec: &str, ctx: &ExpandContext<'_>) -> Option<Vec<String>> {
    let args = tokenize(exec)?;
    let out = expand_args(&args, ctx);
    (!out.is_empty()).then_some(out)
}

/// The command of an entry's expanded `Exec`; a program built on Chromium gets its overlay scroll bars turned on
/// ([`crate::chromium::launch_args`]).
fn spec_from(argv: Vec<String>, entry: &DesktopEntry) -> CommandSpec {
    let mut it = argv.into_iter();
    let program = it.next().unwrap_or_default();
    let mut args: Vec<String> = it.collect();
    crate::chromium::launch_args(&program, &mut args);
    CommandSpec {
        program,
        args,
        working_dir: entry.path.clone(),
        env: Vec::new(),
        desktop_id: Some(entry.id.clone()),
        terminal: entry.terminal,
    }
}

fn context<'a>(
    entry: &'a DesktopEntry,
    icon: Option<&'a str>,
    files: &'a [String],
    path: Option<&'a str>,
) -> ExpandContext<'a> {
    ExpandContext {
        name: &entry.name,
        icon,
        desktop_path: path,
        files,
    }
}

/// Command for launching `entry` (optionally with files), or `None` for an unusable `Exec`.
pub fn command_for_entry(entry: &DesktopEntry, files: &[String]) -> Option<CommandSpec> {
    let path = entry.source.as_ref().and_then(|p| p.to_str());
    let argv = strip_field_codes(
        &entry.exec,
        &context(entry, entry.icon.as_deref(), files, path),
    )?;
    Some(spec_from(argv, entry))
}

pub fn command_for_action(entry: &DesktopEntry, action: &DesktopAction) -> Option<CommandSpec> {
    let path = entry.source.as_ref().and_then(|p| p.to_str());
    let icon = action.icon.as_deref().or(entry.icon.as_deref());
    let argv = strip_field_codes(&action.exec, &context(entry, icon, &[], path))?;
    Some(spec_from(argv, entry))
}
