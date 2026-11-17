//! Auto login (`auto-login` in `login.conf`), the parts that decide whether it may happen. It skips authentication, so the
//! file that turns it on must be one only root can change, and it happens once per boot. design, the design's
//! semantics are unknown (2.10).

use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// The file whose existence says that auto login was decided this boot. It lives in the unit's runtime directory, a tmpfs, so
/// the next boot starts without it; the unit preserves the directory over a restart (`RuntimeDirectoryPreserve=restart`).
pub const DONE_FLAG: &str = "/run/cupertino-login/auto-login-done";

/// Group-write and other-write: the permission bits that let someone besides the owner change a file or a directory.
const WRITABLE_BY_OTHERS: u32 = 0o022;

/// Why a file system object may not vouch for auto login: root must own it and no one else may write to it.
fn untrusted(what: &str, owner: u32, mode: u32) -> Option<String> {
    if owner != 0 {
        return Some(format!("{what} is owned by uid {owner}, not by root"));
    }
    if mode & WRITABLE_BY_OTHERS != 0 {
        return Some(format!(
            "{what} is writable by its group or by everyone (mode {:04o})",
            mode & 0o7777
        ));
    }
    None
}

/// Whether the settings file at `path` may turn auto login on: a regular file (not a link), and it and its directory are
/// owned by root and not writable by group or others. The reason comes back otherwise.
pub fn settings_are_trusted(path: &Path) -> Result<(), String> {
    let file = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !file.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    if let Some(reason) = untrusted(&path.display().to_string(), file.uid(), file.mode()) {
        return Err(reason);
    }
    let directory = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    let metadata = fs::metadata(directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    match untrusted(
        &directory.display().to_string(),
        metadata.uid(),
        metadata.mode(),
    ) {
        Some(reason) => Err(reason),
        None => Ok(()),
    }
}

/// Take the once-per-boot claim: `true` for the caller that created `flag`, `false` when it exists already. The creation is
/// exclusive, so two managers (a restart racing a crash) cannot both log in.
pub fn claim(flag: &Path) -> io::Result<bool> {
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(flag)
    {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}
