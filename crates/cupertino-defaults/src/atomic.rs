//! Atomic file replacement for the settings files: the new contents go to a temporary file in the same directory,
//! which is flushed to disk and renamed over the file, then the directory is flushed. A reader, or a process that
//! dies at any point, sees the old file or the new one, never a mix. A crash can leave the temporary file behind
//! (`.<name>.<pid>.<n>.tmp`); it is never read and the next write does not trip over it. A settings file that is a symbolic
//! link (dotfiles kept elsewhere) stays a link: the file it points to is the one replaced.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes the temporary files of concurrent writes in one process.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// Names tried for the temporary file before giving up: a leftover of a crash can hold one (with a reused pid).
const TEMP_ATTEMPTS: u32 = 16;

/// Replace `path` with `contents` atomically, creating its directory. An existing file keeps its permissions.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_with(path, contents, None, &|_| Ok(()))
}

/// [`write_atomic`] for a file only its owner may read or change: the file gets mode `0600` whatever the old one had,
/// and a link at `path` is replaced by the file rather than followed.
pub fn write_atomic_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_with(path, contents, Some(0o600), &|_| Ok(()))
}

/// [`write_atomic`], or [`write_atomic_private`] with the mode `private`, with `before_rename` run on the flushed
/// temporary file just before the rename (tests use it to fail or crash there).
pub(crate) fn write_atomic_with(
    path: &Path,
    contents: &[u8],
    private: Option<u32>,
    before_rename: &dyn Fn(&Path) -> io::Result<()>,
) -> io::Result<()> {
    // a link is followed: the file it names is replaced, in that file's directory
    let path = &match fs::canonicalize(path) {
        _ if private.is_some() => path.to_path_buf(),
        Ok(resolved) => resolved,
        Err(e) if e.kind() == io::ErrorKind::NotFound => path.to_path_buf(),
        Err(e) => return Err(e),
    };
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} names no file", path.display()),
        )
    })?;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir)?;
    let (tmp, file) = create_temp(&dir, &name.to_string_lossy())?;
    let written = write_temp(file, path, private, contents)
        .and_then(|()| before_rename(&tmp))
        .and_then(|()| fs::rename(&tmp, path));
    if let Err(e) = written {
        return Err(match fs::remove_file(&tmp) {
            Ok(()) => e,
            Err(cleanup) if cleanup.kind() == io::ErrorKind::NotFound => e,
            Err(cleanup) => io::Error::new(
                e.kind(),
                format!("{e}; removing {} also failed: {cleanup}", tmp.display()),
            ),
        });
    }
    File::open(&dir)?.sync_all()
}

/// A new temporary file in `dir` for the file `name`; a name already taken (a crash's leftover) is skipped.
fn create_temp(dir: &Path, name: &str) -> io::Result<(PathBuf, File)> {
    let mut last = None;
    for _ in 0..TEMP_ATTEMPTS {
        let tmp = dir.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("at least one attempt was made"))
}

fn write_temp(
    mut file: File,
    target: &Path,
    private: Option<u32>,
    contents: &[u8],
) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match private {
        Some(mode) => file.set_permissions(fs::Permissions::from_mode(mode))?,
        None => match fs::metadata(target) {
            Ok(meta) => file.set_permissions(meta.permissions())?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        },
    }
    file.write_all(contents)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set in the child process of [`a_process_killed_between_write_and_rename_leaves_the_old_file`]: the file it
    /// writes and aborts on.
    const CRASH_CHILD_ENV: &str = "CUPERTINO_DEFAULTS_CRASH_CHILD";

    fn temp_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/dock.conf");
        write_atomic(&path, b"tile_size = 48\n").unwrap();
        write_atomic(&path, b"tile_size = 64\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "tile_size = 64\n");
        assert!(temp_files(&dir.path().join("sub")).is_empty());
    }

    #[test]
    fn a_symlinked_settings_file_stays_a_link_and_its_target_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let dotfiles = dir.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        let target = dotfiles.join("dock.conf");
        fs::write(&target, "tile_size = 48\n").unwrap();
        let link = dir.path().join("cupertino/dock.conf");
        fs::create_dir(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        write_atomic(&link, b"tile_size = 64\n").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "still a link"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "tile_size = 64\n");
        assert!(temp_files(&dotfiles).is_empty() && temp_files(link.parent().unwrap()).is_empty());
    }

    #[test]
    fn a_private_write_replaces_a_link_and_an_open_file_with_one_only_its_owner_reads() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere");
        fs::write(&elsewhere, "kept\n").unwrap();
        let path = dir.path().join("data-control.conf");
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();

        write_atomic_private(&path, b"allow = /usr/bin/x\n").unwrap();
        let meta = fs::symlink_metadata(&path).unwrap();
        assert!(meta.is_file(), "the link is replaced by a file");
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::read_to_string(&elsewhere).unwrap(), "kept\n");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        write_atomic_private(&path, b"allow = /usr/bin/y\n").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "allow = /usr/bin/y\n");
    }

    #[test]
    fn a_leftover_temporary_name_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("power.conf");
        // the names the next writes of this process would take, as a crash with this pid would have left them
        let next = NEXT_TEMP.load(Ordering::Relaxed);
        for n in next..next + 4 {
            fs::write(
                dir.path()
                    .join(format!(".power.conf.{}.{n}.tmp", std::process::id())),
                "old",
            )
            .unwrap();
        }
        write_atomic(&path, b"dim_before_display_sleep = off\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "dim_before_display_sleep = off\n"
        );
    }

    #[test]
    fn an_existing_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("power.conf");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        write_atomic(&path, b"new\n").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_failure_before_the_rename_keeps_the_old_file_and_removes_the_temporary_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dock.conf");
        fs::write(&path, "tile_size = 48\n").unwrap();
        let error = write_atomic_with(&path, b"tile_size = 64\n", None, &|_| {
            Err(io::Error::other("disk full"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "disk full");
        assert_eq!(fs::read_to_string(&path).unwrap(), "tile_size = 48\n");
        assert!(temp_files(dir.path()).is_empty());
    }

    #[test]
    fn a_process_killed_between_write_and_rename_leaves_the_old_file() {
        if let Some(target) = std::env::var_os(CRASH_CHILD_ENV) {
            // the child: the new contents are on disk in the temporary file when the process dies
            write_atomic_with(Path::new(&target), b"tile_size = 64\n", None, &|_| {
                std::process::abort()
            })
            .unwrap();
            unreachable!("the child aborts before the rename");
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dock.conf");
        fs::write(&path, "tile_size = 48\n").unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "atomic::tests::a_process_killed_between_write_and_rename_leaves_the_old_file",
                "--test-threads=1",
            ])
            .env("CUPERTINO_ASSET_SEARCH_PATH", "")
            .env(CRASH_CHILD_ENV, &path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "the child crashed");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "tile_size = 48\n",
            "the old file survives"
        );
        let leftovers = temp_files(dir.path());
        assert_eq!(
            leftovers.len(),
            1,
            "the crash leaves the temporary file: {leftovers:?}"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(&leftovers[0])).unwrap(),
            "tile_size = 64\n"
        );
        write_atomic(&path, b"tile_size = 32\n").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "tile_size = 32\n",
            "the next write is not hindered by the leftover"
        );
    }
}
