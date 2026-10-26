//! Default applications and the Open With list (freedesktop MIME Applications Associations
//! specification 1.0.1, with the shared MIME database for subclass and alias fallback).
//!
//! `mimeapps.list` files are read in the specification's precedence order (for each desktop of
//! `XDG_CURRENT_DESKTOP`, lower-cased, the `<desktop>-mimeapps.list` before `mimeapps.list`):
//! `$XDG_CONFIG_HOME`, each of `$XDG_CONFIG_DIRS`, `$XDG_DATA_HOME/applications`, each of
//! `$XDG_DATA_DIRS/applications`. Desktop-specific files only set defaults. `mimeinfo.cache` is not
//! read: the `MimeType` keys of the desktop entries are its source and are read directly.
//!
//! The Open With order is the design's: the default application first, then the rest by
//! name, case-insensitively and with numbers compared by value; the design's tie-break on the bundle
//! version has no counterpart in a desktop entry, so equal names fall back to the desktop file ID.
//! As in the design's second, broader pass, handlers of
//! `application/octet-stream` join the list only when nothing handles the type more specifically.

use crate::entry::{parse_handler, DesktopEntry, Handler, Locale, Visibility};
use crate::mime::{MimeDatabase, OCTET_STREAM};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

/// The XDG base directories and desktop names the resolver reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XdgDirs {
    pub config_home: PathBuf,
    pub config_dirs: Vec<PathBuf>,
    pub data_home: PathBuf,
    pub data_dirs: Vec<PathBuf>,
    /// `XDG_CURRENT_DESKTOP`, in order, as given (lower-cased for file names).
    pub current_desktops: Vec<String>,
}

fn env_dir(var: &str, home: &Option<PathBuf>, under_home: &str) -> Result<PathBuf, String> {
    match std::env::var_os(var).filter(|v| !v.is_empty()) {
        Some(v) => Ok(PathBuf::from(v)),
        None => home.as_ref().map(|h| h.join(under_home)).ok_or_else(|| format!("neither {var} nor HOME is set")),
    }
}

fn env_list(var: &str, default: &str) -> Vec<PathBuf> {
    let value = std::env::var(var).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string());
    value.split(':').filter(|d| !d.is_empty()).map(PathBuf::from).collect()
}

impl XdgDirs {
    /// The directories of the environment, with the XDG Base Directory defaults (`~/.config`, `/etc/xdg`,
    /// `~/.local/share`, `/usr/local/share:/usr/share`). `Err` when a home-relative default is needed and
    /// `HOME` is not set.
    pub fn from_env() -> Result<XdgDirs, String> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
        Ok(XdgDirs {
            config_home: env_dir("XDG_CONFIG_HOME", &home, ".config")?,
            config_dirs: env_list("XDG_CONFIG_DIRS", "/etc/xdg"),
            data_home: env_dir("XDG_DATA_HOME", &home, ".local/share")?,
            data_dirs: env_list("XDG_DATA_DIRS", "/usr/local/share:/usr/share"),
            current_desktops: std::env::var("XDG_CURRENT_DESKTOP").map(|v| v.split(':').filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_default(),
        })
    }

    /// The XDG data directories, most important first.
    pub fn all_data_dirs(&self) -> Vec<PathBuf> {
        std::iter::once(self.data_home.clone()).chain(self.data_dirs.iter().cloned()).collect()
    }
}

/// The three groups of one `mimeapps.list`, keys canonical, values in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ListFile {
    defaults: HashMap<String, Vec<String>>,
    added: HashMap<String, Vec<String>>,
    removed: HashMap<String, Vec<String>>,
}

impl ListFile {
    /// Parse a `mimeapps.list`; a desktop-specific file keeps only its defaults (the specification allows
    /// only `[Default Applications]` there).
    fn parse(text: &str, desktop_specific: bool, db: &MimeDatabase) -> ListFile {
        let mut file = ListFile::default();
        let mut group: Option<&mut HashMap<String, Vec<String>>> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                group = match name {
                    "Default Applications" => Some(&mut file.defaults),
                    "Added Associations" if !desktop_specific => Some(&mut file.added),
                    "Removed Associations" if !desktop_specific => Some(&mut file.removed),
                    _ => None,
                };
                continue;
            }
            let (Some(map), Some((mime, ids))) = (group.as_deref_mut(), line.split_once('=')) else { continue };
            let list = map.entry(db.canonical(mime.trim()).to_string()).or_default();
            for id in ids.split(';').map(str::trim).filter(|id| !id.is_empty()) {
                if !list.iter().any(|x| x == id) {
                    list.push(id.to_string());
                }
            }
        }
        file
    }
}

/// One level of the search: the list files of a directory and, for an `applications` directory, its
/// position among the application directories.
#[derive(Clone, Debug)]
struct Level {
    lists: Vec<ListFile>,
    app_dir: Option<usize>,
}

/// An installed application: the definition of its desktop file ID in the most important directory
/// that has it, and the least important directory that has it (changes of a level only apply to
/// entries at that level or below).
#[derive(Clone, Debug)]
struct Installed {
    handler: Handler,
    dir: usize,
    deepest: usize,
}

/// One row of the Open With list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenWithItem<'a> {
    pub entry: &'a DesktopEntry,
    /// The type's default application.
    pub is_default: bool,
}

/// The default-application resolver of one session.
#[derive(Clone, Debug)]
pub struct MimeApps {
    levels: Vec<Level>,
    installed: HashMap<String, Installed>,
    db: MimeDatabase,
}

fn read_optional(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
    }
}

/// Every `.desktop` file below `dir` with its desktop file ID (subdirectories joined with `-`), sorted.
fn desktop_files(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io::Error::new(e.kind(), format!("{}: {e}", dir.display()))),
    };
    let mut items = read.collect::<io::Result<Vec<_>>>()?;
    items.sort_by_key(|e| e.file_name());
    for item in items {
        let name = item.file_name().to_string_lossy().into_owned();
        let path = item.path();
        if path.is_dir() {
            desktop_files(&path, &format!("{prefix}{name}-"), out)?;
        } else if name.ends_with(".desktop") {
            out.push((format!("{prefix}{name}"), path));
        }
    }
    Ok(())
}

/// `a` before `b` by name: case-insensitive, digit runs by value (the design's options 0x41,
/// ); equal names by desktop file ID.
fn compare_names(a: &DesktopEntry, b: &DesktopEntry) -> Ordering {
    natural(&a.name, &b.name).then_with(|| a.id.cmp(&b.id))
}

fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        digits.push(c);
                    }
                    digits
                };
                let (m, n) = (take(&mut x), take(&mut y));
                let (m, n) = (m.trim_start_matches('0'), n.trim_start_matches('0'));
                let order = m.len().cmp(&n.len()).then_with(|| m.cmp(n));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(c), Some(d)) => {
                let order = c.to_lowercase().cmp(d.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                x.next();
                y.next();
            }
        }
    }
}

impl MimeApps {
    /// Read the list files and the application directories of `dirs`. Unreadable files are errors; files
    /// and directories that do not exist are skipped. Hidden entries and entries whose `TryExec` fails
    /// are not installed but still shadow less important entries of the same ID.
    pub fn load(dirs: &XdgDirs, db: MimeDatabase, locale: &Locale, vis: &Visibility) -> io::Result<MimeApps> {
        let desktops: Vec<String> = dirs.current_desktops.iter().map(|d| d.to_lowercase()).collect();
        let read_lists = |dir: &Path| -> io::Result<Vec<ListFile>> {
            let mut lists = Vec::new();
            for desktop in &desktops {
                if let Some(text) = read_optional(&dir.join(format!("{desktop}-mimeapps.list")))? {
                    lists.push(ListFile::parse(&text, true, &db));
                }
            }
            if let Some(text) = read_optional(&dir.join("mimeapps.list"))? {
                lists.push(ListFile::parse(&text, false, &db));
            }
            Ok(lists)
        };
        let mut levels = Vec::new();
        for dir in std::iter::once(&dirs.config_home).chain(&dirs.config_dirs) {
            levels.push(Level { lists: read_lists(dir)?, app_dir: None });
        }
        let app_dirs: Vec<PathBuf> = dirs.all_data_dirs().iter().map(|d| d.join("applications")).collect();
        let mut installed: HashMap<String, Installed> = HashMap::new();
        let mut shadowed: HashSet<String> = HashSet::new();
        for (position, dir) in app_dirs.iter().enumerate() {
            levels.push(Level { lists: read_lists(dir)?, app_dir: Some(position) });
            let mut files = Vec::new();
            desktop_files(dir, "", &mut files)?;
            for (id, path) in files {
                if let Some(app) = installed.get_mut(&id) {
                    app.deepest = position;
                    continue;
                }
                if !shadowed.insert(id.clone()) {
                    continue;
                }
                let text = std::fs::read_to_string(&path).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))?;
                if let Ok(mut handler) = parse_handler(&id, &text, locale, vis) {
                    handler.entry.source = Some(path);
                    handler.mime_types = handler.mime_types.iter().map(|m| db.canonical(m).to_string()).collect();
                    installed.insert(id, Installed { handler, dir: position, deepest: position });
                }
            }
        }
        Ok(MimeApps { levels, installed, db })
    }

    /// The MIME database the resolver canonicalises types with.
    pub fn database(&self) -> &MimeDatabase {
        &self.db
    }

    /// The installed application of a desktop file ID.
    pub fn entry(&self, id: &str) -> Option<&DesktopEntry> {
        self.installed.get(id).map(|a| &a.handler.entry)
    }

    /// Whether `id` is installed in an application directory at `level` or below (a configuration level
    /// sees every application directory).
    fn visible_from(&self, id: &str, level: &Level) -> bool {
        self.installed.get(id).is_some_and(|a| level.app_dir.is_none_or(|p| a.deepest >= p))
    }

    /// The applications associated with exactly `mime` (canonical), most preferred first: the
    /// specification's walk over the levels, each adding its `[Added Associations]`, then removing its
    /// `[Removed Associations]` from what less important levels contribute, then adding the entries of its
    /// application directory that list the type.
    fn associations(&self, mime: &str) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        let mut blocked: HashSet<&str> = HashSet::new();
        for level in &self.levels {
            for list in &level.lists {
                for id in list.added.get(mime).into_iter().flatten() {
                    if !blocked.contains(id.as_str()) && self.visible_from(id, level) && !out.contains(&id.as_str()) {
                        out.push(id);
                    }
                }
                blocked.extend(list.removed.get(mime).into_iter().flatten().map(String::as_str));
            }
            let Some(position) = level.app_dir else { continue };
            let mut here: Vec<&str> = self.installed.iter().filter(|(_, a)| a.dir == position && a.handler.mime_types.iter().any(|m| m == mime)).map(|(id, _)| id.as_str()).collect();
            here.sort_unstable();
            for id in here {
                if !blocked.contains(id) && !out.contains(&id) {
                    out.push(id);
                }
                blocked.insert(id);
            }
        }
        out
    }

    /// The default application of `mime`. For the type and then each ancestor (nearest first): the first
    /// `[Default Applications]` entry of the most important list that is installed and associated with
    /// it, else its most preferred association. So an application for a more specific type wins over a
    /// default set for a broader one.
    pub fn default_for(&self, mime: &str) -> Option<&DesktopEntry> {
        for ty in self.db.ancestors(mime) {
            let associated = self.associations(&ty);
            let listed = self.levels.iter().flat_map(|l| &l.lists).flat_map(|list| list.defaults.get(&ty).into_iter().flatten());
            if let Some(id) = listed.map(String::as_str).find(|id| associated.contains(id)) {
                return self.entry(id);
            }
            if let Some(id) = associated.first() {
                return self.entry(id);
            }
        }
        None
    }

    /// The Open With list of `mime`: the applications associated with the type or an ancestor other than
    /// `application/octet-stream` (that one only when the list is otherwise empty), the default first and
    /// the rest by name.
    pub fn open_with(&self, mime: &str) -> Vec<OpenWithItem<'_>> {
        let ancestry = self.db.ancestors(mime);
        let collect = |types: &mut dyn Iterator<Item = &String>| {
            let mut ids: Vec<&str> = Vec::new();
            for ty in types {
                for id in self.associations(ty) {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            ids
        };
        let mut ids = collect(&mut ancestry.iter().filter(|t| *t != OCTET_STREAM));
        if ids.is_empty() {
            ids = collect(&mut ancestry.iter());
        }
        let default = self.default_for(mime).map(|e| e.id.as_str());
        let mut entries: Vec<&DesktopEntry> = ids.iter().filter_map(|id| self.entry(id)).collect();
        entries.sort_by(|a, b| (Some(b.id.as_str()) == default).cmp(&(Some(a.id.as_str()) == default)).then_with(|| compare_names(a, b)));
        entries.into_iter().map(|entry| OpenWithItem { entry, is_default: Some(entry.id.as_str()) == default }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture tree: `config/`, `etc/`, `local/`, `usr/` as the four kinds of directory.
    struct Tree {
        root: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Tree {
            let tree = Tree { root: tempfile::tempdir().unwrap() };
            tree.write("usr/mime/subclasses", "application/x-shellscript text/plain\ntext/x-csrc text/plain\n");
            tree.write("usr/mime/aliases", "text/x-c text/x-csrc\n");
            tree
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.root.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn app(&self, dir: &str, id: &str, name: &str, mime: &str) {
            self.write(&format!("{dir}/applications/{id}"), &format!("[Desktop Entry]\nType=Application\nName={name}\nExec={id} %f\nMimeType={mime}\n"));
        }

        fn dirs(&self, desktops: &[&str]) -> XdgDirs {
            let p = |d: &str| self.root.path().join(d);
            XdgDirs { config_home: p("config"), config_dirs: vec![p("etc")], data_home: p("local"), data_dirs: vec![p("usr")], current_desktops: desktops.iter().map(|d| d.to_string()).collect() }
        }

        fn load(&self, desktops: &[&str]) -> MimeApps {
            let dirs = self.dirs(desktops);
            let db = MimeDatabase::load(&dirs.all_data_dirs()).unwrap();
            let vis = Visibility { current_desktops: vec![], path_dirs: vec![], skip_try_exec: true };
            MimeApps::load(&dirs, db, &Locale::default(), &vis).unwrap()
        }
    }

    fn default_id<'a>(apps: &'a MimeApps, mime: &str) -> Option<&'a str> {
        apps.default_for(mime).map(|e| e.id.as_str())
    }

    fn open_with<'a>(apps: &'a MimeApps, mime: &str) -> Vec<(&'a str, bool)> {
        apps.open_with(mime).iter().map(|i| (i.entry.id.as_str(), i.is_default)).collect()
    }

    #[test]
    fn without_lists_the_installed_handlers_decide_in_directory_order() {
        let t = Tree::new();
        t.app("usr", "viewer.desktop", "Viewer", "image/png;");
        t.app("local", "paint.desktop", "Paint", "image/png;");
        let apps = t.load(&[]);
        assert_eq!(default_id(&apps, "image/png"), Some("paint.desktop"), "the user's applications come first");
        assert_eq!(default_id(&apps, "image/jpeg"), None);
    }

    #[test]
    fn the_most_important_default_that_is_installed_and_associated_wins() {
        let t = Tree::new();
        t.app("usr", "viewer.desktop", "Viewer", "image/png;");
        t.app("usr", "paint.desktop", "Paint", "image/png;");
        t.app("usr", "editor.desktop", "Editor", "text/plain;");
        t.write("usr/applications/mimeapps.list", "[Default Applications]\nimage/png=viewer.desktop\n");
        t.write("config/mimeapps.list", "[Default Applications]\nimage/png=missing.desktop;editor.desktop;paint.desktop\n");
        let apps = t.load(&[]);
        assert_eq!(default_id(&apps, "image/png"), Some("paint.desktop"), "missing is not installed, the editor not associated");
    }

    #[test]
    fn desktop_specific_lists_come_first_and_only_set_defaults() {
        let t = Tree::new();
        t.app("usr", "viewer.desktop", "Viewer", "image/png;");
        t.app("usr", "paint.desktop", "Paint", "image/png;");
        t.app("usr", "draw.desktop", "Draw", "image/x-other;");
        t.write("config/mimeapps.list", "[Default Applications]\nimage/png=viewer.desktop\n");
        t.write("config/cupertino-mimeapps.list", "[Default Applications]\nimage/png=paint.desktop\n[Added Associations]\nimage/png=draw.desktop\n");
        let apps = t.load(&["Cupertino"]);
        assert_eq!(default_id(&apps, "image/png"), Some("paint.desktop"));
        assert_eq!(open_with(&apps, "image/png"), [("paint.desktop", true), ("viewer.desktop", false)], "a desktop-specific list adds nothing");
        assert_eq!(default_id(&t.load(&[]), "image/png"), Some("viewer.desktop"));
    }

    #[test]
    fn added_and_removed_associations_apply_to_lower_levels_only() {
        let t = Tree::new();
        t.app("usr", "viewer.desktop", "Viewer", "image/png;");
        t.app("usr", "paint.desktop", "Paint", "image/x-other;");
        t.app("local", "mine.desktop", "Mine", "image/x-other;");
        t.write("config/mimeapps.list", "[Removed Associations]\nimage/png=viewer.desktop\n[Added Associations]\nimage/png=paint.desktop\n");
        // a system list cannot add an application that only the user's directory, above it, has
        t.write("usr/applications/mimeapps.list", "[Added Associations]\nimage/png=mine.desktop\n");
        let apps = t.load(&[]);
        assert_eq!(open_with(&apps, "image/png"), [("paint.desktop", true)]);
    }

    #[test]
    fn a_removal_for_a_broader_type_keeps_the_specific_association() {
        let t = Tree::new();
        t.app("usr", "editor.desktop", "Editor", "text/plain;text/x-csrc;");
        t.write("config/mimeapps.list", "[Removed Associations]\ntext/plain=editor.desktop\n");
        let apps = t.load(&[]);
        assert_eq!(default_id(&apps, "text/x-csrc"), Some("editor.desktop"));
        assert_eq!(default_id(&apps, "text/plain"), None);
    }

    #[test]
    fn subclasses_and_aliases_fall_back_to_the_parent_handlers() {
        let t = Tree::new();
        t.app("usr", "editor.desktop", "Editor", "text/plain;");
        t.app("usr", "ccode.desktop", "C Code", "text/x-c;");
        t.write("config/mimeapps.list", "[Default Applications]\ntext/plain=editor.desktop\n");
        let apps = t.load(&[]);
        assert_eq!(default_id(&apps, "application/x-shellscript"), Some("editor.desktop"));
        assert_eq!(default_id(&apps, "text/x-csrc"), Some("ccode.desktop"), "a handler of the specific type beats a parent's default");
        assert_eq!(default_id(&apps, "text/x-c"), Some("ccode.desktop"), "an alias is its canonical type");
        assert_eq!(open_with(&apps, "text/x-csrc"), [("ccode.desktop", true), ("editor.desktop", false)]);
    }

    #[test]
    fn open_with_puts_the_default_first_and_sorts_the_rest_naturally() {
        let t = Tree::new();
        for (id, name) in [("b.desktop", "beta"), ("a10.desktop", "Editor 10"), ("a9.desktop", "Editor 9"), ("z.desktop", "Zed")] {
            t.app("usr", id, name, "text/plain;");
        }
        t.write("config/mimeapps.list", "[Default Applications]\ntext/plain=z.desktop\n");
        let apps = t.load(&[]);
        assert_eq!(open_with(&apps, "text/plain"), [("z.desktop", true), ("b.desktop", false), ("a9.desktop", false), ("a10.desktop", false)]);
    }

    #[test]
    fn octet_stream_handlers_appear_only_without_specific_ones() {
        let t = Tree::new();
        t.app("usr", "hex.desktop", "Hex", "application/octet-stream;");
        t.app("usr", "viewer.desktop", "Viewer", "image/png;");
        let apps = t.load(&[]);
        assert_eq!(open_with(&apps, "image/png"), [("viewer.desktop", true)]);
        assert_eq!(open_with(&apps, "application/x-unknown"), [("hex.desktop", true)]);
    }

    #[test]
    fn handlers_hidden_from_listings_still_open_files_but_hidden_entries_do_not() {
        let t = Tree::new();
        t.write("usr/applications/handler.desktop", "[Desktop Entry]\nType=Application\nName=Handler\nExec=h %u\nNoDisplay=true\nMimeType=x-scheme-handler/demo;\n");
        t.app("usr", "gone.desktop", "Gone", "image/png;");
        t.write("local/applications/gone.desktop", "[Desktop Entry]\nType=Application\nName=Gone\nExec=g\nHidden=true\n");
        let apps = t.load(&[]);
        assert_eq!(default_id(&apps, "x-scheme-handler/demo"), Some("handler.desktop"));
        assert_eq!(default_id(&apps, "image/png"), None, "the user's hidden copy removes the application");
    }

    #[test]
    fn natural_order_compares_numbers_by_value_and_ignores_case() {
        assert_eq!(natural("file2", "file10"), Ordering::Less);
        assert_eq!(natural("File", "file"), Ordering::Equal);
        assert_eq!(natural("a007", "a7"), Ordering::Equal);
        assert_eq!(natural("abc", "abd"), Ordering::Less);
    }
}
