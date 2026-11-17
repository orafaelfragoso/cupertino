//! Builds in the development folder that are older than the installed program they start instead of. The folder takes
//! no part in package updates, so a build copied into it before a newer package was installed keeps overriding that
//! package's program at every start of its process.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

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
