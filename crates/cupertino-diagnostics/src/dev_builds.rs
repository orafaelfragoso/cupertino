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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::path::PathBuf;
    use std::time::Duration;

    /// A file at `dir/name` modified `seconds` after a fixed time.
    fn file(dir: &Path, name: &str, seconds: u64) -> PathBuf {
        let path = dir.join(name);
        let f = File::create(&path).unwrap();
        f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_398_405 + seconds))
            .unwrap();
        path
    }

    #[test]
    fn a_build_modified_before_the_installed_program_is_older() {
        let dir = tempfile::tempdir().unwrap();
        let installed = file(dir.path(), "installed", 60);
        let dev = file(dir.path(), "dev", 0);
        assert!(older_than_installed(&dev, &installed));
    }

    #[test]
    fn a_newer_build_or_one_of_the_same_time_is_not_older() {
        let dir = tempfile::tempdir().unwrap();
        let installed = file(dir.path(), "installed", 60);
        assert!(!older_than_installed(
            &file(dir.path(), "newer", 120),
            &installed
        ));
        assert!(!older_than_installed(
            &file(dir.path(), "same", 60),
            &installed
        ));
    }

    #[test]
    fn a_missing_file_on_either_side_is_not_older() {
        let dir = tempfile::tempdir().unwrap();
        let installed = file(dir.path(), "installed", 60);
        let dev = file(dir.path(), "dev", 0);
        let missing = dir.path().join("missing");
        assert!(!older_than_installed(&missing, &installed));
        assert!(!older_than_installed(&dev, &missing));
    }

    #[test]
    fn metadata_that_cannot_be_read_or_a_relative_path_is_not_older() {
        let dir = tempfile::tempdir().unwrap();
        let installed = file(dir.path(), "installed", 60);
        let dev = file(dir.path(), "dev", 0);
        // a path through a regular file: reading its metadata fails with something other than "not found"
        let through_a_file = dev.join("inside");
        assert!(
            fs::metadata(&through_a_file).is_err_and(|e| e.kind() != std::io::ErrorKind::NotFound)
        );
        assert!(!older_than_installed(&through_a_file, &installed));
        assert!(!older_than_installed(&dev, &installed.join("inside")));
        assert!(!older_than_installed(Path::new("dev"), &installed));
        assert!(!older_than_installed(&dev, Path::new("cupertino-dock")));
    }
}
