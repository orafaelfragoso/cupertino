//! Programs built on Chromium: the browser and Electron applications. On Linux they show scroll bars over the content
//! only with Chromium's overlay scroll bar feature on; with it on, they follow GTK's `gtk-overlay-scrolling` setting,
//! changes included, while they run. A launch from a desktop entry turns the feature on ([`with_overlay_scroll_bars`]),
//! so the desktop's scroll bar setting reaches them through GTK.
//!
//! A program counts as built on Chromium ([`launcher`]) when it is an executable beside the file every Chromium build
//! ships beside its executable, or one folder below it, or when it is a launcher that starts one with the launch's
//! arguments: a script whose `exec` line runs it and passes `"$@"` on, or a small program (up to [`READ`] bytes) that
//! names it by its full path; at most [`DEPTH`] launchers deep. What is found is kept for each program until one of
//! the files read for it changes.
//!
//! Chromium takes only the last `--enable-features` switch it is given, and the usual launchers put the switches of a
//! flags file (`~/.config/NAME-flags.conf`, the `NAME` the launcher names) before the launch's own. So the switch a
//! launch gets lists the features of every list in the flags file and in the launch's arguments, then the overlay scroll
//! bar feature. A launcher script whose `exec` line puts what it reads after `"$@"` gives Chromium the file's switches
//! after the launch's: a list among them is the one Chromium takes, which no switch of the launch can follow, so the
//! launch is then left as it is.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

/// The resource file every Chromium build ships beside its executable.
const MARKER: &str = "chrome_100_percent.pak";
/// The switch that turns Chromium features on, a comma-separated list after it.
const ENABLE: &str = "--enable-features=";
/// The switch that turns Chromium features off.
const DISABLE: &str = "--disable-features=";
/// Chromium's overlay scroll bar feature.
pub const OVERLAY_SCROLL_BARS: &str = "OverlayScrollbar";
/// The end of a flags file's name; the launcher's name for it comes before.
const FLAGS_FILE: &str = "-flags.conf";
/// The system-wide folder of flags files, read before the person's own.
const SYSTEM_FLAGS: &str = "/etc";
/// How much of a launcher is read for the program it starts; a larger program is no launcher.
pub const READ: u64 = 64 * 1024;
/// How many launchers deep the real program is looked for.
pub const DEPTH: usize = 3;
/// How many full paths a small launcher program may name before the rest are ignored.
const PATHS: usize = 64;
/// The words of a script's `exec` line that pass the launch's arguments on, quotes taken off.
const ARGUMENTS: [&str; 3] = ["$@", "${@}", "$*"];

/// A program built on Chromium, as its launch needs to know it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launcher {
    /// The flags files read on the way to Chromium, one for each launcher that reads one.
    flags: Vec<FlagsFile>,
}

/// The flags file a launcher reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FlagsFile {
    /// The names it tries, in order, of which it reads the first that exists.
    names: Vec<String>,
    /// The launcher puts the file's switches after the launch's arguments (`"$@"`), not before them.
    after: bool,
}

/// The feature switches of the flags files a launch's launchers read, by where Chromium is given them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlagsSwitches {
    /// Those put before the launch's arguments.
    pub before: Vec<String>,
    /// Those put after them, which a list of the launch's own cannot follow.
    pub after: Vec<String>,
}

impl Launcher {
    /// The names of the flags files read on the way, each launcher's in turn.
    pub fn flags_names(&self) -> Vec<&[String]> {
        self.flags.iter().map(|f| &f.names[..]).collect()
    }

    /// The feature switches (`--enable-features=…`, `--disable-features=…`) of the flags files the launchers read, in
    /// the order Chromium is given them, those before the launch's arguments and those after: for each file, the
    /// system-wide one and then the one in `config_home`.
    pub fn flags_switches(&self, config_home: Option<&Path>) -> FlagsSwitches {
        let mut switches = FlagsSwitches::default();
        for file in &self.flags {
            let read = |dir: &Path, name: &str| {
                std::fs::read_to_string(dir.join(format!("{name}{FLAGS_FILE}")))
            };
            let chosen = file.names.iter().find_map(|name| {
                let system = read(Path::new(SYSTEM_FLAGS), name).ok();
                let own = config_home.and_then(|home| read(home, name).ok());
                (system.is_some() || own.is_some()).then_some((system, own))
            });
            let to = if file.after {
                &mut switches.after
            } else {
                &mut switches.before
            };
            for text in chosen.into_iter().flat_map(|(s, o)| [s, o]).flatten() {
                to.extend(feature_switches(&text));
            }
        }
        switches
    }
}

/// The feature switches of a flags file's text: its words, split on white space with quotes taken off, after any line
/// or the part of a line that starts with `#`.
pub fn feature_switches(text: &str) -> Vec<String> {
    text.lines()
        .flat_map(|line| {
            line.split_whitespace()
                .take_while(|w| !w.starts_with('#'))
                .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
        })
        .filter(|w| w.starts_with(ENABLE) || w.starts_with(DISABLE))
        .map(str::to_owned)
        .collect()
}

/// The person's configuration folder, where launchers look for flags files: `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn config_home() -> Option<PathBuf> {
    let set = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    set("XDG_CONFIG_HOME").or_else(|| set("HOME").map(|h| h.join(".config")))
}

/// Whether `program` (a path, or a name looked for in `path_dirs`) is built on Chromium.
pub fn is_chromium(program: &str, path_dirs: &[PathBuf]) -> bool {
    launcher(program, path_dirs).is_some()
}

/// What a launch of `program` (a path, or a name looked for in `path_dirs`) needs to know of it, when it is built on
/// Chromium. Kept for each program until a file read for it changes.
pub fn launcher(program: &str, path_dirs: &[PathBuf]) -> Option<Launcher> {
    let path = std::fs::canonicalize(resolve(program, path_dirs)?).ok()?;
    // the process's one table: its entries never change, and the lock is held only to look one up or put one in
    static FOUND: Mutex<Option<HashMap<PathBuf, Arc<Found>>>> = Mutex::new(None);
    let known = FOUND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|found| found.get(&path).cloned());
    if let Some(known) = known.filter(|f| f.current()) {
        return known.launcher.clone();
    }
    let mut read = Vec::new();
    let launcher = inspect(&path, path_dirs, DEPTH, &mut read);
    let found = Arc::new(Found {
        read,
        launcher: launcher.clone(),
    });
    FOUND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(HashMap::new)
        .insert(path, found);
    launcher
}

/// What was found for a program, and the files read to find it with the time each was last changed.
struct Found {
    read: Vec<(PathBuf, Option<SystemTime>)>,
    launcher: Option<Launcher>,
}

impl Found {
    fn current(&self) -> bool {
        self.read
            .iter()
            .all(|(path, changed)| modified(path) == *changed)
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// `args` of `program` (a path, or a name looked for on `PATH`) as a launch gives them: a program built on Chromium gets
/// its overlay scroll bars turned on ([`with_overlay_scroll_bars`]), its launchers' flags files read from the person's
/// configuration folder ([`config_home`]); any other program's are left as they are. Every launch of an application
/// goes through it, whatever starts it.
pub fn launch_args(program: &str, args: &mut Vec<String>) {
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(chromium) = launcher(program, &path_dirs) {
        let flags = chromium.flags_switches(config_home().as_deref());
        with_overlay_scroll_bars(args, &flags);
    }
}

/// `args` (a Chromium program's arguments) with the overlay scroll bar feature on, `flags` being the feature switches
/// its launchers put before and after them ([`Launcher::flags_switches`]). Unless the list Chromium would take already
/// names the feature, one switch lists every feature of the lists in `flags` and `args`, then the overlay scroll bar
/// feature: in the place of the arguments' last list (their others removed), else before a `--` that ends the
/// switches, else first. A switch that turns the feature off leaves `args` as they are, and so does a list a launcher
/// puts after the arguments: Chromium takes that one whatever the arguments hold.
pub fn with_overlay_scroll_bars(args: &mut Vec<String>, flags: &FlagsSwitches) {
    let named = |list: &str| list.split(',').any(is_overlay_scroll_bars);
    // what follows a `--` is no switch
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    // in the order Chromium is given them
    let all = || {
        flags
            .before
            .iter()
            .chain(args[..end].iter())
            .chain(flags.after.iter())
    };
    if all().any(|a| a.strip_prefix(DISABLE).is_some_and(named)) {
        return;
    }
    let lists: Vec<&str> = all().filter_map(|a| a.strip_prefix(ENABLE)).collect();
    if lists.last().is_some_and(|list| named(list)) {
        return;
    }
    // the last list comes after the arguments: no switch of the launch can follow it
    if flags.after.iter().any(|a| a.starts_with(ENABLE)) {
        return;
    }
    let mut features: Vec<&str> = Vec::new();
    for feature in lists.iter().flat_map(|l| l.split(',')) {
        if !feature.is_empty() && !features.contains(&feature) {
            features.push(feature);
        }
    }
    if !features.iter().any(|f| is_overlay_scroll_bars(f)) {
        features.push(OVERLAY_SCROLL_BARS);
    }
    let switch = format!("{ENABLE}{}", features.join(","));
    let own: Vec<usize> = (0..end).filter(|&i| args[i].starts_with(ENABLE)).collect();
    match own.split_last() {
        Some((&last, earlier)) => {
            args[last] = switch;
            for &i in earlier.iter().rev() {
                args.remove(i);
            }
        }
        None => args.insert(if end < args.len() { end } else { 0 }, switch),
    }
}

/// Whether a feature list's entry (`Name`, `Name:param/value`, `Name<Trial`, `*Name`) names the overlay scroll bars.
fn is_overlay_scroll_bars(feature: &str) -> bool {
    let name = feature.trim_start_matches('*');
    name.split([':', '<']).next() == Some(OVERLAY_SCROLL_BARS)
}

fn resolve(program: &str, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    path_dirs
        .iter()
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn has_marker(dir: Option<&Path>) -> bool {
    dir.is_some_and(|d| d.join(MARKER).is_file())
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `path` (its real path) as a Chromium program or a launcher of one; every file read is added to `read`.
fn inspect(
    path: &Path,
    path_dirs: &[PathBuf],
    depth: usize,
    read: &mut Vec<(PathBuf, Option<SystemTime>)>,
) -> Option<Launcher> {
    read.push((path.to_owned(), modified(path)));
    let size = std::fs::metadata(path)
        .ok()
        .filter(|_| is_executable(path))?
        .len();
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0; 2];
    let script = file.read_exact(&mut head).is_ok() && head == b"#!";
    let dir = path.parent();
    if !script && (has_marker(dir) || has_marker(dir.and_then(Path::parent))) {
        return Some(Launcher::default());
    }
    if depth == 0 || size > READ {
        return None;
    }
    file.take(READ).read_to_end(&mut head).ok()?;
    let (names, after, targets) = if script {
        let text = String::from_utf8_lossy(&head);
        let script = Script::read(&text);
        (
            script.flags_names(),
            script.passes_more_after_arguments(),
            script.targets(path),
        )
    } else {
        (
            flags_names(&String::from_utf8_lossy(&head)),
            false,
            named_paths(&head),
        )
    };
    let mut launcher = targets
        .into_iter()
        .filter_map(|target| resolve(&target, path_dirs))
        .filter_map(|target| std::fs::canonicalize(target).ok())
        .filter(|target| target != path)
        .find_map(|target| inspect(&target, path_dirs, depth - 1, read))?;
    if !names.is_empty() {
        launcher.flags.insert(0, FlagsFile { names, after });
    }
    Some(launcher)
}

/// The names of the flags files `text` names (`NAME-flags.conf`), in the order it names them.
fn flags_names(text: &str) -> Vec<String> {
    let name_char = |c: char| c.is_ascii_alphanumeric() || "._+-".contains(c);
    let mut names: Vec<String> = Vec::new();
    for (at, _) in text.match_indices(FLAGS_FILE) {
        let before = &text[..at];
        let start = before
            .char_indices()
            .rev()
            .find(|(_, c)| !name_char(*c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let name = &before[start..];
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// The full paths a small launcher program names.
fn named_paths(launcher: &[u8]) -> Vec<String> {
    let path_byte = |b: &u8| b.is_ascii_alphanumeric() || b"/._+-".contains(b);
    launcher
        .split(|b| !path_byte(b))
        .filter(|w| w.len() > 1 && w[0] == b'/')
        .take(PATHS)
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect()
}

/// A launcher script, its comments left out, with the plain values it gives its variables (`name=value`).
struct Script<'a> {
    lines: Vec<&'a str>,
    variables: HashMap<&'a str, &'a str>,
}

impl<'a> Script<'a> {
    fn read(text: &'a str) -> Self {
        let lines: Vec<&str> = text
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        let mut variables = HashMap::new();
        for line in &lines {
            let line = line.strip_prefix("readonly ").unwrap_or(line);
            if let Some((name, value)) = line.split_once('=') {
                let value = value.trim_matches(|c| c == '"' || c == '\'');
                let plain = |c: char| c.is_ascii_alphanumeric() || "/._+-".contains(c);
                if !name.is_empty()
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    && !value.is_empty()
                    && value.chars().all(plain)
                {
                    variables.insert(name, value);
                }
            }
        }
        Self { lines, variables }
    }

    /// `text` with the script's plain variables (`$name`, `${name}`) put in; any other is left as it is.
    fn expand(&self, text: &str) -> String {
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find('$') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let (name, len) = match after.strip_prefix('{') {
                Some(inner) => match inner.find('}') {
                    Some(end) if inner[..end].chars().all(ident) => (&inner[..end], end + 2),
                    _ => ("", 0),
                },
                None => {
                    let end = after.find(|c: char| !ident(c)).unwrap_or(after.len());
                    (&after[..end], end)
                }
            };
            match self.variables.get(name) {
                Some(value) if !name.is_empty() => out.push_str(value),
                _ => out.push_str(&rest[at..at + 1 + len]),
            }
            rest = &after[len..];
        }
        out.push_str(rest);
        out
    }

    fn flags_names(&self) -> Vec<String> {
        flags_names(&self.expand(&self.lines.join("\n")))
    }

    /// Whether an `exec` line that passes the launch's arguments on (`"$@"`) puts something the script reads after them
    /// (a variable, `"${flags[@]}"`, or what a command prints, `$(cat …)`): the switches of a flags file come after the
    /// launch's own there.
    fn passes_more_after_arguments(&self) -> bool {
        self.lines.iter().any(|line| {
            let Some(rest) = line.strip_prefix("exec ") else {
                return false;
            };
            let words: Vec<&str> = rest
                .split_whitespace()
                .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
                .collect();
            words
                .iter()
                .position(|w| ARGUMENTS.contains(w))
                .is_some_and(|at| {
                    words[at + 1..]
                        .iter()
                        .any(|w| w.contains('$') || w.contains('`'))
                })
        })
    }

    /// The programs the script's `exec` lines start with the launch's arguments (`"$@"`). A program named through a
    /// variable that is not set plainly in the script (`"$DIR/app"`, `"${here}/app"`) is looked for beside the script.
    fn targets(&self, path: &Path) -> Vec<String> {
        let dir = path.parent().unwrap_or(Path::new("/"));
        let mut targets = Vec::new();
        for line in &self.lines {
            let Some(rest) = line.strip_prefix("exec ") else {
                continue;
            };
            let words: Vec<&str> = rest
                .split_whitespace()
                .map(|w| w.trim_matches(|c| c == '"' || c == '\''))
                .collect();
            if !words.iter().any(|w| ARGUMENTS.contains(w)) {
                continue;
            }
            // `exec -a NAME PROGRAM` names the process
            let words = match words.first() {
                Some(&"-a") => words.get(2..).unwrap_or_default(),
                _ => &words[..],
            };
            let Some(program) = words.iter().find(|w| !w.starts_with('-')) else {
                continue;
            };
            let program = self.expand(program);
            let program = match program.rsplit_once('/') {
                Some((prefix, name)) if prefix.starts_with('$') => {
                    dir.join(name).display().to_string()
                }
                _ if program.contains('$') => continue,
                Some(_) if !program.starts_with('/') => dir.join(&program).display().to_string(),
                _ => program,
            };
            targets.push(program);
        }
        targets
    }
}
