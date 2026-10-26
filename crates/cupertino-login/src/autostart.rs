//! Login items: the XDG autostart entries (Desktop Application Autostart Specification 0.5) the session agent starts once
//! per session. Pure apart from [`list`], which reads the directories, and [`Environment::from_process`].
//!
//! * Directories: `$XDG_CONFIG_HOME/autostart` (default `~/.config/autostart`), then `<dir>/autostart` for each entry of
//!   `$XDG_CONFIG_DIRS` (default `/etc/xdg`). An entry in a more important directory replaces the one with the same file
//!   name in a less important one, so a user's copy with `Hidden=true` switches a system entry off.
//! * Skipped: `Type` other than `Application`, `Hidden=true`, `OnlyShowIn` without a desktop of `$XDG_CURRENT_DESKTOP`,
//!   `NotShowIn` with one, `X-GNOME-Autostart-enabled=false`, a `TryExec` that is not an executable, a missing or
//!   malformed `Exec`, and `Terminal=true` (the session agent has no terminal to start it in).
//! * Order: the entries that come from the system directories, then the user's own, each by file name. The design
//!   starts the global login items before the user's, one at a time; the XDG directories are its
//!   counterpart. Its separate list of delayed items (10 s) has no XDG counterpart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Why an entry is not started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    NotApplication,
    Hidden,
    OnlyShowIn,
    NotShowIn,
    /// `X-GNOME-Autostart-enabled=false`.
    Disabled,
    TryExec(String),
    Terminal,
    /// No `Exec`, or one that does not split into a command.
    Exec(String),
}

/// An entry to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The file name, which identifies the entry across directories.
    pub id: String,
    /// Program and arguments, field codes expanded.
    pub argv: Vec<String>,
    /// Working directory (`Path`).
    pub dir: Option<PathBuf>,
}

/// What decides whether an entry runs.
pub struct Environment {
    /// `$XDG_CURRENT_DESKTOP`, split on `:`.
    pub desktops: Vec<String>,
    /// Whether a `TryExec` value names an executable.
    pub executable: Box<dyn Fn(&str) -> bool>,
}

impl Environment {
    /// The current desktops of this process and `TryExec` looked up on `$PATH`.
    pub fn from_process() -> Environment {
        let desktops = std::env::var("XDG_CURRENT_DESKTOP")
            .map(|v| {
                v.split(':')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        Environment {
            desktops,
            executable: Box::new(move |name| {
                if name.contains('/') {
                    is_executable(Path::new(name))
                } else {
                    path.iter().any(|d| is_executable(&d.join(name)))
                }
            }),
        }
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The autostart directories, most important first.
pub fn directories(config_home: Option<PathBuf>, config_dirs: Option<String>) -> Vec<PathBuf> {
    let system = config_dirs
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/etc/xdg".into());
    config_home
        .into_iter()
        .chain(
            system
                .split(':')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
        )
        .map(|d| d.join("autostart"))
        .collect()
}

/// One file of an autostart directory: its id, path and text, and whether it is the user's own (the first directory).
pub struct Found {
    pub id: String,
    pub path: PathBuf,
    pub text: String,
    pub user: bool,
}

/// The `.desktop` files of `dirs` (most important first) with the shadowed ones removed. A missing directory has no
/// entries; a file or directory that cannot be read is an error.
pub fn list(dirs: &[PathBuf]) -> std::io::Result<Vec<Found>> {
    let mut found: HashMap<String, Found> = HashMap::new();
    for (index, dir) in dirs.iter().enumerate() {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(std::io::Error::new(
                    e.kind(),
                    format!("{}: {e}", dir.display()),
                ))
            }
        };
        for entry in entries {
            let path = entry?.path();
            let Some(id) = path
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| n.ends_with(".desktop"))
                .map(String::from)
            else {
                continue;
            };
            if found.contains_key(&id) {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .map_err(|e| std::io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
            found.insert(
                id.clone(),
                Found {
                    id,
                    path,
                    text,
                    user: index == 0,
                },
            );
        }
    }
    let mut out: Vec<Found> = found.into_values().collect();
    out.sort_by(|a, b| (a.user, &a.id).cmp(&(b.user, &b.id)));
    Ok(out)
}

/// Keys of the `[Desktop Entry]` group, raw values; the first occurrence of a key wins.
fn main_group(text: &str) -> HashMap<String, String> {
    let mut in_main = false;
    let mut values = HashMap::new();
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_main = name == "Desktop Entry";
        } else if let (true, Some((k, v))) = (in_main, line.split_once('=')) {
            values
                .entry(k.trim().to_string())
                .or_insert_with(|| v.trim().to_string());
        }
    }
    values
}

/// The string escapes of the specification: `\s`, `\n`, `\t`, `\r`, `\\`; `\;` is kept for list splitting.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A list value split on unescaped `;`.
fn list_value(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.as_str().starts_with(';') => {
                chars.next();
                current.push(';');
            }
            ';' => out.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    out.push(current);
    out.into_iter()
        .map(|v| unescape(&v))
        .filter(|v| !v.is_empty())
        .collect()
}

fn boolean(values: &HashMap<String, String>, key: &str) -> Option<bool> {
    values.get(key).map(|v| v == "true")
}

/// Decide whether the entry `id` with `text` (found at `path`) starts, and with which command.
pub fn parse(id: &str, path: &Path, text: &str, env: &Environment) -> Result<Item, Skip> {
    let values = main_group(text);
    if values.get("Type").map(String::as_str) != Some("Application") {
        return Err(Skip::NotApplication);
    }
    if boolean(&values, "Hidden") == Some(true) {
        return Err(Skip::Hidden);
    }
    let only = values
        .get("OnlyShowIn")
        .map(|v| list_value(v))
        .unwrap_or_default();
    if !only.is_empty() && !only.iter().any(|d| env.desktops.contains(d)) {
        return Err(Skip::OnlyShowIn);
    }
    if values
        .get("NotShowIn")
        .map(|v| list_value(v))
        .unwrap_or_default()
        .iter()
        .any(|d| env.desktops.contains(d))
    {
        return Err(Skip::NotShowIn);
    }
    if boolean(&values, "X-GNOME-Autostart-enabled") == Some(false) {
        return Err(Skip::Disabled);
    }
    if let Some(try_exec) = values
        .get("TryExec")
        .map(|v| unescape(v))
        .filter(|v| !v.is_empty())
    {
        if !(env.executable)(&try_exec) {
            return Err(Skip::TryExec(try_exec));
        }
    }
    if boolean(&values, "Terminal") == Some(true) {
        return Err(Skip::Terminal);
    }
    let exec = values
        .get("Exec")
        .map(|v| unescape(v))
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| Skip::Exec("no Exec key".into()))?;
    let fields = Fields {
        icon: values
            .get("Icon")
            .map(|v| unescape(v))
            .filter(|v| !v.is_empty()),
        name: values.get("Name").map(|v| unescape(v)),
        file: path.to_string_lossy().into_owned(),
    };
    let argv = exec_argv(&exec, &fields).map_err(Skip::Exec)?;
    let dir = values
        .get("Path")
        .map(|v| PathBuf::from(unescape(v)))
        .filter(|p| !p.as_os_str().is_empty());
    Ok(Item {
        id: id.to_string(),
        argv,
        dir,
    })
}

/// What the field codes `%i`, `%c` and `%k` stand for.
pub struct Fields {
    pub icon: Option<String>,
    pub name: Option<String>,
    pub file: String,
}

/// Split an `Exec` value (string escapes already applied) into arguments by the quoting rules of the Desktop Entry
/// Specification: arguments are separated by spaces, a quoted argument is in double quotes, and inside quotes `"`, `` ` ``,
/// `$` and `\` are escaped with a backslash. Field codes: `%f %F %u %U` expand to nothing (no files are opened at login),
/// `%i` to `--icon <Icon>` (nothing without an icon), `%c` to the name, `%k` to the entry's file, `%%` to `%`; the
/// deprecated `%d %D %n %N %v %m` are removed; any other is an error, as is a field code inside quotes or an unclosed quote.
pub fn exec_argv(exec: &str, fields: &Fields) -> Result<Vec<String>, String> {
    let mut argv: Vec<String> = Vec::new();
    let mut current = String::new();
    // the current argument exists even when empty (`""`)
    let mut started = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' => {
                if started {
                    argv.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        None => return Err("an unclosed quote".into()),
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e @ ('"' | '`' | '$' | '\\')) => current.push(e),
                            Some(other) => return Err(format!("\\{other} inside quotes")),
                            None => return Err("an unclosed quote".into()),
                        },
                        Some(other) => current.push(other),
                    }
                }
            }
            '%' => {
                let code = chars.next().ok_or("a lone % at the end")?;
                match code {
                    '%' => {
                        current.push('%');
                        started = true;
                    }
                    'f' | 'F' | 'u' | 'U' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm' => {}
                    'i' => {
                        if let Some(icon) = &fields.icon {
                            if started {
                                return Err("%i inside an argument".into());
                            }
                            argv.push("--icon".into());
                            argv.push(icon.clone());
                        }
                    }
                    'c' => {
                        current.push_str(fields.name.as_deref().unwrap_or(""));
                        started = true;
                    }
                    'k' => {
                        current.push_str(&fields.file);
                        started = true;
                    }
                    other => return Err(format!("unknown field code %{other}")),
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        argv.push(current);
    }
    if argv.is_empty() {
        return Err("Exec names no program".into());
    }
    Ok(argv)
}

/// Claim the session's single autostart run: creates `<runtime dir>/cupertino-autostart-<session id>` and returns true,
/// or returns false when it exists (the agent was restarted in the same session). The runtime directory is the user's and
/// goes away with the last session, and session ids are unique per boot.
pub fn claim_run(runtime_dir: &Path, session_id: &str) -> std::io::Result<bool> {
    let marker = runtime_dir.join(format!("cupertino-autostart-{session_id}"));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(std::io::Error::new(
            e.kind(),
            format!("{}: {e}", marker.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(desktops: &[&str]) -> Environment {
        Environment {
            desktops: desktops.iter().map(|d| d.to_string()).collect(),
            executable: Box::new(|name| name == "present"),
        }
    }

    fn fields() -> Fields {
        Fields {
            icon: Some("app-icon".into()),
            name: Some("My App".into()),
            file: "/etc/xdg/autostart/app.desktop".into(),
        }
    }

    fn entry(extra: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName=App\nExec=app --start\n{extra}")
    }

    fn check(extra: &str, desktops: &[&str]) -> Result<Item, Skip> {
        parse(
            "app.desktop",
            Path::new("/x/app.desktop"),
            &entry(extra),
            &env(desktops),
        )
    }

    #[test]
    fn a_plain_entry_starts_with_its_command_and_working_directory() {
        let item = check("Path=/tmp\\sdir\n", &[]).unwrap();
        assert_eq!(
            item,
            Item {
                id: "app.desktop".into(),
                argv: vec!["app".into(), "--start".into()],
                dir: Some(PathBuf::from("/tmp dir"))
            }
        );
    }

    #[test]
    fn hidden_desktops_disabled_try_exec_and_terminal_entries_are_skipped() {
        assert_eq!(check("Hidden=true\n", &[]), Err(Skip::Hidden));
        assert_eq!(
            check("OnlyShowIn=GNOME;KDE;\n", &["Cupertino"]),
            Err(Skip::OnlyShowIn)
        );
        assert!(check("OnlyShowIn=GNOME;Cupertino;\n", &["Cupertino"]).is_ok());
        assert_eq!(
            check("NotShowIn=Cupertino;\n", &["X", "Cupertino"]),
            Err(Skip::NotShowIn)
        );
        assert!(check("NotShowIn=GNOME;\n", &["Cupertino"]).is_ok());
        assert_eq!(
            check("X-GNOME-Autostart-enabled=false\n", &[]),
            Err(Skip::Disabled)
        );
        assert!(check("X-GNOME-Autostart-enabled=true\n", &[]).is_ok());
        assert_eq!(
            check("TryExec=missing\n", &[]),
            Err(Skip::TryExec("missing".into()))
        );
        assert!(check("TryExec=present\n", &[]).is_ok());
        assert_eq!(check("Terminal=true\n", &[]), Err(Skip::Terminal));
        assert_eq!(
            parse(
                "a.desktop",
                Path::new("/a"),
                "[Desktop Entry]\nType=Link\nURL=x\n",
                &env(&[])
            ),
            Err(Skip::NotApplication)
        );
        assert!(matches!(
            parse(
                "a.desktop",
                Path::new("/a"),
                "[Desktop Entry]\nType=Application\n",
                &env(&[])
            ),
            Err(Skip::Exec(_))
        ));
    }

    #[test]
    fn only_the_main_group_counts_and_the_first_key_wins() {
        let text = "[Desktop Entry]\nType=Application\nExec=first\nExec=second\n[Desktop Action other]\nHidden=true\n";
        assert_eq!(
            parse("a.desktop", Path::new("/a"), text, &env(&[]))
                .unwrap()
                .argv,
            ["first"]
        );
    }

    #[test]
    fn exec_follows_the_quoting_rules_and_expands_the_field_codes() {
        let f = fields();
        assert_eq!(
            exec_argv(r#"prog "an arg" "with \"quote\" and \$ and \\" plain"#, &f).unwrap(),
            ["prog", "an arg", r#"with "quote" and $ and \"#, "plain"]
        );
        assert_eq!(
            exec_argv("prog %U --flag %f", &f).unwrap(),
            ["prog", "--flag"]
        );
        assert_eq!(
            exec_argv("prog %i %c %k 100%%", &f).unwrap(),
            [
                "prog",
                "--icon",
                "app-icon",
                "My App",
                "/etc/xdg/autostart/app.desktop",
                "100%"
            ]
        );
        assert_eq!(exec_argv(r#"prog """#, &f).unwrap(), ["prog", ""]);
        assert!(exec_argv(r#"prog "open"#, &f).is_err());
        assert!(exec_argv("prog %z", &f).is_err());
        assert!(exec_argv("%U", &f).is_err(), "no program");
        let no_icon = Fields {
            icon: None,
            ..fields()
        };
        assert_eq!(exec_argv("prog %i", &no_icon).unwrap(), ["prog"]);
    }

    #[test]
    fn the_string_escapes_apply_before_the_quoting_rules() {
        // `\\\\` in the file is `\\` after the string escapes, which the quoting rules read as one backslash
        let item = parse(
            "a.desktop",
            Path::new("/a"),
            "[Desktop Entry]\nType=Application\nExec=prog \"a\\\\\\\\b\"\n",
            &env(&[]),
        )
        .unwrap();
        assert_eq!(item.argv, ["prog", "a\\b"]);
    }

    #[test]
    fn the_users_entry_shadows_the_system_one_and_system_entries_start_first() {
        let root = tempfile::tempdir().unwrap();
        let user = root.path().join("home/autostart");
        let system = root.path().join("etc/autostart");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        std::fs::write(system.join("shared.desktop"), entry("")).unwrap();
        std::fs::write(user.join("shared.desktop"), entry("Hidden=true\n")).unwrap();
        std::fs::write(system.join("b-system.desktop"), entry("")).unwrap();
        std::fs::write(user.join("a-user.desktop"), entry("")).unwrap();
        std::fs::write(user.join("notes.txt"), "not an entry").unwrap();
        let found = list(&[user, system, root.path().join("missing")]).unwrap();
        let ids: Vec<(&str, bool)> = found.iter().map(|f| (f.id.as_str(), f.user)).collect();
        assert_eq!(
            ids,
            [
                ("b-system.desktop", false),
                ("a-user.desktop", true),
                ("shared.desktop", true)
            ]
        );
        assert_eq!(
            parse(&found[2].id, &found[2].path, &found[2].text, &env(&[])),
            Err(Skip::Hidden)
        );
    }

    #[test]
    fn the_directories_default_to_etc_xdg() {
        assert_eq!(
            directories(Some("/h/.config".into()), None),
            [
                PathBuf::from("/h/.config/autostart"),
                PathBuf::from("/etc/xdg/autostart")
            ]
        );
        assert_eq!(
            directories(None, Some("/a:/b".into())),
            [PathBuf::from("/a/autostart"), PathBuf::from("/b/autostart")]
        );
    }

    #[test]
    fn a_session_runs_its_login_items_once() {
        let dir = tempfile::tempdir().unwrap();
        assert!(claim_run(dir.path(), "3").unwrap());
        assert!(
            !claim_run(dir.path(), "3").unwrap(),
            "a restarted agent does not start them again"
        );
        assert!(claim_run(dir.path(), "4").unwrap());
        assert!(claim_run(&dir.path().join("missing"), "5").is_err());
    }
}
