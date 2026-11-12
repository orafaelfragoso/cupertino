//! Reopening applications at the next login: the person's choice on the log out, restart and shut down panels, the
//! applications saved when the session ends, and starting them again once when the person logs back in.
//!
//! * The saved state is one small file in the user's state directory, [`state_path`]
//!   (`$XDG_STATE_HOME/cupertino/restore`, default `~/.local/state/cupertino/restore`), replaced atomically and readable by
//!   its owner only. It holds the choice (`reopen`), the session the list was saved in, and desktop file IDs; never a
//!   command. A missing, unreadable or malformed file reads as the defaults: the choice on, no applications.
//! * Saving: the applications the quit sequence asked to quit, by their identity (`app_id`), each resolved to an installed
//!   desktop entry ([`Applications::restorable`]). An application without an entry, an entry hidden from listings, one
//!   that needs a terminal and the shell's own programs ([`SHELL_PROGRAMS`]) are left out; an entry two identities resolve
//!   to is saved once; at most [`MAX_APPS`] are kept.
//! * Starting: [`take_at_login`] empties the list on disk *before* anything is started, so a crash, a restart of the
//!   agent or a session that ends unexpectedly never starts the same list twice. A list saved in the running session
//!   (the agent restarted after a refused restart or shut down) is dropped, not started. Each ID is looked up again among
//!   the installed entries and started through its `Exec` line ([`launch`]); a removed entry, a duplicate, an entry a login
//!   item started already and a start that fails are reported and skipped.
//!
//! What an application shows when it starts again (its windows, its documents) is the application's own business: this
//! module only starts it.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use cupertino_apps::entry::{index_dirs, xdg_application_dirs, DesktopEntry, Locale, Visibility};
use cupertino_apps::exec::{command_for_entry, tokenize, CommandSpec};

/// The state file's name in the `cupertino` directory of the user's state directory.
pub const STATE_NAME: &str = "restore";

/// The checkbox's state when the person has never confirmed a panel: reopening is on.
pub const REOPEN_DEFAULT: bool = true;

/// The most applications one session end saves; the rest are left out.
pub const MAX_APPS: usize = 64;

/// The longest desktop file ID kept, in bytes (a file name's limit).
pub const MAX_ID_BYTES: usize = 255;

/// The longest boot or session identifier kept, in bytes.
const MAX_MARK_BYTES: usize = 64;

/// The version of the file's format this module reads and writes.
const FORMAT_VERSION: &str = "1";

/// Where the kernel names the current boot.
pub const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";

/// The programs of the shell itself, by file name. An application whose desktop entry or identity is one of these is
/// never saved or started, whatever its entry says.
pub const SHELL_PROGRAMS: &[&str] = &[
    "cupertino-btd",
    "cupertino-controlcenter",
    "cupertino-dock",
    "cupertino-keyring-prompter",
    "cupertino-launcher",
    "cupertino-login",
    "cupertino-menubar",
    "cupertino-netd",
    "cupertino-notifications",
    "cupertino-osd",
    "cupertino-polkit-agent",
    "cupertino-portal",
    "cupertino-powerd",
    "cupertino-screenshot",
    "cupertino-server",
    "cupertino-session",
    "cupertino-sessiond",
    "cupertino-windowmanager",
];

/// The session a list was saved in: the boot and logind's session ID, which logind reuses after a restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMark {
    pub boot: String,
    pub session: String,
}

impl SessionMark {
    /// This boot (from [`BOOT_ID_PATH`]) and `session`; `None` when the boot cannot be read or either is not a plain
    /// identifier.
    pub fn current(session: &str) -> Option<SessionMark> {
        let boot = std::fs::read_to_string(BOOT_ID_PATH).ok()?;
        SessionMark::new(boot.trim(), session)
    }

    /// The mark of `boot` and `session` when both are plain identifiers.
    pub fn new(boot: &str, session: &str) -> Option<SessionMark> {
        (plain_mark(boot) && plain_mark(session)).then(|| SessionMark {
            boot: boot.to_owned(),
            session: session.to_owned(),
        })
    }
}

/// What the state file holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedState {
    /// The checkbox as the person last confirmed it.
    pub reopen: bool,
    /// The session the list was saved in; `None` with no list or an unknown session.
    pub saved_in: Option<SessionMark>,
    /// Desktop file IDs to start at the next login, in order, each once.
    pub apps: Vec<String>,
}

impl Default for SavedState {
    fn default() -> Self {
        SavedState {
            reopen: REOPEN_DEFAULT,
            saved_in: None,
            apps: Vec::new(),
        }
    }
}

/// A line of the state file that was not understood; the other lines apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// 1-based.
    pub line: usize,
    pub message: String,
}

/// `$XDG_STATE_HOME/cupertino/restore`, else `$HOME/.local/state/cupertino/restore`; `None` without either.
pub fn state_path(state_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    state_home
        .filter(|d| d.is_absolute())
        .or_else(|| home.map(|h| h.join(".local/state")))
        .map(|d| d.join("cupertino").join(STATE_NAME))
}

/// [`state_path`] from this process's environment.
pub fn state_path_from_env() -> Option<PathBuf> {
    state_path(
        std::env::var_os("XDG_STATE_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
        std::env::var_os("HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from),
    )
}

/// Whether `id` may stand in the file: a desktop file ID of letters, digits, `.`, `-` and `_`, ending in `.desktop`, not
/// starting with a dot, at most [`MAX_ID_BYTES`]. It is only ever looked up among the installed entries.
pub fn valid_desktop_id(id: &str) -> bool {
    id.len() <= MAX_ID_BYTES
        && id.len() > ".desktop".len()
        && id.ends_with(".desktop")
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

fn plain_mark(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MARK_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Read the file's text. Lines are `key = value`; `#` starts a comment. A file of another version keeps only the choice.
/// IDs that are not [`valid_desktop_id`], repeats and those past [`MAX_APPS`] are dropped.
pub fn parse(text: &str) -> (SavedState, Vec<Problem>) {
    let mut state = SavedState::default();
    let mut problems = Vec::new();
    let mut version = None;
    let (mut boot, mut session) = (None, None);
    let mut seen = HashSet::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut problem = |message: String| {
            problems.push(Problem {
                line: index + 1,
                message,
            })
        };
        let Some((key, value)) = line.split_once('=') else {
            problem("not a key = value line".into());
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "version" => version = Some(value.to_owned()),
            "reopen" => match value {
                "true" => state.reopen = true,
                "false" => state.reopen = false,
                _ => problem(format!("reopen is true or false, not {value:?}")),
            },
            "boot" if plain_mark(value) => boot = Some(value.to_owned()),
            "session" if plain_mark(value) => session = Some(value.to_owned()),
            "boot" | "session" => problem(format!("{key} is not a plain identifier")),
            "app" if !valid_desktop_id(value) => problem("app is not a desktop file ID".into()),
            "app" if !seen.insert(value.to_owned()) => {}
            "app" if state.apps.len() >= MAX_APPS => {
                problem(format!("more than {MAX_APPS} applications"))
            }
            "app" => state.apps.push(value.to_owned()),
            _ => problem(format!("unknown key {key:?}")),
        }
    }
    if version.as_deref() != Some(FORMAT_VERSION) {
        if !state.apps.is_empty() {
            problems.push(Problem {
                line: 0,
                message: format!(
                    "format version {} is not {FORMAT_VERSION}: the applications are not read",
                    version.as_deref().unwrap_or("(missing)")
                ),
            });
        }
        state.apps.clear();
    } else if let (Some(boot), Some(session)) = (boot, session) {
        state.saved_in = Some(SessionMark { boot, session });
    }
    (state, problems)
}

/// The file's text for `state`.
pub fn to_text(state: &SavedState) -> String {
    let mut text = format!(
        "# Applications to reopen at the next login, written by cupertino-login.\nversion = {FORMAT_VERSION}\nreopen = {}\n",
        state.reopen
    );
    if let Some(mark) = state.saved_in.as_ref().filter(|_| !state.apps.is_empty()) {
        text.push_str(&format!(
            "boot = {}\nsession = {}\n",
            mark.boot, mark.session
        ));
    }
    let mut seen = HashSet::new();
    for app in state
        .apps
        .iter()
        .filter(|a| valid_desktop_id(a) && seen.insert(a.as_str()))
        .take(MAX_APPS)
    {
        text.push_str(&format!("app = {app}\n"));
    }
    text
}

/// The state at `path`; a missing file is the defaults. The problems of a malformed file are returned beside it.
pub fn load(path: &Path) -> io::Result<(SavedState, Vec<Problem>)> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(parse(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((SavedState::default(), Vec::new())),
        Err(e) => Err(e),
    }
}

/// Replace the file at `path` with `state`, atomically and readable by its owner only; the directory is created.
pub fn store(path: &Path, state: &SavedState) -> io::Result<()> {
    cupertino_defaults::atomic::write_atomic_private(path, to_text(state).as_bytes())
}

/// The checkbox's state for a new panel: the saved choice, else [`REOPEN_DEFAULT`].
pub fn choice(path: &Path) -> bool {
    load(path).map_or(REOPEN_DEFAULT, |(state, _)| state.reopen)
}

/// Why an application is not saved or not started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Skip {
    /// No installed desktop entry, or one hidden from listings, matches.
    NoEntry,
    /// The entry or the identity is one of the shell's own programs.
    Shell,
    /// The entry needs a terminal, which the agent does not have.
    Terminal,
    /// The entry's `Exec` does not split into a command.
    Exec,
    /// Not a desktop file ID.
    Invalid,
    /// The same entry was saved or started already.
    Duplicate,
    /// A login item with the same ID started it already.
    LoginItem,
    /// The command did not start.
    Failed,
}

/// The installed applications the agent saves and starts by.
pub struct Applications {
    entries: Vec<DesktopEntry>,
}

impl Applications {
    pub fn new(entries: Vec<DesktopEntry>) -> Self {
        Applications { entries }
    }

    /// The entries of the XDG application directories this process sees, with the visibility rules of a listing.
    pub fn from_env() -> Self {
        Applications::new(index_dirs(
            &xdg_application_dirs(),
            &Locale::from_env(),
            &Visibility::from_env(),
        ))
    }

    /// The entry that starts the application `identity` (a Wayland `app_id`) again: the entry whose desktop file ID is the
    /// identity (with or without `.desktop`), else the first whose `StartupWMClass` is it, both ignoring case; `Err` when
    /// none matches or the match may not be saved.
    pub fn restorable(&self, identity: &str) -> Result<&DesktopEntry, Skip> {
        let identity = identity.trim();
        if identity.is_empty() {
            return Err(Skip::NoEntry);
        }
        if is_shell_name(identity) {
            return Err(Skip::Shell);
        }
        let wanted = identity.trim_end_matches(".desktop");
        let entry = self
            .entries
            .iter()
            .find(|e| {
                e.id.trim_end_matches(".desktop")
                    .eq_ignore_ascii_case(wanted)
            })
            .or_else(|| {
                self.entries.iter().find(|e| {
                    e.startup_wm_class
                        .as_deref()
                        .is_some_and(|class| class.eq_ignore_ascii_case(wanted))
                })
            })
            .ok_or(Skip::NoEntry)?;
        check(entry)
    }

    /// The installed entry with the desktop file ID `id`, if it may be started.
    pub fn by_id(&self, id: &str) -> Result<&DesktopEntry, Skip> {
        if !valid_desktop_id(id) {
            return Err(Skip::Invalid);
        }
        let entry = self
            .entries
            .iter()
            .find(|e| e.id == id)
            .ok_or(Skip::NoEntry)?;
        check(entry)
    }
}

/// Whether `entry` may be saved and started: not a shell program, no terminal, a usable `Exec`, an ID the file can hold.
fn check(entry: &DesktopEntry) -> Result<&DesktopEntry, Skip> {
    if !valid_desktop_id(&entry.id) {
        return Err(Skip::Invalid);
    }
    let program = tokenize(&entry.exec)
        .and_then(|argv| argv.into_iter().next())
        .ok_or(Skip::Exec)?;
    let program_name = Path::new(&program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if is_shell_name(&entry.id) || is_shell_name(&program_name) {
        return Err(Skip::Shell);
    }
    if entry.terminal {
        return Err(Skip::Terminal);
    }
    Ok(entry)
}

/// Whether `name` (an identity, a desktop file ID or a program's file name) is one of [`SHELL_PROGRAMS`].
fn is_shell_name(name: &str) -> bool {
    let name = name.trim_end_matches(".desktop");
    SHELL_PROGRAMS.iter().any(|p| p.eq_ignore_ascii_case(name))
}

/// The list to save for the applications the quit sequence asked to quit, in that order: each identity resolved to its
/// entry, the skipped ones with the reason.
pub fn saved_list<'a>(
    identities: impl IntoIterator<Item = &'a str>,
    apps: &Applications,
) -> (Vec<String>, Vec<(String, Skip)>) {
    let mut saved: Vec<String> = Vec::new();
    let mut skipped = Vec::new();
    for identity in identities {
        match apps.restorable(identity) {
            Ok(entry) if saved.contains(&entry.id) => {
                skipped.push((identity.to_owned(), Skip::Duplicate))
            }
            Ok(_) if saved.len() >= MAX_APPS => {}
            Ok(entry) => saved.push(entry.id.clone()),
            Err(skip) => skipped.push((identity.to_owned(), skip)),
        }
    }
    (saved, skipped)
}

/// Save what the session end leaves for the next login: the choice and, when it is on, `apps` with the session they were
/// saved in. With the choice off, or no session mark, no list is kept.
pub fn save(
    path: &Path,
    reopen: bool,
    apps: Vec<String>,
    saved_in: Option<SessionMark>,
) -> io::Result<()> {
    let apps = if reopen && saved_in.is_some() {
        apps
    } else {
        Vec::new()
    };
    store(
        path,
        &SavedState {
            reopen,
            saved_in,
            apps,
        },
    )
}

/// Keep the choice and drop any saved list: the session goes on after all (a refused restart), or the list was taken.
pub fn discard(path: &Path) -> io::Result<()> {
    let (state, _) = load(path)?;
    if state.apps.is_empty() && state.saved_in.is_none() {
        return Ok(());
    }
    store(
        path,
        &SavedState {
            reopen: state.reopen,
            saved_in: None,
            apps: Vec::new(),
        },
    )
}

/// What [`take_at_login`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Taken {
    /// Start these, in order.
    Start(Vec<String>),
    /// No list was saved.
    Nothing,
    /// A list was saved with the choice off; it is dropped.
    Disabled,
    /// The list was saved in this very session (the agent restarted); it is dropped.
    SameSession,
}

/// At login: read the saved list and empty it on disk before returning it, keeping the choice. `current` is this
/// session's mark. An error means the list could not be emptied and nothing may be started, so a list is never started
/// twice.
pub fn take_at_login(path: &Path, current: Option<&SessionMark>) -> io::Result<Taken> {
    let (state, _) = load(path)?;
    if state.apps.is_empty() {
        if state.saved_in.is_some() {
            discard(path)?;
        }
        return Ok(Taken::Nothing);
    }
    discard(path)?;
    if !state.reopen {
        return Ok(Taken::Disabled);
    }
    if current.is_some() && state.saved_in.as_ref() == current {
        return Ok(Taken::SameSession);
    }
    Ok(Taken::Start(state.apps))
}

/// What [`launch`] did with each ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub started: Vec<String>,
    pub skipped: Vec<(String, Skip)>,
}

/// Start each of `ids` once through its installed entry's `Exec` (no files, so the field codes for files drop out) and
/// `spawn`, which returns whether the command started. IDs in `login_items` were started by the login items already.
pub fn launch(
    ids: &[String],
    apps: &Applications,
    login_items: &HashSet<String>,
    mut spawn: impl FnMut(&DesktopEntry, CommandSpec) -> bool,
) -> Report {
    let mut report = Report::default();
    let mut done: HashSet<&str> = HashSet::new();
    for id in ids {
        if !done.insert(id.as_str()) {
            report.skipped.push((id.clone(), Skip::Duplicate));
            continue;
        }
        if login_items.contains(id) {
            report.skipped.push((id.clone(), Skip::LoginItem));
            continue;
        }
        let entry = match apps.by_id(id) {
            Ok(entry) => entry,
            Err(skip) => {
                report.skipped.push((id.clone(), skip));
                continue;
            }
        };
        let Some(command) = command_for_entry(entry, &[]) else {
            report.skipped.push((id.clone(), Skip::Exec));
            continue;
        };
        if spawn(entry, command) {
            report.started.push(id.clone());
        } else {
            report.skipped.push((id.clone(), Skip::Failed));
        }
    }
    report
}

#[cfg(test)]
mod tests;
