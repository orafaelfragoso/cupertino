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

fn spec_from(argv: Vec<String>, entry: &DesktopEntry) -> CommandSpec {
    let mut it = argv.into_iter();
    CommandSpec {
        program: it.next().unwrap_or_default(),
        args: it.collect(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(files: &'a [String]) -> ExpandContext<'a> {
        ExpandContext {
            name: "My App",
            icon: Some("my-icon"),
            desktop_path: Some("/usr/share/applications/my.desktop"),
            files,
        }
    }

    fn strip(s: &str) -> Vec<String> {
        strip_field_codes(s, &ctx(&[])).unwrap()
    }

    #[test]
    fn tokenizer_handles_quotes_and_escapes() {
        assert_eq!(tokenize("prog a  b").unwrap(), ["prog", "a", "b"]);
        assert_eq!(tokenize("prog \"a b\" c").unwrap(), ["prog", "a b", "c"]);
        assert_eq!(
            tokenize("prog \"a \\\"q\\\" \\$HOME \\\\\"").unwrap(),
            ["prog", "a \"q\" $HOME \\"]
        );
        assert_eq!(tokenize("prog \"\" x").unwrap(), ["prog", "", "x"]);
        assert_eq!(tokenize("prog \"unterminated"), None);
        assert_eq!(tokenize("   ").unwrap(), Vec::<String>::new());
        assert_eq!(tokenize("a\\b").unwrap(), ["a\\b"]);
    }

    #[test]
    fn field_codes_are_stripped() {
        assert_eq!(strip("browser %u"), ["browser"]);
        assert_eq!(strip("editor %F"), ["editor"]);
        assert_eq!(strip("viewer %f %U %d %D %n %N %v %m"), ["viewer"]);
        assert_eq!(strip("app --file=%f"), ["app", "--file="]);
        assert_eq!(strip("app 100%%"), ["app", "100%"]);
        assert_eq!(
            strip("app %i %c %k"),
            [
                "app",
                "--icon",
                "my-icon",
                "My App",
                "/usr/share/applications/my.desktop"
            ]
        );
        assert_eq!(strip("app %z extra"), ["app", "extra"]);
    }

    #[test]
    fn files_expand_into_file_codes() {
        let f = vec!["/tmp/a.txt".to_string(), "/tmp/b c.txt".to_string()];
        assert_eq!(
            expand_args(&tokenize("edit %F").unwrap(), &ctx(&f)),
            ["edit", "/tmp/a.txt", "/tmp/b c.txt"]
        );
        assert_eq!(
            expand_args(&tokenize("edit %f").unwrap(), &ctx(&f)),
            ["edit", "/tmp/a.txt"]
        );
        assert_eq!(
            expand_args(&tokenize("edit --open=%u").unwrap(), &ctx(&f)),
            ["edit", "--open=/tmp/a.txt"]
        );
    }

    #[test]
    fn missing_icon_drops_icon_code() {
        let c = ExpandContext {
            name: "x",
            icon: None,
            desktop_path: None,
            files: &[],
        };
        assert_eq!(strip_field_codes("app %i", &c).unwrap(), ["app"]);
    }

    #[test]
    fn empty_exec_is_none() {
        assert_eq!(strip_field_codes("%U", &ctx(&[])), None);
        assert_eq!(strip_field_codes("", &ctx(&[])), None);
    }

    fn entry(exec: &str) -> DesktopEntry {
        DesktopEntry {
            id: "my.desktop".into(),
            name: "My App".into(),
            generic_name: String::new(),
            comment: String::new(),
            keywords: vec![],
            categories: vec![],
            icon: Some("my-icon".into()),
            exec: exec.into(),
            try_exec: None,
            terminal: true,
            path: Some("/work".into()),
            startup_wm_class: None,
            actions: vec![],
            source: None,
        }
    }

    #[test]
    fn command_for_entry_carries_metadata() {
        let c = command_for_entry(&entry("my-app --flag %U"), &[]).unwrap();
        assert_eq!(c.program, "my-app");
        assert_eq!(c.args, ["--flag"]);
        assert_eq!(c.working_dir, Some("/work".into()));
        assert_eq!(c.desktop_id.as_deref(), Some("my.desktop"));
        assert!(c.terminal);
        assert!(command_for_entry(&entry("%U"), &[]).is_none());
    }

    #[test]
    fn activation_token_sets_both_variables_once() {
        let c = CommandSpec::new("x", &[])
            .with_activation_token("t1")
            .with_activation_token("t2");
        assert_eq!(
            c.env,
            vec![
                (ACTIVATION_TOKEN_ENV.to_string(), "t2".to_string()),
                (STARTUP_ID_ENV.to_string(), "t2".to_string())
            ]
        );
    }

    #[test]
    fn terminal_wrapping() {
        let c = CommandSpec::new("htop", &["-d", "5"]).in_terminal(&["xterm".into(), "-e".into()]);
        assert_eq!(c.argv(), ["xterm", "-e", "htop", "-d", "5"]);
        let unchanged = CommandSpec::new("htop", &[]).in_terminal(&[]);
        assert_eq!(unchanged.argv(), ["htop"]);
    }

    #[test]
    fn action_command_uses_action_exec_and_icon() {
        let mut e = entry("app");
        e.terminal = false;
        let a = DesktopAction {
            id: "new".into(),
            name: "New".into(),
            exec: "app --new %i".into(),
            icon: Some("new-icon".into()),
        };
        let c = command_for_action(&e, &a).unwrap();
        assert_eq!(c.argv(), ["app", "--new", "--icon", "new-icon"]);
    }
}
