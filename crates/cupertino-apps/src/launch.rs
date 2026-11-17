//! Starting programs from a shell process (the dock, the menu bar): what to start, the command it becomes with the
//! activation token the process fetched for it, and starting that command for the caller to reap. The token itself is the
//! process's to request, on its own Wayland connection.

use crate::entry::DesktopEntry;
use crate::exec::{command_for_entry, CommandSpec};
use crate::mimeapps::MimeApps;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// What opens a target no installed application handles: the session's default handler of its type.
pub const OPEN_COMMAND: &str = "xdg-open";

/// What a launch starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// A program (found on `PATH`) and its arguments.
    Program(Vec<String>),
    /// An application, with no files.
    App(DesktopEntry),
    /// A file opened with an application.
    Open { app: DesktopEntry, file: PathBuf },
}

impl Launch {
    /// The command that starts the launch and hands it `token`; `None` when there is no program to run (an empty
    /// argument list, an unusable `Exec`). An entry with `Terminal=true` keeps [`CommandSpec::terminal`] set.
    pub fn command(&self, token: &str) -> Option<CommandSpec> {
        let spec = match self {
            Launch::Program(argv) => CommandSpec::from_argv(argv.clone())?,
            Launch::App(app) => command_for_entry(app, &[])?,
            Launch::Open { app, file } => {
                command_for_entry(app, &[file.to_string_lossy().into_owned()])?
            }
        };
        Some(spec.with_activation_token(token))
    }
}

/// `file` opened with the default application of its type (a folder as `inode/directory`, a name no glob matches
/// as `application/octet-stream`); `None` when no installed application handles it.
pub fn open_with_default(apps: &MimeApps, file: &Path) -> Option<Launch> {
    let mime = apps.database().file_type(file, file.is_dir());
    apps.default_for(mime).map(|app| Launch::Open {
        app: app.clone(),
        file: file.to_path_buf(),
    })
}

/// Starts `command`, whose exit its caller reaps; `None` when it did not start, which is reported on standard error
/// prefixed with `process`, the caller's name.
pub fn spawn(process: &str, mut command: Command) -> Option<Child> {
    match command.spawn() {
        Ok(child) => Some(child),
        Err(e) => {
            let program = command.get_program().to_string_lossy();
            eprintln!("{process}: cannot run {program}: {e}");
            None
        }
    }
}
