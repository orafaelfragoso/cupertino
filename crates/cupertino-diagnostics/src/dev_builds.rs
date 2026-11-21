//! Builds in the development folder that are older than the installed program they start instead of. A component the
//! folder names runs in a host of its own, started from the folder's `cupertino-shell` instead of the installed one. The
//! folder takes no part in package updates, so a build copied into it before a newer package was installed keeps
//! overriding the package's host at every start of such a host.

use cupertino_defaults::compositor::ShellSection;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

/// The installed shell host, the program a component of the development folder replaces with the folder's file of the
/// same name.
pub const HOST_PROGRAM: &str = "/usr/bin/cupertino-shell";

/// The component that always runs in the installed host, whatever the folder names: the session agent, which holds the
/// session-agent role (`docs/architecture/shell-host.md`, development builds).
const ALWAYS_INSTALLED: &str = "agent";

/// The components a `shell` section of the system file runs: its `components` less its `off`. `None` when the section
/// has no `components` list, which runs every component of the server's table.
pub fn components_run(shell: &ShellSection) -> Option<Vec<String>> {
    let off = shell.off.as_deref().unwrap_or_default();
    Some(
        shell
            .components
            .as_ref()?
            .iter()
            .filter(|name| !off.contains(name))
            .cloned()
            .collect(),
    )
}

/// Whether the development folder may give the component `name` a host of its own started from the folder's file.
pub fn may_run_from_folder(name: &str) -> bool {
    name != ALWAYS_INSTALLED
}

/// Whether `dev_program`, a file of the development folder, was last modified before `installed`, the program it starts
/// instead of. A package's files keep their build time, so a build copied into the folder before that package was built
/// is older than it.
///
/// `false` when either path is relative, when either file is missing or its metadata or time cannot be read, and when
/// the two times are equal: only a comparison that could be made marks a build.
pub fn older_than_installed(dev_program: &Path, installed: &Path) -> bool {
    match (modified(dev_program), modified(installed)) {
        (Some(dev), Some(installed)) => dev < installed,
        _ => false,
    }
}

/// The modification time of the file at the absolute `path`, links followed.
fn modified(path: &Path) -> Option<SystemTime> {
    if !path.is_absolute() {
        return None;
    }
    fs::metadata(path).ok()?.modified().ok()
}
