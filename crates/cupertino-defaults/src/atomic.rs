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
