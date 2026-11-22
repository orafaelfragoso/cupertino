//! Desktop entries (`.desktop` files of the XDG data directories): the application each dock tile
//! stands for, its icon and the command that starts it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One application entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesktopEntry {
    /// File name without `.desktop` (the desktop file ID, with `/` replaced by `-`).
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub startup_wm_class: String,
    pub terminal: bool,
    pub no_display: bool,
    pub path: PathBuf,
    /// The freedesktop `Categories`, in their order.
    pub categories: Vec<String>,
    /// `DBusActivatable=true`: the application takes `org.freedesktop.Application` calls on the bus name of its ID.
    pub dbus_activatable: bool,
}

/// Parse the `[Desktop Entry]` group of a desktop file. `None` for hidden, non-application or
/// unreadable entries.
pub fn parse_entry(id: &str, path: &Path, text: &str) -> Option<DesktopEntry> {
    let mut in_group = false;
    let mut map: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.entry(k.trim()).or_insert(v.trim());
        }
    }
    if map.get("Type").is_some_and(|t| *t != "Application") || map.get("Hidden") == Some(&"true") {
        return None;
    }
    let name = map.get("Name")?.to_string();
    let exec = map.get("Exec")?.to_string();
    Some(DesktopEntry {
        id: id.to_string(),
        name,
        exec,
        icon: map.get("Icon").map(|s| s.to_string()).unwrap_or_default(),
        startup_wm_class: map
            .get("StartupWMClass")
            .map(|s| s.to_string())
            .unwrap_or_default(),
        terminal: map.get("Terminal") == Some(&"true"),
        no_display: map.get("NoDisplay") == Some(&"true"),
        path: path.to_path_buf(),
        categories: map
            .get("Categories")
            .map(|c| {
                c.split(';')
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        dbus_activatable: map.get("DBusActivatable") == Some(&"true"),
    })
}

/// Split an `Exec` line into program and arguments: quotes are honoured, field codes (`%f`, `%U`,
/// ...) are dropped and `%%` becomes `%`.
pub fn exec_command(exec: &str) -> Vec<String> {
    split_exec(exec, None)
}

/// [`exec_command`] for opening `target` (a path or URI): the first file or URL code (`%f`, `%F`,
/// `%u`, `%U`) becomes `target`; an entry without one gets no argument.
pub fn exec_command_with(exec: &str, target: &str) -> Vec<String> {
    split_exec(exec, Some(target))
}

fn split_exec(exec: &str, mut target: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '%' => match chars.next() {
                Some('%') => cur.push('%'),
                Some('f' | 'F' | 'u' | 'U') => {
                    if let Some(t) = target.take() {
                        cur.push_str(t);
                        has = true;
                    }
                }
                _ => {}
            },
            c if c.is_whitespace() && !quoted => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out.retain(|a| !a.is_empty());
    out
}

/// Every application of the XDG data directories, first definition of an ID wins.
#[derive(Clone, Debug, Default)]
pub struct DesktopIndex {
    pub entries: Vec<DesktopEntry>,
}

impl DesktopIndex {
    /// Index the `applications` directories under `dirs` (the XDG data directories).
    #[expect(
        clippy::disallowed_methods,
        reason = "runs on the I/O pool (kit::task::io)"
    )]
    pub fn load_from(dirs: &[PathBuf]) -> Self {
        let mut entries: Vec<DesktopEntry> = Vec::new();
        for base in dirs {
            let apps = base.join("applications");
            let mut stack = vec![apps.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for e in rd.flatten() {
                    let path = e.path();
                    if path.is_dir() {
                        stack.push(path);
                        continue;
                    }
                    if path.extension().and_then(|x| x.to_str()) != Some("desktop") {
                        continue;
                    }
                    let rel = path.strip_prefix(&apps).unwrap_or(&path);
                    let id = rel.with_extension("").to_string_lossy().replace('/', "-");
                    if entries.iter().any(|x| x.id == id) {
                        continue;
                    }
                    if let Some(entry) = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|t| parse_entry(&id, &path, &t))
                    {
                        entries.push(entry);
                    }
                }
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Self { entries }
    }

    /// Entry for an application identity (an `app_id`, a desktop file ID or a command name):
    /// desktop file ID, then `StartupWMClass`, then the program of `Exec`, all case-insensitive. An entry the desktop
    /// hides (`NoDisplay`) stands for the entry it shows that runs the same program, when there is one ([`shown`]):
    /// an application installs a hidden entry by its window's id for its URL handlers beside the one people start it
    /// by, and its windows belong to that one.
    pub fn find(&self, identity: &str) -> Option<&DesktopEntry> {
        self.named(identity).map(|e| self.shown(e))
    }

    /// The entry the desktop shows for `entry`: `entry` itself, unless it is `NoDisplay` and an entry the desktop
    /// shows runs the same program (the first word of `Exec`, compared as a path); the first such entry by ID. A
    /// hidden helper's or portal's entry without one stays itself.
    pub fn shown<'a>(&'a self, entry: &'a DesktopEntry) -> &'a DesktopEntry {
        if !entry.no_display {
            return entry;
        }
        let Some(program) = exec_command(&entry.exec).into_iter().next() else {
            return entry;
        };
        self.entries
            .iter()
            .find(|e| {
                !e.no_display
                    && exec_command(&e.exec)
                        .first()
                        .is_some_and(|p| Path::new(p) == Path::new(&program))
            })
            .unwrap_or(entry)
    }

    /// The entry an identity names, by the rules of [`Self::find`] but without its step from a hidden entry to the one
    /// the desktop shows.
    pub fn named(&self, identity: &str) -> Option<&DesktopEntry> {
        let id = identity.trim_end_matches(".desktop").to_lowercase();
        if id.is_empty() {
            return None;
        }
        let by = |f: &dyn Fn(&DesktopEntry) -> bool| self.entries.iter().find(|e| f(e));
        by(&|e| e.id.to_lowercase() == id)
            .or_else(|| {
                by(&|e| !e.startup_wm_class.is_empty() && e.startup_wm_class.to_lowercase() == id)
            })
            .or_else(|| {
                by(&|e| {
                    exec_command(&e.exec).first().is_some_and(|p| {
                        Path::new(p)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.to_lowercase() == id)
                    })
                })
            })
            .or_else(|| by(&|e| e.id.to_lowercase().rsplit('.').next() == Some(id.as_str())))
    }

    /// The `Icon` of an application identity's desktop entry, when it names one.
    pub fn entry_icon(&self, identity: &str) -> Option<&str> {
        self.find(identity)
            .map(|e| e.icon.as_str())
            .filter(|i| !i.is_empty())
    }
}
