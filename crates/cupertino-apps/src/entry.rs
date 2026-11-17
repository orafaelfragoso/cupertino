//! Desktop entry parsing and indexing (freedesktop Desktop Entry Specification).
//!
//! Applications are collected from `applications/` below every XDG data directory. The first
//! directory that provides a desktop file id wins (user entries shadow system ones); `NoDisplay`,
//! `Hidden`, `OnlyShowIn` / `NotShowIn` (against `XDG_CURRENT_DESKTOP`) and a failing `TryExec` hide
//! an entry. Localised `Name`, `GenericName`, `Comment` and `Keywords` are resolved with the usual
//! `lang_COUNTRY@MODIFIER` fallback order.
//!
//! An entry that only hides itself from listings (`NoDisplay`, `OnlyShowIn`, `NotShowIn`) still handles
//! the MIME types of its `MimeType` key: [`parse_handler`] and [`index_handlers`] read entries for the
//! default-application resolver of [`crate::mimeapps`], which honours only `Hidden` and `TryExec`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopAction {
    pub id: String,
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopEntry {
    /// Desktop file id (`org.example.App.desktop`, subdirectories joined with `-`).
    pub id: String,
    pub name: String,
    pub generic_name: String,
    pub comment: String,
    pub keywords: Vec<String>,
    pub categories: Vec<String>,
    pub icon: Option<String>,
    pub exec: String,
    pub try_exec: Option<String>,
    pub terminal: bool,
    pub path: Option<PathBuf>,
    pub startup_wm_class: Option<String>,
    pub actions: Vec<DesktopAction>,
    pub source: Option<PathBuf>,
}

/// Language preference list used to pick localised values.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Locale {
    /// Keys to try in order, e.g. `["de_AT@euro", "de_AT", "de@euro", "de"]`.
    pub candidates: Vec<String>,
}

impl Locale {
    /// Parse a POSIX locale name (`de_AT.UTF-8@euro`).
    pub fn parse(name: &str) -> Locale {
        let name = name.trim();
        if name.is_empty() || name == "C" || name == "POSIX" {
            return Locale::default();
        }
        let (rest, modifier) = match name.split_once('@') {
            Some((r, m)) => (r, Some(m)),
            None => (name, None),
        };
        let rest = rest.split_once('.').map_or(rest, |(r, _)| r);
        let (lang, country) = match rest.split_once('_') {
            Some((l, c)) => (l, Some(c)),
            None => (rest, None),
        };
        let mut c = Vec::new();
        if let (Some(country), Some(m)) = (country, modifier) {
            c.push(format!("{lang}_{country}@{m}"));
        }
        if let Some(country) = country {
            c.push(format!("{lang}_{country}"));
        }
        if let Some(m) = modifier {
            c.push(format!("{lang}@{m}"));
        }
        c.push(lang.to_string());
        Locale { candidates: c }
    }

    pub fn from_env() -> Locale {
        for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(var) {
                if !v.is_empty() {
                    return Locale::parse(&v);
                }
            }
        }
        Locale::default()
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push(';'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Split a list value on unescaped `;`.
pub fn split_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for c in s.chars() {
        match (esc, c) {
            (true, c) => {
                cur.push('\\');
                cur.push(c);
                esc = false;
            }
            (false, '\\') => esc = true,
            (false, ';') => {
                out.push(unescape(&cur));
                cur.clear();
            }
            (false, c) => cur.push(c),
        }
    }
    if esc {
        cur.push('\\');
    }
    if !cur.is_empty() {
        out.push(unescape(&cur));
    }
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

#[derive(Default)]
struct Group {
    values: HashMap<String, String>,
}

impl Group {
    fn plain(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn localized_raw(&self, key: &str, locale: &Locale) -> Option<&str> {
        locale
            .candidates
            .iter()
            .find_map(|l| self.values.get(&format!("{key}[{l}]")))
            .or_else(|| self.values.get(key))
            .map(String::as_str)
    }

    fn localized(&self, key: &str, locale: &Locale) -> Option<String> {
        self.localized_raw(key, locale).map(unescape)
    }

    fn boolean(&self, key: &str) -> bool {
        self.plain(key).is_some_and(|v| v.trim() == "true")
    }
}

fn parse_groups(text: &str) -> Vec<(String, Group)> {
    let mut groups: Vec<(String, Group)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            groups.push((name.to_string(), Group::default()));
        } else if let (Some((k, v)), Some((_, g))) = (line.split_once('='), groups.last_mut()) {
            g.values
                .entry(k.trim().to_string())
                .or_insert_with(|| v.trim_start().to_string());
        }
    }
    groups
}

/// Whether the desktop file `text` says `DBusActivatable=true` in its `[Desktop Entry]` group: the application takes
/// `org.freedesktop.Application` calls on the bus name of its desktop file ID.
pub fn dbus_activatable(text: &str) -> bool {
    parse_groups(text)
        .iter()
        .find(|(n, _)| n == "Desktop Entry")
        .is_some_and(|(_, g)| g.boolean("DBusActivatable"))
}

/// [`dbus_activatable`] of the desktop file at `path`; false when it cannot be read.
pub fn dbus_activatable_file(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| dbus_activatable(&text))
}

/// Why an entry is not offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hidden {
    NotApplication,
    NoDisplay,
    HiddenKey,
    OnlyShowIn,
    NotShowIn,
    TryExec,
    /// `Terminal=true` or the `ConsoleOnly` category: a command-line program, not an application with a window.
    Terminal,
    /// The `Exec` line names no program that can be found (empty, unbalanced quotes or not on the search path).
    NoProgram,
}

/// Environment that decides visibility.
#[derive(Clone, Debug, Default)]
pub struct Visibility {
    /// Entries of `XDG_CURRENT_DESKTOP` (colon separated list).
    pub current_desktops: Vec<String>,
    /// Directories searched for `TryExec` (default: `PATH`).
    pub path_dirs: Vec<PathBuf>,
    /// Skip the `TryExec` check (tests, containers).
    pub skip_try_exec: bool,
}

impl Visibility {
    pub fn from_env() -> Visibility {
        let current_desktops = std::env::var("XDG_CURRENT_DESKTOP")
            .map(|v| {
                v.split(':')
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        Visibility {
            current_desktops,
            path_dirs,
            skip_try_exec: false,
        }
    }

    fn executable_exists(&self, name: &str) -> bool {
        if name.contains('/') {
            return is_executable(Path::new(name));
        }
        self.path_dirs.iter().any(|d| is_executable(&d.join(name)))
    }
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// What an entry is read for: a listing (launcher, app grid) applies every visibility rule, a MIME
/// handler lookup only those that remove the application (`Hidden`, `TryExec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Purpose {
    Listing,
    Handler,
}

/// An application that can open files: its entry and the MIME types of its `MimeType` key, in file order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handler {
    pub entry: DesktopEntry,
    pub mime_types: Vec<String>,
}

/// Parse one desktop file. `Err` carries the reason the entry is hidden; unparsable files are
/// reported as [`Hidden::NotApplication`].
pub fn parse_desktop_entry(
    id: &str,
    text: &str,
    locale: &Locale,
    vis: &Visibility,
) -> Result<DesktopEntry, Hidden> {
    parse(id, text, locale, vis, Purpose::Listing).map(|h| h.entry)
}

/// Parse one desktop file as a MIME handler: `NoDisplay`, `OnlyShowIn` and `NotShowIn` do not apply
/// (mime-apps specification: such entries still handle their types); `Hidden`, a failing `TryExec`
/// and a missing `Name` or `Exec` give `Err` as in [`parse_desktop_entry`].
pub fn parse_handler(
    id: &str,
    text: &str,
    locale: &Locale,
    vis: &Visibility,
) -> Result<Handler, Hidden> {
    parse(id, text, locale, vis, Purpose::Handler)
}

fn parse(
    id: &str,
    text: &str,
    locale: &Locale,
    vis: &Visibility,
    purpose: Purpose,
) -> Result<Handler, Hidden> {
    let listing = purpose == Purpose::Listing;
    let groups = parse_groups(text);
    let main = groups
        .iter()
        .find(|(n, _)| n == "Desktop Entry")
        .map(|(_, g)| g)
        .ok_or(Hidden::NotApplication)?;
    if main.plain("Type").map(str::trim) != Some("Application") {
        return Err(Hidden::NotApplication);
    }
    if listing && main.boolean("NoDisplay") {
        return Err(Hidden::NoDisplay);
    }
    if main.boolean("Hidden") {
        return Err(Hidden::HiddenKey);
    }
    let list = |k: &str| main.plain(k).map(split_list).unwrap_or_default();
    let (only, not) = (list("OnlyShowIn"), list("NotShowIn"));
    if listing && !only.is_empty() && !only.iter().any(|o| vis.current_desktops.contains(o)) {
        return Err(Hidden::OnlyShowIn);
    }
    if listing && not.iter().any(|o| vis.current_desktops.contains(o)) {
        return Err(Hidden::NotShowIn);
    }
    let try_exec = main
        .plain("TryExec")
        .map(|s| unescape(s.trim()))
        .filter(|s| !s.is_empty());
    if let (Some(t), false) = (&try_exec, vis.skip_try_exec) {
        if !vis.executable_exists(t) {
            return Err(Hidden::TryExec);
        }
    }
    let name = main
        .localized("Name", locale)
        .filter(|n| !n.trim().is_empty())
        .ok_or(Hidden::NotApplication)?;
    let exec = main
        .plain("Exec")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(exec) = exec else {
        return Err(Hidden::NotApplication);
    };
    let keywords = main
        .localized_raw("Keywords", locale)
        .map(split_list)
        .unwrap_or_default();
    let mut actions = Vec::new();
    for aid in list("Actions") {
        let Some((_, g)) = groups
            .iter()
            .find(|(n, _)| *n == format!("Desktop Action {aid}"))
        else {
            continue;
        };
        let (Some(aname), Some(aexec)) = (g.localized("Name", locale), g.plain("Exec")) else {
            continue;
        };
        actions.push(DesktopAction {
            id: aid,
            name: aname,
            exec: aexec.trim().to_string(),
            icon: g.plain("Icon").map(|s| s.trim().to_string()),
        });
    }
    let entry = DesktopEntry {
        id: id.to_string(),
        name,
        generic_name: main.localized("GenericName", locale).unwrap_or_default(),
        comment: main.localized("Comment", locale).unwrap_or_default(),
        keywords,
        categories: list("Categories"),
        icon: main
            .plain("Icon")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        exec,
        try_exec,
        terminal: main.boolean("Terminal"),
        path: main
            .plain("Path")
            .map(|s| PathBuf::from(unescape(s.trim())))
            .filter(|p| !p.as_os_str().is_empty()),
        startup_wm_class: main.plain("StartupWMClass").map(|s| s.trim().to_string()),
        actions,
        source: None,
    };
    Ok(Handler {
        entry,
        mime_types: list("MimeType"),
    })
}

/// `$XDG_DATA_HOME/applications` followed by `<dir>/applications` for each `XDG_DATA_DIRS` entry.
pub fn xdg_application_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home = match std::env::var_os("XDG_DATA_HOME") {
        Some(d) if !d.is_empty() => Some(PathBuf::from(d)),
        _ => home.map(|h| h.join(".local/share")),
    };
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .into_iter()
        .chain(dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .map(|d| d.join("applications"))
        .collect()
}

fn collect_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = rd.flatten().collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if path.is_dir() {
            if depth < 8 {
                collect_files(&path, &format!("{prefix}{name}-"), out, depth + 1);
            }
        } else if name.ends_with(".desktop") {
            out.push((format!("{prefix}{name}"), path));
        }
    }
}

/// Index applications from `dirs` (in precedence order).
pub fn index_dirs(dirs: &[PathBuf], locale: &Locale, vis: &Visibility) -> Vec<DesktopEntry> {
    let mut seen: HashMap<String, ()> = HashMap::new();
    let mut out = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect_files(dir, "", &mut files, 0);
        for (id, path) in files {
            if seen.contains_key(&id) {
                continue;
            }
            seen.insert(id.clone(), ());
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(mut e) = parse_desktop_entry(&id, &text, locale, vis) {
                e.source = Some(path);
                out.push(e);
            }
        }
    }
    out
}

/// Whether `entry` belongs in an application list (the launcher's grid and its search results), on top of the
/// visibility rules [`parse_desktop_entry`] already applied (`Type=Application`, `NoDisplay`, `Hidden`,
/// `OnlyShowIn`, `NotShowIn`, `TryExec`, a `Name` and an `Exec`). A command-line program (`Terminal=true` or the
/// `ConsoleOnly` category) is left out, and so is an entry whose `Exec` names no program that can be found; the
/// program check is skipped with [`Visibility::skip_try_exec`].
pub fn listed_application(entry: &DesktopEntry, vis: &Visibility) -> Result<(), Hidden> {
    if entry.terminal || entry.categories.iter().any(|c| c == "ConsoleOnly") {
        return Err(Hidden::Terminal);
    }
    let program = crate::exec::tokenize(&entry.exec)
        .and_then(|argv| argv.into_iter().next())
        .filter(|p| !p.is_empty() && !p.starts_with('%'))
        .ok_or(Hidden::NoProgram)?;
    if !vis.skip_try_exec && !vis.executable_exists(&program) {
        return Err(Hidden::NoProgram);
    }
    Ok(())
}

/// [`index_dirs`] narrowed to the entries [`listed_application`] keeps: the applications a person can open.
pub fn index_applications(
    dirs: &[PathBuf],
    locale: &Locale,
    vis: &Visibility,
) -> Vec<DesktopEntry> {
    let mut entries = index_dirs(dirs, locale, vis);
    entries.retain(|e| listed_application(e, vis).is_ok());
    entries
}
