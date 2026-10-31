//! Clipboard Access: `data-control.conf` as the Privacy pane shows and changes it, and the applications it can allow.
//!
//! Whether an entry still allows its program is the server's own test ([`Entry::allows_file`]), never derived here. A
//! program is always the canonical path of its file: the server compares the resolved executable of a running client,
//! so a link would never match.

use cupertino_apps::entry::DesktopEntry;
use cupertino_apps::exec::command_for_entry;
use cupertino_defaults::data_control::{refusal, DataControlConf, Entry, Unusable, RUNS_SCRIPTS};
use std::io;
use std::path::{Path, PathBuf};

/// What the file holds, as the pane shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct List {
    pub conf: DataControlConf,
    /// The lines that allow nothing, in file order.
    pub unusable: Vec<Unusable>,
    /// Why the file was refused or could not be read: nothing may then be written over it.
    pub unreadable: Option<String>,
}

/// Where an entry stands now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Allowed,
    /// The file changed since it was allowed, or a path-only program is no longer one only root can change.
    Changed,
    Missing,
}

/// Why an application cannot be allowed, in the order the checks run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Sandboxed,
    RunsScripts,
    WaylandProxy,
    AlreadyAllowed,
    Unreadable,
}

impl Reason {
    /// The short reason a disabled row of the chooser shows.
    pub fn text(self) -> &'static str {
        match self {
            Reason::Sandboxed => "Sandboxed",
            Reason::RunsScripts => "Runs scripts",
            Reason::WaylandProxy => "Wayland proxy",
            Reason::AlreadyAllowed => "Already allowed",
            Reason::Unreadable => "Can't be read",
        }
    }
}

/// An installed application the chooser lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    pub name: String,
    pub icon: Option<String>,
    /// The canonical program, when its `Exec` names one that exists.
    pub program: Option<PathBuf>,
    /// Why it cannot be allowed; `None` when it can.
    pub reason: Option<Reason>,
}

impl List {
    /// The file at `path`. A missing file is an empty list; a refused or unreadable one is [`List::unreadable`].
    pub fn load(path: &Path) -> Self {
        match DataControlConf::load_unusable(path) {
            Ok((conf, unusable)) => List {
                conf,
                unusable,
                unreadable: None,
            },
            Err(e) => List {
                unreadable: Some(e.to_string()),
                ..List::default()
            },
        }
    }

    /// Write the entries, then the lines that allow nothing, into `path`.
    pub fn store(&self, path: &Path) -> io::Result<()> {
        if self.unreadable.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the list can't be read",
            ));
        }
        self.conf.store_keeping(path, &self.unusable)
    }

    pub fn allows(&self, program: &Path) -> bool {
        self.conf.allowed.iter().any(|e| e.program == program)
    }
}

/// Where `entry` stands now.
pub fn state(entry: &Entry) -> State {
    match std::fs::metadata(&entry.program) {
        Err(_) => State::Missing,
        Ok(meta) if entry.allows_file(&meta) => State::Allowed,
        Ok(_) => State::Changed,
    }
}

/// Whether `program` is a script: the process the server sees is its interpreter, so an entry would never match.
pub fn is_script(program: &Path) -> bool {
    use std::io::Read;
    let mut start = [0u8; 2];
    std::fs::File::open(program)
        .and_then(|mut f| f.read_exact(&mut start))
        .is_ok_and(|()| &start == b"#!")
}

/// The entry that allows `program` (canonicalised first), or why it can't be allowed, as a sentence.
pub fn entry_for(program: &Path) -> Result<Entry, String> {
    let program =
        std::fs::canonicalize(program).map_err(|e| format!("{}: {e}", program.display()))?;
    if is_script(&program) {
        return Err("Scripts can't be allowed: the clipboard would see their interpreter.".into());
    }
    if let Some(reason) = refusal(&program) {
        return Err(reason.to_owned());
    }
    if !is_executable(&program) {
        return Err("Not a program".into());
    }
    Entry::for_program(&program)
}

fn is_executable(program: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(program).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The program `entry` names: its `Exec` as the launcher parses it, past a leading `env` and its `NAME=value` words,
/// found in `path_dirs` when it is a bare name. `None` when it names nothing or a bare name no directory has.
pub fn named_program(entry: &DesktopEntry, path_dirs: &[PathBuf]) -> Option<PathBuf> {
    let command = command_for_entry(entry, &[])?;
    let mut words = std::iter::once(command.program).chain(command.args);
    let mut first = words.next()?;
    if Path::new(&first).file_name().is_some_and(|n| n == "env") {
        first = words.find(|w| !w.contains('=') && !w.starts_with('-'))?;
    }
    if first.contains('/') {
        Some(PathBuf::from(first))
    } else {
        path_dirs
            .iter()
            .map(|d| d.join(&first))
            .find(|p| p.is_file())
    }
}

/// The places a sandbox installs its launchers and desktop files.
const SANDBOX_DIRS: [&str; 3] = ["/snap/", "/var/lib/snapd/", "/var/lib/flatpak/"];

fn sandboxed(installed: &Installed) -> bool {
    let in_sandbox = |p: &Path| {
        let text = p.to_string_lossy();
        SANDBOX_DIRS.iter().any(|d| text.starts_with(d)) || text.contains("/flatpak/exports/")
    };
    [installed.named.as_deref(), installed.program.as_deref()]
        .into_iter()
        .flatten()
        .any(|p| in_sandbox(p) || p.file_name().is_some_and(|n| n == "flatpak" || n == "snap"))
        || installed.entry.source.as_deref().is_some_and(in_sandbox)
}

/// An installed application and the canonical program its `Exec` runs, resolved once per showing of the pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Installed {
    pub entry: DesktopEntry,
    /// The program as `Exec` names it ([`named_program`]).
    pub named: Option<PathBuf>,
    /// Its canonical path, when it exists.
    pub program: Option<PathBuf>,
}

/// The applications of `entries` with their programs, found in `path_dirs`.
pub fn installed(entries: Vec<DesktopEntry>, path_dirs: &[PathBuf]) -> Vec<Installed> {
    entries
        .into_iter()
        .map(|entry| {
            let named = named_program(&entry, path_dirs);
            Installed {
                program: named.as_deref().and_then(|p| std::fs::canonicalize(p).ok()),
                named,
                entry,
            }
        })
        .collect()
}

/// The applications the chooser lists, one per program (the first entry of a program wins), sorted by name with
/// `compare`.
pub fn apps(
    installed: &[Installed],
    list: &List,
    compare: impl Fn(&str, &str) -> std::cmp::Ordering,
) -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for app in installed {
        if app.program.is_some() && out.iter().any(|a| a.program == app.program) {
            continue;
        }
        out.push(App {
            name: app.entry.name.clone(),
            icon: app.entry.icon.clone(),
            program: app.program.clone(),
            reason: reason(app, list),
        });
    }
    out.sort_by(|a, b| compare(&a.name, &b.name));
    out
}

fn reason(installed: &Installed, list: &List) -> Option<Reason> {
    if sandboxed(installed) {
        return Some(Reason::Sandboxed);
    }
    let Some(program) = installed.program.as_deref() else {
        return Some(Reason::Unreadable);
    };
    match refusal(program) {
        _ if is_script(program) => return Some(Reason::RunsScripts),
        Some(RUNS_SCRIPTS) => return Some(Reason::RunsScripts),
        Some(_) => return Some(Reason::WaylandProxy),
        None => {}
    }
    if list.allows(program) {
        return Some(Reason::AlreadyAllowed);
    }
    Entry::for_program(program)
        .err()
        .map(|_| Reason::Unreadable)
}

/// The name and icon an allowed program shows: the installed application that runs it, else its file name and no
/// icon.
pub fn identity(program: &Path, installed: &[Installed]) -> (String, Option<String>) {
    installed
        .iter()
        .find(|i| i.program.as_deref() == Some(program))
        .map(|i| (i.entry.name.clone(), i.entry.icon.clone()))
        .unwrap_or_else(|| {
            let name = program.file_name().map_or_else(
                || program.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            (name, None)
        })
}
