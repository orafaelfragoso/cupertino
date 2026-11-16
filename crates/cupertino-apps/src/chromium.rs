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
//! bar feature.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
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

/// A program built on Chromium, as its launch needs to know it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launcher {
    /// The flags files read on the way to Chromium, a list for each launcher that reads one: the names it tries, in
    /// order, of which it reads the first that exists.
    flags: Vec<Vec<String>>,
}

impl Launcher {
    /// The names of the flags files read on the way, each launcher's in turn.
    pub fn flags_names(&self) -> &[Vec<String>] {
        &self.flags
    }

    /// The feature switches (`--enable-features=…`, `--disable-features=…`) of the flags files the launchers read, in
    /// the order Chromium is given them: for each, the system-wide file and then the one in `config_home`.
    pub fn flags_switches(&self, config_home: Option<&Path>) -> Vec<String> {
        let mut switches = Vec::new();
        for names in &self.flags {
            let read = |dir: &Path, name: &str| {
                std::fs::read_to_string(dir.join(format!("{name}{FLAGS_FILE}")))
            };
            let chosen = names.iter().find_map(|name| {
                let system = read(Path::new(SYSTEM_FLAGS), name).ok();
                let own = config_home.and_then(|home| read(home, name).ok());
                (system.is_some() || own.is_some()).then_some((system, own))
            });
            for text in chosen.into_iter().flat_map(|(s, o)| [s, o]).flatten() {
                switches.extend(feature_switches(&text));
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
    static FOUND: Mutex<Option<HashMap<PathBuf, Found>>> = Mutex::new(None);
    let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
    let found = found.get_or_insert_with(HashMap::new);
    if let Some(known) = found.get(&path).filter(|f| f.current()) {
        return known.launcher.clone();
    }
    let mut read = Vec::new();
    let launcher = inspect(&path, path_dirs, DEPTH, &mut read);
    found.insert(
        path,
        Found {
            read,
            launcher: launcher.clone(),
        },
    );
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

/// `args` (a Chromium program's arguments) with the overlay scroll bar feature on, `flags` being the feature switches
/// its launchers put before them ([`Launcher::flags_switches`]). Unless the list Chromium would take already names the
/// feature, one switch lists every feature of the lists in `flags` and `args`, then the overlay scroll bar feature: in
/// the place of the arguments' last list (their others removed), else before a `--` that ends the switches, else first.
/// A switch that turns the feature off leaves `args` as they are.
pub fn with_overlay_scroll_bars(args: &mut Vec<String>, flags: &[String]) {
    let named = |list: &str| list.split(',').any(is_overlay_scroll_bars);
    // what follows a `--` is no switch
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let all = || flags.iter().chain(args[..end].iter());
    if all().any(|a| a.strip_prefix(DISABLE).is_some_and(named)) {
        return;
    }
    let lists: Vec<&str> = all().filter_map(|a| a.strip_prefix(ENABLE)).collect();
    if lists.last().is_some_and(|list| named(list)) {
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
    let (names, targets) = if script {
        let text = String::from_utf8_lossy(&head);
        let script = Script::read(&text);
        (script.flags_names(), script.targets(path))
    } else {
        (
            flags_names(&String::from_utf8_lossy(&head)),
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
        launcher.flags.insert(0, names);
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
            if !words.iter().any(|w| ["$@", "${@}", "$*"].contains(w)) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    fn with_flags(list: &[&str], flags: &[&str]) -> Vec<String> {
        let mut a = args(list);
        with_overlay_scroll_bars(&mut a, &args(flags));
        a
    }

    fn with(list: &[&str]) -> Vec<String> {
        with_flags(list, &[])
    }

    #[test]
    fn the_feature_goes_first_or_joins_the_features_already_enabled() {
        assert_eq!(with(&["%U"]), ["--enable-features=OverlayScrollbar", "%U"]);
        assert_eq!(
            with(&["--enable-features=WaylandWindowDecorations", "x"]),
            [
                "--enable-features=WaylandWindowDecorations,OverlayScrollbar",
                "x"
            ]
        );
        assert_eq!(
            with(&["--enable-features="]),
            ["--enable-features=OverlayScrollbar"]
        );
    }

    #[test]
    fn a_launch_that_already_names_the_feature_is_left_as_it_is() {
        for list in [
            &["--enable-features=OverlayScrollbar:mode/classic"][..],
            &["--disable-features=A,OverlayScrollbar"][..],
        ] {
            assert_eq!(with(list), args(list));
        }
    }

    #[test]
    fn the_launch_s_lists_become_one_in_the_place_of_the_last_and_before_the_end_of_switches() {
        assert_eq!(
            with(&["--enable-features=A,B", "x", "--enable-features=B,C"]),
            ["x", "--enable-features=A,B,C,OverlayScrollbar"]
        );
        assert_eq!(
            with(&["--app", "--", "--enable-features=NotASwitch"]),
            [
                "--app",
                "--enable-features=OverlayScrollbar",
                "--",
                "--enable-features=NotASwitch"
            ]
        );
    }

    #[test]
    fn the_flags_file_s_features_are_kept_in_the_switch_that_comes_after_them() {
        // the launcher gives Chromium the file's switches first, and only the last list counts
        assert_eq!(
            with_flags(&["%U"], &["--enable-features=VaapiVideoDecoder,TouchpadOverscrollHistoryNavigation"]),
            [
                "--enable-features=VaapiVideoDecoder,TouchpadOverscrollHistoryNavigation,OverlayScrollbar",
                "%U"
            ]
        );
        assert_eq!(
            with_flags(
                &["--enable-features=Mine", "%U"],
                &["--enable-features=Filed"]
            ),
            ["--enable-features=Filed,Mine,OverlayScrollbar", "%U"]
        );
        // the file turns it on in the list that counts: nothing to add
        assert_eq!(
            with_flags(&["%U"], &["--enable-features=A,OverlayScrollbar"]),
            ["%U"]
        );
        // a list of the launch's own after it would drop it: it is kept
        assert_eq!(
            with_flags(
                &["--enable-features=Mine"],
                &["--enable-features=OverlayScrollbar"]
            ),
            ["--enable-features=OverlayScrollbar,Mine"]
        );
    }

    #[test]
    fn a_flags_file_that_turns_the_feature_off_keeps_it_off() {
        assert_eq!(
            with_flags(&["%U"], &["--disable-features=OverlayScrollbar"]),
            ["%U"]
        );
    }

    #[test]
    fn a_flags_file_s_feature_switches_are_its_words_outside_comments() {
        let text = "# --enable-features=Commented\n--ozone-platform=wayland\n  \"--enable-features=A,B\" --x # --enable-features=Late\n--disable-features=C\n";
        assert_eq!(
            feature_switches(text),
            ["--enable-features=A,B", "--disable-features=C"]
        );
    }

    fn executable(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn chromium_in(dir: &Path) -> PathBuf {
        executable(&dir.join("chromium"), "\x7fELF");
        std::fs::write(dir.join(MARKER), "").unwrap();
        dir.join("chromium")
    }

    #[test]
    fn a_program_beside_chromium_s_resources_or_started_by_a_launcher_is_built_on_chromium() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let bin = r.join("bin");
        // a browser in its own folder, reached through a link on the search path
        executable(&r.join("lib/browser/browser"), "\x7fELF");
        std::fs::write(r.join("lib/browser").join(MARKER), "").unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(r.join("lib/browser/browser"), bin.join("browser")).unwrap();
        // an application whose script on the search path runs it by its full path
        let app = chromium_in(&r.join("lib/app"));
        executable(
            &bin.join("app"),
            &format!("#!/bin/sh\nexec {} \"$@\"\n", app.display()),
        );
        // a script that runs a runtime by name, whose own script names it through a variable
        chromium_in(&r.join("lib/runtime37"));
        executable(
            &bin.join("runtime37"),
            &format!(
                "#!/usr/bin/bash\nname=runtime37\nexec {}/lib/${{name}}/chromium \"${{flags[@]}}\" \"$@\"\n",
                r.display()
            ),
        );
        executable(
            &bin.join("editor"),
            "#!/bin/sh\nexec runtime37 /usr/lib/editor/main.js \"$@\"\n",
        );
        // a script beside the program that names it through the script's own folder
        chromium_in(&r.join("lib/chat"));
        executable(
            &r.join("lib/chat/launch"),
            "#!/bin/sh\nhere=\"$(dirname \"$(readlink -f \"$0\")\")\"\nexec \"${here}/chromium\" \"$@\"\n",
        );
        std::os::unix::fs::symlink(r.join("lib/chat/launch"), bin.join("chat")).unwrap();
        // an executable one folder below its resources
        executable(&r.join("opt/tool/bin/tool"), "\x7fELF");
        std::fs::write(r.join("opt/tool").join(MARKER), "").unwrap();
        // a small program that names the real one by its full path
        let small = format!("\x7fELF\0/etc\0{}\0", r.join("lib/app/chromium").display());
        executable(&bin.join("small"), &small);

        // programs that are not
        executable(&bin.join("terminal"), "\x7fELF /usr/lib/terminal/data");
        executable(&bin.join("wrapper"), "#!/bin/sh\nexec terminal \"$@\"\n");
        // a script that only mentions the program, or runs it without the launch's arguments
        executable(
            &bin.join("mentions"),
            &format!("#!/bin/sh\n# see {}\nexec terminal \"$@\"\n", app.display()),
        );
        executable(
            &bin.join("fixed"),
            &format!("#!/bin/sh\nexec {} --app=mail\n", app.display()),
        );
        // a command-line script one folder below the resources, whose options are its own
        executable(
            &r.join("opt/code/bin/code"),
            "#!/bin/sh\nELECTRON_RUN_AS_NODE=1 \"$DIR/../code\" cli.js \"$@\"\n",
        );
        chromium_in(&r.join("opt/code"));
        // a large program that names the real one is not a launcher
        let mut large = small.into_bytes();
        large.resize(READ as usize + 1, 0);
        std::fs::write(bin.join("large"), &large).unwrap();
        std::fs::set_permissions(bin.join("large"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        let dirs = [bin.clone()];
        for program in ["browser", "app", "editor", "chat", "small"] {
            assert!(is_chromium(program, &dirs), "{program}");
        }
        assert!(is_chromium(
            &r.join("opt/tool/bin/tool").display().to_string(),
            &dirs
        ));
        for program in [
            "terminal", "wrapper", "mentions", "fixed", "large", "missing",
        ] {
            assert!(!is_chromium(program, &dirs), "{program}");
        }
        assert!(!is_chromium(
            &r.join("opt/code/bin/code").display().to_string(),
            &dirs
        ));
    }

    #[test]
    fn the_flags_files_the_launchers_name_are_read_the_first_of_each_launcher_s_that_exists() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let bin = r.join("bin");
        chromium_in(&r.join("lib/runtime37"));
        executable(
            &bin.join("runtime37"),
            &format!(
                "#!/usr/bin/bash\nname=runtime37\nflags_file=\"${{XDG_CONFIG_HOME:-$HOME/.config}}/${{name}}-flags.conf\"\n\
                 fallback_file=\"${{XDG_CONFIG_HOME:-$HOME/.config}}/runtime-flags.conf\"\n\
                 exec {}/lib/${{name}}/chromium \"${{flags[@]}}\" \"$@\"\n",
                r.display()
            ),
        );
        executable(
            &bin.join("editor"),
            "#!/bin/bash\nif [[ -f $XDG_CONFIG_HOME/editor-flags.conf ]]; then\n  F=\"$(cat $XDG_CONFIG_HOME/editor-flags.conf)\"\nfi\nexec runtime37 /usr/lib/editor $F \"$@\"\n",
        );
        let launcher = launcher("editor", std::slice::from_ref(&bin)).unwrap();
        assert_eq!(
            launcher.flags_names(),
            [args(&["editor"]), args(&["runtime37", "runtime"])]
        );
        let config = r.join("config");
        std::fs::create_dir_all(&config).unwrap();
        assert_eq!(launcher.flags_switches(Some(&config)), Vec::<String>::new());
        std::fs::write(
            config.join("runtime-flags.conf"),
            "--enable-features=Fallback\n",
        )
        .unwrap();
        std::fs::write(
            config.join("editor-flags.conf"),
            "--enable-features=Editor\n",
        )
        .unwrap();
        assert_eq!(
            launcher.flags_switches(Some(&config)),
            ["--enable-features=Editor", "--enable-features=Fallback"]
        );
        std::fs::write(
            config.join("runtime37-flags.conf"),
            "--disable-features=OverlayScrollbar\n",
        )
        .unwrap();
        assert_eq!(
            launcher.flags_switches(Some(&config)),
            [
                "--enable-features=Editor",
                "--disable-features=OverlayScrollbar"
            ]
        );
    }

    #[test]
    fn what_is_found_is_kept_until_a_file_read_for_it_changes() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let bin = r.join("bin");
        let app = chromium_in(&r.join("lib/app"));
        let script = bin.join("app");
        executable(&script, "#!/bin/sh\nexec terminal \"$@\"\n");
        let dirs = [bin.clone()];
        let before = modified(&script).unwrap();
        assert!(!is_chromium("app", &dirs));
        let touch = |at: SystemTime| {
            let file = std::fs::File::options().write(true).open(&script).unwrap();
            file.set_modified(at).unwrap();
        };
        // rewritten with its time kept: the script is not read again
        std::fs::write(
            &script,
            format!("#!/bin/sh\nexec {} \"$@\"\n", app.display()),
        )
        .unwrap();
        touch(before);
        assert!(!is_chromium("app", &dirs));
        // a change of time is seen
        touch(before + std::time::Duration::from_secs(1));
        assert!(is_chromium("app", &dirs));
    }
}
