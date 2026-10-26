//! Desktop entries (`.desktop` files of the XDG data directories): application names, icons and
//! commands for the switcher and application launching.

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
        startup_wm_class: map.get("StartupWMClass").map(|s| s.to_string()).unwrap_or_default(),
        terminal: map.get("Terminal") == Some(&"true"),
        no_display: map.get("NoDisplay") == Some(&"true"),
        path: path.to_path_buf(),
    })
}

/// Split an `Exec` line into program and arguments: quotes are honoured, field codes (`%f`, `%U`,
/// ...) are dropped and `%%` becomes `%`.
pub fn exec_command(exec: &str) -> Vec<String> {
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
            '%' => {
                if let Some('%') = chars.next() {
                    cur.push('%');
                }
            }
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
    pub fn load() -> Self {
        Self::load_from(&cupertino_defaults::data_dirs())
    }

    pub fn load_from(dirs: &[PathBuf]) -> Self {
        let mut entries: Vec<DesktopEntry> = Vec::new();
        for base in dirs {
            let apps = base.join("applications");
            let mut stack = vec![apps.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
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
                    if let Some(entry) = std::fs::read_to_string(&path).ok().and_then(|t| parse_entry(&id, &path, &t)) {
                        entries.push(entry);
                    }
                }
            }
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        Self { entries }
    }

    /// Entry for an application identity (an `app_id`, a desktop file ID or a command name):
    /// desktop file ID, then `StartupWMClass`, then the program of `Exec`, all case-insensitive.
    pub fn find(&self, identity: &str) -> Option<&DesktopEntry> {
        let id = identity.trim_end_matches(".desktop").to_lowercase();
        if id.is_empty() {
            return None;
        }
        let by = |f: &dyn Fn(&DesktopEntry) -> bool| self.entries.iter().find(|e| f(e));
        by(&|e| e.id.to_lowercase() == id)
            .or_else(|| by(&|e| !e.startup_wm_class.is_empty() && e.startup_wm_class.to_lowercase() == id))
            .or_else(|| {
                by(&|e| {
                    exec_command(&e.exec).first().is_some_and(|p| Path::new(p).file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_lowercase() == id))
                })
            })
            .or_else(|| by(&|e| e.id.to_lowercase().rsplit('.').next() == Some(id.as_str())))
    }
}

/// Human readable application name for the menu bar: the desktop entry's name, else the last
/// segment of the identity with its first letter capitalised.
pub fn display_name(index: &DesktopIndex, identity: &str) -> String {
    if let Some(e) = index.find(identity) {
        return e.name.clone();
    }
    let last = identity.rsplit(['.', '/']).next().filter(|s| !s.is_empty()).unwrap_or(identity);
    let mut chars = last.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOT: &str = "[Desktop Entry]\nType=Application\nName=Foot\nExec=foot %U\nIcon=foot\nStartupWMClass=foot\n\n[Desktop Action new]\nName=Other\nExec=nope\n";

    #[test]
    fn parses_the_main_group_only() {
        let e = parse_entry("foot", Path::new("/x/foot.desktop"), FOOT).unwrap();
        assert_eq!((e.name.as_str(), e.exec.as_str(), e.icon.as_str()), ("Foot", "foot %U", "foot"));
        assert!(parse_entry("x", Path::new("x"), "[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true\n").is_none());
        assert!(parse_entry("x", Path::new("x"), "[Desktop Entry]\nType=Link\nName=X\nExec=x\n").is_none());
        assert!(parse_entry("x", Path::new("x"), "[Desktop Entry]\nName=X\n").is_none());
    }

    #[test]
    fn exec_lines_drop_field_codes_and_keep_quotes() {
        assert_eq!(exec_command("foot %U"), ["foot"]);
        assert_eq!(exec_command("env A=1 \"/opt/my app/run\" --flag %f"), ["env", "A=1", "/opt/my app/run", "--flag"]);
        assert_eq!(exec_command("sh -c \"echo 100%%\""), ["sh", "-c", "echo 100%"]);
        assert!(exec_command("  ").is_empty());
    }

    #[test]
    fn identities_resolve_by_id_class_and_program() {
        let idx = DesktopIndex {
            entries: vec![
                parse_entry("org.gnome.Nautilus", Path::new("a"), "[Desktop Entry]\nName=Files\nExec=nautilus --new-window %U\nIcon=org.gnome.Nautilus\n").unwrap(),
                parse_entry("foot", Path::new("b"), FOOT).unwrap(),
                parse_entry("browser", Path::new("c"), "[Desktop Entry]\nName=Web\nExec=/usr/bin/firefox %u\nStartupWMClass=Navigator\n").unwrap(),
            ],
        };
        assert_eq!(idx.find("org.gnome.Nautilus").unwrap().name, "Files");
        assert_eq!(idx.find("nautilus").unwrap().name, "Files");
        assert_eq!(idx.find("FOOT").unwrap().id, "foot");
        assert_eq!(idx.find("navigator").unwrap().id, "browser");
        assert_eq!(idx.find("firefox").unwrap().id, "browser");
        assert!(idx.find("nothing").is_none());
        assert_eq!(display_name(&idx, "foot"), "Foot");
        assert_eq!(display_name(&idx, "org.example.editor"), "Editor");
    }

    #[test]
    fn index_loads_from_directories() {
        let dir = std::env::temp_dir().join(format!("cupertino-desktop-{}", std::process::id()));
        let apps = dir.join("applications");
        std::fs::create_dir_all(apps.join("sub")).unwrap();
        std::fs::write(apps.join("a.desktop"), FOOT).unwrap();
        std::fs::write(apps.join("sub/b.desktop"), "[Desktop Entry]\nName=B\nExec=b\n").unwrap();
        let idx = DesktopIndex::load_from(std::slice::from_ref(&dir));
        let ids: Vec<&str> = idx.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["a", "sub-b"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
