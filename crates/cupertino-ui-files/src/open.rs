//! Opening files with applications: the default application and Open With list of a file's type from
//! `mimeapps.list`, and the command that opens it (`cupertino-apps`, which also types the file).

use cupertino_apps::exec::{command_for_entry, CommandSpec};
use cupertino_apps::{DesktopEntry, MimeApps, OpenWithItem};
use std::fmt;
use std::path::Path;

/// Why a file cannot be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No application handles the type.
    NoApplication { mime: String },
    /// The application's `Exec` line gives no command.
    BadExec { id: String },
    /// The application asks for a terminal (`Terminal=true`), and the file manager has none to run it in.
    NeedsTerminal { id: String },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NoApplication { mime } => write!(f, "no application opens {mime}"),
            OpenError::BadExec { id } => write!(f, "{id} has no usable Exec line"),
            OpenError::NeedsTerminal { id } => {
                write!(f, "{id} runs in a terminal, and none is configured")
            }
        }
    }
}

impl std::error::Error for OpenError {}

/// The command that opens `path` with `entry`; an entry with `Terminal=true` runs under `terminal`, the
/// argv prefix of a terminal emulator (for example `["foot", "-e"]`).
pub fn command_with(
    entry: &DesktopEntry,
    path: &Path,
    terminal: Option<&[String]>,
) -> Result<CommandSpec, OpenError> {
    let spec =
        command_for_entry(entry, &[path.to_string_lossy().into_owned()]).ok_or_else(|| {
            OpenError::BadExec {
                id: entry.id.clone(),
            }
        })?;
    match (spec.terminal, terminal) {
        (false, _) => Ok(spec),
        (true, Some(terminal)) => Ok(spec.in_terminal(terminal)),
        (true, None) => Err(OpenError::NeedsTerminal {
            id: entry.id.clone(),
        }),
    }
}

/// The installed application whose desktop entry is the file at `path`: the entry the session uses for its id (its
/// path below an `applications` folder, subfolders joined with `-`), and that file itself, not a copy elsewhere or one a
/// more important folder shadows. Opening such an item starts the application ([`open_command`]).
pub fn application_at<'a>(apps: &'a MimeApps, path: &Path) -> Option<&'a DesktopEntry> {
    let names: Vec<&str> = path
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<_>>()?;
    let folder = names.iter().rposition(|part| *part == "applications")?;
    let id = names[folder + 1..].join("-");
    if !id.ends_with(".desktop") {
        return None;
    }
    apps.entry(&id)
        .filter(|entry| entry.source.as_deref() == Some(path))
}

/// The command that opens `path` with the default application of its type, or that starts the application whose
/// entry it is ([`application_at`]); `terminal` as for [`command_with`].
pub fn open_command(
    apps: &MimeApps,
    path: &Path,
    is_dir: bool,
    terminal: Option<&[String]>,
) -> Result<CommandSpec, OpenError> {
    if let Some(entry) = application_at(apps, path) {
        let spec = command_for_entry(entry, &[]).ok_or_else(|| OpenError::BadExec {
            id: entry.id.clone(),
        })?;
        return match (spec.terminal, terminal) {
            (false, _) => Ok(spec),
            (true, Some(terminal)) => Ok(spec.in_terminal(terminal)),
            (true, None) => Err(OpenError::NeedsTerminal {
                id: entry.id.clone(),
            }),
        };
    }
    let mime = apps.database().file_type(path, is_dir);
    let entry = apps
        .default_for(mime)
        .ok_or_else(|| OpenError::NoApplication {
            mime: mime.to_string(),
        })?;
    command_with(entry, path, terminal)
}

/// The desktop entry id of the application that opens `path`: the one it is the entry of, else the default
/// application of its type, if it has one.
pub fn default_app_id(apps: &MimeApps, path: &Path, is_dir: bool) -> Option<String> {
    if let Some(entry) = application_at(apps, path) {
        return Some(entry.id.clone());
    }
    apps.default_for(apps.database().file_type(path, is_dir))
        .map(|entry| entry.id.clone())
}

/// The Open With list of `path`: the default application first, then the others by name.
pub fn open_with<'a>(apps: &'a MimeApps, path: &Path, is_dir: bool) -> Vec<OpenWithItem<'a>> {
    apps.open_with(apps.database().file_type(path, is_dir))
}
